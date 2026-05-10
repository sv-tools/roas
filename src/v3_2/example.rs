//! Example object.

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_optional_url};
use crate::v3_2::spec::Spec;
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
        // OAS 3.2: at most one of `value`, `serializedValue`, `dataValue`,
        // `externalValue` may be present.
        let present: Vec<&str> = [
            ("value", self.value.is_some()),
            ("serializedValue", self.serialized_value.is_some()),
            ("dataValue", self.data_value.is_some()),
            ("externalValue", self.external_value.is_some()),
        ]
        .iter()
        .filter_map(|(name, p)| if *p { Some(*name) } else { None })
        .collect();
        if present.len() > 1 {
            ctx.error(
                path.clone(),
                format_args!(
                    "{} are mutually exclusive (at most one may be set)",
                    present.join(", ")
                ),
            );
        }
        validate_optional_url(&self.external_value, ctx, format!("{path}.externalValue"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::validation::Options;
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
            ctx.errors.iter().any(|e| e.contains("mutually exclusive")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn external_value_url_validated() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            external_value: Some("not-a-url".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must be a valid URL")),
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
    fn three_way_value_mutex_reports() {
        // value + serializedValue + dataValue all set ⇒ flagged.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Example {
            value: Some(json!(1)),
            serialized_value: Some("1".into()),
            data_value: Some(json!({"k": 1})),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "ex".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("mutually exclusive")
                && e.contains("value")
                && e.contains("serializedValue")
                && e.contains("dataValue")),
            "errors: {:?}",
            ctx.errors
        );
    }
}
