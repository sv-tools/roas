//! AsyncAPI v3.1 root document.
//!
//! Per [AsyncAPI Object](https://www.asyncapi.com/docs/reference/specification/v3.1.0#A2SObject).
//!
//! Beyond the per-object checks, the root validator resolves the
//! document's internal wiring: an operation's `channel` must name a
//! channel that exists, its `messages` must be a subset of that
//! channel's messages, and a channel's `servers` must name declared
//! servers. References into another document are skipped — following
//! them needs a loader — unless
//! [`ValidationOptions::ErrorOnExternalReference`] asks for a
//! self-contained document.

use crate::common::reference::{RefOr, Reference};
use crate::v3_1::channel::Channel;
use crate::v3_1::components::Components;
use crate::v3_1::info::Info;
use crate::v3_1::operation::Operation;
use crate::v3_1::server::Server;
use crate::v3_1::version::Version;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Root AsyncAPI v3.1 document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Document {
    /// **Required** Exactly `3.1.0` — the AsyncAPI specification
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

/// A channel `$ref` that landed on a declared channel.
struct ResolvedChannel<'a> {
    /// The channel's key, used to check that an operation's messages
    /// come from *this* channel.
    key: String,
    /// The channel itself, or `None` when the entry is a `$ref` and
    /// deeper checks cannot continue.
    channel: Option<&'a Channel>,
}

/// What a `$ref` to a document-level object turned out to be.
enum Target<'a, T> {
    /// Points outside this document; not resolvable here.
    External,
    /// A local pointer that does not name this kind of object.
    Unrecognized,
    /// A local pointer of the right shape whose key does not exist.
    Missing,
    /// Resolved. The item is `None` when the entry is itself a `$ref`,
    /// so deeper checks stop there.
    Found { key: String, item: Option<&'a T> },
}

