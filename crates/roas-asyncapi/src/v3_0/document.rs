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
use crate::v3_0::operation::{Operation, OperationReply};
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

/// Check the declared aliases of every `components` map that has no
/// root counterpart, which is all of them but servers, channels, and
/// operations.
macro_rules! declared_components {
    ($self:ident, $ctx:ident, $components:ident, $( $field:ident => $name:literal ),+ $(,)?) => {
        $(
            $self.check_declared($ctx, $name, &$components.$field, None, Some(&$components.$field));
        )+
    };
}

/// Where an object sits in the document.
///
/// The specification's reference rules turn on this. A channel,
/// operation, or reply in the root "MUST point to a channel definition
/// located in the root Channels Object, and MUST NOT point to a channel
/// definition located in the Components Object or anywhere else", while
/// one under `components` "MAY point to a Channel Object in any
/// location".
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Root,
    Components,
}

/// A channel `$ref` that landed on a declared channel.
struct ResolvedChannel<'a> {
    /// Where the chain ended, as decoded pointer tokens.
    ///
    /// The whole path, not just the key: `#/channels/events` and
    /// `#/components/channels/events` are different channels that may
    /// both declare a message `m`, and an operation may only name the
    /// messages of the one it selected.
    at: Vec<String>,
    /// The channel itself, or `None` when the chain left the document
    /// and deeper checks cannot continue.
    channel: Option<&'a Channel>,
}

/// Render decoded tokens back into a JSON Pointer, for error messages.
fn as_pointer(path: &[String]) -> String {
    let mut out = String::from("#");
    for token in path {
        out.push('/');
        out.push_str(&token.replace('~', "~0").replace('/', "~1"));
    }
    out
}

/// The message key a pointer names, when it names a message of the
/// channel at `channel_path` — that channel's own pointer with
/// `/messages/<key>` on the end, and nothing else.
fn message_key_of<'p>(path: &'p [String], channel_path: &[String]) -> Option<&'p String> {
    let (prefix, tail) = path.split_at(path.len().checked_sub(2)?);
    match tail {
        [messages, key] if prefix == channel_path && messages == "messages" => Some(key),
        _ => None,
    }
}

