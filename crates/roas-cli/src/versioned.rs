//! Version dispatch helpers for the CLI.
//!
//! The four `Spec` types in `roas` are distinct, so the CLI needs a
//! small enum to thread them through `validate` and `convert` without
//! a fork of every command per version. `DetectedSpec` carries a
//! parsed spec at whichever version was detected (or forced) on the
//! command line; `SpecVersion` is the `clap::ValueEnum`-compatible tag
//! shown to users.

use anyhow::{Context, Result, anyhow, bail};
use clap::ValueEnum;
use roas::loader::Loader;
use roas::merge::{Merge, MergeError, MergeOptions, MergeReport};
use roas::validation::{Error as ValidationError, Options, Validate};
use roas::{v2, v3_0, v3_1, v3_2};
use serde_json::Value;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum SpecVersion {
    /// OpenAPI 2.0 (Swagger).
    #[value(name = "v2", alias = "2", alias = "2.0", alias = "swagger")]
    V2,
    /// OpenAPI 3.0.x.
    #[value(name = "v3_0", alias = "3.0", alias = "v3.0")]
    V3_0,
    /// OpenAPI 3.1.x.
    #[value(name = "v3_1", alias = "3.1", alias = "v3.1")]
    V3_1,
    /// OpenAPI 3.2.x.
    #[value(name = "v3_2", alias = "3.2", alias = "v3.2")]
    V3_2,
}

impl SpecVersion {
    pub fn label(self) -> &'static str {
        match self {
            SpecVersion::V2 => "OpenAPI 2.0",
            SpecVersion::V3_0 => "OpenAPI 3.0",
            SpecVersion::V3_1 => "OpenAPI 3.1",
            SpecVersion::V3_2 => "OpenAPI 3.2",
        }
    }
}

/// A parsed spec at the version the user asked for (or that auto-detect
/// inferred).
pub enum DetectedSpec {
    V2(v2::spec::Spec),
    V3_0(v3_0::spec::Spec),
    V3_1(v3_1::spec::Spec),
    V3_2(v3_2::spec::Spec),
}

impl DetectedSpec {
    pub fn version(&self) -> SpecVersion {
        match self {
            DetectedSpec::V2(_) => SpecVersion::V2,
            DetectedSpec::V3_0(_) => SpecVersion::V3_0,
            DetectedSpec::V3_1(_) => SpecVersion::V3_1,
            DetectedSpec::V3_2(_) => SpecVersion::V3_2,
        }
    }

