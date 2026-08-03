//! AsyncAPI v3.1 `Message`, `Message Trait`, and `Message Example`
//! objects.
//!
//! Per [Message Object](https://www.asyncapi.com/docs/reference/specification/v3.1.0#messageObject).
//!
//! A trait carries every Message field except `payload` and `traits`;
//! applying traits to a message is resolution rather than modeling, so
//! this crate parses and validates them without merging.

use crate::common::bindings::Bindings;
use crate::common::reference::RefOr;
use crate::v3_1::correlation_id::CorrelationId;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::schema::SchemaOrMultiFormat;
use crate::v3_1::tag::Tag;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Keep an explicit `"payload": null` (→ `Some(Value::Null)`) distinct
/// from an absent one (→ `None`). A plain `Option<Value>` collapses
/// both, which would turn a valid null-payload example into "must
/// define `headers` and/or `payload`".
fn deserialize_present_payload<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Message {
    /// Schema definition of the application headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<RefOr<SchemaOrMultiFormat>>,

    /// Definition of the message payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<RefOr<SchemaOrMultiFormat>>,

    /// Definition of the correlation ID used for message tracing.
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<RefOr<CorrelationId>>,

    /// The content type to use when encoding / decoding the payload.
    /// Defaults to the document's `defaultContentType`.
    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    /// A machine-friendly name for the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// A human-friendly title for the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short summary of what the message is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A verbose explanation of the message, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether this message is deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    /// Tags for logical grouping and categorization of messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<RefOr<Tag>>,

    /// Additional external documentation for this message.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    /// Protocol-specific definitions for the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<Bindings>>,

    /// Examples of this message, each with headers and / or payload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<MessageExample>,

    /// Traits to apply to the message object. Traits are validated but
    /// not merged into the message by this crate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<RefOr<MessageTrait>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Message {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(headers) = &self.headers {
            ctx.in_field("headers", |ctx| headers.validate_with_context(ctx));
        }
        if let Some(payload) = &self.payload {
            ctx.in_field("payload", |ctx| payload.validate_with_context(ctx));
        }
        if let Some(correlation_id) = &self.correlation_id {
            ctx.in_field("correlationId", |ctx| {
                correlation_id.validate_with_context(ctx);
            });
        }
        if let Some(content_type) = &self.content_type {
            ctx.require_non_empty("contentType", content_type);
        }
        for (i, tag) in self.tags.iter().enumerate() {
            ctx.in_index("tags", i, |ctx| tag.validate_with_context(ctx));
        }
        if let Some(docs) = &self.external_docs {
            ctx.in_field("externalDocs", |ctx| docs.validate_with_context(ctx));
        }
        if let Some(bindings) = &self.bindings {
            ctx.in_field("bindings", |ctx| bindings.validate_with_context(ctx));
        }
        for (i, example) in self.examples.iter().enumerate() {
            ctx.in_index("examples", i, |ctx| example.validate_with_context(ctx));
        }
        for (i, message_trait) in self.traits.iter().enumerate() {
            ctx.in_index("traits", i, |ctx| message_trait.validate_with_context(ctx));
        }
    }
}

/// A trait that MAY be applied to a [`Message`].
///
/// Carries every Message field except `payload` and `traits`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MessageTrait {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<RefOr<SchemaOrMultiFormat>>,

    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<RefOr<CorrelationId>>,

    #[serde(rename = "contentType", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<RefOr<Tag>>,

    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<Bindings>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<MessageExample>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for MessageTrait {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(headers) = &self.headers {
            ctx.in_field("headers", |ctx| headers.validate_with_context(ctx));
        }
        if let Some(correlation_id) = &self.correlation_id {
            ctx.in_field("correlationId", |ctx| {
                correlation_id.validate_with_context(ctx);
            });
        }
        if let Some(content_type) = &self.content_type {
            ctx.require_non_empty("contentType", content_type);
        }
        for (i, tag) in self.tags.iter().enumerate() {
            ctx.in_index("tags", i, |ctx| tag.validate_with_context(ctx));
        }
        if let Some(docs) = &self.external_docs {
            ctx.in_field("externalDocs", |ctx| docs.validate_with_context(ctx));
        }
        if let Some(bindings) = &self.bindings {
            ctx.in_field("bindings", |ctx| bindings.validate_with_context(ctx));
        }
        for (i, example) in self.examples.iter().enumerate() {
            ctx.in_index("examples", i, |ctx| example.validate_with_context(ctx));
        }
    }
}

