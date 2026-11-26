//! Errors from converting a description to the next OpenAPI version.
//!
//! Each conversion (`TryFrom<v2::Spec> for v3_0::Spec`, `TryFrom<v3_0::Spec> for v3_1::Spec`,
//! `TryFrom<v3_1::Spec> for v3_2::Spec`) serializes the input, rewrites the JSON for the target
//! version and deserializes the result as the target's model. A rewritten document the target's
//! model cannot hold — a gap in the conversion, not in the input — fails with a
//! [`ConversionError`] instead of panicking.
//!
//! An object other than a schema carries only its `x-` extensions to the next version: keys that
//! the source version does not define on that object are stripped before serializing, because they
//! have no meaning the target can take on (and the target may define a typed field with the same
//! name). Because stripping is silent during `TryFrom`, callers that want unknown keys reported
//! should validate the source document before converting
//! ([`crate::validation::Options::IgnoreUnknownFields`]). Schemas of v3.0 and later keep all of
//! their keywords (`common::schema_keywords`), and vendor-extension payload objects (`x-logo`,
//! `x-codeSamples`, `x-badges`, `x-tagGroups`, `x-servers`) keep all of their fields.

#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
use crate::strip_unknown::StripUnknownFields;
use thiserror::Error;

/// A description could not be converted from one OpenAPI version to the next.
#[derive(Debug, Error)]
#[error("converting an OpenAPI {from} description to {to} failed")]
pub struct ConversionError {
    /// The version converted from (`"v3.0"`).
    pub from: &'static str,
    /// The version converted to (`"v3.1"`).
    pub to: &'static str,
    #[source]
    source: serde_json::Error,
}

/// One conversion's versions, for its errors. Compiled when two adjacent versions are,
/// which is when a conversion exists.
#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
#[derive(Clone, Copy)]
pub(crate) struct Conversion {
    pub(crate) from: &'static str,
    pub(crate) to: &'static str,
}

#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
impl Conversion {
    /// `spec` as JSON, to be rewritten for the target version. Unknown
    /// (non-`x-`) keys on non-schema objects are stripped explicitly before
    /// serializing (see the module documentation).
    pub(crate) fn to_json<S: serde::Serialize + StripUnknownFields>(
        self,
        mut spec: S,
    ) -> Result<serde_json::Value, ConversionError> {
        spec.strip_unknown_fields();
        serde_json::to_value(spec).map_err(|source| self.error(source))
    }

    /// The rewritten JSON as the target version's model.
    pub(crate) fn to_target<T: serde::de::DeserializeOwned>(
        self,
        value: serde_json::Value,
    ) -> Result<T, ConversionError> {
        serde_json::from_value(value).map_err(|source| self.error(source))
    }

    fn error(self, source: serde_json::Error) -> ConversionError {
        ConversionError {
            from: self.from,
            to: self.to,
            source,
        }
    }
}

#[cfg(test)]
#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[derive(Debug, serde::Deserialize)]
    struct Target {
        openapi: String,
    }

    #[test]
    fn json_the_target_cannot_hold_is_an_error_naming_the_conversion() {
        let conversion = Conversion {
            from: "v3.0",
            to: "v3.1",
        };

        let result = conversion.to_target::<Target>(serde_json::json!({ "openapi": 31 }));

        let error = result.unwrap_err();
        assert_eq!(error.from, "v3.0");
        assert_eq!(error.to, "v3.1");
        assert_eq!(
            error.to_string(),
            "converting an OpenAPI v3.0 description to v3.1 failed"
        );
        assert!(
            error.source().is_some(),
            "keeps serde's error as its source"
        );
    }

    #[test]
    fn json_the_target_can_hold_converts() {
        let conversion = Conversion {
            from: "v3.0",
            to: "v3.1",
        };

        let target: Target = conversion
            .to_target(serde_json::json!({ "openapi": "3.1.2" }))
            .unwrap();

        assert_eq!(target.openapi, "3.1.2");
    }
}
