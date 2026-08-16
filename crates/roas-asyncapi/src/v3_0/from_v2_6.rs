//! Conversion from AsyncAPI v2.6 to v3.0.
//!
//! v3 reorganized the document rather than extending it, so this is not
//! the field-for-field remap that [v3.0 → v3.1](crate::v3_1) is. A
//! channel no longer *is* its address, operations left the channel to
//! stand on their own, and the point of view flipped: what 2.6 called
//! `publish` is what the application *receives*.
//!
//! Some of that cannot be carried across whole. Rather than decide
//! silently, [`convert`] returns a [`ConversionReport`] alongside the
//! document, one [`Note`] per place where a name was invented or
//! something was left behind.
//!
//! Available when both the `v2_6` and `v3_0` features are enabled.

use crate::common::reference::{RefOr, Reference};
use crate::{v2_6, v3_0};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Everything the conversion had to decide or leave behind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConversionReport {
    /// One entry per decision, in document order.
    pub notes: Vec<Note>,
}

impl ConversionReport {
    /// Whether the conversion carried everything across unchanged.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.notes.is_empty()
    }
}

/// One decision, and where in the *source* document it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Note {
    /// A path into the v2.6 document, e.g. `#.channels.user/signedup`.
    pub at: String,
    pub kind: NoteKind,
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.kind)
    }
}

/// What kind of decision a [`Note`] records.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NoteKind {
    /// A channel is keyed by its address in 2.6 and by a name in 3.0,
    /// and this address is not a usable name. The address itself is
    /// kept, in the channel's `address`.
    ChannelKeyDerived { address: String, key: String },
    /// An operation is keyed by name in 3.0, and this one had no
    /// `operationId` to use.
    OperationKeyDerived { key: String },
    /// A message needs a name inside its channel, and this one had
    /// neither `messageId` nor `name`.
    MessageKeyDerived { key: String },
    /// 2.6's `publish` is what the application receives, and
    /// `subscribe` what it sends — the point of view flipped in v3.
    ActionFlipped {
        from: &'static str,
        to: &'static str,
    },
    /// A server's `url` became a `host` and a `pathname`.
    ServerUrlSplit {
        url: String,
        host: String,
        pathname: Option<String>,
    },
    /// 3.0 states security requirements as references to the schemes
    /// themselves, which carry their own scopes; a requirement's own
    /// scopes have nowhere to go.
    SecurityScopesDropped { scheme: String, scopes: Vec<String> },
    /// A 2.6 parameter carries a schema; a 3.0 parameter is a string
    /// with an optional enumeration, so anything else it said is lost.
    ParameterSchemaDropped,
    /// 3.0 has no `deprecated` on a channel.
    ChannelDeprecationDropped,
    /// 2.6's `$ref` on a channel item may carry siblings, whose
    /// behaviour that specification leaves undefined. 3.0's Reference
    /// Object may not, so they are dropped.
    ChannelReferenceSiblingsDropped,
    /// A pointer into the source document that no longer names the same
    /// thing, and that this conversion could not rewrite.
    ReferenceNotRewritten { reference: String },
    /// A value did not survive being re-read as its 3.0 counterpart.
    NotConverted { what: &'static str },
}

impl fmt::Display for NoteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoteKind::ChannelKeyDerived { address, key } => {
                write!(
                    f,
                    "address `{address}` is not a usable key; keyed as `{key}`"
                )
            }
            NoteKind::OperationKeyDerived { key } => {
                write!(f, "no `operationId`; keyed as `{key}`")
            }
            NoteKind::MessageKeyDerived { key } => {
                write!(f, "no `messageId` or `name`; keyed as `{key}`")
            }
            NoteKind::ActionFlipped { from, to } => {
                write!(f, "`{from}` is what the application does not do; `{to}` is")
            }
            NoteKind::ServerUrlSplit {
                url,
                host,
                pathname,
            } => match pathname {
                Some(pathname) => {
                    write!(
                        f,
                        "`{url}` split into host `{host}` and pathname `{pathname}`"
                    )
                }
                None => write!(f, "`{url}` became host `{host}`"),
            },
            NoteKind::SecurityScopesDropped { scheme, scopes } => write!(
                f,
                "scopes {scopes:?} required of `{scheme}` have no place in v3",
            ),
            NoteKind::ParameterSchemaDropped => {
                f.write_str("a v3 parameter is a string, so `schema` is dropped")
            }
            NoteKind::ChannelDeprecationDropped => f.write_str("a v3 channel has no `deprecated`"),
            NoteKind::ChannelReferenceSiblingsDropped => {
                f.write_str("a Reference Object is `$ref` alone, so what sat beside it is dropped")
            }
            NoteKind::ReferenceNotRewritten { reference } => {
                write!(f, "`{reference}` no longer names the same thing in v3")
            }
            NoteKind::NotConverted { what } => write!(f, "`{what}` could not be converted"),
        }
    }
}

/// Convert a v2.6 document to v3.0, reporting what could not be carried
/// across unchanged.
///
/// The conversion always produces a document; the report says what it
/// had to invent or leave behind. An empty report means nothing was
/// lost — see [`ConversionReport::is_clean`].
#[must_use]
pub fn convert(document: v2_6::Document) -> (v3_0::Document, ConversionReport) {
    let mut conversion = Conversion::new(&document);
    let converted = conversion.document(document);
    (
        converted,
        ConversionReport {
            notes: conversion.notes,
        },
    )
}

