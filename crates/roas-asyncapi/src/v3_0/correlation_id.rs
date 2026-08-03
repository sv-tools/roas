//! AsyncAPI v3.0 `Correlation ID` object.
//!
//! Per [Correlation ID Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#correlationIdObject).

use crate::common::runtime_expression;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct CorrelationId {
    /// **Required** A runtime expression selecting the correlation ID,
    /// e.g. `$message.header#/correlationId`.
    pub location: String,

    /// An optional description of the identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for CorrelationId {
    fn validate_with_context(&self, ctx: &mut Context) {
        if self.location.is_empty() {
            ctx.error_field("location", "must not be empty");
        } else if let Err(err) = runtime_expression::parse(&self.location) {
            ctx.error_field(
                "location",
                format!(
                    "`{}` is not a valid runtime expression: {}",
                    self.location,
                    err.message()
                ),
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
    fn accepts_header_and_payload_expressions() {
        for location in [
            "$message.header#/correlationId",
            "$message.payload#/user/id",
            // `#` alone selects the whole payload.
            "$message.payload#",
        ] {
            let id: CorrelationId =
                serde_json::from_value(json!({ "location": location })).unwrap();
            let mut ctx = Context::with_path(EnumSet::empty(), "#.correlationId");
            id.validate_with_context(&mut ctx);
            assert!(ctx.errors.is_empty(), "{location}: {:?}", ctx.errors);
        }
    }

    #[test]
    fn round_trips_with_description() {
        let value = json!({ "location": "$message.header#/id", "description": "The id" });
        let id: CorrelationId = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&id).unwrap(), value);
    }

    #[test]
    fn rejects_empty_and_malformed_locations() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.correlationId");
        CorrelationId::default().validate_with_context(&mut ctx);
        assert!(ctx.errors[0] == "#.correlationId.location: must not be empty");

        // A wrong prefix, and a bare source with no `#` fragment — the
        // schema pattern requires it.
        for location in ["$request.header#/id", "$message.payload"] {
            let bad: CorrelationId =
                serde_json::from_value(json!({ "location": location })).unwrap();
            let mut ctx = Context::with_path(EnumSet::empty(), "#.correlationId");
            bad.validate_with_context(&mut ctx);
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains("is not a valid runtime expression")),
                "{location}: {:?}",
                ctx.errors
            );
        }
    }
}
