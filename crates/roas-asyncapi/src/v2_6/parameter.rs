//! AsyncAPI v2.6 `Parameter` object.
//!
//! Per [Parameter Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#parameterObject).
//!
//! A 2.6 parameter carries a full [`SubSchema`], which is the field v3
//! dropped: there a parameter is always a string constrained by `enum`
//! / `default` / `examples`. Converting 2.6 → 3.0 therefore cannot
//! preserve a non-string parameter schema.

use crate::common::runtime_expression;
use crate::v2_6::schema::SubSchema;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Parameter {
    /// A verbose explanation, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Definition of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SubSchema>,

    /// A runtime expression locating the parameter value inside the
    /// message, e.g. `$message.payload#/user/id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Parameter {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(location) = &self.location {
            if location.is_empty() {
                ctx.error_field("location", "must not be empty");
            } else if let Err(err) = runtime_expression::parse(location) {
                ctx.error_field(
                    "location",
                    format!(
                        "`{location}` is not a valid runtime expression: {}",
                        err.message()
                    ),
                );
            }
        }
        if let Some(schema) = &self.schema {
            ctx.in_field("schema", |ctx| schema.validate_with_context(ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_a_parameter_with_a_schema() {
        let value = json!({
            "description": "Street id",
            "schema": { "type": "string", "pattern": "^[0-9]+$" },
            "location": "$message.payload#/streetId"
        });
        let parameter: Parameter = serde_json::from_value(value.clone()).unwrap();
        assert!(parameter.schema.is_some());
        assert_eq!(serde_json::to_value(&parameter).unwrap(), value);

        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.streetId");
        parameter.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn a_parameter_schema_may_be_any_json_schema() {
        // The v3 model cannot express this, which is why the 2.6 → 3.0
        // conversion has to report it.
        let parameter: Parameter = serde_json::from_value(json!({
            "schema": { "type": "integer", "minimum": 1.0 }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        parameter.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn a_boolean_schema_is_accepted() {
        // draft-07 allows `true` / `false` wherever a schema is
        // expected, and a 2.6 parameter takes a full schema.
        for value in [json!({ "schema": true }), json!({ "schema": false })] {
            let parameter: Parameter = serde_json::from_value(value.clone()).unwrap();
            assert!(matches!(parameter.schema, Some(SubSchema::Bool(_))));
            assert_eq!(serde_json::to_value(&parameter).unwrap(), value);
        }
    }

    #[test]
    fn empty_parameter_is_valid() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        Parameter::default().validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn rejects_malformed_and_empty_location() {
        for (location, needle) in [
            ("payload#/id", "is not a valid runtime expression"),
            ("$message.payload", "is not a valid runtime expression"),
            ("", "must not be empty"),
        ] {
            let parameter = Parameter {
                location: Some(location.to_owned()),
                ..Default::default()
            };
            let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
            parameter.validate_with_context(&mut ctx);
            assert!(
                ctx.errors.iter().any(|e| e.contains(needle)),
                "{location:?}: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn nested_schema_errors_carry_their_path() {
        let parameter: Parameter =
            serde_json::from_value(json!({ "schema": { "minItems": 2, "maxItems": 1 } })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        parameter.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("#.parameters.p.schema.minItems")),
            "got: {:?}",
            ctx.errors
        );
    }
}
