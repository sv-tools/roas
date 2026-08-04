//! AsyncAPI v2.6 `Message`, `Message Trait`, and `Message Example`
//! objects.
//!
//! Per [Message Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#messageObject).
//!
//! Two shapes differ from v3. A message declares its payload dialect
//! with its own `schemaFormat` field and carries the payload directly,
//! where v3 wraps both in a Multi Format Schema Object. And an
//! operation's `message` may be a *set* of alternatives — the
//! `{ "oneOf": [...] }` form modeled by [`OperationMessage`] — which v3
//! replaced with the channel's `messages` map.

use crate::common::bindings::Bindings;
use crate::common::reference::RefOr;
use crate::v2_6::correlation_id::CorrelationId;
use crate::v2_6::external_documentation::ExternalDocumentation;
use crate::v2_6::schema::{SchemaType, SubSchema};
use crate::v2_6::tag::Tag;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// The `schemaFormat` values that make a payload an AsyncAPI Schema
/// Object — the document's own dialect in each of its three flavors,
/// for every 2.x version.
///
/// When `schemaFormat` is absent it defaults to this dialect too, so a
/// payload is a Schema Object unless the message names some *other*
/// format (Avro, OpenAPI, RAML, draft-07 …), in which case its shape is
/// that format's business and the payload stays raw JSON.
pub const ASYNCAPI_SCHEMA_FORMATS: &[&str] = &[
    "application/vnd.aai.asyncapi;version=2.0.0",
    "application/vnd.aai.asyncapi;version=2.1.0",
    "application/vnd.aai.asyncapi;version=2.2.0",
    "application/vnd.aai.asyncapi;version=2.3.0",
    "application/vnd.aai.asyncapi;version=2.4.0",
    "application/vnd.aai.asyncapi;version=2.5.0",
    "application/vnd.aai.asyncapi;version=2.6.0",
    "application/vnd.aai.asyncapi+json;version=2.0.0",
    "application/vnd.aai.asyncapi+json;version=2.1.0",
    "application/vnd.aai.asyncapi+json;version=2.2.0",
    "application/vnd.aai.asyncapi+json;version=2.3.0",
    "application/vnd.aai.asyncapi+json;version=2.4.0",
    "application/vnd.aai.asyncapi+json;version=2.5.0",
    "application/vnd.aai.asyncapi+json;version=2.6.0",
    "application/vnd.aai.asyncapi+yaml;version=2.0.0",
    "application/vnd.aai.asyncapi+yaml;version=2.1.0",
    "application/vnd.aai.asyncapi+yaml;version=2.2.0",
    "application/vnd.aai.asyncapi+yaml;version=2.3.0",
    "application/vnd.aai.asyncapi+yaml;version=2.4.0",
    "application/vnd.aai.asyncapi+yaml;version=2.5.0",
    "application/vnd.aai.asyncapi+yaml;version=2.6.0",
];

/// Whether a payload declared with this `schemaFormat` must be an
/// AsyncAPI Schema Object.
#[must_use]
pub fn payload_is_asyncapi_schema(schema_format: Option<&str>) -> bool {
    match schema_format {
        None => true,
        Some(format) => ASYNCAPI_SCHEMA_FORMATS.contains(&format),
    }
}

/// Validate a payload that must be an AsyncAPI Schema Object.
///
/// The field is stored as raw JSON so any dialect round-trips, so the
/// typing happens here: parse it as a [`SubSchema`] and run the usual
/// schema checks, reporting a parse failure as a diagnostic rather than
/// letting a malformed schema through.
fn validate_schema_payload(ctx: &mut Context, payload: &serde_json::Value) {
    match serde_json::from_value::<SubSchema>(payload.clone()) {
        Ok(schema) => ctx.in_field("payload", |ctx| schema.validate_with_context(ctx)),
        Err(err) => ctx.error_field(
            "payload",
            format!("is not a valid AsyncAPI Schema Object: {err}"),
        ),
    }
}