    pub fn label(&self) -> &'static str {
        self.version().label()
    }

    pub fn validate(
        &self,
        options: enumset::EnumSet<Options>,
        loader: Option<&mut Loader>,
    ) -> Result<(), ValidationError> {
        match self {
            DetectedSpec::V2(s) => s.validate(options, loader),
            DetectedSpec::V3_0(s) => s.validate(options, loader),
            DetectedSpec::V3_1(s) => s.validate(options, loader),
            DetectedSpec::V3_2(s) => s.validate(options, loader),
        }
    }

    /// Chain the existing `From<v_X::Spec> for v_Y::Spec` migrations
    /// to upconvert this spec to the requested target. Returns the
    /// converted spec at its new version, still as a typed
    /// [`DetectedSpec`] so the caller can run further version-specific
    /// operations (e.g. `Spec::collapse`) before serialising.
    ///
    /// Returns an error if the requested conversion is a downconversion
    /// (the caller's responsibility to reject those before calling).
    pub fn convert_to_detected(self, target: SpecVersion) -> Result<DetectedSpec> {
        match (self, target) {
            (DetectedSpec::V2(s), SpecVersion::V2) => Ok(DetectedSpec::V2(s)),
            (DetectedSpec::V3_0(s), SpecVersion::V3_0) => Ok(DetectedSpec::V3_0(s)),
            (DetectedSpec::V3_1(s), SpecVersion::V3_1) => Ok(DetectedSpec::V3_1(s)),
            (DetectedSpec::V3_2(s), SpecVersion::V3_2) => Ok(DetectedSpec::V3_2(s)),

            (DetectedSpec::V2(s), SpecVersion::V3_0) => {
                Ok(DetectedSpec::V3_0(v3_0::spec::Spec::from(s)))
            }
            (DetectedSpec::V2(s), SpecVersion::V3_1) => {
                let v30 = v3_0::spec::Spec::from(s);
                Ok(DetectedSpec::V3_1(v3_1::spec::Spec::from(v30)))
            }
            (DetectedSpec::V2(s), SpecVersion::V3_2) => {
                let v30 = v3_0::spec::Spec::from(s);
                let v31 = v3_1::spec::Spec::from(v30);
                Ok(DetectedSpec::V3_2(v3_2::spec::Spec::from(v31)))
            }
            (DetectedSpec::V3_0(s), SpecVersion::V3_1) => {
                Ok(DetectedSpec::V3_1(v3_1::spec::Spec::from(s)))
            }
            (DetectedSpec::V3_0(s), SpecVersion::V3_2) => {
                let v31 = v3_1::spec::Spec::from(s);
                Ok(DetectedSpec::V3_2(v3_2::spec::Spec::from(v31)))
            }
            (DetectedSpec::V3_1(s), SpecVersion::V3_2) => {
                Ok(DetectedSpec::V3_2(v3_2::spec::Spec::from(s)))
            }

            // Downconversions: rejected here as a safety net; the CLI already
            // errors before getting this far.
            (from, to) => bail!(
                "unsupported conversion: {} → {}",
                DetectedSpec::label_of(&from),
                to.label(),
            ),
        }
    }

    /// Convenience: [`convert_to_detected`](Self::convert_to_detected)
    /// followed by [`into_value`](Self::into_value). Used by callers
    /// that don't need the intermediate typed `DetectedSpec` (e.g.
    /// `preview`, which only needs the final JSON to feed the
    /// renderer).
    pub fn convert_to(self, target: SpecVersion) -> Result<Value> {
        self.convert_to_detected(target)?.into_value()
    }

    /// Merge `other` into `self` in place. Requires both sides to be
    /// the same version — the caller is responsible for converting
    /// merge sources to the target version before calling. Returns
    /// the resulting [`MergeReport`] (success) or a [`MergeError`]
    /// when the underlying `Spec::merge` aborts under
    /// [`MergeOptions::ErrorOnConflict`]. Version mismatches are a
    /// programming error and bail with an `anyhow` context.
    pub fn merge_into(
        &mut self,
        other: DetectedSpec,
        options: enumset::EnumSet<MergeOptions>,
    ) -> Result<Result<MergeReport, MergeError>> {
        let outcome = match (self, other) {
            (DetectedSpec::V2(base), DetectedSpec::V2(incoming)) => base.merge(incoming, options),
            (DetectedSpec::V3_0(base), DetectedSpec::V3_0(incoming)) => {
                base.merge(incoming, options)
            }
            (DetectedSpec::V3_1(base), DetectedSpec::V3_1(incoming)) => {
                base.merge(incoming, options)
            }
            (DetectedSpec::V3_2(base), DetectedSpec::V3_2(incoming)) => {
                base.merge(incoming, options)
            }
            (base, incoming) => bail!(
                "merge requires matching versions: base is {}, incoming is {}",
                base.label(),
                incoming.label(),
            ),
        };
        Ok(outcome)
    }

    /// Run `Spec::collapse` against this spec, lifting every inline
    /// component into its matching root bag and replacing the call
    /// site with a `$ref`. Each version dispatches to its own
    /// `collapse::collapse_spec` implementation; the underlying
    /// per-version `CollapseError` is re-wrapped with an `anyhow`
    /// context tag so users can see which version's pipeline tripped.
    pub fn collapse(&mut self, loader: Option<&mut Loader>) -> Result<()> {
        match self {
            DetectedSpec::V2(s) => s.collapse(loader).context("collapsing OpenAPI 2.0 spec"),
            DetectedSpec::V3_0(s) => s.collapse(loader).context("collapsing OpenAPI 3.0 spec"),
            DetectedSpec::V3_1(s) => s.collapse(loader).context("collapsing OpenAPI 3.1 spec"),
            DetectedSpec::V3_2(s) => s.collapse(loader).context("collapsing OpenAPI 3.2 spec"),
        }
    }

    /// Serialise the wrapped spec to a [`serde_json::Value`] for
    /// printing.
    pub fn into_value(self) -> Result<Value> {
        match self {
            DetectedSpec::V2(s) => to_value("v2", &s),
            DetectedSpec::V3_0(s) => to_value("v3_0", &s),
            DetectedSpec::V3_1(s) => to_value("v3_1", &s),
            DetectedSpec::V3_2(s) => to_value("v3_2", &s),
        }
    }

    fn label_of(spec: &DetectedSpec) -> &'static str {
        spec.label()
    }
}

