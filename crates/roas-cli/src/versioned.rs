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
    /// converted spec serialised as a [`Value`] for printing.
    ///
    /// Returns an error if the requested conversion is a downconversion
    /// (the caller's responsibility to reject those before calling).
    pub fn convert_to(self, target: SpecVersion) -> Result<Value> {
        match (self, target) {
            (DetectedSpec::V2(s), SpecVersion::V2) => to_value("v2", &s),
            (DetectedSpec::V3_0(s), SpecVersion::V3_0) => to_value("v3_0", &s),
            (DetectedSpec::V3_1(s), SpecVersion::V3_1) => to_value("v3_1", &s),
            (DetectedSpec::V3_2(s), SpecVersion::V3_2) => to_value("v3_2", &s),

            (DetectedSpec::V2(s), SpecVersion::V3_0) => {
                to_value("v3_0", &v3_0::spec::Spec::from(s))
            }
            (DetectedSpec::V2(s), SpecVersion::V3_1) => {
                let v30 = v3_0::spec::Spec::from(s);
                to_value("v3_1", &v3_1::spec::Spec::from(v30))
            }
            (DetectedSpec::V2(s), SpecVersion::V3_2) => {
                let v30 = v3_0::spec::Spec::from(s);
                let v31 = v3_1::spec::Spec::from(v30);
                to_value("v3_2", &v3_2::spec::Spec::from(v31))
            }
            (DetectedSpec::V3_0(s), SpecVersion::V3_1) => {
                to_value("v3_1", &v3_1::spec::Spec::from(s))
            }
            (DetectedSpec::V3_0(s), SpecVersion::V3_2) => {
                let v31 = v3_1::spec::Spec::from(s);
                to_value("v3_2", &v3_2::spec::Spec::from(v31))
            }
            (DetectedSpec::V3_1(s), SpecVersion::V3_2) => {
                to_value("v3_2", &v3_2::spec::Spec::from(s))
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
}
