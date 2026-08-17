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

    /// **Required** The value set within the target location — any
    /// JSON value, typically a runtime expression.
    ///
    /// The prose specification says `Any | {expression}`, the same as
    /// [`Parameter::value`](crate::v1_0::Parameter::value); the
    /// published JSON Schema narrows it to a string. The prose is what
    /// this crate follows, and it is what v1.1 does with the same field.
    pub value: serde_json::Value,

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
    fn a_replacement_value_may_be_any_json_the_prose_allows() {
        // The published JSON Schema says `string`; the prose says
        // `Any | {expression}`, as it does for `Parameter.value`, and a
        // replacement that sets a number or an object is what a real
        // description writes.
        let body: RequestBody = serde_json::from_value(json!({
            "replacements": [
                { "target": "/quantity", "value": 2 },
                { "target": "/tags", "value": ["cat", "small"] },
                { "target": "/pet", "value": { "id": "$steps.find.outputs.id" } },
                { "target": "/gift", "value": true },
                { "target": "/note", "value": "for {$inputs.who}" }
            ]
        }))
        .unwrap();
        let values: Vec<&serde_json::Value> = body.replacements.iter().map(|r| &r.value).collect();
        assert_eq!(
            values,
            [
                &json!(2),
                &json!(["cat", "small"]),
                &json!({ "id": "$steps.find.outputs.id" }),
                &json!(true),
                &json!("for {$inputs.who}"),
            ]
        );

        // And each keeps its type on the way out again.
        assert_eq!(
            serde_json::to_value(&body).unwrap()["replacements"][0]["value"],
            json!(2)
        );
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
