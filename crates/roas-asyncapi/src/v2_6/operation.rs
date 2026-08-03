//! AsyncAPI v2.6 `Operation` and `Operation Trait` objects.
//!
//! Per [Operation Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#operationObject).
//!
//! In 2.6 an operation is not a top-level object with an `action`: it
//! is the `publish` or `subscribe` member of a channel, and those names
//! are stated from the *consumer's* point of view — `publish` describes
//! messages an application may publish *to* the channel, so the
//! application described here receives them. v3 inverted this to
//! `receive` / `send` stated from the application's own point of view.

use crate::common::bindings::Bindings;
use crate::common::reference::RefOr;
use crate::v2_6::external_documentation::ExternalDocumentation;
use crate::v2_6::message::OperationMessage;
use crate::v2_6::security_scheme::SecurityRequirement;
use crate::v2_6::tag::Tag;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which half of a channel an operation is.
///
/// Not a document field — the position under `publish` / `subscribe`
/// carries it — but named so diagnostics and conversions can talk about
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    /// Messages an application may publish to the channel; the
    /// application described by this document *receives* them.
    Publish,
    /// Messages an application may subscribe to; the application
    /// described by this document *sends* them.
    Subscribe,
}

impl OperationKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Subscribe => "subscribe",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Operation {
    /// A machine-friendly identifier, unique across the document.
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,

    /// A short summary of what the operation is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A verbose explanation, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The security requirements for this operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,

    /// Tags for logical grouping and categorization of operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,

    /// Additional external documentation for this operation.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    /// Protocol-specific definitions for the operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<Bindings>>,

    /// Traits to apply to the operation. Validated but not merged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<RefOr<OperationTrait>>,

    /// The message (or set of alternatives) this operation carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<OperationMessage>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Operation {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(operation_id) = &self.operation_id {
            ctx.require_non_empty("operationId", operation_id);
        }
        for (i, requirement) in self.security.iter().enumerate() {
            ctx.in_index("security", i, |ctx| requirement.validate_with_context(ctx));
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
        for (i, operation_trait) in self.traits.iter().enumerate() {
            ctx.in_index("traits", i, |ctx| {
                operation_trait.validate_with_context(ctx)
            });
        }
        if let Some(message) = &self.message {
            ctx.in_field("message", |ctx| message.validate_with_context(ctx));
        }
    }
}

/// A trait that MAY be applied to an [`Operation`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OperationTrait {
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,

    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<Bindings>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for OperationTrait {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(operation_id) = &self.operation_id {
            ctx.require_non_empty("operationId", operation_id);
        }
        for (i, requirement) in self.security.iter().enumerate() {
            ctx.in_index("security", i, |ctx| requirement.validate_with_context(ctx));
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

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn errors_for(value: serde_json::Value) -> Vec<String> {
        let operation: Operation = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.channels.user.publish");
        operation.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn round_trips_a_full_operation() {
        let value = json!({
            "operationId": "receiveSignup",
            "summary": "Consume signups",
            "description": "Long form",
            "security": [ { "user_pass": [] } ],
            "tags": [ { "name": "user" } ],
            "externalDocs": { "url": "https://example.com" },
            "bindings": { "kafka": { "groupId": { "type": "string" } } },
            "traits": [ { "$ref": "#/components/operationTraits/common" } ],
            "message": { "name": "UserSignedUp" },
            "x-owner": "team"
        });
        let operation: Operation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(operation.operation_id.as_deref(), Some("receiveSignup"));
        assert_eq!(serde_json::to_value(&operation).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn an_empty_operation_is_valid() {
        // Every field is optional in 2.6 — a channel may declare
        // `publish: {}` to say "messages flow here" and nothing more.
        assert!(errors_for(json!({})).is_empty());
    }

    #[test]
    fn operation_kind_names_itself() {
        assert_eq!(OperationKind::Publish.as_str(), "publish");
        assert_eq!(OperationKind::Subscribe.as_str(), "subscribe");
        assert_ne!(OperationKind::Publish, OperationKind::Subscribe);
    }

    #[test]
    fn nested_errors_carry_their_path() {
        let errors = errors_for(json!({
            "operationId": "",
            "security": [ { "auth": ["read", "read"] } ],
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "kafka": 1 },
            "traits": [ { "operationId": "" } ],
            "message": { "oneOf": [] }
        }));
        for expected in [
            "#.channels.user.publish.operationId: must not be empty",
            "#.channels.user.publish.security[0].auth: duplicate scope `read`",
            "#.channels.user.publish.tags[0].name: must not be empty",
            "#.channels.user.publish.externalDocs.url: must not be empty",
            "#.channels.user.publish.traits[0].operationId: must not be empty",
            "#.channels.user.publish.message.oneOf: must contain at least one message",
        ] {
            assert!(
                errors.iter().any(|e| e == expected),
                "missing {expected}: {errors:?}"
            );
        }
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.channels.user.publish.bindings.kafka"))
        );
    }

    #[test]
    fn operation_trait_validates_its_nested_objects() {
        let operation_trait: OperationTrait = serde_json::from_value(json!({
            "operationId": "",
            "security": [ { "auth": ["a", "a"] } ],
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "amqp": 1 }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.operationTraits.common");
        operation_trait.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        for needle in [
            ".operationId",
            ".security[0].auth",
            ".tags[0].name",
            ".externalDocs.url",
            ".bindings.amqp",
        ] {
            assert!(
                msgs.iter().any(|e| e.contains(needle)),
                "missing {needle}: {msgs:?}"
            );
        }
    }
}
