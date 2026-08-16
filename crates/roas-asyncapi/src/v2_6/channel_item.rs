//! AsyncAPI v2.6 `Channel Item` object.
//!
//! Per [Channel Item Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#channelItemObject).
//!
//! A 2.6 channel *is* its path: the `channels` map is keyed by the
//! address, which may carry `{parameter}` placeholders, and the
//! operations hang off it as `publish` / `subscribe`. v3 split those
//! apart — a channel key, a separate `address`, and operations hoisted
//! to the document root.

use crate::common::bindings::ChannelBindings;
use crate::common::reference::RefOr;
use crate::v2_6::operation::Operation;
use crate::v2_6::parameter::Parameter;
use crate::v2_6::server::placeholders;
use crate::validation::{Context, ValidateWithContext, ValidationOptions};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ChannelItem {
    /// A reference to another Channel Item.
    ///
    /// `$ref` is a *field* of the Channel Item here, not a replacement
    /// for it, so it may sit alongside `description`, `servers`,
    /// `parameters` and the rest — all of which survive a round-trip.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,

    /// An optional description of this channel item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The names of the servers on which this channel is available.
    /// Absent or empty means every server. Unlike v3's `$ref` list,
    /// these are plain server names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<String>,

    /// A description of the operation that publishes messages *to* this
    /// channel — which this application therefore receives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish: Option<Operation>,

    /// A description of the operation that subscribes to this channel —
    /// whose messages this application therefore sends.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<Operation>,

    /// Parameters for each `{placeholder}` in the channel path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, RefOr<Parameter>>,

    /// Whether this channel is deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    /// Protocol-specific definitions for the channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<ChannelBindings>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ChannelItem {
    /// Check a channel path's `{placeholders}` against the parameters
    /// declared anywhere along its `$ref` chain.
    ///
    /// `$ref` siblings are preserved, so a parameter may sit beside the
    /// reference, on the target, or on a hop in between — any of which
    /// satisfies the path.
    pub(crate) fn validate_path_parameters(ctx: &mut Context, path: &str, chain: &[&ChannelItem]) {
        let declared: BTreeSet<&str> = chain
            .iter()
            .flat_map(|item| item.parameters.keys().map(String::as_str))
            .collect();
        let used = placeholders(path);
        for name in &used {
            if !declared.contains(*name) {
                ctx.error_field(
                    "parameters",
                    format!("`{{{name}}}` in the channel path is not declared in `parameters`"),
                );
            }
        }
        if !ctx.is_option(ValidationOptions::IgnoreUnusedChannelParameter) {
            for name in declared {
                if !used.contains(&name) {
                    ctx.error_field(
                        "parameters",
                        format!("`{name}` is declared but never used in the channel path"),
                    );
                }
            }
        }
    }
}

impl ChannelItem {
    /// Whether this item is (also) a reference to another one.
    #[must_use]
    pub fn is_reference(&self) -> bool {
        self.reference.is_some()
    }
}

impl ValidateWithContext for ChannelItem {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(reference) = &self.reference {
            ctx.require_non_empty("$ref", reference);
            crate::common::reference::check_external(ctx, reference);
            crate::common::resolve::check_names_something(ctx, reference);
            // A field-style `$ref` is still a reference to one kind of
            // object, and this position says which.
            crate::common::resolve::check_names_kind::<ChannelItem>(ctx, reference, "channels");
        }
        ctx.validate_map_keys("parameters", &self.parameters);

        for (i, server) in self.servers.iter().enumerate() {
            if server.is_empty() {
                ctx.in_index("servers", i, |ctx| ctx.error("must not be empty"));
            } else if self.servers[..i].contains(server) {
                ctx.in_index("servers", i, |ctx| {
                    ctx.error(format!("duplicate server `{server}`"));
                });
            }
        }
        for (kind, operation) in [("publish", &self.publish), ("subscribe", &self.subscribe)] {
            if let Some(operation) = operation {
                ctx.in_field(kind, |ctx| operation.validate_with_context(ctx));
            }
        }
        for (name, parameter) in &self.parameters {
            ctx.in_key("parameters", name, |ctx| {
                parameter.validate_with_context(ctx);
            });
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

    fn errors_against(path: &str, value: serde_json::Value) -> Vec<String> {
        let item: ChannelItem = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.channels.user");
        item.validate_with_context(&mut ctx);
        ChannelItem::validate_path_parameters(&mut ctx, path, &[&item]);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn round_trips_a_full_channel_item() {
        let value = json!({
            "description": "User events",
            "servers": ["production"],
            "publish": { "operationId": "receiveSignup", "message": { "name": "Signup" } },
            "subscribe": { "operationId": "sendWelcome", "message": { "name": "Welcome" } },
            "parameters": { "userId": { "description": "Id", "schema": { "type": "string" } } },
            "deprecated": false,
            "bindings": { "kafka": { "topic": "signups" } },
            "x-owner": "team"
        });
        let item: ChannelItem = serde_json::from_value(value.clone()).unwrap();
        assert!(item.publish.is_some() && item.subscribe.is_some());
        assert_eq!(serde_json::to_value(&item).unwrap(), value);
        assert!(errors_against("user/{userId}/signedup", value).is_empty());
    }

    #[test]
    fn path_placeholders_and_parameters_must_agree() {
        let errors = errors_against(
            "user/{userId}/{tenant}",
            json!({ "parameters": { "userId": {}, "unused": {} } }),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`{tenant}` in the channel path is not declared")),
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
        let item: ChannelItem =
            serde_json::from_value(json!({ "parameters": { "unused": {} } })).unwrap();
        let mut ctx = Context::with_path(
            EnumSet::only(ValidationOptions::IgnoreUnusedChannelParameter),
            "#.channels.user",
        );
        item.validate_with_context(&mut ctx);
        ChannelItem::validate_path_parameters(&mut ctx, "user/signedup", &[&item]);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn server_names_must_be_non_empty_and_unique() {
        let errors = errors_against("user", json!({ "servers": ["prod", "", "prod"] }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.servers[1]: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.servers[2]: duplicate server `prod`")
        );
    }

    #[test]
    fn invalid_parameter_keys_are_reported() {
        let errors = errors_against("user/{bad key}", json!({ "parameters": { "bad key": {} } }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("#.channels.user.parameters.bad key")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn nested_errors_carry_their_path() {
        let errors = errors_against(
            "user",
            json!({
                "publish": { "operationId": "" },
                "subscribe": { "tags": [ { "name": "" } ] },
                "bindings": { "kafka": 1 }
            }),
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.publish.operationId: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.channels.user.subscribe.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.channels.user.bindings.kafka"))
        );
    }
}