/// The conversion in progress: what it has decided, and what it has to
/// say about it.
struct Conversion {
    notes: Vec<Note>,
    /// v2.6 channel address → the v3.0 key it was given.
    channel_keys: BTreeMap<String, String>,
    taken_operations: BTreeSet<String>,
}

impl Conversion {
    /// Key every channel first: an operation names its channel, and a
    /// `$ref` may name one too, so the names have to exist before
    /// anything that points at them is converted.
    fn new(document: &v2_6::Document) -> Self {
        let mut notes = Vec::new();
        let mut taken = BTreeSet::new();
        let mut channel_keys = BTreeMap::new();
        // An address that is already a usable key keeps it, whatever
        // some other address would sanitize to.
        for address in document.channels.keys() {
            if !address.is_empty() && sanitize(address) == *address {
                taken.insert(address.clone());
                channel_keys.insert(address.clone(), address.clone());
            }
        }
        for address in document.channels.keys() {
            if channel_keys.contains_key(address) {
                continue;
            }
            let key = unique(sanitize(address), &mut taken);
            if key != *address {
                notes.push(Note {
                    at: format!("#.channels.{address}"),
                    kind: NoteKind::ChannelKeyDerived {
                        address: address.clone(),
                        key: key.clone(),
                    },
                });
            }
            channel_keys.insert(address.clone(), key);
        }
        Self {
            notes,
            channel_keys,
            taken_operations: BTreeSet::new(),
        }
    }

    fn note(&mut self, at: &str, kind: NoteKind) {
        self.notes.push(Note {
            at: at.to_owned(),
            kind,
        });
    }

    fn document(&mut self, document: v2_6::Document) -> v3_0::Document {
        let mut channels = BTreeMap::new();
        let mut operations = BTreeMap::new();
        for (address, item) in document.channels {
            let key = self.channel_keys[&address].clone();
            let at = format!("#.channels.{address}");
            let (channel, channel_operations) = self.channel(&at, &address, &key, item);
            channels.insert(key, channel);
            operations.extend(channel_operations);
        }

        v3_0::Document {
            asyncapi: v3_0::Version::default(),
            id: document.id,
            // v3 moved the document's own tags and documentation under
            // `info`, where the rest of its metadata already lived.
            info: self.info(document.info, document.tags, document.external_docs),
            servers: document
                .servers
                .into_iter()
                .map(|(name, server)| {
                    let at = format!("#.servers.{name}");
                    (name, self.ref_or(&at, server, Self::server))
                })
                .collect(),
            default_content_type: document.default_content_type,
            channels,
            operations,
            components: document
                .components
                .map(|components| self.components(components)),
            extensions: document.extensions,
        }
    }

    fn info(
        &mut self,
        info: v2_6::Info,
        tags: Vec<v2_6::Tag>,
        external_docs: Option<v2_6::ExternalDocumentation>,
    ) -> v3_0::Info {
        v3_0::Info {
            title: info.title,
            version: info.version,
            description: info.description,
            terms_of_service: info.terms_of_service,
            contact: info
                .contact
                .and_then(|contact| self.reinterpret("info.contact", &contact)),
            license: info
                .license
                .and_then(|license| self.reinterpret("info.license", &license)),
            tags: tags
                .into_iter()
                .map(|tag| RefOr::Item(self.tag(tag)))
                .collect(),
            external_docs: external_docs.map(|docs| RefOr::Item(external_documentation(docs))),
            extensions: info.extensions,
        }
    }

    fn server(&mut self, at: &str, server: v2_6::Server) -> v3_0::Server {
        let (host, pathname) = split_url(&server.url);
        if host != server.url {
            self.note(
                at,
                NoteKind::ServerUrlSplit {
                    url: server.url.clone(),
                    host: host.clone(),
                    pathname: pathname.clone(),
                },
            );
        }
        v3_0::Server {
            host,
            protocol: server.protocol,
            pathname,
            protocol_version: server.protocol_version,
            title: None,
            summary: None,
            description: server.description,
            variables: server
                .variables
                .into_iter()
                .map(|(name, variable)| {
                    let at = format!("{at}.variables.{name}");
                    (name, self.ref_or(&at, variable, Self::server_variable))
                })
                .collect(),
            security: self.security(at, server.security),
            tags: server
                .tags
                .into_iter()
                .map(|tag| RefOr::Item(self.tag(tag)))
                .collect(),
            external_docs: None,
            bindings: server.bindings,
            extensions: server.extensions,
        }
    }

    fn server_variable(
        &mut self,
        at: &str,
        variable: v2_6::ServerVariable,
    ) -> v3_0::ServerVariable {
        self.reinterpret(at, &variable).unwrap_or_default()
    }

    /// A 2.6 requirement names a scheme and the scopes it needs; a 3.0
    /// one names the scheme, which carries its own scopes.
    fn security(
        &mut self,
        at: &str,
        requirements: Vec<v2_6::SecurityRequirement>,
    ) -> Vec<RefOr<v3_0::SecurityScheme>> {
        let mut schemes = Vec::new();
        for requirement in requirements {
            for (scheme, scopes) in requirement.0 {
                if !scopes.is_empty() {
                    self.note(
                        at,
                        NoteKind::SecurityScopesDropped {
                            scheme: scheme.clone(),
                            scopes,
                        },
                    );
                }
                schemes.push(RefOr::Reference(Reference {
                    reference: format!("#/components/securitySchemes/{scheme}"),
                }));
            }
        }
        schemes
    }

