//! Arazzo v1.1 `Request Body` and `Payload Replacement` objects.
//!
//! Per [Request Body Object](https://spec.openapis.org/arazzo/v1.1.0.html#request-body-object)
//! and [Payload Replacement Object](https://spec.openapis.org/arazzo/v1.1.0.html#payload-replacement-object).
//! New in v1.1: `PayloadReplacement.targetSelectorType`, and `value` may
//! be a `Selector`.

use crate::v1_1::selector::{SelectorType, ValueOrSelector};
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct RequestBody {
    /// The `Content-Type` for the request content.
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    /// The request body payload (any JSON type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,

    /// Locations and values to set within the payload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replacements: Vec<PayloadReplacement>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for RequestBody {
    fn validate_with_context(&self, ctx: &mut Context) {
        for (i, replacement) in self.replacements.iter().enumerate() {
            ctx.in_index("replacements", i, |ctx| {
                replacement.validate_with_context(ctx)
            });
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct PayloadReplacement {
    /// **Required** A JSONPath, JSON Pointer, or XPath expression
    /// resolved against the request body.
    pub target: String,

    /// The selector expression type for `target`. Added in v1.1.
    #[serde(rename = "targetSelectorType", skip_serializing_if = "Option::is_none")]
    pub target_selector_type: Option<SelectorType>,

    /// **Required** The value to set — a literal / runtime expression,
    /// or a `Selector`.
    pub value: ValueOrSelector,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for PayloadReplacement {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("target", &self.target);
        if let Some(selector_type) = &self.target_selector_type {
            ctx.in_field("targetSelectorType", |ctx| {
                selector_type.validate_with_context(ctx)
            });
        }
        ctx.in_field("value", |ctx| self.value.validate_with_context(ctx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_with_selector_type() {
        let rb: RequestBody = serde_json::from_value(json!({
            "contentType": "application/json",
            "replacements": [
                { "target": "/role", "targetSelectorType": "jsonpointer", "value": "admin" }
            ],
        }))
        .unwrap();
        assert_eq!(rb.replacements.len(), 1);
        assert!(rb.replacements[0].target_selector_type.is_some());
    }

    #[test]
    fn validate_recurses_into_replacements() {
        let mut c = Context::with_path(EnumSet::empty(), "#.requestBody");
        let rb = RequestBody {
            replacements: vec![PayloadReplacement::default()],
            ..Default::default()
        };
        rb.validate_with_context(&mut c);
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.requestBody.replacements[0].target: must not be empty")
        );
    }
}