impl Document {
    /// Resolve a `$ref` against `#/<field>/<key>` and
    /// `#/components/<field>/<key>`, following the chain to its end.
    ///
    /// Returns where the chain ended alongside the outcome — the path,
    /// not the key, because the caller uses it as the target's
    /// identity. `None` means the chain has no location in this
    /// document: it left for another one, or was never a pointer.
    fn resolve<'a, T>(
        &'a self,
        reference: &Reference,
        field: &str,
        inline: Option<&'a BTreeMap<String, RefOr<T>>>,
        components: Option<&'a BTreeMap<String, RefOr<T>>>,
    ) -> (Option<Vec<String>>, Resolution<'a, T>)
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
            [this, key] if this == field => inline.and_then(|map| map.get(key)),
            _ => None,
        };

        // Where the chain *ended*, not where it started: an alias and
        // its target are the same object, and a caller comparing
        // identities has to see the target's.
        let (terminal, resolution) = match lookup(&path) {
            Some(entry) => follow_tracked(self, path, entry, field, lookup),
            None => (path, classify_unresolved(self, local, field)),
        };
        (Some(terminal), resolution)
    }

    /// Check a `$ref` the specification pins to a particular place.
    ///
    /// From the root that is `#/<field>/<key>` and nothing else — not
    /// `#/components/<field>/<key>`, and not "anywhere else" either.
    /// From `components` anything goes, and this says nothing.
    ///
    /// An external reference is left alone: where it lands is inside
    /// another document, which this crate cannot see.
    fn check_location(reference: &Reference, field: &str, origin: Origin) -> Option<String> {
        if origin == Origin::Components || reference.is_external() {
            return None;
        }
        // An unusable pointer is not a *location* problem, and the
        // resolver has a better word for it.
        let path = reference.local_pointer().and_then(pointer::tokens)?;
        let rooted = matches!(path.as_slice(), [this, _] if this == field);
        (!rooted).then(|| format!("must point into the root `{field}` object"))
    }

    /// Report a declared entry that is a `$ref` leading nowhere.
    ///
    /// A dangling alias is a document bug whether or not anything uses
    /// it, and an unused one is exactly what no other check would
    /// notice.
    fn check_declared<T>(
        &self,
        ctx: &mut Context,
        field: &str,
        map: &BTreeMap<String, RefOr<T>>,
        inline: Option<&BTreeMap<String, RefOr<T>>>,
        components: Option<&BTreeMap<String, RefOr<T>>>,
    ) where
        T: DeserializeOwned,
    {
        for (key, entry) in map {
            let RefOr::Reference(reference) = entry else {
                continue;
            };
            let (_, resolution) = self.resolve(reference, field, inline, components);
            if let Some(problem) = resolution.problem() {
                ctx.in_key(field, key, |ctx| {
                    ctx.error_field("$ref", format!("`{}` {problem}", reference.reference));
                });
            }
        }
    }

    fn components_map<'a, T>(
        &'a self,
        pick: impl Fn(&'a Components) -> &'a BTreeMap<String, RefOr<T>>,
    ) -> Option<&'a BTreeMap<String, RefOr<T>>> {
        self.components.as_ref().map(pick)
    }

    /// Check that every `$ref` in `channel.servers` names a server the
    /// channel is allowed to name.
    fn validate_channel_servers(&self, ctx: &mut Context, channel: &Channel, origin: Origin) {
        for (i, server) in channel.servers.iter().enumerate() {
            let problem = Self::check_location(server, "servers", origin).or_else(|| {
                let (_, resolution) = self.resolve(
                    server,
                    "servers",
                    Some(&self.servers),
                    self.components_map(|c| &c.servers),
                );
                resolution.problem().map(ToOwned::to_owned)
            });
            if let Some(problem) = problem {
                ctx.in_index("servers", i, |ctx| {
                    ctx.error_field("$ref", format!("server `{}` {problem}", server.reference));
                });
            }
        }
    }

    /// Check an operation's `channel` and that its `messages` are a
    /// subset of that channel's messages. Returns nothing: every
    /// finding is recorded on `ctx`.
    fn validate_operation_wiring(&self, ctx: &mut Context, operation: &Operation, origin: Origin) {
        let channel = self.check_channel_ref(ctx, "channel", &operation.channel, origin);
        self.check_message_refs(ctx, "messages", &operation.messages, channel.as_ref());

        let Some(reply) = &operation.reply else {
            return;
        };
        ctx.in_field("reply", |ctx| match reply {
            // A reusable reply is wired where it is declared, so here
            // there is only the alias itself to check.
            RefOr::Reference(reference) => {
                let (_, resolution) = self.resolve(
                    reference,
                    "replies",
                    None,
                    self.components_map(|c| &c.replies),
                );
                if let Some(problem) = resolution.problem() {
                    ctx.error_field("$ref", format!("reply `{}` {problem}", reference.reference));
                }
            }
            RefOr::Item(reply) => self.validate_reply_wiring(ctx, reply, origin),
        });
    }

    /// Check a reply's `channel` and `messages`, which are wired like
    /// an operation's own.
    fn validate_reply_wiring(&self, ctx: &mut Context, reply: &OperationReply, origin: Origin) {
        let channel = reply
            .channel
            .as_ref()
            .and_then(|reference| self.check_channel_ref(ctx, "channel", reference, origin));
        self.check_message_refs(ctx, "messages", &reply.messages, channel.as_ref());
    }

    /// Resolve a channel `$ref` at `<field>`, reporting when it may not
    /// point where it does, or does not land on a declared channel.
    fn check_channel_ref<'a>(
        &'a self,
        ctx: &mut Context,
        field: &str,
        reference: &Reference,
        origin: Origin,
    ) -> Option<ResolvedChannel<'a>> {
        let mut report = |problem: &str| {
            ctx.in_field(field, |ctx| {
                ctx.error_field(
                    "$ref",
                    format!("channel `{}` {problem}", reference.reference),
                );
            });
        };
        if let Some(problem) = Self::check_location(reference, "channels", origin) {
            report(&problem);
            return None;
        }
        let (at, resolution) = self.resolve(
            reference,
            "channels",
            Some(&self.channels),
            self.components_map(|c| &c.channels),
        );
        if let Some(problem) = resolution.problem() {
            report(problem);
            return None;
        }
        // A chain that leaves the document still has an identity here —
        // the deeper checks simply stop at it.
        at.map(|at| ResolvedChannel {
            at,
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

    /// Check that each `$ref` in `messages` names one of the channel's
    /// own messages.
    ///
    /// The specification allows exactly one shape: the channel's own
    /// pointer with `/messages/<key>` on the end. A message "MUST
    /// contain a subset of the messages defined in the channel
    /// referenced in this operation, and MUST NOT point to a subset of
    /// message definitions located in the Messages Object in the
    /// Components Object or anywhere else" — so this is as much a check
    /// on where the pointer points as on what it finds there.
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
            let Some(key) = message_key_of(&path, &resolved.at) else {
                report(
                    ctx,
                    format!("must point at a message of `{}`", as_pointer(&resolved.at)),
                );
                continue;
            };

            // The channel left the document, so its messages are not
            // here to be a subset of.
            let Some(channel) = resolved.channel else {
                continue;
            };
            let Some(entry) = channel.messages.get(key) else {
                report(ctx, "is not one of the channel's `messages`".to_owned());
                continue;
            };
            if let Some(problem) = self.message_problem(entry) {
                report(ctx, problem.to_owned());
            }
        }
    }

    /// Wire the reusable objects.
    ///
    /// Wiring needs the whole document, so it cannot run from
    /// `Components` itself. A reusable channel, operation, or reply is
    /// wired exactly like an inline one — except that it may reference
    /// freely, being outside the root.
    fn validate_components_wiring(&self, ctx: &mut Context, components: &Components) {
        for (name, channel) in &components.channels {
            if let Some(channel) = channel.item() {
                ctx.in_key("channels", name, |ctx| {
                    self.validate_channel_servers(ctx, channel, Origin::Components);
                });
            }
        }
        for (name, operation) in &components.operations {
            if let Some(operation) = operation.item() {
                ctx.in_key("operations", name, |ctx| {
                    self.validate_operation_wiring(ctx, operation, Origin::Components);
                });
            }
        }
        for (name, reply) in &components.replies {
            if let Some(reply) = reply.item() {
                ctx.in_key("replies", name, |ctx| {
                    self.validate_reply_wiring(ctx, reply, Origin::Components);
                });
            }
        }

        // Every declared alias, whether or not anything uses it.
        self.check_declared(
            ctx,
            "servers",
            &components.servers,
            Some(&self.servers),
            Some(&components.servers),
        );
        self.check_declared(
            ctx,
            "channels",
            &components.channels,
            Some(&self.channels),
            Some(&components.channels),
        );
        self.check_declared(
            ctx,
            "operations",
            &components.operations,
            Some(&self.operations),
            Some(&components.operations),
        );
        // Messages resolve through channels as well as `components`,
        // so they get the message lookup rather than the generic one.
        for (key, entry) in &components.messages {
            let Some(reference) = entry.reference() else {
                continue;
            };
            if let Some(problem) = self.message_problem(entry) {
                ctx.in_key("messages", key, |ctx| {
                    ctx.error_field("$ref", format!("`{}` {problem}", reference.reference));
                });
            }
        }
        declared_components! {
            self, ctx, components,
            schemas => "schemas",
            security_schemes => "securitySchemes",
            server_variables => "serverVariables",
            parameters => "parameters",
            correlation_ids => "correlationIds",
            replies => "replies",
            reply_addresses => "replyAddresses",
            external_docs => "externalDocs",
            tags => "tags",
            operation_traits => "operationTraits",
            message_traits => "messageTraits",
            server_bindings => "serverBindings",
            channel_bindings => "channelBindings",
            operation_bindings => "operationBindings",
            message_bindings => "messageBindings",
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
        self.check_declared(
            &mut ctx,
            "servers",
            &self.servers,
            Some(&self.servers),
            self.components_map(|c| &c.servers),
        );

        ctx.validate_map_keys("channels", &self.channels);
        for (name, channel) in &self.channels {
            ctx.in_key("channels", name, |ctx| {
                channel.validate_with_context(ctx);
                if let Some(channel) = channel.item() {
                    self.validate_channel_servers(ctx, channel, Origin::Root);
                }
            });
        }
        self.check_declared(
            &mut ctx,
            "channels",
            &self.channels,
            Some(&self.channels),
            self.components_map(|c| &c.channels),
        );

        ctx.validate_map_keys("operations", &self.operations);
        for (name, operation) in &self.operations {
            ctx.in_key("operations", name, |ctx| {
                operation.validate_with_context(ctx);
                if let Some(operation) = operation.item() {
                    self.validate_operation_wiring(ctx, operation, Origin::Root);
                }
            });
        }
        self.check_declared(
            &mut ctx,
            "operations",
            &self.operations,
            Some(&self.operations),
            self.components_map(|c| &c.operations),
        );

        if let Some(components) = &self.components {
            ctx.in_field("components", |ctx| {
                components.validate_with_context(ctx);
                self.validate_components_wiring(ctx, components);
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

    /// The same wiring with the operation under `components`, where a
    /// reference "MAY point to a Channel Object in any location".
    fn wired_from_components() -> serde_json::Value {
        let mut value = wired();
        let operation = value["operations"]["receiveSignups"].take();
        value["operations"] = json!({});
        value["components"] = json!({ "operations": { "receiveSignups": operation } });
        value
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
        // From the root the location rule answers first: whatever
        // `#/servers/production` is, it is not in `#/channels`.
        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/servers/production" });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("must point into the root `channels` object")),
            "got: {errors:?}"
        );

        // From `components`, where any location is allowed, what it
        // points *at* is what disqualifies it.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
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
                "message `#/channels/other/messages/ping` must point at a message of `#/channels/userSignedUp`"
            )),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_pointer_at_a_value_of_the_wrong_shape_is_not_opaque() {
        // `$ref` may name anything, so a location this crate does not
        // model is legal — but only if what is there could be the kind
        // the position calls for.
        let mut scalar = wired_from_components();
        scalar["x-note"] = json!("just a string");
        scalar["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/x-note" });
        assert!(
            errors_for(scalar)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // A root singleton is that object however its JSON reads.
        let mut info = wired_from_components();
        info["components"]["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/info" });
        assert!(
            errors_for(info)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // So is a container: a map of channels is not a channel, however
        // willingly its JSON reads as one whose every field is absent.
        for container in ["#", "#/channels", "#/components", "#/components/channels"] {
            let mut value = wired_from_components();
            value["components"]["operations"]["receiveSignups"]["channel"] =
                json!({ "$ref": container });
            let errors = errors_for(value);
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains("does not point at an object of the expected kind")),
                "`{container}` got: {errors:?}"
            );
        }

        // A channel-shaped extension stays legal from `components`.
        let mut shaped = wired_from_components();
        shaped["x-shared-channel"] = json!({ "address": "shared" });
        shaped["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/x-shared-channel" });
        shaped["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
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
                .any(|e| e.contains("must point at a message of `#/channels/userSignedUp`")),
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
        // A reusable channel, named through `components` by an
        // operation that is allowed to, with its message named the
        // same way.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/channels/reusable" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/channels/reusable/messages/m" } ]);
        value["components"]["channels"] =
            json!({ "reusable": { "messages": { "m": { "name": "M" } } } });
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
        // so there is no message list to be a subset of. The pointer
        // still has to be one of *that* channel's messages.
        let mut value = wired();
        value["channels"]["elsewhere"] = json!({ "$ref": "./other.yaml#/channels/userSignedUp" });
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/elsewhere" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/elsewhere/messages/m" } ]);
        assert!(errors_for(value).is_empty());

        // A pointer at some other channel's message is still wrong.
        let mut value = wired();
        value["channels"]["elsewhere"] = json!({ "$ref": "./other.yaml#/channels/userSignedUp" });
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/elsewhere" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/userSignedUp/messages/signup" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `#/channels/elsewhere`")),
        );
    }

    #[test]
    fn reusable_entries_that_are_themselves_refs_are_left_alone() {
        // Wiring a `$ref` to a `$ref` is the target's business; the
        // alias itself has nothing to check.
        let mut value = wired();
        value["components"] = json!({
            "channels": { "alias": { "$ref": "#/channels/userSignedUp" } },
            "operations": { "alias": { "$ref": "#/operations/receiveSignups" } },
            "messages": {
                "real": { "name": "M" },
                "alias": { "$ref": "#/components/messages/real" }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());
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

        // A component pointer of the right shape, naming nothing, from
        // an operation allowed to use one.
        let mut missing_component = wired_from_components();
        missing_component["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/channels/nope" });
        let errors = errors_for(missing_component);
        assert!(
            errors.iter().any(|e| e
                .contains("channel `#/components/channels/nope` names nothing in this document")),
            "got: {errors:?}"
        );

        // A pointer at a location this crate does not model is legal
        // where the specification allows any location, so long as what
        // is there could be a channel.
        let mut unmodeled = wired_from_components();
        unmodeled["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/userSignedUp/messages/signup" });
        unmodeled["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert_eq!(errors_for(unmodeled), Vec::<String>::new());
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
    fn operation_messages_may_not_name_components_directly() {
        // "MUST NOT point to a subset of message definitions located in
        // the Messages Object in the Components Object or anywhere
        // else" — even when the channel lists exactly that message.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"] =
            json!({ "signup": { "$ref": "#/components/messages/signup" } });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/components/messages/signup" } ]);
        value["components"] = json!({ "messages": { "signup": { "name": "Signup" } } });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e.contains(
                "message `#/components/messages/signup` must point at a message of `#/channels/userSignedUp`"
            )),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_reusable_message_is_named_through_the_channel_that_lists_it() {
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
                    "messages": [ { "$ref": "#/channels/user/messages/signup" } ]
                }
            },
            "components": { "messages": { "signup": { "name": "Signup" } } }
        });
        assert!(errors_for(value.clone()).is_empty());

        // The channel stops listing it, so the operation's pointer no
        // longer names anything of that channel's.
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

        // From a root channel the location rule answers first.
        let mut wrong_kind = wired();
        wrong_kind["channels"]["userSignedUp"]["servers"] =
            json!([ { "$ref": "#/channels/userSignedUp" } ]);
        let errors = errors_for(wrong_kind);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("must point into the root `servers` object")),
            "got: {errors:?}"
        );

        // A reusable channel may point anywhere, so there the target
        // itself is what disqualifies it.
        let mut wrong_kind = wired();
        wrong_kind["components"] = json!({
            "channels": {
                "reusable": { "servers": [ { "$ref": "#/channels/userSignedUp" } ] }
            }
        });
        let errors = errors_for(wrong_kind);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn components_channels_and_servers_resolve_too() {
        // Both the operation and the channel are reusable, so both are
        // free to reference into `components`.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "operations": {
                    "send": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/user" }
                    }
                },
                "servers": { "prod": { "host": "h", "protocol": "kafka" } },
                "channels": {
                    "user": { "address": "user", "servers": [ { "$ref": "#/components/servers/prod" } ] }
                }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn root_objects_may_only_reference_the_root() {
        // "MUST NOT point to a subset of server definitions located in
        // the Components Object or anywhere else."
        let mut value = wired();
        value["channels"]["userSignedUp"]["servers"] =
            json!([ { "$ref": "#/components/servers/s" } ]);
        value["components"] = json!({ "servers": { "s": { "host": "h", "protocol": "kafka" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.channels.userSignedUp.servers[0].$ref: server `#/components/servers/s` must point into the root `servers` object"),
        );

        // "If the operation is located in the root Operations Object,
        // it MUST point to a channel definition located in the root
        // Channels Object."
        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/channels/c" });
        value["operations"]["receiveSignups"]["messages"] = json!([]);
        value["components"] = json!({ "channels": { "c": { "address": "c" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.operations.receiveSignups.channel.$ref: channel `#/components/channels/c` must point into the root `channels` object"),
        );

        // The same for a reply inside a root operation.
        let mut value = wired();
        value["operations"]["receiveSignups"]["reply"] =
            json!({ "channel": { "$ref": "#/components/channels/c" } });
        value["components"] = json!({ "channels": { "c": { "address": "c" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.operations.receiveSignups.reply.channel.$ref: channel `#/components/channels/c` must point into the root `channels` object"),
        );
    }

    #[test]
    fn a_dangling_alias_is_reported_even_if_nothing_uses_it() {
        for (field, alias) in [
            ("servers", "#/servers/ghost"),
            ("channels", "#/channels/ghost"),
            ("operations", "#/operations/ghost"),
        ] {
            let mut value = wired();
            value[field]["unused"] = json!({ "$ref": alias });
            let errors = errors_for(value);
            assert!(
                errors.iter().any(|e| e
                    == &format!("#.{field}.unused.$ref: `{alias}` names nothing in this document")),
                "got: {errors:?}"
            );
        }

        // And the same under `components`, messages and replies
        // included.
        let mut value = wired();
        value["components"] = json!({
            "servers": { "unused": { "$ref": "#/servers/ghost" } },
            "channels": { "unused": { "$ref": "#/channels/ghost" } },
            "operations": { "unused": { "$ref": "#/operations/ghost" } },
            "messages": { "unused": { "$ref": "#/components/messages/ghost" } },
            "replies": { "unused": { "$ref": "#/components/replies/ghost" } },
            "securitySchemes": { "unused": { "$ref": "#/components/securitySchemes/ghost" } }
        });
        let errors = errors_for(value);
        for field in [
            "servers",
            "channels",
            "operations",
            "messages",
            "replies",
            "securitySchemes",
        ] {
            assert!(
                errors.iter().any(
                    |e| e.starts_with(&format!("#.components.{field}.unused.$ref:"))
                        && e.contains("names nothing in this document")
                ),
                "{field} got: {errors:?}"
            );
        }
    }

    #[test]
    fn replies_are_wired_wherever_they_are_declared() {
        // A reusable reply is wired where it is declared, so a bad
        // channel in it is reported even though the operation that
        // uses it looks fine.
        let mut value = wired();
        value["operations"]["receiveSignups"]["reply"] =
            json!({ "$ref": "#/components/replies/shared" });
        value["components"] = json!({
            "replies": { "shared": { "channel": { "$ref": "#/channels/missing" } } }
        });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.components.replies.shared.channel.$ref: channel `#/channels/missing` names nothing in this document"),
            "got: {errors:?}"
        );

        // An alias that leads nowhere is reported at the operation.
        let mut value = wired();
        value["operations"]["receiveSignups"]["reply"] =
            json!({ "$ref": "#/components/replies/ghost" });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.operations.receiveSignups.reply.$ref: reply `#/components/replies/ghost` names nothing in this document"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_channel_is_more_than_its_key() {
        // Both namespaces declare `events`, and both declare a message
        // `m`. The operation selects the reusable one, so the root
        // one's message is not its to name.
        let mut value = wired();
        value["operations"] = json!({});
        value["channels"]["events"] = json!({ "messages": { "m": { "name": "Root" } } });
        value["components"] = json!({
            "channels": { "events": { "messages": { "m": { "name": "Reusable" } } } },
            "operations": {
                "send": {
                    "action": "send",
                    "channel": { "$ref": "#/components/channels/events" },
                    "messages": [ { "$ref": "#/channels/events/messages/m" } ]
                }
            }
        });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("must point at a message of `#/components/channels/events`")),
            "got: {errors:?}"
        );
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
