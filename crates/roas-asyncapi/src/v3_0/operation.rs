//! AsyncAPI v3.0 `Operation`, `Operation Trait`, and reply objects.
//!
//! Per [Operation Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#operationObject).
//!
//! v3 hoists operations out of channels into a top-level map, and states
//! them from the application's point of view: `send` means this
//! application sends to the channel, `receive` means it consumes from
//! it. (AsyncAPI 2.x said `subscribe` / `publish`, describing the *other*
//! side — the inversion is the classic migration trap.)

use crate::common::bindings::OperationBindings;
use crate::common::reference::{RefOr, Reference};
use crate::common::runtime_expression;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::security_scheme::SecurityScheme;
use crate::v3_0::tag::Tag;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Whether the application sends to, or receives from, the channel.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OperationAction {
    /// The application sends a message to the channel.
    #[default]
    Send,
    /// The application receives messages from the channel.
    Receive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Operation {
    /// **Required** Whether the application sends or receives.
    pub action: OperationAction,

    /// **Required** A `$ref` to the channel this operation belongs to.
    pub channel: Reference,

    /// A `$ref` subset of the channel's messages that this operation
    /// processes. Empty means every message on the channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Reference>,

    /// The reply part of a request/reply operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<RefOr<OperationReply>>,

    /// Traits to apply to the operation. Traits are validated but not
    /// merged into the operation by this crate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<RefOr<OperationTrait>>,

    /// A human-friendly title for the operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short summary of what the operation is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A verbose explanation, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The security requirements for this operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<RefOr<SecurityScheme>>,

    /// Tags for logical grouping and categorization of operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<RefOr<Tag>>,

    /// Additional external documentation for this operation.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    /// Protocol-specific definitions for the operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<OperationBindings>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Operation {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.in_field("channel", |ctx| self.channel.validate_with_context(ctx));

        for (i, message) in self.messages.iter().enumerate() {
            ctx.in_index("messages", i, |ctx| message.validate_with_context(ctx));
        }
        if let Some(reply) = &self.reply {
            ctx.in_field("reply", |ctx| reply.validate_with_context(ctx));
        }
        for (i, operation_trait) in self.traits.iter().enumerate() {
            ctx.in_index("traits", i, |ctx| {
                operation_trait.validate_with_context(ctx)
            });
        }
        for (i, scheme) in self.security.iter().enumerate() {
            ctx.in_index("security", i, |ctx| scheme.validate_with_context(ctx));
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
    }
}

/// A trait that MAY be applied to an [`Operation`].
///
/// Carries every Operation field except `action`, `channel`, `messages`,
/// `reply`, and `traits`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OperationTrait {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<RefOr<SecurityScheme>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<RefOr<Tag>>,

    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<OperationBindings>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for OperationTrait {
    fn validate_with_context(&self, ctx: &mut Context) {
        for (i, scheme) in self.security.iter().enumerate() {
            ctx.in_index("security", i, |ctx| scheme.validate_with_context(ctx));
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
    }
}

/// The reply half of a request/reply operation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OperationReply {
    /// Where to send the reply — a fixed location or a runtime
    /// expression evaluated against the request message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<RefOr<OperationReplyAddress>>,

    /// A `$ref` to the channel the reply travels over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<Reference>,

    /// A `$ref` subset of the reply channel's messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Reference>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for OperationReply {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(address) = &self.address {
            ctx.in_field("address", |ctx| address.validate_with_context(ctx));
        }
        if let Some(channel) = &self.channel {
            ctx.in_field("channel", |ctx| channel.validate_with_context(ctx));
        }
        for (i, message) in self.messages.iter().enumerate() {
            ctx.in_index("messages", i, |ctx| message.validate_with_context(ctx));
        }
        // Naming reply messages without naming the channel they belong
        // to leaves them unresolvable.
        if !self.messages.is_empty() && self.channel.is_none() {
            ctx.error_field("channel", "is required when `messages` is set");
        }
    }
}

