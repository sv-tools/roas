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

use crate::common::bindings::Bindings;
use crate::common::pointer;
use crate::common::reference::{RefOr, Reference};
use crate::common::resolve::{Resolution, Terminus, classify_unresolved, follow, follow_tracked};
use crate::v3_0::channel::Channel;
use crate::v3_0::components::Components;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::info::Info;
use crate::v3_0::message::{Message, MessageTrait};
use crate::v3_0::operation::{Operation, OperationReply, OperationTrait};
use crate::v3_0::server::Server;
use crate::v3_0::tag::Tag;
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
    /// Where the chain ended: the document it ended in as well as the
    /// pointer within it.
    ///
    /// The whole identity, not just the key. `#/channels/events` and
    /// `#/components/channels/events` are different channels that may
    /// both declare a message `m`, and a channel reached through an
    /// external alias declares its messages over there, not here.
    at: Terminus,
    /// The channel itself, or `None` when the chain left the document
    /// and deeper checks cannot continue.
    channel: Option<&'a Channel>,
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
    ) -> (Option<Terminus>, Resolution<'a, T>)
    where
        T: DeserializeOwned,
    {
        if reference.is_external() {
            // Another document, but still an identity: a message of
            // that channel is named over there too.
            return match Terminus::parse(&reference.reference) {
                Some(terminus) => (Some(terminus), Resolution::Opaque),
                None => (None, Resolution::Unrecognized),
            };
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
            None => (
                Terminus {
                    resource: String::new(),
                    at: path,
                },
                classify_unresolved(self, local, field),
            ),
        };
        (Some(terminal), resolution)
    }

    /// Check a `$ref` the specification pins to a particular place.
    ///
    /// From the root that is `#/<field>/<key>` and nothing else — not
    /// `#/components/<field>/<key>`, and not "anywhere else" either.
    /// From `components` anything goes, and this says nothing.
    ///
    /// An external reference is not exempt. "Anywhere else" includes
    /// another document, and which document a reference leaves for is
    /// visible in the reference itself — no fetching required.
    fn check_location(reference: &Reference, field: &str, origin: Origin) -> Option<String> {
        if origin == Origin::Components {
            return None;
        }
        if reference.is_external() {
            return Some(format!("must point into the root `{field}` object"));
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
            if let Some(problem) = resolution.kind_problem() {
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
    /// an operation's own, and that its channel is one a reply address
    /// may be substituted into.
    fn validate_reply_wiring(&self, ctx: &mut Context, reply: &OperationReply, origin: Origin) {
        let channel = reply
            .channel
            .as_ref()
            .and_then(|reference| self.check_channel_ref(ctx, "channel", reference, origin));
        self.check_message_refs(ctx, "messages", &reply.messages, channel.as_ref());

        // "When address is specified, the address property of the
        // channel referenced by this property MUST be either null or
        // not defined" — the reply address is the address, so the
        // channel must not also name one.
        if reply.address.is_some()
            && let Some(channel) = channel.as_ref().and_then(|resolved| resolved.channel)
            && let Some(Some(address)) = channel.address.as_ref()
        {
            ctx.error_field(
                "address",
                format!(
                    "requires the channel's `address` to be `null` or absent, but it is `{address}`"
                ),
            );
        }
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

    /// Whether a message entry leads somewhere that is not a message.
    fn message_kind_problem(&self, entry: &RefOr<Message>) -> Option<&'static str> {
        follow(self, entry, "messages", |path| self.message_entry(path)).kind_problem()
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
            if message.reference.is_empty() {
                continue;
            }

            let report = |ctx: &mut Context, reason: String| {
                ctx.in_index(field, i, |ctx| {
                    ctx.error_field("$ref", format!("message `{}` {reason}", message.reference));
                });
            };

            let Some(named) = Terminus::parse(&message.reference) else {
                report(ctx, "is not a usable JSON Pointer".to_owned());
                continue;
            };
            let Some(key) = resolved.at.child_key("messages", &named) else {
                report(ctx, format!("must point at a message of `{}`", resolved.at));
                continue;
            };

            // The channel is declared in another document, so its
            // messages are not here to be a subset of.
            let Some(channel) = resolved.channel else {
                continue;
            };
            if !channel.messages.contains_key(key) {
                report(ctx, "is not one of the channel's `messages`".to_owned());
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
        for (name, server) in &components.servers {
            if let Some(server) = server.item() {
                ctx.in_key("servers", name, |ctx| {
                    self.check_server_references(ctx, server)
                });
            }
        }
        for (name, channel) in &components.channels {
            if let Some(channel) = channel.item() {
                ctx.in_key("channels", name, |ctx| {
                    self.validate_channel_servers(ctx, channel, Origin::Components);
                    self.check_channel_references(ctx, channel);
                });
            }
        }
        for (name, operation) in &components.operations {
            if let Some(operation) = operation.item() {
                ctx.in_key("operations", name, |ctx| {
                    self.validate_operation_wiring(ctx, operation, Origin::Components);
                    self.check_operation_references(ctx, operation);
                });
            }
        }
        for (name, message) in &components.messages {
            if let Some(message) = message.item() {
                ctx.in_key("messages", name, |ctx| {
                    self.check_message_references(ctx, message)
                });
            }
        }
        for (name, operation_trait) in &components.operation_traits {
            if let Some(operation_trait) = operation_trait.item() {
                ctx.in_key("operationTraits", name, |ctx| {
                    self.check_operation_trait_references(ctx, operation_trait);
                });
            }
        }
        for (name, message_trait) in &components.message_traits {
            if let Some(message_trait) = message_trait.item() {
                ctx.in_key("messageTraits", name, |ctx| {
                    self.check_message_trait_references(ctx, message_trait);
                });
            }
        }
        for (name, reply) in &components.replies {
            if let Some(reply) = reply.item() {
                ctx.in_key("replies", name, |ctx| {
                    self.validate_reply_wiring(ctx, reply, Origin::Components);
                    self.check_reply_references(ctx, reply);
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
            if let Some(problem) = self.message_kind_problem(entry) {
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

    /// Report a declared reference that leads nowhere.
    ///
    /// Only a reference is checked here: an inline object is validated
    /// where it sits.
    fn check_entry<T>(
        &self,
        ctx: &mut Context,
        entry: &RefOr<T>,
        kind: &str,
        components: Option<&BTreeMap<String, RefOr<T>>>,
    ) where
        T: DeserializeOwned,
    {
        let RefOr::Reference(reference) = entry else {
            return;
        };
        let (_, resolution) = self.resolve(reference, kind, None, components);
        if let Some(problem) = resolution.kind_problem() {
            ctx.error_field("$ref", format!("`{}` {problem}", reference.reference));
        }
    }

    fn check_entry_map<T>(
        &self,
        ctx: &mut Context,
        field: &str,
        map: &BTreeMap<String, RefOr<T>>,
        kind: &str,
        components: Option<&BTreeMap<String, RefOr<T>>>,
    ) where
        T: DeserializeOwned,
    {
        for (key, entry) in map {
            ctx.in_key(field, key, |ctx| {
                self.check_entry(ctx, entry, kind, components)
            });
        }
    }

    fn check_entry_list<T>(
        &self,
        ctx: &mut Context,
        field: &str,
        items: &[RefOr<T>],
        kind: &str,
        components: Option<&BTreeMap<String, RefOr<T>>>,
    ) where
        T: DeserializeOwned,
    {
        for (index, entry) in items.iter().enumerate() {
            ctx.in_index(field, index, |ctx| {
                self.check_entry(ctx, entry, kind, components);
            });
        }
    }

    fn check_entry_option<T>(
        &self,
        ctx: &mut Context,
        field: &str,
        entry: &Option<RefOr<T>>,
        kind: &str,
        components: Option<&BTreeMap<String, RefOr<T>>>,
    ) where
        T: DeserializeOwned,
    {
        if let Some(entry) = entry {
            ctx.in_field(field, |ctx| self.check_entry(ctx, entry, kind, components));
        }
    }

    /// The `tags`, `externalDocs`, and `bindings` every object carries.
    fn check_shared_references(
        &self,
        ctx: &mut Context,
        tags: &[RefOr<Tag>],
        external_docs: &Option<RefOr<ExternalDocumentation>>,
        bindings: &Option<RefOr<Bindings>>,
        bindings_kind: &str,
        bindings_map: Option<&BTreeMap<String, RefOr<Bindings>>>,
    ) {
        let components = self.components.as_ref();
        self.check_entry_list(ctx, "tags", tags, "tags", components.map(|c| &c.tags));
        self.check_entry_option(
            ctx,
            "externalDocs",
            external_docs,
            "externalDocs",
            components.map(|c| &c.external_docs),
        );
        self.check_entry_option(ctx, "bindings", bindings, bindings_kind, bindings_map);
    }

    fn check_server_references(&self, ctx: &mut Context, server: &Server) {
        let components = self.components.as_ref();
        self.check_entry_map(
            ctx,
            "variables",
            &server.variables,
            "serverVariables",
            components.map(|c| &c.server_variables),
        );
        self.check_entry_list(
            ctx,
            "security",
            &server.security,
            "securitySchemes",
            components.map(|c| &c.security_schemes),
        );
        self.check_shared_references(
            ctx,
            &server.tags,
            &server.external_docs,
            &server.bindings,
            "serverBindings",
            components.map(|c| &c.server_bindings),
        );
    }

    fn check_channel_references(&self, ctx: &mut Context, channel: &Channel) {
        let components = self.components.as_ref();
        for (key, entry) in &channel.messages {
            ctx.in_key("messages", key, |ctx| {
                // A message may be declared anywhere a message lives,
                // so it gets the message lookup rather than one map.
                if let Some(reference) = entry.reference()
                    && let Some(problem) = self.message_kind_problem(entry)
                {
                    ctx.error_field("$ref", format!("`{}` {problem}", reference.reference));
                }
                if let Some(message) = entry.item() {
                    self.check_message_references(ctx, message);
                }
            });
        }
        self.check_entry_map(
            ctx,
            "parameters",
            &channel.parameters,
            "parameters",
            components.map(|c| &c.parameters),
        );
        self.check_shared_references(
            ctx,
            &channel.tags,
            &channel.external_docs,
            &channel.bindings,
            "channelBindings",
            components.map(|c| &c.channel_bindings),
        );
    }

    fn check_message_references(&self, ctx: &mut Context, message: &Message) {
        let components = self.components.as_ref();
        for (field, schema) in [("headers", &message.headers), ("payload", &message.payload)] {
            self.check_entry_option(
                ctx,
                field,
                schema,
                "schemas",
                components.map(|c| &c.schemas),
            );
        }
        self.check_entry_option(
            ctx,
            "correlationId",
            &message.correlation_id,
            "correlationIds",
            components.map(|c| &c.correlation_ids),
        );
        self.check_entry_list(
            ctx,
            "traits",
            &message.traits,
            "messageTraits",
            components.map(|c| &c.message_traits),
        );
        for (index, message_trait) in message.traits.iter().enumerate() {
            if let Some(message_trait) = message_trait.item() {
                ctx.in_index("traits", index, |ctx| {
                    self.check_message_trait_references(ctx, message_trait);
                });
            }
        }
        self.check_shared_references(
            ctx,
            &message.tags,
            &message.external_docs,
            &message.bindings,
            "messageBindings",
            components.map(|c| &c.message_bindings),
        );
    }

    /// An operation trait carries the same references an operation
    /// does, and a `bindings` reference is only judged by the position
    /// holding it — bindings being declared under four names.
    fn check_operation_trait_references(
        &self,
        ctx: &mut Context,
        operation_trait: &OperationTrait,
    ) {
        let components = self.components.as_ref();
        self.check_entry_list(
            ctx,
            "security",
            &operation_trait.security,
            "securitySchemes",
            components.map(|c| &c.security_schemes),
        );
        self.check_shared_references(
            ctx,
            &operation_trait.tags,
            &operation_trait.external_docs,
            &operation_trait.bindings,
            "operationBindings",
            components.map(|c| &c.operation_bindings),
        );
    }

    /// The same for a message trait, whose bindings are message ones.
    fn check_message_trait_references(&self, ctx: &mut Context, message_trait: &MessageTrait) {
        let components = self.components.as_ref();
        self.check_entry_option(
            ctx,
            "headers",
            &message_trait.headers,
            "schemas",
            components.map(|c| &c.schemas),
        );
        self.check_entry_option(
            ctx,
            "correlationId",
            &message_trait.correlation_id,
            "correlationIds",
            components.map(|c| &c.correlation_ids),
        );
        self.check_shared_references(
            ctx,
            &message_trait.tags,
            &message_trait.external_docs,
            &message_trait.bindings,
            "messageBindings",
            components.map(|c| &c.message_bindings),
        );
    }

    fn check_operation_references(&self, ctx: &mut Context, operation: &Operation) {
        let components = self.components.as_ref();
        self.check_entry_list(
            ctx,
            "traits",
            &operation.traits,
            "operationTraits",
            components.map(|c| &c.operation_traits),
        );
        for (index, operation_trait) in operation.traits.iter().enumerate() {
            if let Some(operation_trait) = operation_trait.item() {
                ctx.in_index("traits", index, |ctx| {
                    self.check_operation_trait_references(ctx, operation_trait);
                });
            }
        }
        self.check_entry_list(
            ctx,
            "security",
            &operation.security,
            "securitySchemes",
            components.map(|c| &c.security_schemes),
        );
        self.check_shared_references(
            ctx,
            &operation.tags,
            &operation.external_docs,
            &operation.bindings,
            "operationBindings",
            components.map(|c| &c.operation_bindings),
        );
        if let Some(reply) = operation.reply.as_ref().and_then(RefOr::item) {
            ctx.in_field("reply", |ctx| self.check_reply_references(ctx, reply));
        }
    }

    fn check_reply_references(&self, ctx: &mut Context, reply: &OperationReply) {
        self.check_entry_option(
            ctx,
            "address",
            &reply.address,
            "replyAddresses",
            self.components.as_ref().map(|c| &c.reply_addresses),
        );
    }

    fn validate_inner(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        let mut ctx = Context::for_document(options, self);

        if let Some(id) = &self.id {
            ctx.require_non_empty("id", id);
        }
        if let Some(content_type) = &self.default_content_type {
            ctx.require_non_empty("defaultContentType", content_type);
        }

        ctx.in_field("info", |ctx| self.info.validate_with_context(ctx));

        ctx.validate_map_keys("servers", &self.servers);
        for (name, server) in &self.servers {
            ctx.in_key("servers", name, |ctx| {
                if let Some(server) = server.item() {
                    self.check_server_references(ctx, server);
                }
                server.validate_with_context(ctx);
            });
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
                if let Some(channel) = channel.item() {
                    self.validate_channel_servers(ctx, channel, Origin::Root);
                    self.check_channel_references(ctx, channel);
                }
                channel.validate_with_context(ctx);
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
                if let Some(operation) = operation.item() {
                    self.validate_operation_wiring(ctx, operation, Origin::Root);
                    self.check_operation_references(ctx, operation);
                }
                operation.validate_with_context(ctx);
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
                self.validate_components_wiring(ctx, components);
                components.validate_with_context(ctx);
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
            json!([ { "$ref": "./other.yaml#/channels/userSignedUp/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A pointer at some other channel's message is still wrong.
        let mut value = wired();
        value["channels"]["elsewhere"] = json!({ "$ref": "./other.yaml#/channels/userSignedUp" });
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/elsewhere" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/userSignedUp/messages/signup" } ]);
        assert!(errors_for(value).iter().any(|e| {
            e.contains("must point at a message of `other.yaml#/channels/userSignedUp`")
        }),);
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
            },
            "operationTraits": {
                "real": { "title": "T" },
                "alias": { "$ref": "#/components/operationTraits/real" }
            },
            "messageTraits": {
                "real": { "title": "T" },
                "alias": { "$ref": "#/components/messageTraits/real" }
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

        // The same for a reference into another document whose
        // fragment is not a pointer either, named directly…
        let mut outside = wired_from_components();
        outside["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "./other.yaml#bad" });
        assert!(
            errors_for(outside)
                .iter()
                .any(|e| e.contains("is not a usable JSON Pointer"))
        );

        // …or reached through an alias.
        let mut aliased = wired();
        aliased["channels"]["alias"] = json!({ "$ref": "./other.yaml#bad" });
        aliased["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/channels/alias" });
        aliased["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(aliased)
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

        // "Any location" still means a Channel Object. A message is a
        // message however deep it sits, and it reads as a channel with
        // no fields set only because the model drops what it does not
        // know.
        let mut wrong_kind = wired_from_components();
        wrong_kind["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/channels/userSignedUp/messages/signup" });
        wrong_kind["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(wrong_kind)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );
    }

    #[test]
    fn only_unresolvable_message_refs_are_skipped() {
        // An empty `$ref` is reported for being empty, not here…
        let mut value = wired();
        value["operations"]["receiveSignups"]["messages"] = json!([ { "$ref": "" } ]);
        let errors = errors_for(value);
        assert!(
            !errors.iter().any(|e| e.contains("must point at a message")),
            "got: {errors:?}"
        );

        // …but a reference into another document demonstrably is not
        // one of *this* channel's messages.
        let mut value = wired();
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "./messages.yaml#/signup" } ]);
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e.contains(
                "message `./messages.yaml#/signup` must point at a message of `#/channels/userSignedUp`"
            )),
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
    fn a_declared_alias_may_not_name_another_kind() {
        // The target exists, so nothing is dangling — it is simply not
        // a channel.
        let mut value = wired();
        value["components"] = json!({
            "channels": { "alias": { "$ref": "#/components/messages/m" } },
            "messages": { "m": { "name": "M" } }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.channels.alias.$ref: `#/components/messages/m` does not point at an object of the expected kind"),
        );

        // Reusable messages get the same treatment.
        let mut value = wired();
        value["components"] = json!({ "messages": { "alias": { "$ref": "#/info" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.messages.alias.$ref: `#/info` does not point at an object of the expected kind"),
        );
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
    fn a_nested_reference_is_followed_without_being_used() {
        // Nothing is wired through this channel at all.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "c": { "messages": { "m": { "$ref": "#/components/messages/missing" } } }
            }
        });
        assert_eq!(
            errors_for(value),
            vec![
                "#.channels.c.messages.m.$ref: `#/components/messages/missing` names nothing in this document"
            ]
        );

        // A pointer that steps into a scalar names nothing either.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "c": { "messages": { "m": { "$ref": "#/info/title/deeper" } } } }
        });
        assert_eq!(
            errors_for(value),
            vec![
                "#.channels.c.messages.m.$ref: `#/info/title/deeper` names nothing in this document"
            ]
        );

        // The same anywhere else a reference may sit.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "servers": {
                "s": {
                    "host": "h",
                    "protocol": "kafka",
                    "variables": { "v": { "$ref": "#/components/serverVariables/missing" } }
                }
            },
            "channels": { "c": { "parameters": { "p": { "$ref": "#/components/parameters/missing" } } } }
        });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.servers.s.variables.v.$ref: `#/components/serverVariables/missing` names nothing in this document"),
            "got: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e
                == "#.channels.c.parameters.p.$ref: `#/components/parameters/missing` names nothing in this document"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_wired_reference_is_reported_once() {
        // The wiring check and the sweep both see this one; the
        // specific message is the one that survives.
        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/channels/nope" });
        value["operations"]["receiveSignups"]["messages"] = json!([]);
        assert_eq!(
            errors_for(value),
            vec![
                "#.operations.receiveSignups.channel.$ref: channel `#/channels/nope` names nothing in this document"
            ]
        );
    }

    #[test]
    fn a_reference_out_of_the_document_is_still_out_of_the_root() {
        // "MUST NOT point to ... anywhere else" — which document a
        // reference leaves for is visible without fetching it.
        let mut value = wired();
        value["channels"]["userSignedUp"]["servers"] =
            json!([ { "$ref": "./other.yaml#/servers/s" } ]);
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.channels.userSignedUp.servers[0].$ref: server `./other.yaml#/servers/s` must point into the root `servers` object"),
        );

        let mut value = wired();
        value["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "./other.yaml#/channels/c" });
        value["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.operations.receiveSignups.channel.$ref: channel `./other.yaml#/channels/c` must point into the root `channels` object"),
        );
    }

    #[test]
    fn a_reply_address_needs_a_channel_without_one() {
        // "When address is specified, the address property of the
        // channel referenced by this property MUST be either null or
        // not defined."
        let mut value = wired();
        value["channels"]["replies"] = json!({ "address": "reply-topic" });
        value["operations"]["receiveSignups"]["reply"] = json!({
            "channel": { "$ref": "#/channels/replies" },
            "address": { "location": "$message.header#/replyTo" }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.operations.receiveSignups.reply.address: requires the channel's `address` to be `null` or absent, but it is `reply-topic`"),
        );

        // An explicit null, and an absent address, are both fine.
        for address in [json!(null), json!("ABSENT")] {
            let mut value = wired();
            let mut channel = json!({});
            if address != json!("ABSENT") {
                channel["address"] = address.clone();
            }
            value["channels"]["replies"] = channel;
            value["operations"]["receiveSignups"]["reply"] = json!({
                "channel": { "$ref": "#/channels/replies" },
                "address": { "location": "$message.header#/replyTo" }
            });
            assert_eq!(
                errors_for(value),
                Vec::<String>::new(),
                "address {address:?}"
            );
        }

        // A reply with no address of its own does not care.
        let mut value = wired();
        value["channels"]["replies"] = json!({ "address": "reply-topic" });
        value["operations"]["receiveSignups"]["reply"] =
            json!({ "channel": { "$ref": "#/channels/replies" } });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn application_data_is_not_searched_for_references() {
        // An example payload is the application's own data. A `$ref`
        // in it is whatever that application means by one.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["examples"] = json!([
            { "name": "one", "payload": { "$ref": "#/business/id" } }
        ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // So is a schema in a dialect this crate does not speak.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] = json!({
            "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
            "schema": { "type": "record", "fields": { "$ref": "#/dialect/type" } }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());

        // And so are binding values.
        let mut value = wired();
        value["channels"]["userSignedUp"]["bindings"] =
            json!({ "kafka": { "topic": { "$ref": "#/broker/topic" } } });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_channel_in_another_document_keeps_its_messages_there() {
        // The operation names a channel in another file directly, so
        // its messages are named there too — the subset relationship
        // still compares, it just compares over there.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "./other.yaml#/channels/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "./other.yaml#/channels/c/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A message of some channel in *this* document is not one of
        // that channel's.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "./other.yaml#/channels/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/userSignedUp/messages/signup" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `other.yaml#/channels/c`")),
        );
    }

    #[test]
    fn an_extension_does_not_declare_kinds() {
        // `#/x-store/messages/c` is a channel that happens to live in
        // an extension whose map is spelled `messages`. Extensions are
        // arbitrary JSON, and a reusable operation may name a Channel
        // Object in any location.
        let mut value = wired_from_components();
        value["x-store"] = json!({ "messages": { "c": { "address": "stored" } } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/x-store/messages/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn every_reference_the_model_holds_is_followed() {
        // None of these sit anywhere a wiring check would walk.
        let mut value = wired();
        value["info"]["tags"] = json!([ { "$ref": "#/components/tags/ghost" } ]);
        assert!(errors_for(value).iter().any(|e| e
            == "#.info.tags[0].$ref: `#/components/tags/ghost` names nothing in this document"),);

        // Deep inside a schema.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] = json!({
            "type": "object",
            "properties": { "p": { "$ref": "#/components/schemas/ghost" } }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.channels.userSignedUp.messages.signup.payload.properties.p.$ref: `#/components/schemas/ghost` names nothing in this document"),
        );

        // And inside an inline trait.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["traits"] =
            json!([ { "headers": { "$ref": "#/components/schemas/ghost" } } ]);
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.channels.userSignedUp.messages.signup.traits[0].headers.$ref: `#/components/schemas/ghost` names nothing in this document"),
        );
    }

    #[test]
    fn the_same_resource_spelled_differently_is_the_same_resource() {
        // `./channels.yaml` and `channels.yaml` resolve against the
        // same base, so they name the same file (RFC 3986 §5.2).
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "./channels.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "channels.yaml#/c/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A different file is still a different file.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "./channels.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "../channels.yaml#/c/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `channels.yaml#/c`")),
        );
    }

    #[test]
    fn an_extension_inside_the_document_declares_nothing_either() {
        // `#/components/x-store/messages/c` walks into an extension,
        // and what an extension spells `messages` is its own business.
        let mut value = wired_from_components();
        value["components"]["x-store"] = json!({ "messages": { "c": { "address": "stored" } } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/x-store/messages/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A channel named `x-thing` is a name, not an extension.
        let mut value = wired();
        value["channels"]["x-thing"] = json!({ "messages": { "m": { "name": "M" } } });
        value["operations"]["receiveSignups"]["channel"] = json!({ "$ref": "#/channels/x-thing" });
        value["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "#/channels/x-thing/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn an_alias_is_followed_before_its_kind_is_judged() {
        // The tag is reached through a location this crate does not
        // model, which holds a Reference Object rather than a tag.
        // Judging that object as a tag would call every alias wrong.
        let mut value = wired();
        value["x-shared"] = json!({ "tags": [ { "$ref": "#/components/tags/real" } ] });
        value["servers"]["production"]["tags"] = json!([ { "$ref": "#/x-shared/tags/0" } ]);
        value["components"] = json!({ "tags": { "real": { "name": "real" } } });
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A chain that ends at something else is still reported.
        let mut value = wired();
        value["x-shared"] = json!({ "tags": [ { "$ref": "#/components/schemas/notATag" } ] });
        value["servers"]["production"]["tags"] = json!([ { "$ref": "#/x-shared/tags/0" } ]);
        value["components"] = json!({ "schemas": { "notATag": { "type": "object" } } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );
    }

    #[test]
    fn a_nested_reference_is_judged_by_its_kind_too() {
        // `info.tags` is nowhere a wiring check reaches, but the
        // position still says a tag belongs there.
        let mut value = wired();
        value["info"]["tags"] = json!([ { "$ref": "#/components/schemas/notATag" } ]);
        value["components"] = json!({ "schemas": { "notATag": { "type": "object" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.info.tags[0].$ref: `#/components/schemas/notATag` does not point at an object of the expected kind"),
        );

        // The same inside a schema…
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] = json!({
            "properties": { "p": { "$ref": "#/components/messages/notASchema" } }
        });
        value["components"] = json!({ "messages": { "notASchema": { "name": "M" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                .starts_with("#.channels.userSignedUp.messages.signup.payload.properties.p.$ref:")
                && e.contains("does not point at an object of the expected kind")),
        );

        // A fragment that is not a pointer is reported as that, not as
        // the wrong kind.
        let mut value = wired();
        value["info"]["tags"] = json!([ { "$ref": "#/bad~2escape" } ]);
        assert_eq!(
            errors_for(value),
            vec!["#.info.tags[0].$ref: `#/bad~2escape` is not a usable JSON Pointer"]
        );

        // …and a reference of the right kind is left alone.
        let mut value = wired();
        value["info"]["tags"] = json!([ { "$ref": "#/components/tags/real" } ]);
        value["components"] = json!({ "tags": { "real": { "name": "real" } } });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_pointer_below_a_reference_object_names_nothing() {
        // A pointer does not dereference as it walks (RFC 6901 §4), so
        // only `#/components/tags/alias` lands on the alias; a step
        // past it lands nowhere.
        let mut value = wired();
        value["info"]["tags"] = json!([ { "$ref": "#/components/tags/alias/name" } ]);
        value["components"] = json!({ "tags": { "alias": { "$ref": "other.yaml#/tag" } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.info.tags[0].$ref: `#/components/tags/alias/name` names nothing in this document"),
        );
    }

    #[test]
    fn a_component_key_may_look_like_an_extension() {
        // `x-thing` here is a channel's name, not an extension of the
        // Components Object, so what is under it is still structure.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "channels": { "x-thing": { "messages": { "m": { "name": "M" } } } },
                "operations": {
                    "o": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/x-thing/messages/m" }
                    }
                }
            }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // The channel itself is nameable, of course.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "channels": { "x-thing": { "messages": { "m": { "name": "M" } } } },
                "operations": {
                    "o": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/x-thing" },
                        "messages": [ { "$ref": "#/components/channels/x-thing/messages/m" } ]
                    }
                }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn an_empty_path_segment_is_a_segment() {
        // `a//b.yaml` and `a/b.yaml` are different paths, so a message
        // in one is not a message of a channel in the other.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "a//b.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "a/b.yaml#/c/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `a//b.yaml#/c`")),
        );
    }

    #[test]
    fn structure_continues_below_a_singleton() {
        // `#/info/tags/0` is a tag: `info` is the document's own
        // structure, so what hangs off it is too, and a tag reads as a
        // channel only because the model is permissive.
        let mut value = wired_from_components();
        value["info"]["tags"] = json!([ { "name": "t" } ]);
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/info/tags/0" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.operations.receiveSignups.channel.$ref: channel `#/info/tags/0` does not point at an object of the expected kind"),
        );

        // Named where a tag belongs, it is exactly right.
        let mut value = wired();
        value["info"]["tags"] = json!([ { "name": "t" }, { "$ref": "#/info/tags/0" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_traits_bindings_are_the_traits_kind_of_bindings() {
        // Bindings are declared under four names, so which one a
        // reference may use is decided by the position holding it —
        // including inside a trait, inline or reusable.
        let mut value = wired();
        value["components"] = json!({
            "messageBindings": { "mb": { "kafka": {} } },
            "operationTraits": { "t": { "bindings": { "$ref": "#/components/messageBindings/mb" } } }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.operationTraits.t.bindings.$ref: `#/components/messageBindings/mb` does not point at an object of the expected kind"),
        );

        // The same inline, on an operation's trait.
        let mut value = wired();
        value["operations"]["receiveSignups"]["traits"] =
            json!([ { "bindings": { "$ref": "#/components/messageBindings/mb" } } ]);
        value["components"] = json!({ "messageBindings": { "mb": { "kafka": {} } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.operations.receiveSignups.traits[0].bindings.$ref: `#/components/messageBindings/mb` does not point at an object of the expected kind"),
        );

        // The same inline, on a message's trait.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["traits"] =
            json!([ { "bindings": { "$ref": "#/components/operationBindings/ob" } } ]);
        value["components"] = json!({ "operationBindings": { "ob": { "kafka": {} } } });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.channels.userSignedUp.messages.signup.traits[0].bindings.$ref: `#/components/operationBindings/ob` does not point at an object of the expected kind"),
        );

        // Bindings of the right kind are left alone.
        let mut value = wired();
        value["components"] = json!({
            "messageBindings": { "mb": { "kafka": {} } },
            "messageTraits": { "t": { "bindings": { "$ref": "#/components/messageBindings/mb" } } }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_key_may_look_like_an_extension_at_any_depth() {
        // `x-message` is a message's name, not an extension of the
        // channel, so the structure below it still declares kinds.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "channels": { "c": { "messages": { "x-message": { "name": "M" } } } },
                "operations": {
                    "o": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/c/messages/x-message" }
                    }
                }
            }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // Naming that message as a message is fine.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "channels": { "c": { "messages": { "x-message": { "name": "M" } } } },
                "operations": {
                    "o": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/c" },
                        "messages": [ { "$ref": "#/components/channels/c/messages/x-message" } ]
                    }
                }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_resource_is_compared_by_its_path_alone() {
        // `a//../b.yaml` and `a/b.yaml` are the same file.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "a//../b.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "a/b.yaml#/c/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A slash inside a query is not a path separator, so these are
        // two different resources.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "http://host?x=/a/../b#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "http://host?x=/b#/c/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `http://host?x=/a/../b#/c`")),
        );
    }

    #[test]
    fn a_boolean_is_a_schema() {
        // JSON Schema allows `true` and `false` wherever a schema is
        // expected, so a reference to one is a reference to a schema —
        // no struct deserializes it, which is not the same as it being
        // the wrong kind.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] =
            json!({ "properties": { "p": { "$ref": "#/components/schemas/always" } } });
        value["components"] = json!({ "schemas": { "always": true } });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_nested_map_says_what_it_holds() {
        // `properties` holds schemas wherever it appears, so a schema
        // is what this names — not a channel.
        let mut value = wired_from_components();
        value["components"]["schemas"] =
            json!({ "s": { "properties": { "p": { "type": "string" } } } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/schemas/s/properties/p" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.operations.receiveSignups.channel.$ref: channel `#/components/schemas/s/properties/p` does not point at an object of the expected kind"),
        );

        // And a map is never one of the objects in it, however
        // agreeably an empty one reads as a channel.
        let mut value = wired_from_components();
        value["servers"]["production"]["variables"] = json!({ "v": { "default": "d" } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/servers/production/variables" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // A schema named where a schema belongs is right, of course.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] =
            json!({ "properties": { "p": { "$ref": "#/components/schemas/s/properties/inner" } } });
        value["components"] = json!({
            "schemas": { "s": { "properties": { "inner": { "type": "string" } } } }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_chain_is_judged_where_it_ends() {
        // The alias sits somewhere unmodelled, so the only position
        // that says anything is the one it leads to.
        let mut value = wired_from_components();
        value["x-alias"] = json!({ "$ref": "#/components/messages/m" });
        value["components"]["messages"] = json!({ "m": { "name": "M" } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/x-alias" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.operations.receiveSignups.channel.$ref: channel `#/x-alias` does not point at an object of the expected kind"),
        );

        // Ending somewhere that really is a channel is fine.
        let mut value = wired_from_components();
        value["x-alias"] = json!({ "$ref": "#/components/channels/c" });
        value["components"]["channels"] = json!({ "c": { "address": "a" } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/x-alias" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_single_object_is_not_a_map() {
        // `externalDocs` names one object, so `x-store` under it is an
        // extension rather than a key — and what an extension holds is
        // its own business.
        let mut value = wired_from_components();
        value["info"]["externalDocs"] = json!({
            "url": "https://example.com",
            "x-store": { "messages": { "c": { "address": "stored" } } }
        });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/info/externalDocs/x-store/messages/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn a_resource_is_compared_with_its_escapes_decoded() {
        // `%62` is `b`, an unreserved character, so these name the same
        // file (RFC 3986 §6.2.2.2).
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "a/%62.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "a/b.yaml#/c/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A reserved one stays encoded, since decoding `%2F` would turn
        // one path segment into two.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "a%2Fb.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "a/b.yaml#/c/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `a%2Fb.yaml#/c`")),
        );
    }

    #[test]
    fn a_trait_or_binding_is_whatever_holds_it() {
        // A message's traits are message traits, so an operation may
        // not borrow one.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "channels": { "c": { "address": "a" } },
                "messages": { "m": { "name": "M", "traits": [ { "title": "mt" } ] } },
                "operations": {
                    "o": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/c" },
                        "traits": [ { "$ref": "#/components/messages/m/traits/0" } ]
                    }
                }
            }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.operations.o.traits[0].$ref: `#/components/messages/m/traits/0` does not point at an object of the expected kind"),
        );

        // And a message's bindings are message bindings.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "messages": { "m": { "name": "M", "bindings": { "kafka": {} } } },
                "channels": {
                    "c": {
                        "address": "a",
                        "bindings": { "$ref": "#/components/messages/m/bindings" }
                    }
                }
            }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.channels.c.bindings.$ref: `#/components/messages/m/bindings` does not point at an object of the expected kind"),
        );

        // The other way round too: an operation's traits are
        // operation traits.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "channels": { "c": { "address": "a" } },
                "operations": {
                    "o": {
                        "action": "send",
                        "channel": { "$ref": "#/components/channels/c" },
                        "traits": [ { "title": "ot" } ]
                    }
                },
                "messages": {
                    "m": { "name": "M", "traits": [ { "$ref": "#/components/operations/o/traits/0" } ] }
                }
            }
        });
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.components.messages.m.traits[0].$ref: `#/components/operations/o/traits/0` does not point at an object of the expected kind"),
        );

        // Borrowing from the right sort of object is fine.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "components": {
                "messages": {
                    "m": { "name": "M", "bindings": { "kafka": {} } },
                    "other": { "name": "O", "bindings": { "$ref": "#/components/messages/m/bindings" } }
                }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());
    }

    #[test]
    fn items_is_one_schema_or_a_list_of_them() {
        // draft-07 allows either, and which one shows in what follows.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] =
            json!({ "$ref": "#/components/schemas/list/items" });
        value["components"] = json!({
            "schemas": { "list": { "type": "array", "items": { "type": "string" } } }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());

        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] =
            json!({ "$ref": "#/components/schemas/tuple/items/1" });
        value["components"] = json!({
            "schemas": {
                "tuple": { "items": [ { "type": "string" }, { "type": "number" } ] }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());

        // A pointer continuing past the single form walks into that
        // schema, so what it names is a schema too.
        let mut value = wired();
        value["channels"]["userSignedUp"]["messages"]["signup"]["payload"] =
            json!({ "$ref": "#/components/schemas/list/items/properties/p" });
        value["components"] = json!({
            "schemas": {
                "list": { "items": { "properties": { "p": { "type": "string" } } } }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());

        // Either way it is a schema, and not a channel.
        let mut value = wired_from_components();
        value["components"]["schemas"] = json!({
            "list": { "type": "array", "items": { "type": "string" } }
        });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/schemas/list/items" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );
    }

    #[test]
    fn every_schema_bearing_keyword_holds_schemas() {
        for keyword in ["additionalItems", "propertyNames", "contains", "not", "if"] {
            let mut value = wired_from_components();
            value["components"]["schemas"] = json!({ "s": { keyword: { "type": "string" } } });
            value["components"]["operations"]["receiveSignups"]["channel"] =
                json!({ "$ref": format!("#/components/schemas/s/{keyword}") });
            value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
            let errors = errors_for(value);
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains("does not point at an object of the expected kind")),
                "{keyword} got: {errors:?}"
            );
        }

        // `dependencies` may hold a schema, so it holds schemas.
        let mut value = wired_from_components();
        value["components"]["schemas"] =
            json!({ "s": { "dependencies": { "d": { "type": "object" } } } });
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "#/components/schemas/s/dependencies/d" });
        value["components"]["operations"]["receiveSignups"]["messages"] = json!([]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );
    }

    #[test]
    fn a_scheme_and_host_are_compared_without_regard_to_case() {
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "HTTP://EXAMPLE.COM/a.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "http://example.com/a.yaml#/c/messages/m" } ]);
        assert_eq!(errors_for(value), Vec::<String>::new());

        // The path is not case-insensitive, though.
        let mut value = wired_from_components();
        value["components"]["operations"]["receiveSignups"]["channel"] =
            json!({ "$ref": "http://example.com/A.yaml#/c" });
        value["components"]["operations"]["receiveSignups"]["messages"] =
            json!([ { "$ref": "http://example.com/a.yaml#/c/messages/m" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("must point at a message of `http://example.com/A.yaml#/c`")),
        );
    }

    #[test]
    fn external_references_are_skipped_unless_strictness_is_requested() {
        // A root operation must name a channel in the root Channels
        // Object, so in a split document the *entry* is what points
        // outside. Nothing here resolves locally, and nothing here is
        // an error by default.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user": { "$ref": "./other.yaml#/channels/user" } },
            "operations": {
                "send": { "action": "send", "channel": { "$ref": "#/channels/user" } }
            }
        });
        assert_eq!(errors_for(value.clone()), Vec::<String>::new());

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
        // The channel is over in `channels.yaml`, and so are its
        // messages — that is where they are named. A pointer does not
        // dereference as it walks (RFC 6901 §4), so
        // `#/channels/user/messages/…` would name nothing at all.
        let value = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user": { "$ref": "./channels.yaml#/user" } },
            "operations": {
                "send": {
                    "action": "send",
                    "channel": { "$ref": "#/channels/user" },
                    "messages": [ { "$ref": "./channels.yaml#/user/messages/anything" } ]
                }
            }
        });
        assert_eq!(errors_for(value), Vec::<String>::new());

        // Walking into the alias as though it were the channel is the
        // mistake, and it is the one reported.
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
        assert!(
            errors_for(value).iter().any(|e| e
                == "#.operations.send.messages[0].$ref: message `#/channels/user/messages/anything` must point at a message of `channels.yaml#/user`"),
        );
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