    fn tag(&mut self, tag: v2_6::Tag) -> v3_0::Tag {
        v3_0::Tag {
            name: tag.name,
            description: tag.description,
            external_docs: tag
                .external_docs
                .map(|docs| RefOr::Item(external_documentation(docs))),
            extensions: tag.extensions,
        }
    }

    /// Split a channel item into the channel it describes and the
    /// operations that were nested inside it.
    fn channel(
        &mut self,
        at: &str,
        address: &str,
        key: &str,
        item: v2_6::ChannelItem,
    ) -> (RefOr<v3_0::Channel>, Vec<(String, RefOr<v3_0::Operation>)>) {
        if let Some(reference) = &item.reference {
            // 2.6 leaves the meaning of a `$ref` sibling undefined and
            // deprecates the field; 3.0 has no room for one at all.
            if item.publish.is_some()
                || item.subscribe.is_some()
                || item.description.is_some()
                || !item.parameters.is_empty()
            {
                self.note(at, NoteKind::ChannelReferenceSiblingsDropped);
            }
            let reference = self.rewrite(at, reference);
            return (RefOr::Reference(Reference { reference }), Vec::new());
        }

        if item.deprecated == Some(true) {
            self.note(at, NoteKind::ChannelDeprecationDropped);
        }

        let mut messages = BTreeMap::new();
        let mut operations = Vec::new();
        for (action, operation) in [
            (v2_6::OperationKind::Publish, item.publish),
            (v2_6::OperationKind::Subscribe, item.subscribe),
        ] {
            let Some(operation) = operation else { continue };
            let at = format!("{at}.{}", action.as_str());
            let (name, converted) = self.operation(&at, key, action, operation, &mut messages);
            operations.push((name, RefOr::Item(converted)));
        }

        let channel = v3_0::Channel {
            address: Some(Some(address.to_owned())),
            messages,
            parameters: item
                .parameters
                .into_iter()
                .map(|(name, parameter)| {
                    let at = format!("{at}.parameters.{name}");
                    (name, self.ref_or(&at, parameter, Self::parameter))
                })
                .collect(),
            title: None,
            summary: None,
            description: item.description,
            servers: item
                .servers
                .into_iter()
                .map(|name| Reference {
                    reference: format!("#/servers/{name}"),
                })
                .collect(),
            tags: Vec::new(),
            external_docs: None,
            bindings: item.bindings,
            extensions: item.extensions,
        };
        (RefOr::Item(channel), operations)
    }

    /// A 2.6 parameter describes its value with a schema; a 3.0 one is
    /// a string, with an enumeration and a default at most.
    fn parameter(&mut self, at: &str, parameter: v2_6::Parameter) -> v3_0::Parameter {
        let mut converted = v3_0::Parameter {
            description: parameter.description,
            location: parameter.location,
            ..v3_0::Parameter::default()
        };
        let Some(schema) = parameter.schema else {
            return converted;
        };
        // Only what a v3 parameter can still say survives.
        let carried = match serde_json::to_value(&schema) {
            Ok(serde_json::Value::Object(map)) => {
                converted.enum_values = strings(map.get("enum"));
                converted.examples = strings(map.get("examples"));
                converted.default = map
                    .get("default")
                    .and_then(|d| d.as_str())
                    .map(str::to_owned);
                map.len()
                    == usize::from(map.contains_key("enum"))
                        + usize::from(map.contains_key("examples"))
                        + usize::from(map.contains_key("default"))
            }
            _ => false,
        };
        if !carried {
            self.note(at, NoteKind::ParameterSchemaDropped);
        }
        converted
    }

    /// Convert one of a channel's two operations, adding its messages
    /// to the channel on the way.
    fn operation(
        &mut self,
        at: &str,
        channel_key: &str,
        action: v2_6::OperationKind,
        operation: v2_6::Operation,
        messages: &mut BTreeMap<String, RefOr<v3_0::Message>>,
    ) -> (String, v3_0::Operation) {
        // The point of view flipped: 2.6 describes what a *client* does
        // with the channel, 3.0 what the application does.
        let converted_action = match action {
            v2_6::OperationKind::Publish => v3_0::OperationAction::Receive,
            v2_6::OperationKind::Subscribe => v3_0::OperationAction::Send,
        };
        self.note(
            at,
            NoteKind::ActionFlipped {
                from: action.as_str(),
                to: match converted_action {
                    v3_0::OperationAction::Receive => "receive",
                    v3_0::OperationAction::Send => "send",
                },
            },
        );

        let key = match operation.operation_id.as_deref().map(sanitize) {
            Some(id) if !id.is_empty() => unique(id, &mut self.taken_operations),
            _ => {
                let key = unique(
                    format!("{channel_key}_{}", action.as_str()),
                    &mut self.taken_operations,
                );
                self.note(at, NoteKind::OperationKeyDerived { key: key.clone() });
                key
            }
        };

        let references = self.messages(at, channel_key, operation.message, messages);
        let converted = v3_0::Operation {
            action: converted_action,
            channel: Reference {
                reference: format!("#/channels/{channel_key}"),
            },
            messages: references,
            reply: None,
            traits: operation
                .traits
                .into_iter()
                .map(|operation_trait| {
                    self.ref_or(at, operation_trait, |this, at, item| {
                        this.reinterpret(at, &item).unwrap_or_default()
                    })
                })
                .collect(),
            title: None,
            summary: operation.summary,
            description: operation.description,
            security: self.security(at, operation.security),
            tags: operation
                .tags
                .into_iter()
                .map(|tag| RefOr::Item(self.tag(tag)))
                .collect(),
            external_docs: operation
                .external_docs
                .map(|docs| RefOr::Item(external_documentation(docs))),
            bindings: operation.bindings,
            extensions: operation.extensions,
        };
        (key, converted)
    }

