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
/// this crate parses itself, can yield [`Fidelity::Exact`]. A YAML
/// reader rounds through `f64`, and a ready-made `Value` has unknown
/// provenance whatever its caller believes about it. There is
/// deliberately no way to assert exactness on a `Value`.
#[derive(Debug, Clone)]
pub enum Input {
    /// JSON text. The only way to obtain exact numbers.
    Json(Vec<u8>),
    /// YAML text. Numbers are read through `f64`.
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
    #[error("the input is not valid YAML")]
    Yaml(#[source] serde_yaml_ng::Error),
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
                serde_yaml_ng::from_slice(&bytes).map_err(SourceError::Yaml)?,
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
