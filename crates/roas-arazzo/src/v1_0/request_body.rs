//! Arazzo v1.0 `Request Body` and `Payload Replacement` objects.
//!
//! Per [Request Body Object](https://spec.openapis.org/arazzo/v1.0.1.html#request-body-object)
//! and [Payload Replacement Object](https://spec.openapis.org/arazzo/v1.0.1.html#payload-replacement-object).

use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct RequestBody {
    /// The `Content-Type` for the request content.
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    /// The request body payload (any JSON type, typically containing
    /// runtime expressions).
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
    /// **Required** A JSON Pointer or XPath expression resolved against
    /// the request body.
    pub target: String,

    /// **Required** The value set within the target location.
    pub value: String,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for PayloadReplacement {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("target", &self.target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn deserialize_round_trips() {
        let rb: RequestBody = serde_json::from_value(json!({
            "contentType": "application/json",
            "payload": { "id": "$inputs.id" },
            "replacements": [ { "target": "/role", "value": "admin" } ],
        }))
        .unwrap();
        assert_eq!(rb.content_type.as_deref(), Some("application/json"));
        assert_eq!(rb.replacements.len(), 1);

        let v = serde_json::to_value(&rb).unwrap();
        assert_eq!(v["contentType"], json!("application/json"));
    }

    #[test]
    fn empty_request_body_omits_optionals() {
        let rb = RequestBody::default();
        let v = serde_json::to_value(&rb).unwrap();
        assert_eq!(v, json!({}));
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