    /// Move an operation's messages onto its channel, and name them
    /// there — which is how 3.0 says an operation carries a message.
    fn messages(
        &mut self,
        at: &str,
        channel_key: &str,
        message: Option<v2_6::OperationMessage>,
        channel_messages: &mut BTreeMap<String, RefOr<v3_0::Message>>,
    ) -> Vec<Reference> {
        let mut references = Vec::new();
        for message in flatten(message) {
            let at = format!("{at}.message");
            let (key, converted) = match message {
                RefOr::Reference(reference) => {
                    let key = reference
                        .component_key("messages")
                        .map(|key| sanitize(&key))
                        .filter(|key| !key.is_empty());
                    let reference = self.rewrite(&at, &reference.reference);
                    (key, RefOr::Reference(Reference { reference }))
                }
                RefOr::Item(message) => {
                    let key = message
                        .message_id
                        .as_deref()
                        .or(message.name.as_deref())
                        .map(sanitize)
                        .filter(|key| !key.is_empty());
                    (key, RefOr::Item(self.message(&at, message)))
                }
            };
            let mut taken: BTreeSet<String> = channel_messages.keys().cloned().collect();
            let key = match key {
                Some(key) => unique(key, &mut taken),
                None => {
                    let key = unique("message".to_owned(), &mut taken);
                    self.note(&at, NoteKind::MessageKeyDerived { key: key.clone() });
                    key
                }
            };
            references.push(Reference {
                reference: format!("#/channels/{channel_key}/messages/{key}"),
            });
            channel_messages.insert(key, converted);
        }
        references
    }

    fn message(&mut self, at: &str, message: v2_6::Message) -> v3_0::Message {
        v3_0::Message {
            headers: message
                .headers
                .and_then(|headers| self.schema(at, "headers", &headers, None)),
            payload: message.payload.and_then(|payload| {
                self.schema(at, "payload", &payload, message.schema_format.as_deref())
            }),
            correlation_id: message.correlation_id.map(|correlation_id| {
                self.ref_or(at, correlation_id, |this, at, item| {
                    this.reinterpret(at, &item).unwrap_or_default()
                })
            }),
            content_type: message.content_type,
            name: message.name,
            title: message.title,
            summary: message.summary,
            description: message.description,
            deprecated: message.deprecated,
            tags: message
                .tags
                .into_iter()
                .map(|tag| RefOr::Item(self.tag(tag)))
                .collect(),
            external_docs: message
                .external_docs
                .map(|docs| RefOr::Item(external_documentation(docs))),
            bindings: message.bindings,
            examples: message
                .examples
                .into_iter()
                .filter_map(|example| self.reinterpret(at, &example))
                .collect(),
            traits: message
                .traits
                .into_iter()
                .map(|message_trait| {
                    self.ref_or(at, message_trait, |this, at, item| {
                        this.reinterpret(at, &item).unwrap_or_default()
                    })
                })
                .collect(),
            extensions: message.extensions,
        }
    }

    /// Carry a schema across, wrapping it in a Multi Format Schema
    /// Object where 2.6 named a dialect of its own.
    fn schema<T: Serialize>(
        &mut self,
        at: &str,
        what: &'static str,
        schema: &T,
        schema_format: Option<&str>,
    ) -> Option<RefOr<v3_0::SchemaOrMultiFormat>> {
        let value = serde_json::to_value(schema).ok()?;
        if crate::v2_6::message::payload_is_asyncapi_schema(schema_format) {
            return match serde_json::from_value(value) {
                Ok(schema) => Some(schema),
                Err(_) => {
                    self.note(at, NoteKind::NotConverted { what });
                    None
                }
            };
        }
        // Another dialect keeps its shape, and says which it is the way
        // 3.0 says it.
        Some(RefOr::Item(v3_0::SchemaOrMultiFormat::MultiFormat(
            v3_0::MultiFormatSchema {
                schema_format: schema_format.map(str::to_owned),
                schema: value,
                extensions: None,
            },
        )))
    }

