//! AsyncAPI v3.0 root document.
//!
//! Per [AsyncAPI Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#A2SObject).
//!
//! Beyond the per-object checks, the root validator resolves the
//! document's internal wiring: an operation's `channel` must name a
//! channel that exists, its `messages` must be a subset of that
//! channel's messages, and a channel's `servers` must name declared
//! servers. References into another document are skipped — following
//! them needs a loader — unless
//! [`ValidationOptions::ErrorOnExternalReference`] asks for a
//! self-contained document.

use crate::common::pointer;
use crate::common::reference::{RefOr, Reference};
use crate::common::resolve::{Resolution, classify_unresolved, follow, follow_tracked};
use crate::v3_0::channel::Channel;
use crate::v3_0::components::Components;
use crate::v3_0::info::Info;
use crate::v3_0::message::Message;
use crate::v3_0::operation::Operation;
use crate::v3_0::server::Server;
use crate::v3_0::version::Version;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Root AsyncAPI v3.0 document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Document {
    /// **Required** Exactly `3.0.0` — the AsyncAPI specification
    /// version, which the schema pins with `const`.
    pub asyncapi: Version,

    /// A unique identifier of the application this document describes,
    /// as a URN or URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// **Required** Metadata about the API.
    pub info: Info,

    /// The servers the application connects to, keyed by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, RefOr<Server>>,

    /// The default content type to use when one is not set on a
    /// message.
    #[serde(rename = "defaultContentType", skip_serializing_if = "Option::is_none")]
    pub default_content_type: Option<String>,

    /// The channels used by this application, keyed by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub channels: BTreeMap<String, RefOr<Channel>>,

    /// The operations this application performs, keyed by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub operations: BTreeMap<String, RefOr<Operation>>,

    /// Reusable objects referenced throughout the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,

    /// `x-`-prefixed Specification Extensions on the root.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// The component key a pointer names, when it names one of `field`'s
/// entries — either `#/<field>/<key>` or `#/components/<field>/<key>`.
fn key_in(path: &[String], field: &str) -> Option<String> {
    match path {
        [c, this, key] if c == "components" && this == field => Some(key.clone()),
        [this, key] if this == field => Some(key.clone()),
        _ => None,
    }
}

/// A channel `$ref` that landed on a declared channel.
struct ResolvedChannel<'a> {
    /// The channel's key, used to check that an operation's messages
    /// come from *this* channel.
    key: String,
    /// The channel itself, or `None` when the entry is a `$ref` and
    /// deeper checks cannot continue.
    channel: Option<&'a Channel>,
}