/// A runtime expression locating where a reply is sent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OperationReplyAddress {
    /// **Required** A runtime expression, e.g.
    /// `$message.header#/replyTo`.
    pub location: String,

    /// An optional description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for OperationReplyAddress {
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

    fn errors_for(value: serde_json::Value) -> Vec<String> {
        let operation: Operation = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.operations.sendSignup");
        operation.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn round_trips_a_full_operation() {
        let value = json!({
            "action": "receive",
            "channel": { "$ref": "#/channels/user" },
            "messages": [ { "$ref": "#/channels/user/messages/signup" } ],
            "reply": {
                "address": { "location": "$message.header#/replyTo" },
                "channel": { "$ref": "#/channels/replies" },
                "messages": [ { "$ref": "#/channels/replies/messages/ack" } ]
            },
            "traits": [ { "$ref": "#/components/operationTraits/common" } ],
            "title": "Receive signups",
            "summary": "Consume signup events",
            "description": "Long form",
            "security": [ { "type": "userPassword" } ],
            "tags": [ { "name": "user" } ],
            "externalDocs": { "url": "https://example.com" },
            "bindings": { "kafka": { "groupId": { "type": "string" } } },
            "x-owner": "team"
        });
        let operation: Operation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(operation.action, OperationAction::Receive);
        assert_eq!(serde_json::to_value(&operation).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn action_round_trips_both_directions() {
        for (text, expected) in [
            ("send", OperationAction::Send),
            ("receive", OperationAction::Receive),
        ] {
            let operation: Operation =
                serde_json::from_value(json!({ "action": text, "channel": { "$ref": "#/c" } }))
                    .unwrap();
            assert_eq!(operation.action, expected);
            assert_eq!(serde_json::to_value(operation.action).unwrap(), json!(text));
        }
        assert!(
            serde_json::from_value::<Operation>(
                json!({ "action": "publish", "channel": { "$ref": "#/c" } })
            )
            .is_err(),
            "AsyncAPI 2.x actions must not parse as v3"
        );
    }

    #[test]
    fn action_and_channel_are_required_by_the_parser() {
        assert!(serde_json::from_value::<Operation>(json!({ "action": "send" })).is_err());
        assert!(
            serde_json::from_value::<Operation>(json!({ "channel": { "$ref": "#/c" } })).is_err()
        );
    }

    #[test]
    fn empty_channel_ref_is_reported() {
        let errors = errors_for(json!({ "action": "send", "channel": { "$ref": "" } }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.operations.sendSignup.channel.$ref: must not be empty")
        );
    }

    #[test]
    fn reply_messages_need_a_channel() {
        let errors = errors_for(json!({
            "action": "send",
            "channel": { "$ref": "#/channels/user" },
            "reply": { "messages": [ { "$ref": "#/channels/replies/messages/ack" } ] }
        }));
        assert!(
            errors.iter().any(|e| e
                == "#.operations.sendSignup.reply.channel: is required when `messages` is set"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn reply_address_must_be_a_runtime_expression() {
        let errors = errors_for(json!({
            "action": "send",
            "channel": { "$ref": "#/channels/user" },
            "reply": { "address": { "location": "replyTo" } }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is not a valid runtime expression")),
            "got: {errors:?}"
        );

        let mut ctx = Context::with_path(EnumSet::empty(), "#.reply.address");
        OperationReplyAddress::default().validate_with_context(&mut ctx);
        assert!(ctx.errors[0] == "#.reply.address.location: must not be empty");
    }

    #[test]
    fn nested_errors_carry_their_path() {
        let errors = errors_for(json!({
            "action": "send",
            "channel": { "$ref": "#/channels/user" },
            "messages": [ { "$ref": "" } ],
            "traits": [ { "tags": [ { "name": "" } ] } ],
            "security": [ { "type": "http" } ],
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "kafka": 1 }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.operations.sendSignup.messages[0].$ref: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.operations.sendSignup.traits[0].tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.operations.sendSignup.security[0].scheme"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.operations.sendSignup.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.operations.sendSignup.externalDocs.url: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.operations.sendSignup.bindings.kafka"))
        );
    }

    #[test]
    fn operation_trait_validates_its_nested_objects() {
        let operation_trait: OperationTrait = serde_json::from_value(json!({
            "security": [ { "type": "oauth2" } ],
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "amqp": 1 }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.operationTraits.common");
        operation_trait.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(msgs.iter().any(|e| e.contains(".security[0].flows")));
        assert!(msgs.iter().any(|e| e.contains(".tags[0].name")));
        assert!(msgs.iter().any(|e| e.contains(".externalDocs.url")));
        assert!(msgs.iter().any(|e| e.contains(".bindings.amqp")));
    }
}
