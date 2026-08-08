//! AsyncAPI v2.6 root document.
//!
//! Per [AsyncAPI Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#A2SObject).
//!
//! Beyond the per-object checks, the root validator resolves what only
//! it can see: a channel's `servers` must name declared servers, an
//! `operationId` must be unique across every operation in the document,
//! and a security requirement must name a declared scheme — listing
//! scopes only where the scheme's type allows them.

use crate::common::reference::{RefOr, Reference};
use crate::v2_6::channel_item::ChannelItem;
use crate::v2_6::components::Components;
use crate::v2_6::external_documentation::ExternalDocumentation;
use crate::v2_6::info::Info;
use crate::v2_6::message::{Message, OperationMessage};
use crate::v2_6::operation::{Operation, OperationKind};
use crate::v2_6::security_scheme::{SecurityRequirement, SecurityScheme};
use crate::v2_6::server::Server;
use crate::v2_6::tag::Tag;
use crate::v2_6::version::Version;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Root AsyncAPI v2.6 document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Document {
    /// **Required** Exactly `2.6.0` — the AsyncAPI specification
    /// version, which the schema pins to a single-value enumeration.
    pub asyncapi: Version,

    /// A unique identifier of the application this document describes.
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

    /// **Required** The channels this application uses, keyed by their
    /// path. Unlike v3, the key *is* the address.
    ///
    /// A channel item carries any `$ref` as its own field, so this is
    /// not a `RefOr` — see [`ChannelItem::reference`].
    pub channels: BTreeMap<String, ChannelItem>,

    /// Reusable objects referenced throughout the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,

    /// Tags used by the document, with additional metadata. v3 moved
    /// these under `info`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,

    /// Additional external documentation. v3 moved this under `info`.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    /// `x-`-prefixed Specification Extensions on the root.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// What following a document-local pointer turned out to be.
///
/// The distinctions matter: an external terminus is accepted (this
/// crate does not fetch other documents), while a local pointer that
/// names nothing, revisits itself, or has a shape that cannot denote
/// the kind of object expected is a document bug.
#[derive(Debug)]
pub(crate) enum Resolution<'a, T> {
    /// Resolved to a concrete object in this document.
    Found(&'a T),
    /// The chain ends at a reference into another document.
    External,
    /// A local pointer that names nothing.
    Missing,
    /// The chain revisits a pointer it already followed.
    Cycle,
    /// A local pointer whose shape cannot denote this kind of object.
    Unrecognized,
}

impl<'a, T> Resolution<'a, T> {
    /// The object, if the pointer resolved inside this document.
    ///
    /// Takes `self` so the borrow is the document's, not this
    /// `Resolution`'s — callers routinely resolve into a temporary.
    pub(crate) fn found(self) -> Option<&'a T> {
        match self {
            Resolution::Found(item) => Some(item),
            _ => None,
        }
    }

    /// Why this pointer is a document bug, if it is one. `None` means
    /// it resolved or left the document.
    fn problem(&self) -> Option<&'static str> {
        match self {
            Resolution::Found(_) | Resolution::External => None,
            Resolution::Missing => Some("names nothing in this document"),
            Resolution::Cycle => Some("is part of a reference cycle"),
            Resolution::Unrecognized => Some("does not point at an object of the expected kind"),
        }
    }
}

/// Whether a local pointer has the shape of a channel pointer, used to
/// tell "names nothing" from "wrong kind entirely".
fn channel_pointer_shape(pointer: &str) -> bool {
    pointer.starts_with("/channels/") || pointer.starts_with("/components/channels/")
}

/// The same, for a message pointer.
fn message_pointer_shape(pointer: &str) -> bool {
    pointer.starts_with("/components/messages/") || pointer.starts_with("/channels/")
}

/// Split one RFC 6901 segment off a pointer and decode it.
///
/// A pointer travels in a URI fragment, so it is percent-encoded (RFC
/// 3986) around escapes for the pointer's own separators (RFC 6901):
/// `%20` is a space, `~1` is a `/`, and `~0` is a `~`. `source/path` is
/// therefore *two* segments and cannot name one channel — that key is
/// spelled `source~1path`.
///
/// Returns `None` when the segment is malformed: a `~` followed by
/// anything but `0` or `1`, or a truncated / non-hex `%` escape. RFC
/// 6901 leaves those undefined rather than literal.
fn split_segment(pointer: &str) -> Option<(String, &str)> {
    let (segment, rest) = match pointer.find('/') {
        Some(i) => (&pointer[..i], &pointer[i..]),
        None => (pointer, ""),
    };
    Some((decode_segment(segment)?, rest))
}

/// Percent-decode, then undo the RFC 6901 escapes.
fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut percent_decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = segment.get(i + 1..i + 3)?;
            percent_decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            percent_decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(percent_decoded).ok()?;

    let mut out = String::with_capacity(decoded.len());
    let mut chars = decoded.chars();
    while let Some(c) = chars.next() {
        if c != '~' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('0') => out.push('~'),
            Some('1') => out.push('/'),
            // `~` must be followed by `0` or `1`.
            _ => return None,
        }
    }
    Some(out)
}