impl Document {
    /// Resolve a `$ref` against `#/<field>/<key>` and
    /// `#/components/<field>/<key>`, following the chain to its end.
    ///
    /// Returns the key the pointer named alongside the outcome: the
    /// caller needs it to check that an operation's messages come from
    /// the channel it names.
    fn resolve<'a, T>(
        &'a self,
        reference: &Reference,
        field: &str,
        inline: &'a BTreeMap<String, RefOr<T>>,
        components: Option<&'a BTreeMap<String, RefOr<T>>>,
    ) -> (Option<String>, Resolution<'a, T>)
    where
        T: DeserializeOwned,
    {
        if reference.is_external() {
            return (None, Resolution::Opaque);
        }
        let Some(local) = reference.local_pointer() else {
            return (None, Resolution::Unrecognized);
        };
        let Some(path) = pointer::tokens(local) else {
            return (None, Resolution::Unrecognized);
        };

        let lookup = |path: &[String]| match path {
            [c, this, key] if c == "components" && this == field => {
                components.and_then(|map| map.get(key))
            }
            [this, key] if this == field => inline.get(key),
            _ => None,
        };

        // The key comes from where the chain *ended*, not where it
        // started: an alias and its target are the same object, and a
        // caller comparing keys has to see the target's.
        let (terminal, resolution) = match lookup(&path) {
            Some(entry) => follow_tracked(self, path, entry, field, lookup),
            None => (path, classify_unresolved(self, local, field)),
        };
        (key_in(&terminal, field), resolution)
    }

    fn components_map<'a, T>(
        &'a self,
        pick: impl Fn(&'a Components) -> &'a BTreeMap<String, RefOr<T>>,
    ) -> Option<&'a BTreeMap<String, RefOr<T>>> {
        self.components.as_ref().map(pick)
    }

    /// Check that every `$ref` in `channel.servers` names a server.
    fn validate_channel_servers(&self, ctx: &mut Context, channel: &Channel) {
        for (i, server) in channel.servers.iter().enumerate() {
            let (_, resolution) = self.resolve(
                server,
                "servers",
                &self.servers,
                self.components_map(|c| &c.servers),
            );
            if let Some(problem) = resolution.problem() {
                ctx.in_index("servers", i, |ctx| {
                    ctx.error_field("$ref", format!("server `{}` {problem}", server.reference));
                });
            }
        }
    }

    /// Check an operation's `channel` and that its `messages` are a
    /// subset of that channel's messages. Returns nothing: every
    /// finding is recorded on `ctx`.
    fn validate_operation_wiring(&self, ctx: &mut Context, operation: &Operation) {
        let channel = self.check_channel_ref(ctx, "channel", &operation.channel);
        self.check_message_refs(ctx, "messages", &operation.messages, channel.as_ref());

        if let Some(reply) = operation.reply.as_ref().and_then(RefOr::item) {
            ctx.in_field("reply", |ctx| {
                let reply_channel = reply
                    .channel
                    .as_ref()
                    .and_then(|reference| self.check_channel_ref(ctx, "channel", reference));
                self.check_message_refs(ctx, "messages", &reply.messages, reply_channel.as_ref());
            });
        }
    }

    /// Resolve a channel `$ref` at `<field>`, reporting when it does not
    /// land on a declared channel. Returns the channel when it is
    /// inline and resolvable.
    fn check_channel_ref<'a>(
        &'a self,
        ctx: &mut Context,
        field: &str,
        reference: &Reference,
    ) -> Option<ResolvedChannel<'a>> {
        let (key, resolution) = self.resolve(
            reference,
            "channels",
            &self.channels,
            self.components_map(|c| &c.channels),
        );
        if let Some(problem) = resolution.problem() {
            ctx.in_field(field, |ctx| {
                ctx.error_field(
                    "$ref",
                    format!("channel `{}` {problem}", reference.reference),
                );
            });
            return None;
        }
        // A chain that leaves the document, or lands somewhere this
        // crate does not model, still names a key — the deeper checks
        // simply stop there.
        key.map(|key| ResolvedChannel {
            key,
            channel: resolution.found(),
        })
    }

    /// The message entry a pointer names, wherever messages may live:
    /// a channel's own map, inline or under `components`, or the
    /// reusable `components.messages`.
    fn message_entry(&self, path: &[String]) -> Option<&RefOr<Message>> {
        fn channel_message<'a>(
            channels: &'a BTreeMap<String, RefOr<Channel>>,
            name: &str,
            key: &str,
        ) -> Option<&'a RefOr<Message>> {
            channels.get(name)?.item()?.messages.get(key)
        }
        match path {
            [c, m, key] if c == "components" && m == "messages" => {
                self.components.as_ref()?.messages.get(key)
            }
            [c, ch, name, m, key] if c == "components" && ch == "channels" && m == "messages" => {
                channel_message(&self.components.as_ref()?.channels, name, key)
            }
            [ch, name, m, key] if ch == "channels" && m == "messages" => {
                channel_message(&self.channels, name, key)
            }
            _ => None,
        }
    }

    /// Why following a message entry to its end is a document bug, if
    /// it is one.
    fn message_problem(&self, entry: &RefOr<Message>) -> Option<&'static str> {
        follow(self, entry, "messages", |path| self.message_entry(path)).problem()
    }

    /// Check a message named through the channel it belongs to, i.e.
    /// `#/channels/<name>/messages/<key>`.
    fn channel_message_problem(
        &self,
        resolved: &ResolvedChannel<'_>,
        channel_name: &str,
        key: &str,
    ) -> Option<String> {
        if channel_name != resolved.key {
            return Some(format!(
                "belongs to channel `{channel_name}`, not `{}`",
                resolved.key
            ));
        }
        // With the channel itself unresolvable there is nothing left to
        // compare against; its own `$ref` is reported where it sits.
        let channel = resolved.channel?;
        let Some(entry) = channel.messages.get(key) else {
            return Some("is not one of the channel's `messages`".to_owned());
        };
        self.message_problem(entry).map(ToOwned::to_owned)
    }

    /// Check a message named through `components`, which must also be
    /// one of the channel's own.
    fn component_message_problem(
        &self,
        resolved: &ResolvedChannel<'_>,
        key: &str,
    ) -> Option<String> {
        let entry = self.components.as_ref().and_then(|c| c.messages.get(key));
        let Some(entry) = entry else {
            return Some("is not declared".to_owned());
        };
        if let Some(problem) = self.message_problem(entry) {
            return Some(problem.to_owned());
        }
        let channel = resolved.channel?;
        // Compare decoded keys: `#/components/messages/user%2Dsignup`
        // and `#/components/messages/user-signup` are the same message.
        let listed = channel.messages.values().any(|candidate| {
            candidate
                .reference()
                .and_then(|r| r.component_key("messages"))
                .is_some_and(|candidate| candidate == key)
        });
        (!listed).then(|| "is not one of the channel's `messages`".to_owned())
    }

    /// Check that each `$ref` in `messages` names a message of
    /// `channel`. Skipped entirely when the channel could not be
    /// resolved (external, or itself a `$ref`).
    fn check_message_refs(
        &self,
        ctx: &mut Context,
        field: &str,
        messages: &[Reference],
        channel: Option<&ResolvedChannel<'_>>,
    ) {
        let Some(resolved) = channel else { return };
        for (i, message) in messages.iter().enumerate() {
            if message.is_external() || message.reference.is_empty() {
                continue;
            }

            let report = |ctx: &mut Context, reason: String| {
                ctx.in_index(field, i, |ctx| {
                    ctx.error_field("$ref", format!("message `{}` {reason}", message.reference));
                });
            };

            let Some(path) = message.local_pointer().and_then(pointer::tokens) else {
                report(ctx, "is not a usable JSON Pointer".to_owned());
                continue;
            };

            let problem = match path.as_slice() {
                [c, ch, name, m, key]
                    if c == "components" && ch == "channels" && m == "messages" =>
                {
                    self.channel_message_problem(resolved, name, key)
                }
                [ch, name, m, key] if ch == "channels" && m == "messages" => {
                    self.channel_message_problem(resolved, name, key)
                }
                [c, m, key] if c == "components" && m == "messages" => {
                    self.component_message_problem(resolved, key)
                }
                // Any other document-local pointer cannot be a message.
                _ => Some(
                    "must point at a message (`#/channels/…/messages/…` or `#/components/messages/…`)"
                        .to_owned(),
                ),
            };
            if let Some(problem) = problem {
                report(ctx, problem);
            }
        }
    }

    fn validate_inner(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        let mut ctx = Context::new(options);

        if let Some(id) = &self.id {
            ctx.require_non_empty("id", id);
        }
        if let Some(content_type) = &self.default_content_type {
            ctx.require_non_empty("defaultContentType", content_type);
        }

        ctx.in_field("info", |ctx| self.info.validate_with_context(ctx));

        ctx.validate_map_keys("servers", &self.servers);
        for (name, server) in &self.servers {
            ctx.in_key("servers", name, |ctx| server.validate_with_context(ctx));
        }

        ctx.validate_map_keys("channels", &self.channels);
        for (name, channel) in &self.channels {
            ctx.in_key("channels", name, |ctx| {
                channel.validate_with_context(ctx);
                if let Some(channel) = channel.item() {
                    self.validate_channel_servers(ctx, channel);
                }
            });
        }

        ctx.validate_map_keys("operations", &self.operations);
        for (name, operation) in &self.operations {
            ctx.in_key("operations", name, |ctx| {
                operation.validate_with_context(ctx);
                if let Some(operation) = operation.item() {
                    self.validate_operation_wiring(ctx, operation);
                }
            });
        }

        if let Some(components) = &self.components {
            ctx.in_field("components", |ctx| {
                components.validate_with_context(ctx);
                // Wiring needs the whole document, so it cannot run from
                // `Components` itself — and a reusable channel or
                // operation is wired exactly like an inline one.
                for (name, channel) in &components.channels {
                    if let Some(channel) = channel.item() {
                        ctx.in_key("channels", name, |ctx| {
                            self.validate_channel_servers(ctx, channel);
                        });
                    }
                }
                for (name, operation) in &components.operations {
                    if let Some(operation) = operation.item() {
                        ctx.in_key("operations", name, |ctx| {
                            self.validate_operation_wiring(ctx, operation);
                        });
                    }
                }
            });
        }

        ctx.into_result()
    }
}