fn to_value<T: serde::Serialize>(version_tag: &str, spec: &T) -> Result<Value> {
    serde_json::to_value(spec).with_context(|| format!("serialising {version_tag} spec"))
}

/// Parse `raw` into a `serde_json::Value`. Format selection is by `is_yaml`:
/// YAML is parsed with `serde_yaml_ng`, otherwise `serde_json`.
pub fn parse_value(raw: &str, is_yaml: bool) -> Result<Value> {
    if is_yaml {
        serde_yaml_ng::from_str(raw).context("parsing YAML")
    } else {
        serde_json::from_str(raw).context("parsing JSON")
    }
}

/// Detect the spec version from a parsed `Value` (looking at the `openapi` or
/// `swagger` field) and re-deserialise into the matching `Spec` type. If
/// `forced` is provided, skip detection and deserialise as that version.
pub fn detect_or_use(forced: Option<SpecVersion>, value: Value) -> Result<DetectedSpec> {
    let version = match forced {
        Some(v) => v,
        None => detect(&value)?,
    };
    parse_as(version, value)
}

fn detect(value: &Value) -> Result<SpecVersion> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("spec must be an object at the top level"))?;

    if let Some(swagger) = obj.get("swagger").and_then(|v| v.as_str()) {
        let (major, _) = parse_version(swagger)
            .ok_or_else(|| anyhow!("unsupported swagger version: {swagger}"))?;
        if major == 2 {
            return Ok(SpecVersion::V2);
        }
        bail!("unsupported swagger version: {swagger}");
    }

    if let Some(openapi) = obj.get("openapi").and_then(|v| v.as_str()) {
        let (major, minor) = parse_version(openapi)
            .ok_or_else(|| anyhow!("unsupported openapi version: {openapi}"))?;
        return match (major, minor) {
            (3, 0) => Ok(SpecVersion::V3_0),
            (3, 1) => Ok(SpecVersion::V3_1),
            (3, 2) => Ok(SpecVersion::V3_2),
            _ => Err(anyhow!("unsupported openapi version: {openapi}")),
        };
    }

    bail!("could not detect spec version: no `openapi` or `swagger` field at top level")
}

/// Parse the leading `<major>.<minor>` of a version string like
/// `"3.2.0"` / `"3.10.0-beta.1"` into `(3, 2)` / `(3, 10)`. Returns
/// `None` if the input doesn't start with a `<int>.<int>` pair.
fn parse_version(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor_raw = parts.next()?;
    let minor_end = minor_raw
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(minor_raw.len());
    let minor = minor_raw.get(..minor_end)?.parse::<u32>().ok()?;
    Some((major, minor))
}