    fn components(&mut self, components: v2_6::Components) -> v3_0::Components {
        let at = "#.components";
        v3_0::Components {
            schemas: components
                .schemas
                .into_iter()
                .filter_map(|(name, schema)| {
                    let at = format!("{at}.schemas.{name}");
                    Some((name, self.schema(&at, "schema", &schema, None)?))
                })
                .collect(),
            servers: components
                .servers
                .into_iter()
                .map(|(name, server)| {
                    let at = format!("{at}.servers.{name}");
                    (name, self.ref_or(&at, server, Self::server))
                })
                .collect(),
            channels: components
                .channels
                .into_iter()
                .map(|(name, item)| {
                    let at = format!("{at}.channels.{name}");
                    // A reusable channel has no address of its own and
                    // no operations to give up: `name` is both.
                    let (channel, _) = self.channel(&at, &name, &name, item);
                    (name, channel)
                })
                .collect(),
            operations: BTreeMap::new(),
            messages: components
                .messages
                .into_iter()
                .map(|(name, message)| {
                    let at = format!("{at}.messages.{name}");
                    (name, self.ref_or(&at, message, Self::message))
                })
                .collect(),
            security_schemes: components
                .security_schemes
                .into_iter()
                .map(|(name, scheme)| {
                    let at = format!("{at}.securitySchemes.{name}");
                    (
                        name,
                        self.ref_or(&at, scheme, |this, at, item| {
                            this.reinterpret(at, &item).unwrap_or_default()
                        }),
                    )
                })
                .collect(),
            server_variables: components
                .server_variables
                .into_iter()
                .map(|(name, variable)| {
                    let at = format!("{at}.serverVariables.{name}");
                    (name, self.ref_or(&at, variable, Self::server_variable))
                })
                .collect(),
            parameters: components
                .parameters
                .into_iter()
                .map(|(name, parameter)| {
                    let at = format!("{at}.parameters.{name}");
                    (name, self.ref_or(&at, parameter, Self::parameter))
                })
                .collect(),
            correlation_ids: components
                .correlation_ids
                .into_iter()
                .map(|(name, correlation_id)| {
                    let at = format!("{at}.correlationIds.{name}");
                    (
                        name,
                        self.ref_or(&at, correlation_id, |this, at, item| {
                            this.reinterpret(at, &item).unwrap_or_default()
                        }),
                    )
                })
                .collect(),
            replies: BTreeMap::new(),
            reply_addresses: BTreeMap::new(),
            external_docs: BTreeMap::new(),
            tags: BTreeMap::new(),
            operation_traits: components
                .operation_traits
                .into_iter()
                .map(|(name, operation_trait)| {
                    let at = format!("{at}.operationTraits.{name}");
                    (
                        name,
                        self.ref_or(&at, operation_trait, |this, at, item| {
                            this.reinterpret(at, &item).unwrap_or_default()
                        }),
                    )
                })
                .collect(),
            message_traits: components
                .message_traits
                .into_iter()
                .map(|(name, message_trait)| {
                    let at = format!("{at}.messageTraits.{name}");
                    (
                        name,
                        self.ref_or(&at, message_trait, |this, at, item| {
                            this.reinterpret(at, &item).unwrap_or_default()
                        }),
                    )
                })
                .collect(),
            server_bindings: components.server_bindings,
            channel_bindings: components.channel_bindings,
            operation_bindings: components.operation_bindings,
            message_bindings: components.message_bindings,
            extensions: components.extensions,
        }
    }

    /// Convert the object inside a `RefOr`, rewriting a reference that
    /// no longer names what it did.
    fn ref_or<A, B>(
        &mut self,
        at: &str,
        value: RefOr<A>,
        convert: impl FnOnce(&mut Self, &str, A) -> B,
    ) -> RefOr<B> {
        match value {
            RefOr::Reference(reference) => {
                let reference = self.rewrite(at, &reference.reference);
                RefOr::Reference(Reference { reference })
            }
            RefOr::Item(item) => RefOr::Item(convert(self, at, item)),
        }
    }

    /// Rewrite a pointer that v3 moved.
    ///
    /// A channel is the one thing that moved *and* kept a name this
    /// conversion knows: `#/channels/<address>` becomes
    /// `#/channels/<key>`. A pointer into a channel's operations has no
    /// counterpart at all, and is reported rather than guessed at.
    fn rewrite(&mut self, at: &str, reference: &str) -> String {
        let Some(rest) = reference.strip_prefix("#/channels/") else {
            return reference.to_owned();
        };
        let (address, tail) = match rest.split_once('/') {
            Some((address, tail)) => (address, Some(tail)),
            None => (rest, None),
        };
        let address = unescape(address);
        let Some(key) = self.channel_keys.get(&address) else {
            self.note(
                at,
                NoteKind::ReferenceNotRewritten {
                    reference: reference.to_owned(),
                },
            );
            return reference.to_owned();
        };
        match tail {
            // Operations left the channel, so nothing below it survives.
            Some(_) => {
                self.note(
                    at,
                    NoteKind::ReferenceNotRewritten {
                        reference: reference.to_owned(),
                    },
                );
                reference.to_owned()
            }
            None => format!("#/channels/{key}"),
        }
    }

    /// Re-read a value as its v3.0 counterpart, the two versions
    /// modelling it the same way.
    fn reinterpret<A: Serialize, B: DeserializeOwned>(&mut self, at: &str, value: &A) -> Option<B> {
        let converted = serde_json::to_value(value)
            .ok()
            .and_then(|value| serde_json::from_value(value).ok());
        if converted.is_none() {
            self.note(
                at,
                NoteKind::NotConverted {
                    what: std::any::type_name::<A>()
                        .rsplit("::")
                        .next()
                        .unwrap_or("value"),
                },
            );
        }
        converted
    }
}

fn external_documentation(docs: v2_6::ExternalDocumentation) -> v3_0::ExternalDocumentation {
    v3_0::ExternalDocumentation {
        url: docs.url,
        description: docs.description,
        extensions: docs.extensions,
    }
}