/// An example of a message, carrying headers and / or a payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MessageExample {
    /// A machine-friendly name for the example.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// A short summary of what the example is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Example of the application headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, serde_json::Value>>,

    /// Example of the message payload, which may be of any type —
    /// including an explicit `null`, which is preserved as
    /// `Some(Value::Null)` because *presence* is what satisfies the
    /// headers-and/or-payload requirement.
    #[serde(
        default,
        deserialize_with = "deserialize_present_payload",
        skip_serializing_if = "Option::is_none"
    )]
    pub payload: Option<serde_json::Value>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for MessageExample {
    fn validate_with_context(&self, ctx: &mut Context) {
        // The schema requires at least one of `headers` / `payload`;
        // an example carrying neither says nothing.
        if self.headers.is_none() && self.payload.is_none() {
            ctx.error("must define `headers` and/or `payload`");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn errors_for(value: serde_json::Value) -> Vec<String> {
        let message: Message = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.messages.signup");
        message.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn round_trips_a_full_message() {
        let value = json!({
            "headers": { "type": "object", "properties": { "id": { "type": "string" } } },
            "payload": { "type": "object" },
            "correlationId": { "location": "$message.header#/id" },
            "contentType": "application/json",
            "name": "UserSignedUp",
            "title": "User signed up",
            "summary": "A user signed up",
            "description": "Emitted on signup",
            "deprecated": false,
            "tags": [ { "name": "user" } ],
            "externalDocs": { "url": "https://example.com" },
            "bindings": { "kafka": { "key": { "type": "string" } } },
            "examples": [ { "name": "basic", "payload": { "id": "1" } } ],
            "traits": [ { "$ref": "#/components/messageTraits/common" } ],
            "x-owner": "team"
        });
        let message: Message = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(message.name.as_deref(), Some("UserSignedUp"));
        assert_eq!(message.traits.len(), 1);
        assert_eq!(serde_json::to_value(&message).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn empty_message_is_valid() {
        assert!(errors_for(json!({})).is_empty());
    }

    #[test]
    fn payload_may_name_its_own_schema_format() {
        let message: Message = serde_json::from_value(json!({
            "payload": {
                "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
                "schema": { "type": "record", "name": "User" }
            }
        }))
        .unwrap();
        assert!(matches!(
            message.payload.as_ref().and_then(|p| p.item()),
            Some(SchemaOrMultiFormat::MultiFormat(_))
        ));
    }

    #[test]
    fn nested_errors_carry_their_path() {
        let errors = errors_for(json!({
            "headers": { "minItems": 2, "maxItems": 1 },
            "payload": { "schemaFormat": "", "schema": {} },
            "correlationId": { "location": "nope" },
            "contentType": "",
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "kafka": 1 },
            "examples": [ {} ],
            "traits": [ { "contentType": "" } ]
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.headers.minItems"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.payload.schemaFormat"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.correlationId.location"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.contentType: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.externalDocs.url: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.bindings.kafka"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e
                    == "#.messages.signup.examples[0]: must define `headers` and/or `payload`")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.traits[0].contentType: must not be empty")
        );
    }

    #[test]
    fn an_explicit_null_payload_is_a_present_payload() {
        // `payload` accepts any type including null, and it is
        // *presence* that satisfies the headers-or-payload rule.
        let value = json!({ "name": "empty", "payload": null });
        let example: MessageExample = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(example.payload, Some(serde_json::Value::Null));

        let mut ctx = Context::with_path(EnumSet::empty(), "#.examples[0]");
        example.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);

        // …and it survives serialization rather than being dropped.
        assert_eq!(serde_json::to_value(&example).unwrap(), value);
    }

    #[test]
    fn example_needs_headers_or_payload() {
        let with_headers: MessageExample =
            serde_json::from_value(json!({ "headers": { "a": 1 } })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.examples[0]");
        with_headers.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());

        let mut ctx = Context::with_path(EnumSet::empty(), "#.examples[0]");
        MessageExample::default().validate_with_context(&mut ctx);
        assert!(ctx.errors[0] == "#.examples[0]: must define `headers` and/or `payload`");
    }

    #[test]
    fn trait_validates_its_own_nested_objects() {
        let message_trait: MessageTrait = serde_json::from_value(json!({
            "headers": { "minLength": 5, "maxLength": 1 },
            "correlationId": { "location": "bad" },
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "amqp": [] },
            "examples": [ {} ]
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.messageTraits.common");
        message_trait.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(msgs.iter().any(|e| e.contains(".headers.minLength")));
        assert!(msgs.iter().any(|e| e.contains(".correlationId.location")));
        assert!(msgs.iter().any(|e| e.contains(".tags[0].name")));
        assert!(msgs.iter().any(|e| e.contains(".externalDocs.url")));
        assert!(msgs.iter().any(|e| e.contains(".bindings.amqp")));
        assert!(msgs.iter().any(|e| e.contains(".examples[0]")));
    }
}
