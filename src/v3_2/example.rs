//! Example object.

use crate::common::helpers::validate_optional_uri;
use crate::v3_2::spec::Spec;
use crate::validation::{Context, PushError, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Example object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Example {
    /// Short description for the example.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Long description for the example.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Embedded literal example, semantically equivalent to the parent
    /// media type.
    /// `value`, `serializedValue`, `dataValue`, and `externalValue` are
    /// pairwise mutually exclusive.
    /// To represent examples of media types that cannot naturally
    /// represented in JSON or YAML, use a string value to contain the
    /// example, escaping where necessary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,

    /// Pre-serialized form of the example, as it would appear on the wire
    /// (added in OAS 3.2). Mutually exclusive with `value`, `dataValue`,
    /// and `externalValue`.
    #[serde(rename = "serializedValue")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serialized_value: Option<String>,

    /// Structured / data-shape form of the example (added in OAS 3.2).
    /// Useful when the example is a non-JSON-native value such as binary
    /// data described by a Schema. Mutually exclusive with `value`,
    /// `serializedValue`, and `externalValue`.
    #[serde(rename = "dataValue")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_value: Option<serde_json::Value>,

    /// A URL that points to the literal example.
    /// This provides the capability to reference examples that cannot easily
    /// be included in JSON or YAML documents.
    /// Mutually exclusive with `value`, `serializedValue`, and `dataValue`.
    #[serde(rename = "externalValue")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_value: Option<String>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Example {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        // Per the OAS 3.2 JSON Schema, the `not.required` constraints are:
        // value⊕externalValue, value⊕dataValue, value⊕serializedValue, and
        // serializedValue⊕externalValue. dataValue may coexist with
        // serializedValue or externalValue.
        let pairs: &[(&str, bool, &str, bool)] = &[
            (
                "value",
                self.value.is_some(),
                "externalValue",
                self.external_value.is_some(),
            ),
            (
                "value",
                self.value.is_some(),
                "dataValue",
                self.data_value.is_some(),
            ),
            (
                "value",
                self.value.is_some(),
                "serializedValue",
                self.serialized_value.is_some(),
            ),
            (
                "serializedValue",
                self.serialized_value.is_some(),
                "externalValue",
                self.external_value.is_some(),
            ),
        ];
        for (a_name, a, b_name, b) in pairs {
            if *a && *b {
                ctx.error(
                    path.clone(),
                    format_args!("{a_name} and {b_name} are mutually exclusive"),
                );
            }
        }
        validate_optional_uri(&self.external_value, ctx, format!("{path}.externalValue"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Context;
    use crate::validation::Options;
    use crate::validation::ValidationErrorsExt;
    use serde_json::json;

    #[test]
    fn xor_value_and_external_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            value: Some(json!(1)),
            external_value: Some("https://example.com/x.json".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors.mentions("mutually exclusive"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn external_value_uri_reference_validated() {
        // Per OAS 3.2 the schema gives `externalValue` `format: uri-reference`,
        // so relative paths and non-HTTP schemes pass while whitespace /
        // control-char garbage fails.
        let spec = Spec::default();

        // urn: + relative path: accepted.
        for ok in ["./fixtures/example.json", "urn:example:my-example"] {
            let mut ctx = Context::new(&spec, Options::new());
            Example {
                external_value: Some(ok.to_owned()),
                ..Default::default()
            }
            .validate_with_context(&mut ctx, "ex".into());
            assert!(
                ctx.errors.is_empty(),
                "uri-reference `{ok}` should pass: {:?}",
                ctx.errors
            );
        }

        // Whitespace garbage: rejected.
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            external_value: Some("not a uri".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors.mentions("must be a valid URI"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn data_value_serialized_value_round_trip() {
        // OAS 3.2: dataValue (Any) and serializedValue (string) round-trip
        // through their typed fields, separately from `value`.
        let v = serde_json::json!({
            "summary": "structured",
            "dataValue": {"id": 1, "name": "spot"}
        });
        let ex: Example = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(
            ex.data_value,
            Some(serde_json::json!({"id": 1, "name": "spot"}))
        );
        assert_eq!(serde_json::to_value(&ex).unwrap(), v);

        let v = serde_json::json!({
            "serializedValue": "id=1&name=spot"
        });
        let ex: Example = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(ex.serialized_value.as_deref(), Some("id=1&name=spot"));
        assert_eq!(serde_json::to_value(&ex).unwrap(), v);
    }

    #[test]
    fn schema_pairwise_mutex_rules() {
        // value⊕serializedValue, value⊕dataValue, value⊕externalValue,
        // serializedValue⊕externalValue. dataValue+serializedValue and
        // dataValue+externalValue are PERMITTED per the OAS 3.2 JSON
        // Schema.
        let spec = Spec::default();

        // value+serializedValue ⇒ flagged.
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            value: Some(json!(1)),
            serialized_value: Some("1".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("value and serializedValue are mutually exclusive")),
            "errors: {:?}",
            ctx.errors
        );

        // serializedValue+externalValue ⇒ flagged.
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            serialized_value: Some("1".into()),
            external_value: Some("https://example.com/x".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("serializedValue and externalValue are mutually exclusive")),
            "errors: {:?}",
            ctx.errors
        );

        // dataValue+serializedValue ⇒ accepted.
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            data_value: Some(json!({"k": 1})),
            serialized_value: Some("k=1".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("mutually exclusive")),
            "dataValue + serializedValue should be permitted: {:?}",
            ctx.errors
        );

        // dataValue+externalValue ⇒ accepted.
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            data_value: Some(json!({"k": 1})),
            external_value: Some("https://example.com/x".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("mutually exclusive")),
            "dataValue + externalValue should be permitted: {:?}",
            ctx.errors
        );
    }
}
