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

impl Document {
    /// Resolve a channel entry, following document-local
    /// `#/components/channels/…` references.
    ///
    /// Returns `None` for an external reference, one that names nothing,
    /// or a cycle — none of which this crate can see through.
    pub fn resolve_channel<'a>(&'a self, channel: &'a ChannelItem) -> Option<&'a ChannelItem> {
        let mut current = channel;
        // Follow the chain, stopping if it ever revisits a pointer —
        // which also bounds the walk, so no separate hop limit is
        // needed.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        loop {
            let Some(reference) = current.reference.as_deref() else {
                return Some(current);
            };
            if !seen.insert(reference) {
                return None;
            }
            current = self.channel_at(reference)?;
        }
    }

    /// The channel a document-local pointer names, whether it points
    /// into the root `channels` map or into `components.channels`.
    ///
    /// Root channel keys are paths, so their pointers carry RFC 6901
    /// escapes (`~1` for `/`), which [`Reference::local_key`] decodes.
    fn channel_at(&self, reference: &str) -> Option<&ChannelItem> {
        let reference = Reference {
            reference: reference.to_owned(),
        };
        if let Some(key) = reference.component_key("channels") {
            return self.components.as_ref()?.channels.get(&key);
        }
        let pointer = reference.local_pointer()?;
        let key = pointer.strip_prefix("/channels/")?;
        if key.is_empty() {
            return None;
        }
        let key = key.replace("~1", "/").replace("~0", "~");
        self.channels.get(&key)
    }

    /// Every operation in the document, with the channel path and the
    /// half it came from. Channels that are a document-local `$ref` are
    /// resolved first.
    pub fn operations(&self) -> Vec<(&str, OperationKind, &Operation)> {
        let mut found = Vec::new();
        for (path, channel) in &self.channels {
            let Some(channel) = self.resolve_channel(channel) else {
                continue;
            };
            if let Some(operation) = &channel.publish {
                found.push((path.as_str(), OperationKind::Publish, operation));
            }
            if let Some(operation) = &channel.subscribe {
                found.push((path.as_str(), OperationKind::Subscribe, operation));
            }
        }
        found
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

    /// The security scheme declared under `components.securitySchemes`,
    /// following a document-local `$ref` so a referenced scheme is
    /// judged by its real type.
    fn security_scheme(&self, name: &str) -> Option<&SecurityScheme> {
        let mut entry = self.components.as_ref()?.security_schemes.get(name)?;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            match entry {
                RefOr::Item(scheme) => return Some(scheme),
                RefOr::Reference(reference) => {
                    let key = reference.component_key("securitySchemes")?;
                    if !seen.insert(key.clone()) {
                        return None;
                    }
                    entry = self.components.as_ref()?.security_schemes.get(&key)?;
                }
            }
        }
    }

    /// Check that a document-local message `$ref` lands on a declared
    /// component message.
    fn validate_message_refs(&self, ctx: &mut Context, message: &OperationMessage) {
        match message {
            OperationMessage::Single(single) => {
                if let RefOr::Reference(reference) = single.as_ref() {
                    if reference.is_external() || reference.reference.is_empty() {
                        return;
                    }
                    let declared = reference
                        .component_key("messages")
                        .and_then(|key| Some(self.components.as_ref()?.messages.contains_key(&key)))
                        .unwrap_or(false);
                    if !declared {
                        ctx.error_field(
                            "$ref",
                            format!(
                                "message `{}` is not declared in `components.messages`",
                                reference.reference
                            ),
                        );
                    }
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

    /// Whether `components.securitySchemes` names `name` at all — a
    /// `$ref`'d entry counts as declared even though it cannot be
    /// inspected.
    fn declares_security_scheme(&self, name: &str) -> bool {
        self.components
            .as_ref()
            .is_some_and(|c| c.security_schemes.contains_key(name))
    }

    /// Check that every requirement names a declared scheme, and lists
    /// scopes only where the scheme's type allows them.
    fn validate_security(&self, ctx: &mut Context, field: &str, security: &[SecurityRequirement]) {
        for (i, requirement) in security.iter().enumerate() {
            for (name, scopes) in &requirement.0 {
                if !self.declares_security_scheme(name) {
                    ctx.in_index(field, i, |ctx| {
                        ctx.error_field(name, "is not declared in `components.securitySchemes`");
                    });
                    continue;
                }
                if let Some(scheme) = self.security_scheme(name)
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
                if let Some(server) = server.item() {
                    self.validate_security(ctx, "security", &server.security);
                }
            });
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
                match self.resolve_channel(channel) {
                    Some(resolved) => resolved.validate_against_path(ctx, path),
                    None => {
                        if let Some(reference) = channel.reference.as_deref() {
                            let reference = Reference {
                                reference: reference.to_owned(),
                            };
                            if !reference.is_external() && !reference.reference.is_empty() {
                                ctx.error_field(
                                    "$ref",
                                    format!(
                                        "`{}` does not resolve to a declared channel",
                                        reference.reference
                                    ),
                                );
                            }
                        }
                    }
                }
                if let Some(item) = self.resolve_channel(channel) {
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
                .any(|e| e.contains("does not resolve to a declared channel")),
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
        assert!(doc.resolve_channel(&doc.channels["user"]).is_none());
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
        assert!(errors_for(value).iter().any(|e| e.contains(
            "message `#/components/messages/missing` is not declared in `components.messages`"
        )),);

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
        assert!(doc.resolve_channel(&doc.channels["alias"]).is_some());
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