fn parse_as(version: SpecVersion, value: Value) -> Result<DetectedSpec> {
    Ok(match version {
        SpecVersion::V2 => {
            DetectedSpec::V2(serde_json::from_value(value).context("deserialising as OpenAPI 2.0")?)
        }
        SpecVersion::V3_0 => DetectedSpec::V3_0(
            serde_json::from_value(value).context("deserialising as OpenAPI 3.0")?,
        ),
        SpecVersion::V3_1 => DetectedSpec::V3_1(
            serde_json::from_value(value).context("deserialising as OpenAPI 3.1")?,
        ),
        SpecVersion::V3_2 => DetectedSpec::V3_2(
            serde_json::from_value(value).context("deserialising as OpenAPI 3.2")?,
        ),
    })
}

/// Heuristic: does this path look like a YAML file?
pub fn path_looks_like_yaml(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("yaml" | "yml" | "YAML" | "YML"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_extracts_major_minor() {
        assert_eq!(parse_version("3.2.0"), Some((3, 2)));
        assert_eq!(parse_version("3.10"), Some((3, 10)));
        assert_eq!(parse_version("3.10.0-rc1"), Some((3, 10)));
        assert_eq!(parse_version("2.0"), Some((2, 0)));
        assert_eq!(parse_version("12.345.6"), Some((12, 345)));
    }

    #[test]
    fn parse_version_rejects_malformed() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("3"), None);
        assert_eq!(parse_version("v3.2"), None);
        assert_eq!(parse_version("3.x"), None);
    }

    #[test]
    fn detect_distinguishes_3_1_from_3_10() {
        let raw_3_1 = r#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{}}"#;
        let raw_3_10 = r#"{"openapi":"3.10.0","info":{"title":"x","version":"1"},"paths":{}}"#;
        let v31: Value = serde_json::from_str(raw_3_1).unwrap();
        let v310: Value = serde_json::from_str(raw_3_10).unwrap();
        assert!(matches!(detect(&v31).unwrap(), SpecVersion::V3_1));
        let err = detect(&v310).unwrap_err().to_string();
        assert!(err.contains("unsupported openapi version"), "got: {err}");
    }

    #[test]
    fn detect_v2_via_swagger_field() {
        let v: Value = serde_json::from_str(r#"{"swagger":"2.0"}"#).unwrap();
        assert_eq!(detect(&v).unwrap(), SpecVersion::V2);
    }

    #[test]
    fn detect_v3_0_v3_1_v3_2_via_openapi_field() {
        let cases = [
            (r#"{"openapi":"3.0.4"}"#, SpecVersion::V3_0),
            (r#"{"openapi":"3.1.2"}"#, SpecVersion::V3_1),
            (r#"{"openapi":"3.2.0"}"#, SpecVersion::V3_2),
        ];
        for (raw, expected) in cases {
            let v: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(detect(&v).unwrap(), expected, "input was {raw}");
        }
    }

    #[test]
    fn detect_rejects_unsupported_swagger_major() {
        let v: Value = serde_json::from_str(r#"{"swagger":"1.2"}"#).unwrap();
        let err = detect(&v).unwrap_err().to_string();
        assert!(err.contains("unsupported swagger version"), "got: {err}",);
    }

    #[test]
    fn detect_rejects_malformed_swagger() {
        let v: Value = serde_json::from_str(r#"{"swagger":"not-a-version"}"#).unwrap();
        let err = detect(&v).unwrap_err().to_string();
        assert!(err.contains("unsupported swagger version"), "got: {err}",);
    }

    #[test]
    fn detect_rejects_unsupported_openapi_major() {
        let v: Value = serde_json::from_str(r#"{"openapi":"4.0.0"}"#).unwrap();
        let err = detect(&v).unwrap_err().to_string();
        assert!(err.contains("unsupported openapi version"), "got: {err}",);
    }

    #[test]
    fn detect_rejects_document_without_version_field() {
        let v: Value = serde_json::from_str(r#"{"info":{"title":"x"}}"#).unwrap();
        let err = detect(&v).unwrap_err().to_string();
        assert!(err.contains("could not detect spec version"), "got: {err}",);
    }

    #[test]
    fn detect_rejects_non_object_root() {
        let v: Value = serde_json::from_str(r#"[]"#).unwrap();
        let err = detect(&v).unwrap_err().to_string();
        assert!(err.contains("object at the top level"), "got: {err}");
    }

    #[test]
    fn parse_value_handles_both_formats() {
        let json = r#"{"hello":"world"}"#;
        let yaml = "hello: world\n";
        assert_eq!(
            parse_value(json, false).unwrap(),
            serde_json::json!({"hello":"world"})
        );
        assert_eq!(
            parse_value(yaml, true).unwrap(),
            serde_json::json!({"hello":"world"})
        );
    }

    #[test]
    fn path_looks_like_yaml_sniffs_extensions() {
        use std::path::Path;
        assert!(path_looks_like_yaml(Path::new("spec.yaml")));
        assert!(path_looks_like_yaml(Path::new("spec.yml")));
        assert!(path_looks_like_yaml(Path::new("spec.YAML")));
        assert!(!path_looks_like_yaml(Path::new("spec.json")));
        assert!(!path_looks_like_yaml(Path::new("spec")));
    }

    #[test]
    fn parse_value_yaml_format_surfaces_yaml_parser_error() {
        // Tab-indented YAML is forbidden by the YAML 1.2 grammar.
        let err = parse_value("key:\n\tvalue: oops\n", true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("parsing YAML"), "got: {err}");
    }

    #[test]
    fn parse_value_json_format_surfaces_json_parser_error() {
        let err = parse_value("@@@ not json", false).unwrap_err().to_string();
        assert!(err.contains("parsing JSON"), "got: {err}");
    }

    #[test]
    fn detect_or_use_forced_skips_detection_and_uses_target() {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        let s = detect_or_use(Some(SpecVersion::V3_2), v).unwrap();
        assert_eq!(s.version(), SpecVersion::V3_2);
        assert_eq!(s.label(), "OpenAPI 3.2");
    }

    #[test]
    fn detect_or_use_auto_detects_when_unforced() {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.0.4","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        let s = detect_or_use(None, v).unwrap();
        assert_eq!(s.version(), SpecVersion::V3_0);
    }

    /// Helper: assert the `openapi` field starts with `<major>.<minor>`.
    /// Patch bumps within the same minor (e.g. roas's v3.1 currently emits
    /// `3.1.2`) shouldn't churn the test surface.
    fn assert_major_minor(out: &Value, want_prefix: &str) {
        let got = out["openapi"].as_str().expect("openapi must be a string");
        assert!(
            got.starts_with(want_prefix),
            "expected openapi to start with {want_prefix}, got {got}",
        );
    }

    #[test]
    fn convert_to_same_version_serialises_as_is() {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        let s = detect_or_use(None, v).unwrap();
        let out = s.convert_to(SpecVersion::V3_2).unwrap();
        assert_major_minor(&out, "3.2");
    }

    #[test]
    fn convert_to_chains_through_intermediate_versions() {
        // v2 → v3_2 must walk through v3_0 and v3_1.
        let v: Value = serde_json::from_str(
            r#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        let s = detect_or_use(None, v).unwrap();
        let out = s.convert_to(SpecVersion::V3_2).unwrap();
        assert_major_minor(&out, "3.2");
    }

    #[test]
    fn convert_to_single_step_upconvert_changes_minor() {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.0.4","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        let s = detect_or_use(None, v).unwrap();
        let out = s.convert_to(SpecVersion::V3_1).unwrap();
        assert_major_minor(&out, "3.1");
    }

    #[test]
    fn spec_version_label_round_trip() {
        for (v, expected) in [
            (SpecVersion::V2, "OpenAPI 2.0"),
            (SpecVersion::V3_0, "OpenAPI 3.0"),
            (SpecVersion::V3_1, "OpenAPI 3.1"),
            (SpecVersion::V3_2, "OpenAPI 3.2"),
        ] {
            assert_eq!(v.label(), expected);
        }
    }

    // Helpers used by the per-version-arm tests below — minimal in-memory
    // specs we round-trip through `detect_or_use`.
    fn v2_spec() -> DetectedSpec {
        let v: Value = serde_json::from_str(
            r#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        detect_or_use(None, v).unwrap()
    }
    fn v3_0_spec() -> DetectedSpec {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.0.4","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        detect_or_use(None, v).unwrap()
    }
    fn v3_1_spec() -> DetectedSpec {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        detect_or_use(None, v).unwrap()
    }
    fn v3_2_spec() -> DetectedSpec {
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        detect_or_use(None, v).unwrap()
    }

    #[test]
    fn detected_spec_version_and_label_cover_all_arms() {
        assert_eq!(v2_spec().version(), SpecVersion::V2);
        assert_eq!(v2_spec().label(), "OpenAPI 2.0");
        assert_eq!(v3_0_spec().version(), SpecVersion::V3_0);
        assert_eq!(v3_0_spec().label(), "OpenAPI 3.0");
        assert_eq!(v3_1_spec().version(), SpecVersion::V3_1);
        assert_eq!(v3_1_spec().label(), "OpenAPI 3.1");
        assert_eq!(v3_2_spec().version(), SpecVersion::V3_2);
        assert_eq!(v3_2_spec().label(), "OpenAPI 3.2");
    }

    #[test]
    fn detected_spec_validate_dispatches_to_each_version() {
        // Reasonable defaults for v2.0 paths — empty paths object is legal.
        // Each call exercises one arm of `DetectedSpec::validate`.
        let opts = enumset::EnumSet::<Options>::new();
        v2_spec().validate(opts, None).unwrap();
        v3_0_spec().validate(opts, None).unwrap();
        v3_1_spec().validate(opts, None).unwrap();
        v3_2_spec().validate(opts, None).unwrap();
    }

    #[test]
    fn convert_to_v2_same_version_noop_serialises_swagger() {
        let out = v2_spec().convert_to(SpecVersion::V2).unwrap();
        assert_eq!(out["swagger"], "2.0");
    }

    #[test]
    fn convert_to_v3_0_same_version_noop() {
        let out = v3_0_spec().convert_to(SpecVersion::V3_0).unwrap();
        assert_major_minor(&out, "3.0");
    }

    #[test]
    fn convert_to_v3_1_same_version_noop() {
        let out = v3_1_spec().convert_to(SpecVersion::V3_1).unwrap();
        assert_major_minor(&out, "3.1");
    }

    #[test]
    fn convert_to_v2_to_v3_0_single_step() {
        let out = v2_spec().convert_to(SpecVersion::V3_0).unwrap();
        assert_major_minor(&out, "3.0");
    }

    #[test]
    fn convert_to_v2_to_v3_1_chains_through_v3_0() {
        let out = v2_spec().convert_to(SpecVersion::V3_1).unwrap();
        assert_major_minor(&out, "3.1");
    }

    #[test]
    fn convert_to_v3_0_to_v3_2_chains_through_v3_1() {
        let out = v3_0_spec().convert_to(SpecVersion::V3_2).unwrap();
        assert_major_minor(&out, "3.2");
    }

    #[test]
    fn convert_to_v3_1_to_v3_2_single_step() {
        let out = v3_1_spec().convert_to(SpecVersion::V3_2).unwrap();
        assert_major_minor(&out, "3.2");
    }

    #[test]
    fn convert_to_rejects_downconversion_safety_net() {
        // The CLI rejects downconversion before calling `convert_to`; this is
        // the safety-net branch inside `convert_to` itself, exercised here by
        // directly asking for a v3.2 → v2 conversion.
        let err = v3_2_spec()
            .convert_to(SpecVersion::V2)
            .expect_err("downconversion must error")
            .to_string();
        assert!(err.contains("unsupported conversion"), "got: {err}",);
    }

    #[test]
    fn parse_as_errors_when_forced_version_mismatches_doc() {
        // A v3.2 doc force-parsed as v2 must fail at deserialise time with
        // the OpenAPI 2.0 context attached.
        let v: Value = serde_json::from_str(
            r#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        // `Result<DetectedSpec, _>::unwrap_err` would require
        // `DetectedSpec: Debug`; match the result explicitly to avoid that bound.
        let err = match detect_or_use(Some(SpecVersion::V2), v) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected Err, got Ok"),
        };
        assert!(err.contains("deserialising as OpenAPI 2.0"), "got: {err}",);
    }

    #[test]
    fn parse_version_returns_none_when_minor_is_not_parseable_int() {
        // "3.." => minor segment is empty.
        assert_eq!(parse_version("3.."), None);
    }

    // ── DetectedSpec::collapse — dispatch coverage per version. ──────────
    //
    // The collapse machinery itself has dedicated tests in `roas`; here we
    // only confirm the CLI's `match`-over-variants delegates to the right
    // version's implementation and pipes any error through `anyhow`.

    /// Build a spec with one inline titled schema in a response and
    /// run `DetectedSpec::collapse` on it. After collapse, the schema
    /// must have been lifted into the appropriate root bag
    /// (`components.schemas` for v3.x, `definitions` for v2).
    fn collapse_lifts_inline_titled_schema(spec: DetectedSpec, bag_pointer: &str) {
        let mut spec = spec;
        spec.collapse(None).expect("collapse ok");
        let v = spec.into_value().expect("serialize ok");
        let bag = v
            .pointer(bag_pointer)
            .unwrap_or_else(|| panic!("bag `{bag_pointer}` must exist after collapse: {v:#}"));
        let obj = bag
            .as_object()
            .unwrap_or_else(|| panic!("`{bag_pointer}` must be an object: {bag:#}"));
        assert!(
            !obj.is_empty(),
            "collapse must have lifted at least one entry into {bag_pointer}: {bag:#}",
        );
    }

    #[test]
    fn collapse_dispatches_to_v2_definitions() {
        let v: Value = serde_json::from_str(
            r#"{
                "swagger":"2.0",
                "info":{"title":"x","version":"1"},
                "paths":{
                    "/x":{
                        "post":{
                            "operationId":"x",
                            "parameters":[
                                {"in":"body","name":"body","schema":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}}
                            ],
                            "responses":{"200":{"description":"ok"}}
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let spec = detect_or_use(None, v).unwrap();
        collapse_lifts_inline_titled_schema(spec, "/definitions");
    }

    #[test]
    fn collapse_dispatches_to_v3_0_components() {
        let v: Value = serde_json::from_str(
            r#"{
                "openapi":"3.0.4",
                "info":{"title":"x","version":"1"},
                "paths":{
                    "/pets":{
                        "get":{
                            "operationId":"x",
                            "responses":{
                                "200":{
                                    "description":"ok",
                                    "content":{"application/json":{"schema":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}}}
                                }
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let spec = detect_or_use(None, v).unwrap();
        collapse_lifts_inline_titled_schema(spec, "/components/schemas");
    }

    #[test]
    fn collapse_dispatches_to_v3_1_components() {
        let v: Value = serde_json::from_str(
            r#"{
                "openapi":"3.1.0",
                "info":{"title":"x","version":"1"},
                "paths":{
                    "/pets":{
                        "get":{
                            "operationId":"x",
                            "responses":{
                                "200":{
                                    "description":"ok",
                                    "content":{"application/json":{"schema":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}}}
                                }
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let spec = detect_or_use(None, v).unwrap();
        collapse_lifts_inline_titled_schema(spec, "/components/schemas");
    }

    #[test]
    fn collapse_dispatches_to_v3_2_components() {
        let v: Value = serde_json::from_str(
            r#"{
                "openapi":"3.2.0",
                "info":{"title":"x","version":"1"},
                "paths":{
                    "/pets":{
                        "get":{
                            "operationId":"x",
                            "responses":{
                                "200":{
                                    "description":"ok",
                                    "content":{"application/json":{"schema":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}}}
                                }
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let spec = detect_or_use(None, v).unwrap();
        collapse_lifts_inline_titled_schema(spec, "/components/schemas");
    }

    #[test]
    fn convert_to_detected_chains_through_intermediate_versions() {
        // v2 → v3_2 must walk through v3_0 and v3_1, returning a typed
        // `DetectedSpec::V3_2` (not just a Value).
        let v: Value = serde_json::from_str(
            r#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .unwrap();
        let detected = detect_or_use(None, v).unwrap();
        let converted = detected.convert_to_detected(SpecVersion::V3_2).unwrap();
        assert_eq!(converted.version(), SpecVersion::V3_2);
    }

    #[test]
    fn collapse_with_loader_resolves_external_file_ref() {
        // Write a fragment to a temp file, then build a spec that
        // references it via a `file://` `$ref`. With a `Loader` carrying
        // a `FileFetcher`, `collapse` must fetch the fragment, lift it
        // into `components.schemas`, and rewrite the call site as a
        // local `$ref` — proving the loader is actually piped through
        // the dispatch (not silently ignored).
        use roas_file_fetcher::FileFetcher;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let frag_path = std::env::temp_dir().join(format!(
            "roas-cli-collapse-frag-{}-{n}.json",
            std::process::id(),
        ));
        std::fs::write(
            &frag_path,
            br#"{"Pet":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}}"#,
        )
        .expect("write fragment");
        // Cleanup helper so the test doesn't leave stray temp files
        // behind on panic.
        struct CleanupFile(std::path::PathBuf);
        impl Drop for CleanupFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = CleanupFile(frag_path.clone());

        // Construct the `file://` URL by hand to avoid a dev-dep on
        // `url`. POSIX absolute paths map directly; the CI runner is
        // Linux/macOS only.
        let frag_url = format!("file://{}", frag_path.display());
        let spec_json = format!(
            r#"{{
                "openapi":"3.2.0",
                "info":{{"title":"x","version":"1"}},
                "paths":{{
                    "/pets":{{
                        "get":{{
                            "operationId":"x",
                            "responses":{{
                                "200":{{
                                    "description":"ok",
                                    "content":{{"application/json":{{"schema":{{"$ref":"{frag_url}#/Pet"}}}}}}
                                }}
                            }}
                        }}
                    }}
                }}
            }}"#
        );
        let v: Value = serde_json::from_str(&spec_json).expect("parse spec");
        let mut detected = detect_or_use(None, v).expect("detect");

        let mut loader = Loader::new();
        loader.register_fetcher("file://", FileFetcher::new());

        detected.collapse(Some(&mut loader)).expect("collapse ok");
        let out = detected.into_value().expect("serialize");

        // The load-bearing assertion: `components.schemas.Pet` only
        // exists if the loader actually fetched the external fragment
        // and the collapse pipeline lifted it. Without the loader
        // (`None` path), the `file://` ref would be left untouched.
        assert!(
            out.pointer("/components/schemas/Pet").is_some(),
            "external fragment must lift into components.schemas.Pet: {out:#}",
        );
        // And the original `file://` URL must be gone from the spec —
        // every reachable `$ref` is now internal.
        let s = serde_json::to_string(&out).unwrap();
        assert!(
            !s.contains("file://"),
            "no `file://` ref should remain after collapse: {s}",
        );
    }
}