/// Keep an explicit `"payload": null` distinct from an absent one, so a
/// null-payload example or message round-trips as written.
fn deserialize_present_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// What an operation's `message` field holds: either one message (or a
/// `$ref` to one), or a set of alternatives under `oneOf`.
///
/// Deserialization dispatches on the discriminating `oneOf` key.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum OperationMessage {
    /// `{ "oneOf": [ … ] }` — the message is one of these.
    OneOf(MessageOneOf),
    /// A single message, inline or referenced.
    Single(Box<RefOr<Message>>),
}

impl Default for OperationMessage {
    fn default() -> Self {
        Self::Single(Box::new(RefOr::Item(Message::default())))
    }
}

impl<'de> Deserialize<'de> for OperationMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("oneOf").is_some() {
            return serde_json::from_value(value)
                .map(OperationMessage::OneOf)
                .map_err(serde::de::Error::custom);
        }
        serde_json::from_value(value)
            .map(|message| OperationMessage::Single(Box::new(message)))
            .map_err(serde::de::Error::custom)
    }
}

impl ValidateWithContext for OperationMessage {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            OperationMessage::OneOf(one_of) => one_of.validate_with_context(ctx),
            OperationMessage::Single(message) => message.validate_with_context(ctx),
        }
    }
}

/// The `{ "oneOf": [ … ] }` form of an operation's message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MessageOneOf {
    /// **Required** The alternatives this operation may carry.
    ///
    /// Each entry is a whole message *definition*, not just a message
    /// object: the schema recurses into `message.json`, so an
    /// alternative may itself be a `$ref` or another `oneOf` set.
    #[serde(rename = "oneOf")]
    pub one_of: Vec<OperationMessage>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for MessageOneOf {
    fn validate_with_context(&self, ctx: &mut Context) {
        if self.one_of.is_empty() {
            ctx.error_field("oneOf", "must contain at least one message");
        }
        for (i, message) in self.one_of.iter().enumerate() {
            ctx.in_index("oneOf", i, |ctx| message.validate_with_context(ctx));
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Message {
    /// A machine-friendly identifier, unique across the document.
    #[serde(rename = "messageId", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,

    /// The format of the `payload`, as a media type. Any string is
    /// legal; when absent the payload is an AsyncAPI Schema Object.
    #[serde(rename = "schemaFormat", skip_serializing_if = "Option::is_none")]
    pub schema_format: Option<String>,

    /// Schema definition of the application headers, which must
    /// describe an object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<SubSchema>,

    /// Definition of the message payload. Left as raw JSON because
    /// `schemaFormat` may name a dialect this crate does not type; an
    /// explicit `null` is preserved.
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub payload: Option<serde_json::Value>,

    /// Definition of the correlation ID used for message tracing.
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<RefOr<CorrelationId>>,

    /// The content type to use when encoding / decoding the payload.
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
    pub tags: Vec<Tag>,

    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<Bindings>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<MessageExample>,

    /// Traits to apply to the message. Validated but not merged.
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
        if let Some(format) = &self.schema_format {
            ctx.require_non_empty("schemaFormat", format);
        }
        if let Some(content_type) = &self.content_type {
            ctx.require_non_empty("contentType", content_type);
        }
        if let Some(message_id) = &self.message_id {
            ctx.require_non_empty("messageId", message_id);
        }
        if let Some(headers) = &self.headers {
            validate_headers(ctx, headers);
        }
        // A payload in the default dialect is a Schema Object and gets
        // the schema checks; one in a named foreign dialect does not.
        if let Some(payload) = &self.payload
            && payload_is_asyncapi_schema(self.schema_format.as_deref())
        {
            validate_schema_payload(ctx, payload);
        }
        if let Some(correlation_id) = &self.correlation_id {
            ctx.in_field("correlationId", |ctx| {
                correlation_id.validate_with_context(ctx);
            });
        }
        validate_tags(ctx, &self.tags);
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

/// Validate a headers schema, which must describe an *object*.
///
/// The schema pins `headers` to `allOf: [schema.json, { properties: {
/// type: { const: "object" } } }]`, so a declared `type` has to be
/// `object`. A boolean schema declares no type and stays valid.
fn validate_headers(ctx: &mut Context, headers: &SubSchema) {
    ctx.in_field("headers", |ctx| {
        headers.validate_with_context(ctx);
        if let SubSchema::Schema(schema) = headers {
            match &schema.schema_type {
                Some(SchemaType::Single(name)) if name != "object" => {
                    ctx.error_field("type", format!("must be `object`, not `{name}`"));
                }
                Some(SchemaType::Multiple(names)) if !names.iter().all(|name| name == "object") => {
                    ctx.error_field("type", "must be `object`");
                }
                _ => {}
            }
        }
    });
}

/// Validate a tag list and enforce the schema's `uniqueItems: true`.
///
/// Lives here because every object carrying tags needs it; the
/// comparison is whole-value, as `uniqueItems` specifies.
pub(crate) fn validate_tags(ctx: &mut Context, tags: &[Tag]) {
    // `uniqueItems` compares JSON *instances*, where `1` and `1.0` are
    // the same value — so serialize and use the schema module's
    // instance equality rather than Rust's `PartialEq`, which would let
    // two tags differing only by `x-order: 1` vs `1.0` through.
    let as_json: Vec<serde_json::Value> = tags
        .iter()
        .map(|tag| serde_json::to_value(tag).unwrap_or(serde_json::Value::Null))
        .collect();
    for (i, tag) in tags.iter().enumerate() {
        ctx.in_index("tags", i, |ctx| {
            tag.validate_with_context(ctx);
            if as_json[..i]
                .iter()
                .any(|seen| crate::v2_6::schema::json_instance_eq(seen, &as_json[i]))
            {
                ctx.error("duplicate tag");
            }
        });
    }
}

/// A trait that MAY be applied to a [`Message`].
///
/// Carries every Message field except `payload` and `traits`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MessageTrait {
    #[serde(rename = "messageId", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,

    #[serde(rename = "schemaFormat", skip_serializing_if = "Option::is_none")]
    pub schema_format: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<SubSchema>,

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
    pub tags: Vec<Tag>,

    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<Bindings>,

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
        if let Some(format) = &self.schema_format {
            ctx.require_non_empty("schemaFormat", format);
        }
        if let Some(content_type) = &self.content_type {
            ctx.require_non_empty("contentType", content_type);
        }
        if let Some(headers) = &self.headers {
            validate_headers(ctx, headers);
        }
        if let Some(correlation_id) = &self.correlation_id {
            ctx.in_field("correlationId", |ctx| {
                correlation_id.validate_with_context(ctx);
            });
        }
        crate::v2_6::message::validate_tags(ctx, &self.tags);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, serde_json::Value>>,

    /// An explicit `null` is preserved: presence is what satisfies the
    /// headers-and/or-payload requirement.
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
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
            "messageId": "userSignedUp",
            "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
            "headers": { "type": "object", "properties": { "id": { "type": "string" } } },
            "payload": { "type": "record", "name": "User" },
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
        assert_eq!(message.message_id.as_deref(), Some("userSignedUp"));
        assert_eq!(serde_json::to_value(&message).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn any_schema_format_is_accepted() {
        // 2.6 puts no enumeration on `schemaFormat` at all.
        for format in [
            "application/vnd.aai.asyncapi;version=2.6.0",
            "application/vnd.apache.avro;version=1.9.0",
            "application/vnd.example.custom+json;version=1.0",
        ] {
            let errors = errors_for(json!({ "schemaFormat": format, "payload": {} }));
            assert!(errors.is_empty(), "{format}: {errors:?}");
        }
        assert!(
            errors_for(json!({ "schemaFormat": "" }))
                .iter()
                .any(|e| e.contains("schemaFormat: must not be empty"))
        );
    }

    #[test]
    fn a_default_dialect_payload_is_validated_as_a_schema() {
        // Absent `schemaFormat` means the payload is an AsyncAPI Schema
        // Object, so the schema checks apply to it.
        let errors = errors_for(json!({ "payload": { "type": "bogus", "allOf": [] } }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.payload.type: `bogus` is not a JSON Schema type"),
            "got: {errors:?}"
        );
        assert!(
            errors.iter().any(
                |e| e == "#.messages.signup.payload.allOf: must contain at least one subschema"
            ),
            "got: {errors:?}"
        );

        // Naming an AsyncAPI dialect explicitly is the same thing.
        let errors = errors_for(json!({
            "schemaFormat": "application/vnd.aai.asyncapi;version=2.6.0",
            "payload": { "type": "bogus" }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is not a JSON Schema type"))
        );

        // A payload that is not a schema at all is reported rather than
        // silently accepted.
        let errors = errors_for(json!({ "payload": ["not", "a", "schema"] }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is not a valid AsyncAPI Schema Object")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_foreign_dialect_payload_is_left_alone() {
        // Avro's `type: record` is not a JSON Schema type, and must not
        // be judged as one.
        for format in [
            "application/vnd.apache.avro;version=1.9.0",
            "application/schema+json;version=draft-07",
            "application/vnd.example.custom+json;version=1.0",
        ] {
            let errors = errors_for(json!({
                "schemaFormat": format,
                "payload": { "type": "record", "name": "User", "allOf": [] }
            }));
            assert!(errors.is_empty(), "{format}: {errors:?}");
            assert!(!payload_is_asyncapi_schema(Some(format)));
        }
        assert!(payload_is_asyncapi_schema(None));
        for format in ASYNCAPI_SCHEMA_FORMATS {
            assert!(payload_is_asyncapi_schema(Some(format)), "{format}");
        }
    }

    #[test]
    fn boolean_schemas_are_accepted_where_a_schema_is_expected() {
        // draft-07 allows `true` / `false` as a whole schema.
        let value = json!({ "headers": true, "payload": false });
        let message: Message = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(message.headers, Some(SubSchema::Bool(true))));
        assert_eq!(serde_json::to_value(&message).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn a_nested_one_of_round_trips() {
        // The schema recurses into the whole message definition, so an
        // alternative may itself be a `$ref` or another `oneOf`.
        let value = json!({
            "oneOf": [
                { "name": "A" },
                { "$ref": "#/components/messages/b" },
                { "oneOf": [ { "name": "C" }, { "$ref": "#/components/messages/d" } ] }
            ]
        });
        let message: OperationMessage = serde_json::from_value(value.clone()).unwrap();
        match &message {
            OperationMessage::OneOf(one_of) => {
                assert_eq!(one_of.one_of.len(), 3);
                assert!(matches!(one_of.one_of[2], OperationMessage::OneOf(_)));
            }
            other => panic!("expected the oneOf form, got {other:?}"),
        }
        // The nested alternative survives instead of collapsing to `{}`.
        assert_eq!(serde_json::to_value(&message).unwrap(), value);

        // Errors inside a nested alternative still carry their path.
        let broken: OperationMessage = serde_json::from_value(json!({
            "oneOf": [ { "oneOf": [ { "contentType": "" } ] } ]
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.publish.message");
        broken.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.publish.message.oneOf[0].oneOf[0].contentType: must not be empty"),
            "got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn headers_must_describe_an_object() {
        let errors = errors_for(json!({ "headers": { "type": "string" } }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.headers.type: must be `object`, not `string`"),
            "got: {errors:?}"
        );

        assert!(errors_for(json!({ "headers": { "type": "object" } })).is_empty());
        // No declared type, or a boolean schema, stays valid.
        assert!(errors_for(json!({ "headers": { "properties": {} } })).is_empty());
        assert!(errors_for(json!({ "headers": true })).is_empty());

        // The list form must not admit anything but `object`.
        assert!(
            errors_for(json!({ "headers": { "type": ["object", "null"] } }))
                .iter()
                .any(|e| e.contains("headers.type: must be `object`"))
        );
    }

    #[test]
    fn tag_uniqueness_uses_json_instance_equality() {
        // `x-order: 1` and `x-order: 1.0` are the same JSON instance,
        // so these are duplicate tags.
        let errors = errors_for(json!({
            "tags": [ { "name": "a", "x-order": 1 }, { "name": "a", "x-order": 1.0 } ]
        }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.tags[1]: duplicate tag"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn tags_must_be_unique() {
        let errors = errors_for(json!({
            "tags": [ { "name": "a" }, { "name": "b" }, { "name": "a" } ]
        }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.messages.signup.tags[2]: duplicate tag"),
            "got: {errors:?}"
        );

        // Same name, different description, is a different tag value.
        let errors = errors_for(json!({
            "tags": [ { "name": "a" }, { "name": "a", "description": "d" } ]
        }));
        assert!(errors.is_empty(), "got: {errors:?}");
    }

    #[test]
    fn external_docs_does_not_accept_a_reference() {
        // 2.6 points at `externalDocs.json` directly, with no
        // `oneOf: [Reference, …]`, so a `$ref` is not a valid value.
        assert!(
            serde_json::from_value::<Message>(json!({ "externalDocs": { "$ref": "#/x" } }))
                .is_err(),
            "a `$ref` must not parse as an External Documentation Object",
        );
    }

    #[test]
    fn an_explicit_null_payload_is_preserved() {
        let value = json!({ "payload": null });
        let message: Message = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(message.payload, Some(serde_json::Value::Null));
        assert_eq!(serde_json::to_value(&message).unwrap(), value);

        let example: MessageExample =
            serde_json::from_value(json!({ "name": "empty", "payload": null })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.examples[0]");
        example.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn empty_message_is_valid_and_example_needs_content() {
        assert!(errors_for(json!({})).is_empty());

        let mut ctx = Context::with_path(EnumSet::empty(), "#.examples[0]");
        MessageExample::default().validate_with_context(&mut ctx);
        assert!(ctx.errors[0] == "#.examples[0]: must define `headers` and/or `payload`");
    }

    #[test]
    fn operation_message_picks_the_one_of_form_by_key() {
        let value = json!({
            "oneOf": [
                { "name": "A" },
                { "$ref": "#/components/messages/b" }
            ]
        });
        let message: OperationMessage = serde_json::from_value(value.clone()).unwrap();
        match &message {
            OperationMessage::OneOf(one_of) => assert_eq!(one_of.one_of.len(), 2),
            other => panic!("expected the oneOf form, got {other:?}"),
        }
        assert_eq!(serde_json::to_value(&message).unwrap(), value);

        let single: OperationMessage = serde_json::from_value(json!({ "name": "A" })).unwrap();
        assert!(matches!(single, OperationMessage::Single(_)));

        let referenced: OperationMessage =
            serde_json::from_value(json!({ "$ref": "#/components/messages/a" })).unwrap();
        assert!(matches!(referenced, OperationMessage::Single(_)));
        assert!(matches!(
            OperationMessage::default(),
            OperationMessage::Single(_)
        ));
    }

    #[test]
    fn an_empty_one_of_is_reported_and_members_are_validated() {
        let empty: OperationMessage = serde_json::from_value(json!({ "oneOf": [] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.publish.message");
        empty.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.publish.message.oneOf: must contain at least one message")
        );

        let members: OperationMessage =
            serde_json::from_value(json!({ "oneOf": [ { "contentType": "" } ] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.publish.message");
        members.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.publish.message.oneOf[0].contentType: must not be empty")
        );
    }

    #[test]
    fn nested_errors_carry_their_path() {
        let errors = errors_for(json!({
            "messageId": "",
            "headers": { "minItems": 2, "maxItems": 1 },
            "correlationId": { "location": "nope" },
            "contentType": "",
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "kafka": 1 },
            "examples": [ {} ],
            "traits": [ { "contentType": "" } ]
        }));
        for expected in [
            "#.messages.signup.messageId: must not be empty",
            "#.messages.signup.contentType: must not be empty",
            "#.messages.signup.tags[0].name: must not be empty",
            "#.messages.signup.externalDocs.url: must not be empty",
            "#.messages.signup.examples[0]: must define `headers` and/or `payload`",
            "#.messages.signup.traits[0].contentType: must not be empty",
        ] {
            assert!(
                errors.iter().any(|e| e == expected),
                "missing {expected}: {errors:?}"
            );
        }
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.headers.minItems"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.correlationId.location"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.messages.signup.bindings.kafka"))
        );
    }

    #[test]
    fn trait_validates_its_own_nested_objects() {
        let message_trait: MessageTrait = serde_json::from_value(json!({
            "schemaFormat": "",
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
        for needle in [
            ".schemaFormat",
            ".headers.minLength",
            ".correlationId.location",
            ".tags[0].name",
            ".externalDocs.url",
            ".bindings.amqp",
            ".examples[0]",
        ] {
            assert!(
                msgs.iter().any(|e| e.contains(needle)),
                "missing {needle}: {msgs:?}"
            );
        }
    }
}
