//! Example object.

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_optional_url};
use crate::v3_0::spec::Spec;
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

    /// Embedded literal example.
    /// The `value` field and `externalValue` field are mutually exclusive.
    /// To represent examples of media types that cannot naturally represented in JSON or YAML,
    /// use a string value to contain the example, escaping where necessary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,

    /// A URL that points to the literal example.
    /// This provides the capability to reference examples that cannot easily
    /// be included in JSON or YAML documents.
    /// The `value` field and `externalValue` field are mutually exclusive.
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
        if self.value.is_some() && self.external_value.is_some() {
            ctx.error(
                path.clone(),
                "value and externalValue are mutually exclusive",
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
    fn round_trip_with_value() {
        let v = json!({
            "summary": "Cat",
            "description": "An example cat",
            "value": {"name": "Fluffy"}
        });
        let e: Example = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&e).unwrap(), v);
    }

    #[test]
    fn round_trip_with_external_value() {
        let v = json!({
            "externalValue": "https://example.com/example.json"
        });
        let e: Example = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&e).unwrap(), v);
    }

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
            ctx.errors
                .iter()
                .any(|e| e.contains("mutually exclusive")),
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
}
