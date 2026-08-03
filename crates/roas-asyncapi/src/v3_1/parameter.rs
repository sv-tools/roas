//! AsyncAPI v3.1 `Parameter` object.
//!
//! Per [Parameter Object](https://www.asyncapi.com/docs/reference/specification/v3.1.0#parameterObject).
//!
//! A channel parameter describes one `{placeholder}` in a channel
//! address. Unlike AsyncAPI 2.x — where a parameter carried a full
//! schema — v3 parameters are always strings, constrained by `enum` /
//! `default` / `examples`.

use crate::common::runtime_expression;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Parameter {
    /// An enumeration of string values to be used if the substitution
    /// options are from a limited set.
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,

    /// The default value to use for substitution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// An optional description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Example parameter values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,

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

        // A default outside the enumeration can never be substituted.
        if let Some(default) = &self.default
            && !self.enum_values.is_empty()
            && !self.enum_values.contains(default)
        {
            ctx.error_field(
                "default",
                format!("`{default}` is not one of the values listed in `enum`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_a_full_parameter() {
        let value = json!({
            "enum": ["1", "2"],
            "default": "1",
            "description": "Street id",
            "examples": ["1"],
            "location": "$message.payload#/streetId"
        });
        let parameter: Parameter = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parameter).unwrap(), value);

        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.streetId");
        parameter.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn empty_parameter_is_valid() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.streetId");
        Parameter::default().validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn rejects_malformed_and_empty_location() {
        let bad: Parameter = serde_json::from_value(json!({ "location": "payload#/id" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        bad.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("is not a valid runtime expression"))
        );

        let empty = Parameter {
            location: Some(String::new()),
            ..Default::default()
        };
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        empty.validate_with_context(&mut ctx);
        assert!(ctx.errors[0] == "#.parameters.p.location: must not be empty");
    }

    #[test]
    fn default_must_be_one_of_the_enumerated_values() {
        let parameter: Parameter =
            serde_json::from_value(json!({ "enum": ["a", "b"], "default": "c" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        parameter.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("is not one of the values listed in `enum`")),
            "got: {:?}",
            ctx.errors
        );

        // Without an `enum` any default is acceptable.
        let free: Parameter = serde_json::from_value(json!({ "default": "c" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.parameters.p");
        free.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }
}
