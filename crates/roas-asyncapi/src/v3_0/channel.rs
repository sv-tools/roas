//! AsyncAPI v3.0 `Channel` object.
//!
//! Per [Channel Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#channelObject).
//!
//! v3 separates a channel's *key* (how it is referenced) from its
//! `address` (where it actually is), and hangs the messages that travel
//! over it off the channel rather than off an operation.

use crate::common::bindings::Bindings;
use crate::common::reference::{RefOr, Reference};
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::message::Message;
use crate::v3_0::parameter::Parameter;
use crate::v3_0::server::placeholders;
use crate::v3_0::tag::Tag;
use crate::validation::{Context, ValidateWithContext, ValidationOptions};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Distinguish an explicit `"address": null` (→ `Some(None)`) from an
/// absent `address` (→ `None`, supplied by `#[serde(default)]`). A plain
/// `Option<Option<T>>` collapses both to `None`.
fn deserialize_address<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Channel {
    /// The channel address — a topic, routing key, event type, or path.
    /// `null` marks an address that is unknown at design time; absent
    /// means the address is the channel key itself.
    ///
    /// The two are distinct in the specification, so the outer `Option`
    /// tracks presence and the inner one the explicit `null`.
    #[serde(
        default,
        deserialize_with = "deserialize_address",
        skip_serializing_if = "Option::is_none"
    )]
    pub address: Option<Option<String>>,

    /// The messages that can be sent to or received from this channel.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub messages: BTreeMap<String, RefOr<Message>>,

    /// Parameters for each `{placeholder}` in the address.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, RefOr<Parameter>>,

    /// A human-friendly title for the channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short summary of the channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// An optional description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// References to the servers on which this channel is available.
    /// Absent or empty means every server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<Reference>,

    /// Tags for logical grouping and categorization of channels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<RefOr<Tag>>,

    /// Additional external documentation for this channel.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    /// Protocol-specific definitions for the channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<Bindings>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Channel {
    /// The address as a string, if one is set and not explicitly `null`.
    #[must_use]
    pub fn address(&self) -> Option<&str> {
        self.address.as_ref()?.as_deref()
    }
}

impl ValidateWithContext for Channel {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.validate_map_keys("messages", &self.messages);
        ctx.validate_map_keys("parameters", &self.parameters);

        // The address's `{placeholders}` and the declared parameters
        // must line up in both directions.
        if let Some(address) = self.address() {
            if address.is_empty() {
                ctx.error_field("address", "must not be empty (use `null` if unknown)");
            }
            let used = placeholders(address);
            for name in &used {
                if !self.parameters.contains_key(*name) {
                    ctx.error_field(
                        "parameters",
                        format!("`{{{name}}}` in `address` is not declared in `parameters`"),
                    );
                }
            }
            if !ctx.is_option(ValidationOptions::IgnoreUnusedChannelParameter) {
                for name in self.parameters.keys() {
                    if !used.contains(&name.as_str()) {
                        ctx.error_field(
                            "parameters",
                            format!("`{name}` is declared but never used in `address`"),
                        );
                    }
                }
            }
        }

        for (name, message) in &self.messages {
            ctx.in_key("messages", name, |ctx| message.validate_with_context(ctx));
        }
        for (name, parameter) in &self.parameters {
            ctx.in_key("parameters", name, |ctx| {
                parameter.validate_with_context(ctx);
            });
        }
        for (i, server) in self.servers.iter().enumerate() {
            ctx.in_index("servers", i, |ctx| server.validate_with_context(ctx));
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

    fn errors_with(
        options: enumset::EnumSet<ValidationOptions>,
        value: serde_json::Value,
    ) -> Vec<String> {
        let channel: Channel = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(options, "#.channels.user");
        channel.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    fn errors_for(value: serde_json::Value) -> Vec<String> {
        errors_with(EnumSet::empty(), value)
    }

    #[test]
    fn round_trips_a_full_channel() {
        let value = json!({
            "address": "user/{userId}/signedup",
            "messages": { "signup": { "$ref": "#/components/messages/signup" } },
            "parameters": { "userId": { "description": "Id of the user" } },
            "title": "User channel",
            "summary": "Signup events",
            "description": "Where signups land",
            "servers": [ { "$ref": "#/servers/production" } ],
            "tags": [ { "name": "user" } ],
            "externalDocs": { "url": "https://example.com" },
            "bindings": { "kafka": { "topic": "signups" } },
            "x-owner": "team"
        });
        let channel: Channel = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(channel.address(), Some("user/{userId}/signedup"));
        assert_eq!(serde_json::to_value(&channel).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn null_address_is_distinct_from_absent_and_round_trips() {
        let explicit_null: Channel = serde_json::from_value(json!({ "address": null })).unwrap();
        assert_eq!(explicit_null.address, Some(None));
        assert_eq!(explicit_null.address(), None);
        assert_eq!(
            serde_json::to_value(&explicit_null).unwrap(),
            json!({ "address": null })
        );

        let absent: Channel = serde_json::from_value(json!({})).unwrap();
        assert_eq!(absent.address, None);
        assert_eq!(serde_json::to_value(&absent).unwrap(), json!({}));

        // Neither form triggers the placeholder checks.
        assert!(errors_for(json!({ "address": null })).is_empty());
    }

    #[test]
    fn empty_address_is_reported() {
        let errors = errors_for(json!({ "address": "" }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.address: must not be empty (use `null` if unknown)")
        );
    }

    #[test]
    fn address_placeholders_and_parameters_must_agree() {
        let errors = errors_for(json!({
            "address": "user/{userId}/{tenant}",
            "parameters": { "userId": {}, "unused": {} }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`{tenant}` in `address` is not declared")),
            "got: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`unused` is declared but never used")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn unused_parameters_can_be_ignored() {
        let errors = errors_with(
            EnumSet::only(ValidationOptions::IgnoreUnusedChannelParameter),
            json!({ "address": "user/{userId}", "parameters": { "userId": {}, "unused": {} } }),
        );
        assert!(errors.is_empty(), "got: {errors:?}");
    }

    #[test]
    fn invalid_map_keys_are_reported() {
        let errors = errors_for(json!({
            "messages": { "bad key": {} },
            "parameters": { "also bad": {} }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("#.channels.user.messages.bad key"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("#.channels.user.parameters.also bad"))
        );
    }

    #[test]
    fn nested_errors_carry_their_path() {
        let errors = errors_for(json!({
            "messages": { "signup": { "contentType": "" } },
            "parameters": { "p": { "location": "bad" } },
            "servers": [ { "$ref": "" } ],
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "kafka": 1 }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.messages.signup.contentType: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.channels.user.parameters.p.location"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.servers[0].$ref: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.externalDocs.url: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.channels.user.bindings.kafka"))
        );
    }
}