impl Validate for Document {
    fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        self.validate_inner(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal() -> serde_json::Value {
        json!({
            "asyncapi": "3.0.0",
            "info": { "title": "Streetlights", "version": "1.0.0" }
        })
    }

    fn wired() -> serde_json::Value {
        json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "servers": { "production": { "host": "broker:9092", "protocol": "kafka" } },
            "channels": {
                "userSignedUp": {
                    "address": "user/signedup",
                    "servers": [ { "$ref": "#/servers/production" } ],
                    "messages": { "signup": { "name": "UserSignedUp" } }
                }
            },
            "operations": {
                "receiveSignups": {
                    "action": "receive",
                    "channel": { "$ref": "#/channels/userSignedUp" },
                    "messages": [ { "$ref": "#/channels/userSignedUp/messages/signup" } ]
                }
            }
        })
    }

    fn errors_for(value: serde_json::Value) -> Vec<String> {
        let doc: Document = serde_json::from_value(value).unwrap();
        match doc.validate(EnumSet::empty()) {
            Ok(()) => Vec::new(),
            Err(err) => err.errors.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn minimal_document_parses_validates_and_round_trips() {
        let doc: Document = serde_json::from_value(minimal()).unwrap();
        assert_eq!(doc.asyncapi, Version::V3_0_0());
        doc.validate(EnumSet::empty()).expect("valid");
        assert_eq!(serde_json::to_value(&doc).unwrap(), minimal());
    }

    #[test]
    fn fully_wired_document_validates() {
        let errors = errors_for(wired());
        assert!(errors.is_empty(), "got: {errors:?}");
    }

    #[test]
    fn rejects_other_spec_versions_at_parse_time() {
        for version in ["2.6.0", "3.1.0"] {
            let mut value = minimal();
            value["asyncapi"] = json!(version);
            assert!(
                serde_json::from_value::<Document>(value).is_err(),
                "{version} must not parse as v3.0"
            );
        }
    }

    #[test]
    fn empty_id_and_default_content_type_are_reported() {
        let mut value = minimal();
        value["id"] = json!("");
        value["defaultContentType"] = json!("");
        let errors = errors_for(value);
        assert!(errors.iter().any(|e| e == "#.id: must not be empty"));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.defaultContentType: must not be empty")
        );
    }

    #[test]
    fn operation_channel_must_be_declared() {
        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/channels/nope" });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.operations.receiveSignups.channel.$ref: channel `#/channels/nope` names nothing in this document"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn operation_channel_must_point_at_a_channel() {
        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/servers/production" });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn operation_messages_must_belong_to_the_channel() {
        let mut value = wired();
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/userSignedUp/messages/other" } ]);
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is not one of the channel's `messages`")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn operation_messages_must_come_from_its_own_channel() {
        let mut value = wired();
        value["channels"]["other"] = json!({
            "address": "other",
            "messages": { "ping": { "name": "Ping" } }
        });
        // A message that exists — but on a different channel.
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/other/messages/ping" } ]);
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e.contains(
                "message `#/channels/other/messages/ping` belongs to channel `other`, not `userSignedUp`"
            )),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_pointer_at_a_value_of_the_wrong_shape_is_not_opaque() {
        // `$ref` may name anything, so a location this crate does not
        // model is legal — but only if what is there could be the kind
        // the position calls for.
        let mut scalar = wired();
        scalar["x-note"] = json!("just a string");
        scalar["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/x-note" });
        assert!(
            errors_for(scalar)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // A root singleton is that object however its JSON reads.
        let mut info = wired();
        info["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/info" });
        assert!(
            errors_for(info)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // A channel-shaped extension stays legal.
        let mut shaped = wired();
        shaped["x-shared-channel"] = json!({ "address": "shared" });
        shaped["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/x-shared-channel" });
        shaped["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(errors_for(shaped).is_empty());
    }

    #[test]
    fn message_pointers_are_decoded_before_they_are_compared() {
        // `%2D` is an unreserved character, so this names `sign-up`
        // (RFC 3986 §2.3) and must be accepted.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "sign-up": { "name": "UserSignedUp" } });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/userSignedUp/messages/sign%2Dup" } ]);
        assert!(errors_for(value).is_empty());

        // The channel side decodes the same way, so the operation and
        // its messages still agree on which channel they mean.
        let mut value = wired();
        value["channels"] = json!({
            "user-signed": { "messages": { "sign-up": { "name": "UserSignedUp" } } }
        });
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/user%2Dsigned" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/user-signed/messages/sign%2Dup" } ]);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn a_channels_message_is_followed_to_its_end() {
        // The channel lists the message, but the entry it lists leads
        // nowhere — checking the key alone would miss it.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/components/messages/ghost" } });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("names nothing in this document")),
            "got: {errors:?}"
        );

        // The same entry resolving to a declared component is fine.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/components/messages/real" } });
        value["components"] = json!({ "messages": { "real": { "name": "UserSignedUp" } } });
        assert!(errors_for(value).is_empty());

        // A declared component message is followed too: being declared
        // is not the same as leading anywhere.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/components/messages/alias" } });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/messages/alias" } ]);
        value["components"] =
            json!({ "messages": { "alias": { "$ref": "#/components/messages/ghost" } } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("names nothing in this document")),
        );
    }

    #[test]
    fn a_channel_alias_is_the_channel_it_names() {
        // The operation reaches the channel through an alias; its
        // messages name the channel directly. Both are the same
        // channel, so this is wiring, not a mismatch.
        let mut value = wired();
        value["channels"]["alias"] = json!({ "$ref": "#/channels/userSignedUp" });
        value["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/channels/alias" });
        assert!(errors_for(value).is_empty());

        // A message of some *other* channel is still a mismatch.
        let mut value = wired();
        value["channels"]["alias"] = json!({ "$ref": "#/channels/userSignedUp" });
        value["channels"]["other"] = json!({ "messages": { "m": { "name": "M" } } });
        value["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/channels/alias" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/other/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("belongs to channel `other`, not `userSignedUp`")),
        );
    }

    #[test]
    fn reusable_channels_and_operations_are_wired_too() {
        // A reusable channel's servers are checked where it is
        // declared, whether or not anything references it.
        let mut value = wired();
        value["components"] = json!({
            "channels": { "reusable": { "servers": [ { "$ref": "#/servers/missing" } ] } }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.channels.reusable.servers[0].$ref: server `#/servers/missing` names nothing in this document"),
        );

        // And so is a reusable operation's channel.
        let mut value = wired();
        value["components"] = json!({
            "operations": {
                "reusable": { "action": "send", "channel": { "$ref": "#/channels/missing" } }
            }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.operations.reusable.channel.$ref: channel `#/channels/missing` names nothing in this document"),
        );
    }

    #[test]
    fn messages_are_found_wherever_the_channel_lives() {
        // A reusable channel, named through `components`, with its
        // message named the same way.
        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/channels/reusable" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/channels/reusable/messages/m" } ]);
        value["components"] = json!({
            "channels": { "reusable": { "messages": { "m": { "name": "M" } } } }
        });
        assert!(errors_for(value).is_empty());

        // One channel's message entry may point at another channel's,
        // and that chain is followed too.
        let mut value = wired();
        value["channels"]["other"] = json!({ "messages": { "m": { "name": "M" } } });
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/channels/other/messages/m" } });
        assert!(errors_for(value).is_empty());

        // …including into `components`, and including when it leads
        // nowhere.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/components/channels/reusable/messages/m" } });
        value["components"] = json!({
            "channels": { "reusable": { "messages": { "m": { "name": "M" } } } }
        });
        assert!(errors_for(value).is_empty());

        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/channels/other/messages/m" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("names nothing in this document")),
        );

        // An entry pointing somewhere that is not a message at all is
        // judged like any other pointer.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] = json!({ "signup": { "$ref": "#/info" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );
    }

    #[test]
    fn a_message_pointer_that_is_not_a_pointer_is_reported() {
        let mut value = wired();
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/bad~2escape/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("is not a usable JSON Pointer")),
        );
    }

    #[test]
    fn message_checks_stop_where_the_channel_does() {
        // The channel is declared here but leads out of the document,
        // so there is no message list to compare against — the
        // component message is checked for existence and left at that.
        let mut value = wired();
        value["channels"]["elsewhere"] = json!({ "$ref": "./other.yaml#/channels/userSignedUp" });
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/elsewhere" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/messages/real" } ]);
        value["components"] = json!({ "messages": { "real": { "name": "M" } } });
        assert!(errors_for(value).is_empty());

        // A component message that is not declared is still caught.
        let mut value = wired();
        value["channels"]["elsewhere"] = json!({ "$ref": "./other.yaml#/channels/userSignedUp" });
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/elsewhere" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/messages/ghost" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("is not declared")),
        );
    }

    #[test]
    fn reusable_entries_that_are_themselves_refs_are_left_alone() {
        // Wiring a `$ref` to a `$ref` is the target's business; the
        // alias itself has nothing to check.
        let mut value = wired();
        value["components"] = json!({
            "channels": { "alias": { "$ref": "#/channels/userSignedUp" } },
            "operations": { "alias": { "$ref": "#/operations/receiveSignups" } }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn unresolvable_reference_shapes_are_each_reported() {
        // An empty `$ref` is neither local nor external, so it is not
        // a usable pointer.
        let mut empty = wired();
        empty["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "" });
        let errors = errors_for(empty);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is not a usable JSON Pointer"))
        );

        // A malformed escape is undefined rather than literal, so the
        // pointer names nothing it could name.
        let mut malformed = wired();
        malformed["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/bad~2escape" });
        assert!(
            errors_for(malformed)
                .iter()
                .any(|e| e.contains("is not a usable JSON Pointer"))
        );

        // A component pointer of the right shape, naming nothing.
        let mut missing_component = wired();
        missing_component["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/channels/nope" });
        let errors = errors_for(missing_component);
        assert!(
            errors.iter().any(|e| e
                .contains("channel `#/components/channels/nope` names nothing in this document")),
            "got: {errors:?}"
        );

        // A pointer at a *different* declared kind is wrong, and says
        // so — unlike one at a location this crate does not model,
        // which is legal.
        let mut wrong_kind = wired();
        wrong_kind["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/servers/production" });
        assert!(
            errors_for(wrong_kind)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        let mut unmodeled = wired();
        unmodeled["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/userSignedUp/messages/signup" });
        assert!(errors_for(unmodeled).is_empty());
    }

    #[test]
    fn only_unresolvable_message_refs_are_skipped() {
        // External and empty refs cannot be checked here…
        let mut value = wired();
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "./messages.yaml#/signup" }, { "$ref": "" } ]);
        let errors = errors_for(value);
        assert!(
            !errors.iter().any(|e| e.contains("must point at a message")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn local_message_refs_of_the_wrong_kind_are_reported() {
        // …but a local pointer that names something other than a
        // message is resolvable, and wrong.
        let mut value = wired();
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/schemas/notAMessage" } ]);
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e.contains("must point at a message")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn component_message_refs_must_be_declared() {
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/components/messages/ghost" } });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/messages/ghost" } ]);
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("message `#/components/messages/ghost` is not declared")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn component_messages_count_when_the_channel_lists_them() {
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "user": {
                    "address": "user",
                    "messages": { "signup": { "$ref": "#/components/messages/signup" } }
                }
            },
            "operations": {
                "send": {
                    "action": "send",
                    "channel": { "$ref": "#/channels/user" },
                    "messages": [ { "$ref": "#/components/messages/signup" } ]
                }
            },
            "components": { "messages": { "signup": { "name": "Signup" } } }
        });
        assert!(errors_for(value.clone()).is_empty());

        // The same component message, not listed on the channel.
        let mut detached = value;
        detached["channels"]["user"]["messages"] = json!({});
        let errors = errors_for(detached);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is not one of the channel's `messages`")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn channel_servers_must_be_declared() {
        let mut value = wired();
        value["channels"]["userSignedUp"]["servers"] = json!([ { "$ref": "#/servers/staging" } ]);
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.channels.userSignedUp.servers[0].$ref: server `#/servers/staging` names nothing in this document"),
            "got: {errors:?}"
        );

        let mut wrong_kind = wired();
        wrong_kind["channels"]["userSignedUp"]["servers"] =
            json!([ { "$ref": "#/channels/userSignedUp" } ]);
        let errors = errors_for(wrong_kind);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind"))
        );
    }

    #[test]
    fn components_channels_and_servers_resolve_too() {
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "operations": {
                "send": {
                    "action": "send",
                    "channel": { "$ref": "#/components/channels/user" }
                }
            },
            "components": {
                "servers": { "prod": { "host": "h", "protocol": "kafka" } },
                "channels": {
                    "user": { "address": "user", "servers": [ { "$ref": "#/components/servers/prod" } ] }
                }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn external_references_are_skipped_unless_strictness_is_requested() {
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "operations": {
                "send": { "action": "send", "channel": { "$ref": "./other.yaml#/channels/user" } }
            }
        });
        assert!(errors_for(value.clone()).is_empty());

        let doc: Document = serde_json::from_value(value).unwrap();
        let err = doc
            .validate(EnumSet::only(ValidationOptions::ErrorOnExternalReference))
            .unwrap_err();
        assert!(
            err.errors.iter().any(|e| e.contains("external reference")),
            "got: {err}"
        );
    }

    #[test]
    fn a_channel_that_is_itself_a_ref_stops_deeper_checks() {
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user": { "$ref": "./channels.yaml#/user" } },
            "operations": {
                "send": {
                    "action": "send",
                    "channel": { "$ref": "#/channels/user" },
                    "messages": [ { "$ref": "#/channels/user/messages/anything" } ]
                }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn reply_wiring_is_checked_like_the_operation_itself() {
        let mut value = wired();
        value["operations"]["receiveSignups"]["reply"] = json!({
            "channel": { "$ref": "#/channels/missing" },
            "messages": [ { "$ref": "#/channels/userSignedUp/messages/signup" } ]
        });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.operations.receiveSignups.reply.channel.$ref: channel `#/channels/missing` names nothing in this document"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn invalid_top_level_map_keys_are_reported() {
        let mut value = minimal();
        value["channels"] = json!({ "bad key": { "address": "a" } });
        value["servers"] = json!({ "also bad": { "host": "h", "protocol": "p" } });
        value["operations"] = json!({ "worse key": { "action": "send", "channel": { "$ref": "#/channels/bad key" } } });
        let errors = errors_for(value);
        assert!(errors.iter().any(|e| e.contains("#.channels.bad key")));
        assert!(errors.iter().any(|e| e.contains("#.servers.also bad")));
        assert!(errors.iter().any(|e| e.contains("#.operations.worse key")));
    }

    #[test]
    fn nested_object_errors_still_surface_from_the_root() {
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "", "version": "" },
            "components": { "servers": { "s": { "host": "", "protocol": "" } } }
        });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e == "#.info.title: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.components.servers.s.host: must not be empty")
        );
    }

    #[test]
    fn full_document_round_trips_through_json() {
        let doc: Document = serde_json::from_value(wired()).unwrap();
        let json = serde_json::to_string(&doc).unwrap();
        let reparsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed, doc);
    }
}
