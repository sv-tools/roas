//! Where a description comes from, and the one type that holds it.

use crate::diagnostic::Diagnostic;
use crate::report;
use roas::{v2, v3_0, v3_1, v3_2};
use std::fmt;
use thiserror::Error;
use url::Url;

/// The bytes or value a description arrives as.
///
/// The variant records *where the numbers came from*, because exactness
/// cannot be reconstructed after the fact: only [`Input::Json`], which
/// this crate parses itself, can yield [`Fidelity::Exact`]. The YAML
/// reader may round through `f64`, and a ready-made `Value` has unknown
/// provenance whatever its caller believes about it. There is
/// deliberately no way to assert exactness on a `Value`.
#[derive(Debug, Clone)]
pub enum Input {
    /// JSON text. The only way to obtain exact numbers.
    Json(Vec<u8>),
    /// YAML text, read with `serde-saphyr`. An integer within the 64-bit
    /// range is kept exactly; a fraction, an exponent, or an untagged
    /// decimal integer past `u64::MAX` or below `i64::MIN` is read through
    /// `f64`.
    Yaml(Vec<u8>),
    /// An already-parsed value of unknown provenance.
    Value(serde_json::Value),
}

/// How faithfully the numbers in a document reflect what was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fidelity {
    /// Every number is the literal that was written. Requires the
    /// `exact-numbers` feature and JSON input; without the feature even
    /// JSON input is [`Fidelity::F64`], since `serde_json` rounds.
    Exact,
    /// Numbers went through `f64` on the way in.
    F64,
    /// The provenance is not known.
    Unknown,
}

/// The OpenAPI version a description was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceVersion {
    V2,
    V3_0,
    V3_1,
    V3_2,
}

impl SourceVersion {
    fn detect(raw: &serde_json::Value) -> Result<Self, SourceError> {
        let Some(object) = raw.as_object() else {
            return Err(SourceError::NotAnObject);
        };
        if let Some(swagger) = object.get("swagger") {
            return match swagger.as_str() {
                Some(v) if v.starts_with("2.0") => Ok(SourceVersion::V2),
                _ => Err(SourceError::UnknownVersion {
                    field: "swagger",
                    value: swagger.to_string(),
                }),
            };
        }
        if let Some(openapi) = object.get("openapi") {
            return match openapi.as_str() {
                Some(v) if v.starts_with("3.0.") || v == "3.0" => Ok(SourceVersion::V3_0),
                Some(v) if v.starts_with("3.1.") || v == "3.1" => Ok(SourceVersion::V3_1),
                Some(v) if v.starts_with("3.2.") || v == "3.2" => Ok(SourceVersion::V3_2),
                _ => Err(SourceError::UnknownVersion {
                    field: "openapi",
                    value: openapi.to_string(),
                }),
            };
        }
        Err(SourceError::NoVersion)
    }
}

impl fmt::Display for SourceVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SourceVersion::V2 => "OpenAPI 2.0",
            SourceVersion::V3_0 => "OpenAPI 3.0",
            SourceVersion::V3_1 => "OpenAPI 3.1",
            SourceVersion::V3_2 => "OpenAPI 3.2",
        })
    }
}