/// A v2.6 server names one URL; a v3 server names a host and a path.
fn split_url(url: &str) -> (String, Option<String>) {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest);
    match authority.split_once('/') {
        Some((host, path)) => (host.to_owned(), Some(format!("/{path}"))),
        None => (authority.to_owned(), None),
    }
}

/// Every message an operation carries, however 2.6 spelled them.
fn flatten(message: Option<v2_6::OperationMessage>) -> Vec<RefOr<v2_6::Message>> {
    match message {
        None => Vec::new(),
        Some(v2_6::OperationMessage::Single(message)) => vec![*message],
        Some(v2_6::OperationMessage::OneOf(one_of)) => one_of
            .one_of
            .into_iter()
            .flat_map(|message| flatten(Some(message)))
            .collect(),
    }
}

/// A v3 key is `^[A-Za-z0-9\.\-_]+$`, which a v2.6 channel address —
/// being a path, a topic, or a routing key — routinely is not.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Make `key` one nothing else has taken, and take it.
fn unique(key: String, taken: &mut BTreeSet<String>) -> String {
    let key = if key.is_empty() {
        "unnamed".to_owned()
    } else {
        key
    };
    if taken.insert(key.clone()) {
        return key;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{key}_{suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// The RFC 6901 escapes a channel address carries inside a pointer.
fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// The strings in a JSON array, for the parameter fields v3 keeps.
fn strings(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn convert_json(value: serde_json::Value) -> (v3_0::Document, Vec<String>) {
        let source: v2_6::Document = serde_json::from_value(value).expect("a v2.6 document");
        let (document, report) = convert(source);
        (
            document,
            report.notes.iter().map(ToString::to_string).collect(),
        )
    }

    fn minimal(channels: serde_json::Value) -> serde_json::Value {
        json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": channels
        })
    }

    #[test]
    fn addresses_that_sanitize_alike_still_get_their_own_keys() {
        let (document, notes) = convert_json(minimal(json!({
            "a/b": { "publish": {} },
            "a-b": { "publish": {} },
            "a_b": { "publish": {} }
        })));
        // `a-b` and `a_b` are already keys and keep them, so `a/b` is
        // the one renamed, and renamed around them.
        let mut keys: Vec<&String> = document.channels.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["a-b", "a_b", "a_b_2"]);
        assert_eq!(notes.iter().filter(|n| n.contains("usable key")).count(), 1);
        assert!(
            notes
                .iter()
                .any(|note| note.starts_with("#.channels.a/b:") && note.contains("`a_b_2`")),
            "the address that was not already a key is the one renamed: {notes:?}"
        );
    }

    #[test]
    fn an_operation_without_an_id_is_named_after_its_channel() {
        let (document, notes) = convert_json(minimal(json!({
            "orders": { "publish": {}, "subscribe": {} }
        })));
        let mut keys: Vec<&String> = document.operations.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["orders_publish", "orders_subscribe"]);
        assert_eq!(
            notes
                .iter()
                .filter(|note| note.contains("no `operationId`"))
                .count(),
            2,
        );
    }

    #[test]
    fn a_oneof_becomes_a_channels_worth_of_messages() {
        let (document, notes) = convert_json(minimal(json!({
            "orders": {
                "publish": {
                    "message": {
                        "oneOf": [
                            { "name": "Placed" },
                            { "messageId": "cancelled" },
                            { "title": "no name at all" }
                        ]
                    }
                }
            }
        })));
        let channel = document.channels["orders"].item().expect("inline");
        let mut keys: Vec<&String> = channel.messages.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["Placed", "cancelled", "message"]);
        assert!(
            notes
                .iter()
                .any(|note| note.contains("no `messageId` or `name`")),
        );

        let operation = document.operations["orders_publish"]
            .item()
            .expect("inline");
        assert_eq!(operation.messages.len(), 3);
        assert!(operation.messages.iter().all(|reference| {
            reference
                .reference
                .starts_with("#/channels/orders/messages/")
        }),);
    }

    #[test]
    fn another_dialect_keeps_its_shape_and_says_which_it_is() {
        let (document, _) = convert_json(minimal(json!({
            "orders": {
                "publish": {
                    "message": {
                        "name": "Placed",
                        "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
                        "payload": { "type": "record", "name": "Placed" }
                    }
                }
            }
        })));
        let channel = document.channels["orders"].item().expect("inline");
        let message = channel.messages["Placed"].item().expect("inline");
        assert!(
            matches!(
                &message.payload,
                Some(RefOr::Item(v3_0::SchemaOrMultiFormat::MultiFormat(payload)))
                    if payload.schema_format.as_deref()
                        == Some("application/vnd.apache.avro;version=1.9.0")
                        && payload.schema == json!({ "type": "record", "name": "Placed" })
            ),
            "got {:?}",
            message.payload,
        );
    }

    #[test]
    fn a_pointer_at_a_channel_is_rewritten_to_its_new_key() {
        let (document, notes) = convert_json(minimal(json!({
            "a/b": { "publish": {} },
            "alias": { "$ref": "#/channels/a~1b" }
        })));
        assert!(matches!(
            &document.channels["alias"],
            RefOr::Reference(reference) if reference.reference == "#/channels/a_b"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));

        // One that points *inside* a channel has nowhere to go: v3 took
        // the operations out.
        let (_, notes) = convert_json(minimal(json!({
            "a/b": { "publish": { "message": { "name": "M" } } },
            "alias": { "$ref": "#/channels/a~1b/publish/message" }
        })));
        assert!(
            notes.iter().any(|note| note
                .contains("`#/channels/a~1b/publish/message` no longer names the same thing")),
            "got: {notes:?}"
        );
    }

    #[test]
    fn what_v3_has_no_room_for_is_said_rather_than_dropped_quietly() {
        let (_, notes) = convert_json(minimal(json!({
            "orders": {
                "deprecated": true,
                "publish": {
                    "security": [ { "oauth": ["read:orders"] } ]
                }
            },
            "aliased": { "$ref": "#/components/channels/shared", "description": "beside it" }
        })));
        assert!(notes.iter().any(|note| note.contains("no `deprecated`")));
        assert!(
            notes
                .iter()
                .any(|note| note.contains("scopes [\"read:orders\"] required of `oauth`")),
            "got: {notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("what sat beside it is dropped")),
        );
    }

    #[test]
    fn a_url_with_a_path_becomes_a_host_and_a_pathname() {
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": {
                "prod": { "url": "amqp://broker.example.com:5672/vhost", "protocol": "amqp" },
                "plain": { "url": "broker.example.com", "protocol": "kafka" }
            },
            "channels": {}
        }));
        let prod = document.servers["prod"].item().expect("inline");
        assert_eq!(prod.host, "broker.example.com:5672");
        assert_eq!(prod.pathname.as_deref(), Some("/vhost"));

        let plain = document.servers["plain"].item().expect("inline");
        assert_eq!(plain.host, "broker.example.com");
        assert_eq!(plain.pathname, None);
        // Nothing was split, so nothing is said.
        assert_eq!(
            notes.iter().filter(|note| note.contains("plain")).count(),
            0
        );
    }

    #[test]
    fn a_parameter_keeps_what_a_v3_parameter_can_say() {
        let (document, notes) = convert_json(minimal(json!({
            "orders/{id}": {
                "parameters": {
                    "id": {
                        "description": "the order",
                        "location": "$message.payload#/id",
                        "schema": { "enum": ["a", "b"], "default": "a" }
                    },
                    "other": { "schema": { "type": "string", "pattern": "^x" } }
                }
            }
        })));
        let channel = document.channels["orders__id_"].item().expect("inline");
        let id = channel.parameters["id"].item().expect("inline");
        assert_eq!(id.enum_values, vec!["a", "b"]);
        assert_eq!(id.default.as_deref(), Some("a"));
        assert_eq!(id.description.as_deref(), Some("the order"));
        assert_eq!(id.location.as_deref(), Some("$message.payload#/id"));

        // The enumeration crossed whole, so only the other one lost
        // anything.
        assert_eq!(
            notes
                .iter()
                .filter(|note| note.contains("a v3 parameter is a string"))
                .count(),
            1,
        );
    }

    #[test]
    fn every_reusable_object_crosses_over() {
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "components": {
                "schemas": { "user": { "type": "object" }, "any": true },
                "servers": {
                    "prod": { "url": "kafka://b:9092", "protocol": "kafka" },
                    "alias": { "$ref": "#/components/servers/prod" }
                },
                "serverVariables": { "stage": { "default": "prod", "enum": ["prod"] } },
                "channels": { "shared": { "description": "reusable" } },
                "messages": {
                    "placed": { "name": "Placed", "correlationId": { "location": "$message.header#/id" } },
                    "alias": { "$ref": "#/components/messages/placed" }
                },
                "securitySchemes": { "token": { "type": "httpApiKey", "name": "k", "in": "header" } },
                "parameters": { "id": { "schema": { "type": "string" } } },
                "correlationIds": { "trace": { "location": "$message.header#/trace" } },
                "operationTraits": { "kafka": { "bindings": { "kafka": {} } } },
                "messageTraits": { "common": { "contentType": "application/json" } },
                "messageBindings": { "kafka": { "kafka": { "key": {} } } },
                "serverBindings": { "kafka": { "kafka": {} } },
                "channelBindings": { "kafka": { "kafka": {} } },
                "operationBindings": { "kafka": { "kafka": {} } }
            }
        }));
        let components = document.components.expect("components");

        assert_eq!(components.schemas.len(), 2);
        assert!(components.servers.contains_key("prod"));
        assert!(matches!(
            &components.servers["alias"],
            RefOr::Reference(reference) if reference.reference == "#/components/servers/prod"
        ));
        assert_eq!(components.server_variables.len(), 1);
        // A reusable channel has no address of its own.
        let shared = components.channels["shared"].item().expect("inline");
        assert_eq!(shared.description.as_deref(), Some("reusable"));
        assert_eq!(components.messages.len(), 2);
        assert_eq!(components.security_schemes.len(), 1);
        assert_eq!(components.parameters.len(), 1);
        assert_eq!(components.correlation_ids.len(), 1);
        assert_eq!(components.operation_traits.len(), 1);
        assert_eq!(components.message_traits.len(), 1);
        assert_eq!(components.message_bindings.len(), 1);
        assert_eq!(components.server_bindings.len(), 1);
        assert_eq!(components.channel_bindings.len(), 1);
        assert_eq!(components.operation_bindings.len(), 1);
        // v3 grew these; 2.6 had nowhere to put them.
        assert!(components.operations.is_empty());
        assert!(components.replies.is_empty());

        // Only the component parameter's schema is lost.
        assert_eq!(
            notes
                .iter()
                .filter(|note| note.contains("a v3 parameter is a string"))
                .count(),
            1,
        );

        // And the whole thing is still a valid v3.0 document.
        document_is_valid(
            &convert_json(json!({
                "asyncapi": "2.6.0",
                "info": { "title": "T", "version": "1" },
                "channels": {},
                "components": { "messages": { "placed": { "name": "Placed" } } }
            }))
            .0,
        );
    }

    fn document_is_valid(document: &v3_0::Document) {
        use crate::validation::Validate;
        document
            .validate(enumset::EnumSet::empty())
            .expect("a valid v3.0 document");
    }

    #[test]
    fn a_note_reads_as_a_sentence() {
        let notes = [
            NoteKind::ChannelKeyDerived {
                address: "a/b".to_owned(),
                key: "a_b".to_owned(),
            },
            NoteKind::OperationKeyDerived {
                key: "k".to_owned(),
            },
            NoteKind::MessageKeyDerived {
                key: "m".to_owned(),
            },
            NoteKind::ActionFlipped {
                from: "publish",
                to: "receive",
            },
            NoteKind::ServerUrlSplit {
                url: "b:9092".to_owned(),
                host: "b:9092".to_owned(),
                pathname: None,
            },
            NoteKind::ServerUrlSplit {
                url: "amqp://b/v".to_owned(),
                host: "b".to_owned(),
                pathname: Some("/v".to_owned()),
            },
            NoteKind::SecurityScopesDropped {
                scheme: "oauth".to_owned(),
                scopes: vec!["read".to_owned()],
            },
            NoteKind::ParameterSchemaDropped,
            NoteKind::ChannelDeprecationDropped,
            NoteKind::ChannelReferenceSiblingsDropped,
            NoteKind::ReferenceNotRewritten {
                reference: "#/channels/a/publish".to_owned(),
            },
            NoteKind::NotConverted { what: "contact" },
        ];
        for kind in notes {
            let note = Note {
                at: "#.channels.a".to_owned(),
                kind,
            };
            let rendered = note.to_string();
            assert!(
                rendered.starts_with("#.channels.a: ") && rendered.len() > 20,
                "{rendered}"
            );
        }
    }

    #[test]
    fn an_unnamable_channel_is_still_named() {
        let (document, notes) = convert_json(minimal(json!({ "/": { "publish": {} } })));
        assert_eq!(document.channels.keys().collect::<Vec<_>>(), vec!["_"]);
        assert!(notes.iter().any(|note| note.contains("usable key")));

        // An address of nothing at all still needs a key.
        let (document, _) = convert_json(minimal(json!({ "": { "publish": {} } })));
        assert_eq!(
            document.channels.keys().collect::<Vec<_>>(),
            vec!["unnamed"]
        );
    }

    #[test]
    fn what_cannot_be_carried_is_said_and_the_rest_goes_on() {
        let (document, notes) = convert_json(minimal(json!({
            "orders": {
                "parameters": {
                    "plain": { "description": "no schema at all" },
                    "boolean": { "schema": true }
                },
                "publish": {
                    "traits": [ { "operationId": "inlineTrait", "summary": "s" } ],
                    "message": {
                        "name": "Placed",
                        "traits": [ { "contentType": "application/json" } ],
                        "correlationId": { "location": "$message.header#/id" },
                        "payload": "not a schema at all"
                    }
                }
            },
            "aliased": { "$ref": "#/channels/nowhere" }
        })));

        // A parameter with nothing to lose says nothing; a boolean
        // schema has nothing a v3 parameter can keep.
        let channel = document.channels["orders"].item().expect("inline");
        assert!(channel.parameters["plain"].item().is_some());
        assert_eq!(
            notes
                .iter()
                .filter(|note| note.contains("a v3 parameter is a string"))
                .count(),
            1,
        );

        // Inline traits and a correlation id cross over.
        let operation = document.operations["orders_publish"]
            .item()
            .expect("inline");
        assert_eq!(operation.traits.len(), 1);
        let message = channel.messages["Placed"].item().expect("inline");
        assert_eq!(message.traits.len(), 1);
        assert!(message.correlation_id.is_some());

        // A payload that is not a schema in the document's own dialect
        // is left behind, and said so.
        assert!(message.payload.is_none());
        assert!(
            notes
                .iter()
                .any(|note| note.contains("`payload` could not be converted")),
            "got: {notes:?}"
        );

        // A pointer at a channel that is not there cannot be rewritten.
        assert!(
            notes
                .iter()
                .any(|note| note.contains("`#/channels/nowhere` no longer names the same thing")),
            "got: {notes:?}"
        );
    }

    #[test]
    fn a_key_is_found_however_many_addresses_want_it() {
        let (document, _) = convert_json(minimal(json!({
            "a/b": { "publish": {} },
            "a|b": { "publish": {} },
            "a b": { "publish": {} }
        })));
        let mut keys: Vec<&String> = document.channels.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["a_b", "a_b_2", "a_b_3"]);
    }

    #[test]
    fn a_clean_conversion_says_nothing() {
        let (_, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "orders": {
                    "publish": {
                        "operationId": "receiveOrders",
                        "message": { "name": "Placed" }
                    }
                }
            }
        }));
        // Only the point of view, which every 2.6 operation changes.
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("`publish` is what the application does not do"));
    }
}