impl Document {
    /// Resolve a channel item, following its `$ref` field through both
    /// `#/channels/…` and `#/components/channels/…` pointers.
    ///
    /// An item without a `$ref` resolves to itself.
    pub(crate) fn resolve_channel<'a>(
        &'a self,
        channel: &'a ChannelItem,
    ) -> Resolution<'a, ChannelItem> {
        let mut current = channel;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        loop {
            let Some(reference) = current.reference.as_deref() else {
                return Resolution::Found(current);
            };
            let reference = Reference {
                reference: reference.to_owned(),
            };
            if reference.is_external() {
                return Resolution::External;
            }
            let Some(pointer) = reference.local_pointer() else {
                return Resolution::Unrecognized;
            };
            if !seen.insert(current.reference.as_deref().unwrap_or_default()) {
                return Resolution::Cycle;
            }
            match self.channel_at(pointer) {
                Some(next) => current = next,
                None => {
                    return match channel_pointer_shape(pointer) {
                        true => Resolution::Missing,
                        false => Resolution::Unrecognized,
                    };
                }
            }
        }
    }

    /// The channel a document-local pointer names, in either map.
    fn channel_at<'a>(&'a self, pointer: &str) -> Option<&'a ChannelItem> {
        let (map, rest) = self.channel_map_at(pointer)?;
        let (key, rest) = split_segment(rest)?;
        rest.is_empty().then(|| map.get(&key))?
    }

    /// The channel map a pointer addresses, and what follows the map's
    /// own prefix.
    fn channel_map_at<'a, 'p>(
        &'a self,
        pointer: &'p str,
    ) -> Option<(&'a BTreeMap<String, ChannelItem>, &'p str)> {
        if let Some(rest) = pointer.strip_prefix("/components/channels/") {
            return Some((&self.components.as_ref()?.channels, rest));
        }
        Some((&self.channels, pointer.strip_prefix("/channels/")?))
    }

    fn security_scheme_at(&self, name: &str) -> Resolution<'_, SecurityScheme> {
        let Some(components) = self.components.as_ref() else {
            return Resolution::Missing;
        };
        let Some(mut entry) = components.security_schemes.get(name) else {
            return Resolution::Missing;
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            match entry {
                RefOr::Item(scheme) => return Resolution::Found(scheme),
                RefOr::Reference(reference) => {
                    if reference.is_external() {
                        return Resolution::External;
                    }
                    let Some(key) = reference.component_key("securitySchemes") else {
                        return Resolution::Unrecognized;
                    };
                    if !seen.insert(key.clone()) {
                        return Resolution::Cycle;
                    }
                    match components.security_schemes.get(&key) {
                        Some(next) => entry = next,
                        None => return Resolution::Missing,
                    }
                }
            }
        }
    }

    /// Resolve a `RefOr<Server>`, following `#/components/servers/…`.
    fn resolve_server<'a>(&'a self, entry: &'a RefOr<Server>) -> Resolution<'a, Server> {
        let mut current = entry;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            match current {
                RefOr::Item(server) => return Resolution::Found(server),
                RefOr::Reference(reference) => {
                    if reference.is_external() {
                        return Resolution::External;
                    }
                    let Some(pointer) = reference.local_pointer() else {
                        return Resolution::Unrecognized;
                    };
                    if !seen.insert(pointer.to_owned()) {
                        return Resolution::Cycle;
                    }
                    // Either map may hold the target: `#/servers/…` as
                    // well as `#/components/servers/…`.
                    let entry = if let Some(rest) = pointer.strip_prefix("/components/servers/") {
                        match split_segment(rest) {
                            Some((key, "")) => {
                                self.components.as_ref().and_then(|c| c.servers.get(&key))
                            }
                            _ => return Resolution::Unrecognized,
                        }
                    } else if let Some(rest) = pointer.strip_prefix("/servers/") {
                        match split_segment(rest) {
                            Some((key, "")) => self.servers.get(&key),
                            _ => return Resolution::Unrecognized,
                        }
                    } else {
                        return Resolution::Unrecognized;
                    };
                    match entry {
                        Some(next) => current = next,
                        None => return Resolution::Missing,
                    }
                }
            }
        }
    }

    /// Resolve a message `$ref`, following component aliases and
    /// pointers into a channel's operations.
    fn resolve_message(&self, reference: &Reference) -> Resolution<'_, Message> {
        let mut current = reference.clone();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            if current.is_external() {
                return Resolution::External;
            }
            let Some(pointer) = current.local_pointer() else {
                return Resolution::Unrecognized;
            };
            if !seen.insert(current.reference.clone()) {
                return Resolution::Cycle;
            }
            match self.message_at(pointer) {
                Some(RefOr::Item(message)) => return Resolution::Found(message),
                Some(RefOr::Reference(next)) => current = next.clone(),
                None => {
                    return if message_pointer_shape(pointer) {
                        Resolution::Missing
                    } else {
                        Resolution::Unrecognized
                    };
                }
            }
        }
    }

    /// The message entry a document-local pointer names — either a
    /// component message or one hanging off a channel's operation.
    ///
    /// A JSON Pointer walks the document *as written*: the channel it
    /// steps through is the item at that key, not whatever that item's
    /// `$ref` resolves to, so a message declared beside a `$ref` is
    /// reachable and one on the target is not (through this pointer).
    fn message_at<'a>(&'a self, pointer: &str) -> Option<&'a RefOr<Message>> {
        if let Some(rest) = pointer.strip_prefix("/components/messages/") {
            let (key, rest) = split_segment(rest)?;
            if !rest.is_empty() {
                return None;
            }
            return self.components.as_ref()?.messages.get(&key);
        }

        // `#/channels/<path>/publish|subscribe/message[/oneOf/<i>]`,
        // and the same under `#/components/channels/…`.
        let (map, rest) = self.channel_map_at(pointer)?;
        let (key, rest) = split_segment(rest)?;
        let channel = map.get(&key)?;
        let (kind, rest) = split_segment(rest.strip_prefix('/')?)?;
        let operation = match kind.as_str() {
            "publish" => channel.publish.as_ref()?,
            "subscribe" => channel.subscribe.as_ref()?,
            _ => return None,
        };
        let (field, rest) = split_segment(rest.strip_prefix('/')?)?;
        if field != "message" {
            return None;
        }
        let mut message = operation.message.as_ref()?;
        let mut rest = rest;
        // Walk any `oneOf/<index>` steps.
        while !rest.is_empty() {
            let (step, tail) = split_segment(rest.strip_prefix('/')?)?;
            if step != "oneOf" {
                return None;
            }
            let (index, tail) = split_segment(tail.strip_prefix('/')?)?;
            let index: usize = index.parse().ok()?;
            let OperationMessage::OneOf(one_of) = message else {
                return None;
            };
            message = one_of.one_of.get(index)?;
            rest = tail;
        }
        match message {
            OperationMessage::Single(single) => Some(single.as_ref()),
            OperationMessage::OneOf(_) => None,
        }
    }

    /// Every operation in the document, with the channel path and the
    /// half it came from.
    ///
    /// A channel item contributes its own operations *and* those of the
    /// channel it references, since `$ref` siblings are preserved here
    /// rather than discarded.
    pub fn operations(&self) -> Vec<(&str, OperationKind, &Operation)> {
        let mut found = Vec::new();
        for (path, channel) in &self.channels {
            for item in self.channel_chain(channel) {
                if let Some(operation) = &item.publish {
                    found.push((path.as_str(), OperationKind::Publish, operation));
                }
                if let Some(operation) = &item.subscribe {
                    found.push((path.as_str(), OperationKind::Subscribe, operation));
                }
            }
        }
        found
    }

    /// Every channel item along a `$ref` chain, starting with the one
    /// given.
    ///
    /// `$ref` siblings are preserved here rather than discarded, so an
    /// intermediate hop can declare servers, parameters and operations
    /// of its own — all of which the document has to check.
    fn channel_chain<'a>(&'a self, channel: &'a ChannelItem) -> Vec<&'a ChannelItem> {
        let mut chain = vec![channel];
        let mut current = channel;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(reference) = current.reference.as_deref() {
            if !seen.insert(reference) {
                break;
            }
            let Some(pointer) = Reference {
                reference: reference.to_owned(),
            }
            .local_pointer()
            .map(str::to_owned) else {
                break;
            };
            let Some(next) = self.channel_at(&pointer) else {
                break;
            };
            chain.push(next);
            current = next;
        }
        chain
    }

    /// Every message the document declares, inline or in `components`,
    /// walking through `oneOf` sets.
    fn messages(&self) -> Vec<&Message> {
        fn collect<'a>(message: &'a OperationMessage, found: &mut Vec<&'a Message>) {
            match message {
                OperationMessage::Single(single) => {
                    if let Some(message) = single.item() {
                        found.push(message);
                    }
                }
                OperationMessage::OneOf(one_of) => {
                    for alternative in &one_of.one_of {
                        collect(alternative, found);
                    }
                }
            }
        }

        let mut found = Vec::new();
        if let Some(components) = &self.components {
            for message in components.messages.values() {
                if let Some(message) = message.item() {
                    found.push(message);
                }
            }
        }
        for (_, _, operation) in self.operations() {
            if let Some(message) = &operation.message {
                collect(message, &mut found);
            }
        }
        found
    }

    /// Check that a message `$ref` resolves inside this document.
    fn validate_message_refs(&self, ctx: &mut Context, message: &OperationMessage) {
        match message {
            OperationMessage::Single(single) => {
                if let RefOr::Reference(reference) = single.as_ref()
                    && !reference.reference.is_empty()
                    && let Some(problem) = self.resolve_message(reference).problem()
                {
                    ctx.error_field(
                        "$ref",
                        format!("message `{}` {problem}", reference.reference),
                    );
                }
            }
            OperationMessage::OneOf(one_of) => {
                for (i, alternative) in one_of.one_of.iter().enumerate() {
                    ctx.in_index("oneOf", i, |ctx| {
                        self.validate_message_refs(ctx, alternative);
                    });
                }
            }
        }
    }

    /// Check that every requirement names a scheme that resolves, and
    /// lists scopes only where that scheme's type allows them.
    fn validate_security(&self, ctx: &mut Context, field: &str, security: &[SecurityRequirement]) {
        for (i, requirement) in security.iter().enumerate() {
            for (name, scopes) in &requirement.0 {
                let declared = self
                    .components
                    .as_ref()
                    .is_some_and(|c| c.security_schemes.contains_key(name));
                if !declared {
                    ctx.in_index(field, i, |ctx| {
                        ctx.error_field(name, "is not declared in `components.securitySchemes`");
                    });
                    continue;
                }
                let resolution = self.security_scheme_at(name);
                if let Some(problem) = resolution.problem() {
                    ctx.in_index(field, i, |ctx| {
                        ctx.error_field(name, problem);
                    });
                    continue;
                }
                if let Some(scheme) = resolution.found()
                    && !scopes.is_empty()
                    && !scheme.scheme_type.takes_scopes()
                {
                    ctx.in_index(field, i, |ctx| {
                        ctx.error_field(
                            name,
                            format!(
                                "must not list scopes: the `{}` scheme type takes none",
                                scheme.scheme_type.as_str()
                            ),
                        );
                    });
                }
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
            ctx.in_key("servers", name, |ctx| {
                server.validate_with_context(ctx);
                // Follow a `$ref` so a referenced server's security is
                // checked as well as an inline one's.
                let resolution = self.resolve_server(server);
                if let Some(problem) = resolution.problem()
                    && let RefOr::Reference(reference) = server
                {
                    ctx.error_field(
                        "$ref",
                        format!("server `{}` {problem}", reference.reference),
                    );
                }
                if let Some(server) = resolution.found() {
                    self.validate_security(ctx, "security", &server.security);
                }
            });
        }

        // Component servers are reachable only through a `$ref`, so
        // their own security requirements are checked here.
        if let Some(components) = &self.components {
            for (name, server) in &components.servers {
                if let Some(server) = server.item() {
                    ctx.in_key("components", "servers", |ctx| {
                        ctx.in_field(name, |ctx| {
                            self.validate_security(ctx, "security", &server.security);
                        });
                    });
                }
            }
        }

        for (path, channel) in &self.channels {
            if path.is_empty() {
                ctx.error_field("channels", "a channel path must not be empty");
            }
            ctx.in_key("channels", path, |ctx| {
                // The item's own fields are checked either way; a
                // `$ref` additionally has to land on a channel, and the
                // *resolved* item's parameters are what this path's
                // placeholders must match.
                channel.validate_with_context(ctx);
                let resolution = self.resolve_channel(channel);
                if let Some(problem) = resolution.problem()
                    && let Some(reference) = channel.reference.as_deref()
                    && !reference.is_empty()
                {
                    ctx.error_field("$ref", format!("channel `{reference}` {problem}"));
                }
                // The path's placeholders answer to the parameters
                // declared anywhere along the chain, since `$ref`
                // siblings are kept.
                ChannelItem::validate_path_parameters(ctx, path, &self.channel_chain(channel));
                // `$ref` siblings are preserved, so the item as written
                // is checked as well as the channel it references.
                for item in self.channel_chain(channel) {
                    // A channel's servers are plain names into `servers`.
                    for (i, server) in item.servers.iter().enumerate() {
                        if !server.is_empty() && !self.servers.contains_key(server) {
                            ctx.in_index("servers", i, |ctx| {
                                ctx.error(format!("server `{server}` is not declared"));
                            });
                        }
                    }
                    for (kind, operation) in
                        [("publish", &item.publish), ("subscribe", &item.subscribe)]
                    {
                        if let Some(operation) = operation {
                            ctx.in_field(kind, |ctx| {
                                self.validate_security(ctx, "security", &operation.security);
                                if let Some(message) = &operation.message {
                                    ctx.in_field("message", |ctx| {
                                        self.validate_message_refs(ctx, message);
                                    });
                                }
                            });
                        }
                    }
                }
            });
        }

        // `operationId` MUST be unique among all operations in the API.
        let mut seen: BTreeMap<&str, (&str, OperationKind)> = BTreeMap::new();
        for (path, kind, operation) in self.operations() {
            let Some(operation_id) = operation.operation_id.as_deref() else {
                continue;
            };
            if operation_id.is_empty() {
                continue;
            }
            match seen.get(operation_id) {
                Some((first_path, first_kind)) => {
                    ctx.in_key("channels", path, |ctx| {
                        ctx.in_field(kind.as_str(), |ctx| {
                            ctx.error_field(
                                "operationId",
                                format!(
                                    "duplicate operationId `{operation_id}`, already used by `#.channels.{first_path}.{}`",
                                    first_kind.as_str()
                                ),
                            );
                        });
                    });
                }
                None => {
                    seen.insert(operation_id, (path, kind));
                }
            }
        }

        // The same uniqueness rule applies to `messageId`, across
        // every message in the document — component *and* inline,
        // including the members of a `oneOf` set.
        let mut message_ids: BTreeSet<&str> = BTreeSet::new();
        for message in self.messages() {
            let Some(message_id) = message.message_id.as_deref() else {
                continue;
            };
            if !message_id.is_empty() && !message_ids.insert(message_id) {
                ctx.error_field(
                    "messageId",
                    format!("duplicate messageId `{message_id}` in the document"),
                );
            }
        }

        crate::v2_6::message::validate_tags(&mut ctx, &self.tags);
        if let Some(docs) = &self.external_docs {
            ctx.in_field("externalDocs", |ctx| docs.validate_with_context(ctx));
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
            "asyncapi": "2.6.0",
            "info": { "title": "Streetlights", "version": "1.0.0" },
            "channels": {}
        })
    }

    fn wired() -> serde_json::Value {
        json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": {
                "production": { "url": "kafka://broker:9092", "protocol": "kafka" }
            },
            "channels": {
                "user/{userId}/signedup": {
                    "servers": ["production"],
                    "parameters": { "userId": { "schema": { "type": "string" } } },
                    "publish": {
                        "operationId": "receiveSignup",
                        "message": { "name": "UserSignedUp" }
                    },
                    "subscribe": {
                        "operationId": "sendWelcome",
                        "message": { "name": "Welcome" }
                    }
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
        assert_eq!(doc.asyncapi, Version::V2_6_0());
        doc.validate(EnumSet::empty()).expect("valid");
        assert_eq!(serde_json::to_value(&doc).unwrap(), minimal());
    }

    #[test]
    fn channels_is_required_by_the_parser() {
        assert!(
            serde_json::from_value::<Document>(json!({
                "asyncapi": "2.6.0",
                "info": { "title": "T", "version": "1" }
            }))
            .is_err(),
            "2.6 requires `channels`"
        );
    }

    #[test]
    fn rejects_v3_documents_at_parse_time() {
        for version in ["3.0.0", "3.1.0"] {
            let mut value = minimal();
            value["asyncapi"] = json!(version);
            assert!(
                serde_json::from_value::<Document>(value).is_err(),
                "{version}"
            );
        }
    }

    #[test]
    fn fully_wired_document_validates() {
        assert!(
            errors_for(wired()).is_empty(),
            "got: {:?}",
            errors_for(wired())
        );
    }

    #[test]
    fn channel_servers_must_name_declared_servers() {
        let mut value = wired();
        value["channels"]["user/{userId}/signedup"]["servers"] = json!(["staging"]);
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.channels.user/{userId}/signedup.servers[0]: server `staging` is not declared"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn operation_ids_must_be_unique_across_the_document() {
        let mut value = wired();
        value["channels"]["other"] = json!({
            "publish": { "operationId": "receiveSignup" }
        });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("duplicate operationId `receiveSignup`")),
            "got: {errors:?}"
        );

        // The same id on publish and subscribe of one channel collides
        // too.
        let mut same_channel = wired();
        same_channel["channels"]["user/{userId}/signedup"]["subscribe"]["operationId"] =
            json!("receiveSignup");
        assert!(
            errors_for(same_channel)
                .iter()
                .any(|e| e.contains("duplicate operationId")),
        );
    }

    #[test]
    fn a_local_channel_reference_is_resolved_for_document_checks() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": { "production": { "url": "kafka://e", "protocol": "kafka" } },
            "channels": {
                "user/{userId}": { "$ref": "#/components/channels/user" }
            },
            "components": {
                "channels": {
                    "user": {
                        "servers": ["staging"],
                        "publish": { "operationId": "handle" }
                    }
                }
            }
        });
        let errors = errors_for(value);
        // The referenced channel's server list is checked…
        assert!(
            errors
                .iter()
                .any(|e| e.contains("server `staging` is not declared")),
            "got: {errors:?}"
        );
        // …and its parameters are checked against *this* path.
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`{userId}` in the channel path is not declared")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_local_channel_reference_that_names_nothing_is_reported() {
        let mut value = minimal();
        value["channels"] = json!({ "user": { "$ref": "#/components/channels/ghost" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("names nothing in this document")),
        );
    }

    #[test]
    fn a_channel_reference_cycle_terminates() {
        // Two component channels pointing at each other must not hang
        // the resolver.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user": { "$ref": "#/components/channels/a" } },
            "components": {
                "channels": {
                    "a": { "$ref": "#/components/channels/b" },
                    "b": { "$ref": "#/components/channels/a" }
                }
            }
        });
        let doc: Document = serde_json::from_value(value).unwrap();
        assert!(doc.resolve_channel(&doc.channels["user"]).found().is_none());
        assert!(doc.operations().is_empty());
        // Validation still terminates and reports the unresolvable ref.
        let _ = doc.validate(EnumSet::empty());
    }

    #[test]
    fn an_external_channel_reference_is_left_alone() {
        let mut value = minimal();
        value["channels"] = json!({ "user": { "$ref": "./channels.yaml#/user" } });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn message_ids_must_be_unique_across_the_whole_document() {
        // Two component messages…
        let mut value = minimal();
        value["components"] = json!({
            "messages": {
                "a": { "messageId": "signup" },
                "b": { "messageId": "signup" }
            }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("duplicate messageId `signup`")),
        );

        // …a component message and an inline one…
        let mut value = minimal();
        value["components"] = json!({ "messages": { "a": { "messageId": "signup" } } });
        value["channels"] = json!({
            "user": { "publish": { "message": { "messageId": "signup" } } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("duplicate messageId `signup`")),
            "an inline message must collide with a component one",
        );

        // …and two inline messages inside a `oneOf` set.
        let mut value = minimal();
        value["channels"] = json!({
            "user": {
                "publish": {
                    "message": {
                        "oneOf": [ { "messageId": "dup" }, { "messageId": "dup" } ]
                    }
                }
            }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("duplicate messageId `dup`")),
            "oneOf members must be walked",
        );

        // Distinct ids are fine.
        let mut value = minimal();
        value["channels"] = json!({
            "user": {
                "publish": { "message": { "oneOf": [ { "messageId": "a" }, { "messageId": "b" } ] } }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn a_referenced_security_scheme_is_resolved_before_judging_scopes() {
        let mut value = minimal();
        value["components"] = json!({
            "securitySchemes": {
                "alias": { "$ref": "#/components/securitySchemes/basic" },
                "basic": { "type": "userPassword" }
            }
        });
        value["channels"] = json!({
            "user": { "publish": { "security": [ { "alias": ["read"] } ] } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e
                    .contains("must not list scopes: the `userPassword` scheme type takes none")),
            "a referenced scheme must be followed to its type",
        );
    }

    #[test]
    fn a_message_reference_must_name_a_declared_component_message() {
        let mut value = minimal();
        value["channels"] = json!({
            "user": { "publish": { "message": { "$ref": "#/components/messages/missing" } } }
        });
        assert!(errors_for(value).iter().any(|e| {
            e.contains("message `#/components/messages/missing` names nothing in this document")
        }),);

        // Declared is fine; external is left alone; `oneOf` members are
        // checked individually.
        let mut value = minimal();
        value["components"] = json!({ "messages": { "signup": { "name": "S" } } });
        value["channels"] = json!({
            "user": {
                "publish": {
                    "message": {
                        "oneOf": [
                            { "$ref": "#/components/messages/signup" },
                            { "$ref": "./other.yaml#/signup" }
                        ]
                    }
                }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn a_root_channel_pointer_resolves_too() {
        // `#/channels/<path>` is as legal as the components form, and
        // the path's `/` is RFC 6901-escaped.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "user/signedup": { "publish": { "operationId": "handle" } },
                "alias": { "$ref": "#/channels/user~1signedup" }
            }
        });
        let doc: Document = serde_json::from_value(value.clone()).unwrap();
        assert!(
            doc.resolve_channel(&doc.channels["alias"])
                .found()
                .is_some()
        );
        // Both the target and the alias contribute their operation, so
        // the duplicate `operationId` is reported.
        assert_eq!(doc.operations().len(), 2);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("duplicate operationId `handle`")),
        );
    }

    #[test]
    fn ref_siblings_are_validated_as_well_as_the_target() {
        // A `$ref` sibling survives serialization, so it is checked
        // too: this item's own `servers` names nothing, and its own
        // operation joins the target's.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": { "production": { "url": "kafka://e", "protocol": "kafka" } },
            "channels": {
                "user": {
                    "$ref": "#/components/channels/target",
                    "servers": ["missing"],
                    "publish": { "operationId": "own" }
                }
            },
            "components": {
                "channels": { "target": { "subscribe": { "operationId": "target" } } }
            }
        });
        let doc: Document = serde_json::from_value(value.clone()).unwrap();
        let ids: Vec<_> = doc
            .operations()
            .iter()
            .filter_map(|(_, _, op)| op.operation_id.as_deref())
            .collect();
        assert!(
            ids.contains(&"own"),
            "the item's own operation counts: {ids:?}"
        );
        assert!(ids.contains(&"target"), "so does the target's: {ids:?}");

        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("server `missing` is not declared")),
            "a sibling `servers` list is still checked",
        );
    }

    #[test]
    fn a_message_pointer_into_a_channel_operation_resolves() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source": { "publish": { "message": { "name": "S" } } },
                "target": {
                    "subscribe": { "message": { "$ref": "#/channels/source/publish/message" } }
                }
            }
        });
        assert!(
            errors_for(value).is_empty(),
            "a legal local pointer must resolve"
        );

        // …including into a `oneOf` member.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source": {
                    "publish": { "message": { "oneOf": [ { "name": "A" }, { "name": "B" } ] } }
                },
                "target": {
                    "subscribe": {
                        "message": { "$ref": "#/channels/source/publish/message/oneOf/1" }
                    }
                }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn a_message_alias_chain_must_terminate_somewhere_real() {
        // components alias → missing target.
        let mut value = minimal();
        value["components"] = json!({
            "messages": { "alias": { "$ref": "#/components/messages/missing" } }
        });
        value["channels"] = json!({
            "user": { "publish": { "message": { "$ref": "#/components/messages/alias" } } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("names nothing in this document")),
            "an alias to a missing message must be reported",
        );

        // A cycle terminates and is named as one.
        let mut value = minimal();
        value["components"] = json!({
            "messages": {
                "a": { "$ref": "#/components/messages/b" },
                "b": { "$ref": "#/components/messages/a" }
            }
        });
        value["channels"] = json!({
            "user": { "publish": { "message": { "$ref": "#/components/messages/a" } } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("is part of a reference cycle")),
        );
    }

    #[test]
    fn a_dangling_security_alias_is_reported() {
        let mut value = minimal();
        value["components"] = json!({
            "securitySchemes": { "alias": { "$ref": "#/components/securitySchemes/missing" } }
        });
        value["channels"] = json!({
            "user": { "publish": { "security": [ { "alias": [] } ] } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("names nothing in this document")),
            "an alias that resolves to nothing must not count as declared",
        );

        // A cyclic alias likewise.
        let mut value = minimal();
        value["components"] = json!({
            "securitySchemes": {
                "a": { "$ref": "#/components/securitySchemes/b" },
                "b": { "$ref": "#/components/securitySchemes/a" }
            }
        });
        value["channels"] = json!({ "user": { "publish": { "security": [ { "a": [] } ] } } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("is part of a reference cycle")),
        );
    }

    #[test]
    fn a_chain_ending_outside_the_document_is_accepted() {
        // root alias → component channel → external `$ref`.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "alias": { "$ref": "#/components/channels/hop" } },
            "components": {
                "channels": { "hop": { "$ref": "./other.yaml#/channels/real" } }
            }
        });
        assert!(
            errors_for(value).is_empty(),
            "an external terminus is not a document bug",
        );
    }

    #[test]
    fn a_referenced_server_still_has_its_security_checked() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "servers": { "prod": { "$ref": "#/components/servers/real" } },
            "components": {
                "servers": {
                    "real": {
                        "url": "kafka://e",
                        "protocol": "kafka",
                        "security": [ { "missing": [] } ]
                    }
                }
            }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("is not declared in `components.securitySchemes`")),
            "a referenced server's security must be checked",
        );
    }

    #[test]
    fn a_root_channel_pointer_must_escape_its_separators() {
        // `#/channels/source/path` is two segments, so it cannot name
        // the key `source/path` — RFC 6901 wants `source~1path`.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source/path": { "publish": {} },
                "alias": { "$ref": "#/channels/source/path" }
            }
        });
        let doc: Document = serde_json::from_value(value.clone()).unwrap();
        assert!(
            doc.resolve_channel(&doc.channels["alias"])
                .found()
                .is_none()
        );
        assert!(
            !errors_for(value).is_empty(),
            "the unescaped pointer must be reported"
        );

        // The escaped spelling resolves.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source/path": { "publish": {} },
                "alias": { "$ref": "#/channels/source~1path" }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn pointers_of_the_wrong_kind_are_told_apart_from_missing_ones() {
        // A local pointer that cannot denote a channel at all.
        let mut value = minimal();
        value["channels"] = json!({ "user": { "$ref": "#/components/schemas/thing" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // …and the same for a message.
        let mut value = minimal();
        value["channels"] = json!({
            "user": { "publish": { "message": { "$ref": "#/components/schemas/thing" } } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );
    }

    #[test]
    fn external_aliases_terminate_every_resolver() {
        // Security scheme, server, and message chains that leave the
        // document are all accepted.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": { "prod": { "$ref": "#/components/servers/hop" } },
            "channels": {
                "user": {
                    "publish": {
                        "security": [ { "alias": [] } ],
                        "message": { "$ref": "#/components/messages/hop" }
                    }
                }
            },
            "components": {
                "servers": { "hop": { "$ref": "./other.yaml#/servers/real" } },
                "securitySchemes": { "alias": { "$ref": "./other.yaml#/schemes/real" } },
                "messages": { "hop": { "$ref": "./other.yaml#/messages/real" } }
            }
        });
        assert!(
            errors_for(value).is_empty(),
            "external termini are accepted"
        );
    }

    #[test]
    fn server_aliases_are_resolved_like_the_others() {
        // Missing target.
        let mut value = minimal();
        value["servers"] = json!({ "prod": { "$ref": "#/components/servers/ghost" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("names nothing in this document")),
        );

        // Wrong kind of pointer.
        let mut value = minimal();
        value["servers"] = json!({ "prod": { "$ref": "#/components/schemas/thing" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("does not point at an object of the expected kind")),
        );

        // Cycle.
        let mut value = minimal();
        value["servers"] = json!({ "prod": { "$ref": "#/components/servers/a" } });
        value["components"] = json!({
            "servers": {
                "a": { "$ref": "#/components/servers/b" },
                "b": { "$ref": "#/components/servers/a" }
            }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("is part of a reference cycle")),
        );
    }

    #[test]
    fn message_pointers_into_channels_reject_every_wrong_step() {
        let base = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source": {
                    "publish": { "message": { "oneOf": [ { "name": "A" } ] } },
                    "subscribe": { "message": { "name": "S" } }
                },
                "target": { "subscribe": {} }
            }
        });
        for pointer in [
            "#/channels/ghost/publish/message",            // no such channel
            "#/channels/source/deliver/message",           // no such operation half
            "#/channels/source/publish/payload",           // not the message field
            "#/channels/source/publish/message/oneOf/9",   // index past the end
            "#/channels/source/publish/message/oneOf/x",   // non-numeric index
            "#/channels/source/publish/message/anyOf/0",   // not a oneOf step
            "#/channels/source/subscribe/message/oneOf/0", // not a set at all
            "#/channels/source/publish/message",           // terminates on a set
        ] {
            let mut value = base.clone();
            value["channels"]["target"]["subscribe"]["message"] = json!({ "$ref": pointer });
            assert!(!errors_for(value).is_empty(), "{pointer} must not resolve");
        }
    }

    #[test]
    fn every_hop_of_a_ref_chain_is_validated() {
        // A → B → C: B's own siblings must not be skipped.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": { "prod": { "url": "kafka://e", "protocol": "kafka" } },
            "channels": { "a": { "$ref": "#/components/channels/b" } },
            "components": {
                "channels": {
                    "b": {
                        "$ref": "#/components/channels/c",
                        "servers": ["missing"],
                        "publish": { "operationId": "onB" }
                    },
                    "c": { "subscribe": { "operationId": "onC" } }
                }
            }
        });
        let doc: Document = serde_json::from_value(value.clone()).unwrap();
        let ids: Vec<_> = doc
            .operations()
            .iter()
            .filter_map(|(_, _, op)| op.operation_id.as_deref())
            .collect();
        assert!(
            ids.contains(&"onB"),
            "the middle hop's operation counts: {ids:?}"
        );
        assert!(ids.contains(&"onC"), "so does the terminal one: {ids:?}");
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("server `missing` is not declared")),
            "a middle hop's servers are checked too",
        );
    }

    #[test]
    fn parameters_may_sit_anywhere_along_the_chain() {
        // Declared beside the `$ref`, with the target declaring none.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "user/{id}": {
                    "$ref": "#/components/channels/target",
                    "parameters": { "id": { "schema": { "type": "string" } } }
                }
            },
            "components": { "channels": { "target": { "publish": {} } } }
        });
        assert!(
            errors_for(value).is_empty(),
            "a sibling parameter satisfies the path"
        );

        // …and on the target, with the reference declaring none.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user/{id}": { "$ref": "#/components/channels/target" } },
            "components": {
                "channels": {
                    "target": { "parameters": { "id": { "schema": { "type": "string" } } } }
                }
            }
        });
        assert!(errors_for(value).is_empty());

        // A placeholder declared nowhere is still reported.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user/{id}": { "$ref": "#/components/channels/target" } },
            "components": { "channels": { "target": { "publish": {} } } }
        });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e.contains("`{id}` in the channel path is not declared")),
        );
    }

    #[test]
    fn message_pointers_walk_the_document_as_written() {
        // A message declared *beside* a channel's `$ref` is reachable
        // by pointer, because a JSON Pointer does not resolve as it
        // walks.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source": {
                    "$ref": "#/components/channels/elsewhere",
                    "publish": { "message": { "name": "Beside" } }
                },
                "target": {
                    "subscribe": { "message": { "$ref": "#/channels/source/publish/message" } }
                }
            },
            "components": { "channels": { "elsewhere": { "subscribe": {} } } }
        });
        assert!(
            errors_for(value).is_empty(),
            "a sibling message is reachable"
        );

        // Pointers through a component channel work too.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "target": {
                    "subscribe": {
                        "message": { "$ref": "#/components/channels/source/publish/message" }
                    }
                }
            },
            "components": {
                "channels": { "source": { "publish": { "message": { "name": "S" } } } }
            }
        });
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn a_server_may_reference_another_root_server() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "servers": {
                "base": { "url": "kafka://e", "protocol": "kafka" },
                "alias": { "$ref": "#/servers/base" }
            }
        });
        assert!(
            errors_for(value).is_empty(),
            "`#/servers/…` is a legal target"
        );
    }

    #[test]
    fn pointer_segments_are_percent_decoded_and_escape_checked() {
        // `%20` decodes to a space…
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source path": { "publish": {} },
                "alias": { "$ref": "#/channels/source%20path" }
            }
        });
        let doc: Document = serde_json::from_value(value.clone()).unwrap();
        assert!(
            doc.resolve_channel(&doc.channels["alias"])
                .found()
                .is_some()
        );
        assert!(errors_for(value).is_empty());

        // …while `~2` is not a valid escape, so the pointer is not a
        // literal key.
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source~2path": { "publish": {} },
                "alias": { "$ref": "#/channels/source~2path" }
            }
        });
        assert!(
            !errors_for(value).is_empty(),
            "an invalid tilde escape must not resolve",
        );
    }

    #[test]
    fn strict_mode_reports_external_refs_on_channel_items_and_schemas() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "user": { "$ref": "./other.yaml#/channels/user" } },
            "components": {
                "schemas": { "s": { "$ref": "./other.yaml#/schemas/s" } }
            }
        });
        let doc: Document = serde_json::from_value(value).unwrap();
        doc.validate(EnumSet::empty())
            .expect("external refs pass by default");

        let err = doc
            .validate(EnumSet::only(ValidationOptions::ErrorOnExternalReference))
            .unwrap_err();
        let errors: Vec<_> = err.errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            errors
                .iter()
                .filter(|e| e.contains("external reference"))
                .count(),
            2,
            "both the channel item and the schema `$ref` count: {errors:?}",
        );
    }

    #[test]
    fn a_channel_item_keeps_its_ref_siblings() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "user": { "$ref": "#/components/channels/target", "description": "d" }
            },
            "components": { "channels": { "target": { "publish": {} } } }
        });
        let doc: Document = serde_json::from_value(value.clone()).unwrap();
        let channel = &doc.channels["user"];
        assert_eq!(channel.description.as_deref(), Some("d"));
        assert!(channel.is_reference());
        assert_eq!(serde_json::to_value(&doc).unwrap(), value);
    }

    #[test]
    fn root_tags_must_be_unique_and_external_docs_takes_no_reference() {
        let mut value = minimal();
        value["tags"] = json!([ { "name": "a" }, { "name": "a" } ]);
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e == "#.tags[1]: duplicate tag"),
        );

        let mut value = minimal();
        value["externalDocs"] = json!({ "$ref": "#/anything" });
        assert!(
            serde_json::from_value::<Document>(value).is_err(),
            "2.6 requires a concrete External Documentation Object",
        );
    }

    #[test]
    fn security_requirements_must_name_declared_schemes() {
        let mut value = minimal();
        value["servers"] = json!({
            "prod": { "url": "kafka://e", "protocol": "kafka", "security": [ { "missing": [] } ] }
        });
        let errors = errors_for(value);
        assert!(
            errors.iter().any(|e| e
                == "#.servers.prod.security[0].missing: is not declared in `components.securitySchemes`"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn scopes_are_only_allowed_on_oauth_style_schemes() {
        let mut value = minimal();
        value["components"] = json!({
            "securitySchemes": {
                "user_pass": { "type": "userPassword" },
                "oauth": {
                    "type": "oauth2",
                    "flows": { "implicit": { "authorizationUrl": "https://e/a", "scopes": {} } }
                }
            }
        });
        value["channels"] = json!({
            "user": {
                "publish": {
                    "security": [ { "user_pass": ["read"] }, { "oauth": ["read"] } ]
                }
            }
        });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e
                    .contains("must not list scopes: the `userPassword` scheme type takes none")),
            "got: {errors:?}"
        );
        // The oauth2 requirement is fine.
        assert!(
            !errors.iter().any(|e| e.contains("oauth:")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn operations_enumerates_both_halves_of_every_channel() {
        let doc: Document = serde_json::from_value(wired()).unwrap();
        let operations = doc.operations();
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].1, OperationKind::Publish);
        assert_eq!(operations[1].1, OperationKind::Subscribe);
        assert_eq!(operations[0].0, "user/{userId}/signedup");

        // A channel that is a `$ref` contributes nothing.
        let mut referenced = minimal();
        referenced["channels"] = json!({ "user": { "$ref": "./channels.yaml#/user" } });
        let doc: Document = serde_json::from_value(referenced).unwrap();
        assert!(doc.operations().is_empty());
    }

    #[test]
    fn an_empty_channel_path_is_reported() {
        let mut value = minimal();
        value["channels"] = json!({ "": { "publish": {} } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e == "#.channels: a channel path must not be empty")
        );
    }

    #[test]
    fn root_tags_and_external_docs_are_validated() {
        let mut value = minimal();
        value["tags"] = json!([ { "name": "" } ]);
        value["externalDocs"] = json!({ "url": "" });
        let errors = errors_for(value);
        assert!(
            errors
                .iter()
                .any(|e| e == "#.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.externalDocs.url: must not be empty")
        );
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
    fn a_referenced_channel_is_validated_as_a_reference() {
        let mut value = minimal();
        value["channels"] = json!({ "user": { "$ref": "" } });
        assert!(
            errors_for(value)
                .iter()
                .any(|e| e == "#.channels.user.$ref: must not be empty")
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