/// Why a description could not become a [`SourceDocument`].
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("the input is not valid JSON")]
    Json(#[source] serde_json::Error),
    /// The YAML reader rejected the input: a syntax error, a duplicate
    /// mapping key, or a document past the reader's size and alias budget.
    #[error("the input is not valid YAML")]
    Yaml(#[source] serde_saphyr::Error),
    #[error("a description must be a JSON object")]
    NotAnObject,
    #[error("neither `swagger` nor `openapi` is present, so the version is unknown")]
    NoVersion,
    #[error("`{field}: {value}` is not a supported version")]
    UnknownVersion { field: &'static str, value: String },
    #[error("the input is not a valid {version} description")]
    Parse {
        version: SourceVersion,
        #[source]
        source: serde_json::Error,
    },
}

/// A description, parsed and normalized to OpenAPI 3.2, together with
/// the source it came from.
///
/// The raw and typed views are the same document by construction: the
/// fields are private and [`SourceDocument::parse`] is the only way to
/// make one, so a caller cannot pair the raw JSON of one document with
/// the typed model of another. The raw view keeps what the typed model
/// cannot express — whether `type` was written at all, the exact text a
/// diagnostic should quote — and every diagnostic pointer indexes it,
/// in the version it was written in.
#[derive(Debug, Clone)]
pub struct SourceDocument {
    uri: Url,
    version: SourceVersion,
    fidelity: Fidelity,
    raw: serde_json::Value,
    spec: v3_2::spec::Spec,
    normalization: Vec<Diagnostic>,
}

impl SourceDocument {
    /// Detect the version, parse the typed model at that version, record
    /// what upconverting to 3.2 loses, and upconvert.
    ///
    /// `uri` identifies the document in every [`SchemaId`](crate::SchemaId)
    /// it produces. Input with no natural URI takes a synthetic one, by
    /// convention `stdin:///`.
    pub fn parse(input: Input, uri: Url) -> Result<SourceDocument, SourceError> {
        let (raw, fidelity) = match input {
            Input::Json(bytes) => {
                let value = serde_json::from_slice(&bytes).map_err(SourceError::Json)?;
                let fidelity = if cfg!(feature = "exact-numbers") {
                    Fidelity::Exact
                } else {
                    Fidelity::F64
                };
                (value, fidelity)
            }
            Input::Yaml(bytes) => (
                parse_yaml(&bytes).map_err(SourceError::Yaml)?,
                Fidelity::F64,
            ),
            Input::Value(value) => (value, Fidelity::Unknown),
        };
        let version = SourceVersion::detect(&raw)?;
        let parse = |source| SourceError::Parse { version, source };
        let mut normalization = Vec::new();
        let spec = match version {
            SourceVersion::V2 => {
                let spec: v2::spec::Spec = serde_json::from_value(raw.clone()).map_err(parse)?;
                report::v2_losses(&raw, &uri, &mut normalization);
                let v30 = v3_0::spec::Spec::from(spec);
                let v31 = v3_1::spec::Spec::from(v30);
                v3_2::spec::Spec::from(v31)
            }
            SourceVersion::V3_0 => {
                let spec: v3_0::spec::Spec = serde_json::from_value(raw.clone()).map_err(parse)?;
                let v31 = v3_1::spec::Spec::from(spec);
                v3_2::spec::Spec::from(v31)
            }
            SourceVersion::V3_1 => {
                let spec: v3_1::spec::Spec = serde_json::from_value(raw.clone()).map_err(parse)?;
                v3_2::spec::Spec::from(spec)
            }
            SourceVersion::V3_2 => serde_json::from_value(raw.clone()).map_err(parse)?,
        };
        Ok(SourceDocument {
            uri,
            version,
            fidelity,
            raw,
            spec,
            normalization,
        })
    }

    /// The document's identity.
    pub fn uri(&self) -> &Url {
        &self.uri
    }

    /// The version the description was written in — not the version
    /// [`spec`](Self::spec) is at, which is always 3.2.
    pub fn version(&self) -> SourceVersion {
        self.version
    }

    /// How faithfully the numbers reflect what was written.
    pub fn fidelity(&self) -> Fidelity {
        self.fidelity
    }

    /// The description exactly as given, before any conversion. Every
    /// diagnostic pointer indexes this value.
    pub fn raw(&self) -> &serde_json::Value {
        &self.raw
    }

    /// The description normalized to OpenAPI 3.2.
    pub fn spec(&self) -> &v3_2::spec::Spec {
        &self.spec
    }

    /// What normalizing to 3.2 lost. Recorded by the source-version
    /// adapter before the lossy conversion runs, since a 2.0
    /// `discriminator` that 3.0 drops cannot be recovered afterwards.
    pub fn normalization(&self) -> &[Diagnostic] {
        &self.normalization
    }
}

/// Read YAML with the options every roas crate uses, so a document means
/// the same thing here as in the fetchers and the CLI.
///
/// - YAML 1.2 booleans only: `yes` / `on` / `no` / `off` stay strings.
/// - Integer spellings `017` and `1_000` stay strings rather than becoming numbers.
/// - `!!binary` scalars are read as their base64 text, not decoded.
/// - `.inf` / `.nan` pass through (a `serde_json::Value` holds them as `null`).
/// - `<<: *anchor` merge keys are expanded.
/// - A duplicate mapping key is an error rather than a silent last-wins.
///
/// A [`serde_saphyr::Budget`] stays on, raised so large specs fit: it rejects
/// alias bombs, nesting past 64 levels, and documents past 10M nodes or
/// 64 MiB of scalar text.
fn parse_yaml(bytes: &[u8]) -> Result<serde_json::Value, serde_saphyr::Error> {
    let options = serde_saphyr::options! {
        // Same options in every roas crate; keep the copies in step
        // (`grep -rn 'strict_booleans: true' crates`). The budget is raised
        // from the defaults (250k nodes, 1M events, 50k aliases, 50k anchors),
        // which reject ordinary large or generated specs. Depth stays at 64: deeper nesting overflows a 2 MiB
        // thread stack in debug builds. The alias-to-anchor ratio check is off:
        // it rejects any document reusing one anchor 100+ times, a common
        // pattern in specs. Alias replay is raised to 10M events so a large
        // schema can be aliased thousands of times; the 1M cap on events
        // recorded under anchors still stops an alias bomb. `!!binary` is
        // read as its text, as before, not decoded.
        budget: serde_saphyr::budget! {
            max_nodes: 10_000_000,
            max_events: 25_000_000,
            max_aliases: 1_000_000,
            max_anchors: 1_000_000,
            enforce_alias_anchor_ratio: false,
        },
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: 10_000_000,
        },
        strict_booleans: true,
        ignore_binary_tag_for_string: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
    };
    serde_saphyr::from_slice_with_options(bytes, options)
}

#[cfg(test)]
mod yaml_tests {
    use super::*;
    use serde_json::{Value, json};

    fn parse(yaml: &str) -> Result<Value, String> {
        parse_yaml(yaml.as_bytes()).map_err(|e| e.to_string())
    }

    /// Each scalar whose meaning depends on the YAML options, next to what
    /// it must read as.
    #[test]
    fn yaml_scalars_read_as_yaml_1_2() {
        for (scalar, expected) in [
            ("yes", json!("yes")),
            ("on", json!("on")),
            ("no", json!("no")),
            ("off", json!("off")),
            ("true", json!(true)),
            ("False", json!(false)),
            ("017", json!("017")),
            ("1_000", json!("1_000")),
            ("17", json!(17)),
            ("0x11", json!(17)),
            ("1.5", json!(1.5)),
            (".inf", json!(null)),
            ("3.1.0", json!("3.1.0")),
            ("!!binary SGVsbG8=", json!("SGVsbG8=")),
            ("!!binary /wAB", json!("/wAB")),
        ] {
            assert_eq!(
                parse(&format!("v: {scalar}\n")).unwrap(),
                json!({ "v": expected }),
                "{scalar}",
            );
        }
    }

    #[test]
    fn yaml_merge_keys_expand_through_an_anchor() {
        let yaml = "base: &base { type: string, minLength: 1 }\n\
                    name:\n  <<: *base\n  minLength: 3\n";
        assert_eq!(
            parse(yaml).unwrap(),
            json!({
                "base": { "type": "string", "minLength": 1 },
                "name": { "type": "string", "minLength": 3 },
            }),
        );
    }

    #[test]
    fn yaml_duplicate_key_is_an_error() {
        let err = parse("a: 1\na: 2\n").unwrap_err();
        assert!(err.contains("duplicate mapping key"), "{err}");
    }

    #[test]
    fn yaml_alias_bomb_is_rejected_by_the_budget() {
        // Billion laughs: each level aliases the previous one ten times, so
        // full expansion is 10^7 scalars. The anchor-replay limits stop it.
        let mut yaml = String::from("l0: &l0 [lol, lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
        for i in 1..7 {
            let prev = format!("*l{}", i - 1);
            yaml.push_str(&format!("l{i}: &l{i} [{}]\n", vec![prev; 10].join(", ")));
        }
        let err = parse(&yaml).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }

    /// One shared anchor reused many times, as specs do for common
    /// responses, is not a bomb.
    #[test]
    fn yaml_anchor_reused_many_times_is_accepted() {
        // 40 properties aliased 6,000 times replays over a million events.
        let props: String = (0..40)
            .map(|i| format!("    p{i}: {{ type: string }}\n"))
            .collect();
        let mut yaml = format!("schema: &s\n  type: object\n  properties:\n{props}responses:\n");
        for i in 0..6_000 {
            yaml.push_str(&format!("  r{i}: *s\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(
            value["responses"]["r5999"]["properties"]["p39"]["type"],
            "string"
        );
    }

    /// Past the library's default caps of 50k aliases and 50k anchors,
    /// which generated specs (one anchor per shared object) can reach.
    #[test]
    fn yaml_many_aliases_and_anchors_fit_the_budget() {
        let mut yaml = String::from("shared: &s x\n");
        for i in 0..60_000 {
            yaml.push_str(&format!("a{i}: *s\nb{i}: &b{i} y\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(value["a59999"], "x");
        assert_eq!(value["b59999"], "y");
    }

    #[test]
    fn yaml_nesting_is_capped_at_64_levels() {
        let nested = |depth: usize| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        parse(&nested(64)).unwrap();
        let err = parse(&nested(65)).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }

    /// Past the library's default budget of 250k nodes, which rejects
    /// ordinary large specs.
    #[test]
    fn yaml_large_document_fits_the_budget() {
        let yaml: String = (0..60_000).map(|i| format!("k{i}: [a, b, c]\n")).collect();
        let value = parse(&yaml).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 60_000);
    }
}