impl Document {
    /// Resolve a `$ref` against `#/<field>/<key>` and
    /// `#/components/<field>/<key>`.
    fn resolve<'a, T>(
        &'a self,
        reference: &Reference,
        field: &str,
        inline: &'a BTreeMap<String, RefOr<T>>,
        components: Option<&'a BTreeMap<String, RefOr<T>>>,
    ) -> Target<'a, T> {
        if reference.is_external() {
            return Target::External;
        }
        let Some(pointer) = reference.local_pointer() else {
            return Target::Unrecognized;
        };

        if let Some(key) = reference.component_key(field) {
            return match components.and_then(|map| map.get(&key)) {
                Some(entry) => Target::Found {
                    key,
                    item: entry.item(),
                },
                None => Target::Missing,
            };
        }

        let Some(rest) = pointer.strip_prefix('/').and_then(|p| {
            let (head, tail) = p.split_once('/')?;
            (head == field).then_some(tail)
        }) else {
            return Target::Unrecognized;
        };
        if rest.is_empty() || rest.contains('/') {
            return Target::Unrecognized;
        }
        match inline.get(rest) {
            Some(entry) => Target::Found {
                key: rest.to_owned(),
                item: entry.item(),
            },
            None => Target::Missing,
        }
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
            let target = self.resolve(
                server,
                "servers",
                &self.servers,
                self.components_map(|c| &c.servers),
            );
            match target {
                Target::Found { .. } | Target::External => {}
                Target::Missing => ctx.in_index("servers", i, |ctx| {
                    ctx.error_field(
                        "$ref",
                        format!("server `{}` is not declared", server.reference),
                    );
                }),
                Target::Unrecognized => ctx.in_index("servers", i, |ctx| {
                    ctx.error_field(
                        "$ref",
                        format!(
                            "`{}` must point at a server (`#/servers/…` or `#/components/servers/…`)",
                            server.reference
                        ),
                    );
                }),
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
        match self.resolve(
            reference,
            "channels",
            &self.channels,
            self.components_map(|c| &c.channels),
        ) {
            Target::Found { key, item } => Some(ResolvedChannel { key, channel: item }),
            Target::External => None,
            Target::Missing => {
                ctx.in_field(field, |ctx| {
                    ctx.error_field(
                        "$ref",
                        format!("channel `{}` is not declared", reference.reference),
                    );
                });
                None
            }
            Target::Unrecognized => {
                ctx.in_field(field, |ctx| {
                    ctx.error_field(
                        "$ref",
                        format!(
                            "`{}` must point at a channel (`#/channels/…` or `#/components/channels/…`)",
                            reference.reference
                        ),
                    );
                });
                None
            }
        }
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
            let Some(pointer) = message.local_pointer() else {
                continue;
            };

            let report = |ctx: &mut Context, reason: String| {
                ctx.in_index(field, i, |ctx| ctx.error_field("$ref", reason));
            };

            // The canonical form is `#/channels/<channel>/messages/<key>`.
            if let Some((channel_key, message_key)) = pointer
                .strip_prefix("/channels/")
                .and_then(|rest| rest.split_once("/messages/"))
                .filter(|(_, key)| !key.is_empty() && !key.contains('/'))
            {
                if channel_key != resolved.key {
                    report(
                        ctx,
                        format!(
                            "message `{}` belongs to channel `{channel_key}`, not `{}`",
                            message.reference, resolved.key
                        ),
                    );
                } else if let Some(channel) = resolved.channel
                    && !channel.messages.contains_key(message_key)
                {
                    report(
                        ctx,
                        format!(
                            "message `{}` is not one of the channel's `messages`",
                            message.reference
                        ),
                    );
                }
                continue;
            }

            // A component message must exist, and count as one of the
            // channel's own messages.
            if let Some(component_key) = message.component_key("messages") {
                let declared = self
                    .components
                    .as_ref()
                    .is_some_and(|c| c.messages.contains_key(&component_key));
                if !declared {
                    report(
                        ctx,
                        format!("message `{}` is not declared", message.reference),
                    );
                } else if let Some(channel) = resolved.channel
                    && !channel.messages.values().any(|candidate| {
                        candidate
                            .reference()
                            .is_some_and(|r| r.reference == message.reference)
                    })
                {
                    report(
                        ctx,
                        format!(
                            "message `{}` is not one of the channel's `messages`",
                            message.reference
                        ),
                    );
                }
                continue;
            }

            // Any other document-local pointer cannot be a message.
            report(
                ctx,
                format!(
                    "`{}` must point at a message (`#/channels/…/messages/…` or `#/components/messages/…`)",
                    message.reference
                ),
            );
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
            ctx.in_field("components", |ctx| components.validate_with_context(ctx));
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
            "asyncapi": "3.1.0",
            "info": { "title": "Streetlights", "version": "1.0.0" }
        })
    }

    fn wired() -> serde_json::Value {
        json!({
            "asyncapi": "3.1.0",
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
        assert_eq!(doc.asyncapi, Version::V3_1_0());
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
        for version in ["2.6.0", "3.0.0"] {
            let mut value = minimal();
            value["asyncapi"] = json!(version);
            assert!(
                serde_json::from_value::<Document>(value).is_err(),
                "{version} must not parse as v3.1"
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
                == "#.operations.receiveSignups.channel.$ref: channel `#/channels/nope` is not declared"),
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
            errors.iter().any(|e| e.contains("must point at a channel")),
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
    fn unresolvable_reference_shapes_are_each_reported() {
        // An empty `$ref` is neither local nor external.
        let mut empty = wired();
        empty["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "" });
        let errors = errors_for(empty);
        assert!(errors.iter().any(|e| e.contains("must point at a channel")));

        // A component pointer of the right shape, naming nothing.
        let mut missing_component = wired();
        missing_component["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/channels/nope" });
        let errors = errors_for(missing_component);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("channel `#/components/channels/nope` is not declared")),
            "got: {errors:?}"
        );

        // A local pointer that is too deep to name a channel.
        let mut too_deep = wired();
        too_deep["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/userSignedUp/messages/signup" });
        let errors = errors_for(too_deep);
        assert!(errors.iter().any(|e| e.contains("must point at a channel")));
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
            "asyncapi": "3.1.0",
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
                == "#.channels.userSignedUp.servers[0].$ref: server `#/servers/staging` is not declared"),
            "got: {errors:?}"
        );

        let mut wrong_kind = wired();
        wrong_kind["channels"]["userSignedUp"]["servers"] =
            json!([ { "$ref": "#/channels/userSignedUp" } ]);
        let errors = errors_for(wrong_kind);
        assert!(errors.iter().any(|e| e.contains("must point at a server")));
    }

    #[test]
    fn components_channels_and_servers_resolve_too() {
        let value = json!({
            "asyncapi": "3.1.0",
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
            "asyncapi": "3.1.0",
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
            "asyncapi": "3.1.0",
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
                == "#.operations.receiveSignups.reply.channel.$ref: channel `#/channels/missing` is not declared"),
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
            "asyncapi": "3.1.0",
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
