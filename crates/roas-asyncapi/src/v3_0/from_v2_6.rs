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

use crate::common::pointer;
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
    /// An operation is keyed by name in 3.0. `from` is the
    /// `operationId` it offered, where that was not a usable key or was
    /// already taken; `None` where it offered nothing at all.
    OperationKeyDerived { from: Option<String>, key: String },
    /// A message needs a name inside its channel, `from` being the
    /// `messageId` or `name` it offered, if any.
    MessageKeyDerived { from: Option<String>, key: String },
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
    /// A v2.6 requirement naming several schemes needs all of them; a
    /// v3 list needs one of what it names. There is no v3 way to say
    /// "and", so the requirement is weaker than it was.
    SecurityRequirementFlattened { schemes: Vec<String> },
    /// A 2.6 parameter carries a schema; a 3.0 parameter is a string
    /// with an optional enumeration, so anything else it said is lost.
    ParameterSchemaDropped,
    /// …unless the document names it, in which case it is kept where
    /// v3 keeps schemas, and pointers at it follow.
    ParameterSchemaMoved { to: String },
    /// 3.0 has no `deprecated` on a channel.
    ChannelDeprecationDropped,
    /// 2.6's `$ref` on a channel item may carry siblings, whose
    /// behaviour that specification leaves undefined. 3.0's Reference
    /// Object may not, so they are dropped.
    ChannelReferenceSiblingsDropped,
    /// A channel named another with `$ref`, and only the naming channel
    /// knows the address — so the named one was copied in rather than
    /// referenced.
    ChannelReferenceInlined { reference: String },
    /// A channel named another with `$ref` that could not be copied in,
    /// so the address the naming channel carried is gone.
    ChannelAddressDropped { address: String },
    /// A pointer into the source document that no longer names the same
    /// thing, and that this conversion could not rewrite.
    ReferenceNotRewritten { reference: String },
    /// A value did not survive being re-read as its 3.0 counterpart.
    NotConverted { what: &'static str },
    /// v3 has no home for these, so re-reading the object left them
    /// behind — a message trait's `messageId`, say, v3 keying a message
    /// by where it sits instead.
    FieldsDropped { fields: Vec<String> },
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
            NoteKind::OperationKeyDerived { from, key } => match from {
                Some(from) => write!(f, "`{from}` is not a usable key; keyed as `{key}`"),
                None => write!(f, "no `operationId`; keyed as `{key}`"),
            },
            NoteKind::MessageKeyDerived { from, key } => match from {
                Some(from) => write!(f, "`{from}` is not a usable key; keyed as `{key}`"),
                None => write!(f, "no `messageId` or `name`; keyed as `{key}`"),
            },
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
            NoteKind::SecurityRequirementFlattened { schemes } => write!(
                f,
                "{schemes:?} were required together; v3 satisfies one of a list",
            ),
            NoteKind::ParameterSchemaDropped => {
                f.write_str("a v3 parameter is a string, so `schema` is dropped")
            }
            NoteKind::ParameterSchemaMoved { to } => {
                write!(
                    f,
                    "a v3 parameter is a string, so `schema` was kept at `{to}`"
                )
            }
            NoteKind::ChannelDeprecationDropped => f.write_str("a v3 channel has no `deprecated`"),
            NoteKind::ChannelReferenceSiblingsDropped => {
                f.write_str("a Reference Object is `$ref` alone, so what sat beside it is dropped")
            }
            NoteKind::ChannelReferenceInlined { reference } => {
                write!(
                    f,
                    "`{reference}` was copied in, this channel's address being its own"
                )
            }
            NoteKind::ChannelAddressDropped { address } => {
                write!(
                    f,
                    "address `{address}` is lost: a Reference Object cannot carry one"
                )
            }
            NoteKind::ReferenceNotRewritten { reference } => {
                write!(f, "`{reference}` no longer names the same thing in v3")
            }
            NoteKind::NotConverted { what } => write!(f, "`{what}` could not be converted"),
            NoteKind::FieldsDropped { fields } => {
                write!(f, "v3 has nowhere to keep {fields:?}")
            }
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
    let mut converted = conversion.document(document);
    conversion.keep_schemas(&mut converted);
    (
        converted,
        ConversionReport {
            notes: conversion.notes,
        },
    )
}

/// Where a channel is going, which is what everything inside it will
/// name.
struct Place<'a> {
    /// The v2.6 pointer that names it, which is how anything keyed
    /// before the conversion started is found again.
    source: String,
    /// The address it carries, which a reusable channel has none of:
    /// in 2.6 the address is the key of the channel that names it.
    address: Option<&'a str>,
    /// The pointer its operations and messages are reached by.
    reference: String,
    /// What to build an operation's name from, where it offers none.
    key: &'a str,
}

/// The conversion in progress: what it has decided, and what it has to
/// say about it.
struct Conversion {
    notes: Vec<Note>,
    /// v2.6 channel address → the v3.0 key it was given.
    channel_keys: BTreeMap<String, String>,
    /// The reusable channels, kept to copy in what a `$ref` names: only
    /// the channel doing the naming knows the address.
    reusable: BTreeMap<String, v2_6::ChannelItem>,
    /// (the v2.6 pointer at the channel, `publish` or `subscribe`) →
    /// the v3.0 pointer the operation went to. Keyed up front for the
    /// same reason channels are: a pointer may name one.
    operation_keys: BTreeMap<(String, &'static str), String>,
    /// The v2.6 pointer at a message → the name v3 files it under. A
    /// message moved from its operation to its channel, so a pointer at
    /// one has somewhere to go only if the name is settled first.
    message_keys: BTreeMap<String, String>,
    /// The v2.6 pointer at a parameter's schema → the name a reusable
    /// schema would keep it under. A v3 parameter is a string and holds
    /// no schema, so one a pointer names has to be kept where v3 keeps
    /// schemas — and the name has to exist before the pointer at it is
    /// rewritten, so every parameter that has a schema gets one.
    parameter_schemas: BTreeMap<String, String>,
    /// Those schemas as 2.6 wrote them, under that name. Which of them
    /// the document keeps is settled at the end, by what the conversion
    /// rewrote a pointer to: generating a component for every parameter
    /// would bulk out documents that never point at one.
    kept_schemas: BTreeMap<String, KeptSchema>,
    /// The names rewriting actually used. Only a modelled reference is
    /// ever rewritten, so this counts the pointers v3 will really
    /// follow — never a `$ref`-shaped value sitting in an extension or
    /// an example, which this conversion leaves exactly as it found it.
    used_schemas: BTreeSet<String>,
}

/// A parameter's schema, kept in case something names it.
struct KeptSchema {
    /// Where the parameter is, for the note about what became of it.
    at: String,
    schema: v2_6::SubSchema,
    /// Whether a v3 parameter could say everything the schema said, so
    /// that dropping it costs nothing.
    carried: bool,
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

        // Operations are named next, for the same reason: v3 moved them
        // out of their channel, and a pointer may name one — including
        // a pointer into a reusable channel, whose operations go to the
        // reusable map rather than the root one.
        let mut taken = BTreeSet::new();
        let mut reusable_taken = BTreeSet::new();
        let mut operation_keys = BTreeMap::new();
        let reusable = document
            .components
            .as_ref()
            .map(|components| components.channels.clone())
            .unwrap_or_default();
        let places = document
            .channels
            .iter()
            .map(|(address, item)| {
                (
                    format!("#.channels.{address}"),
                    format!("#/channels/{address}"),
                    channel_keys[address].clone(),
                    "#/operations",
                    item,
                    false,
                )
            })
            .chain(reusable.iter().map(|(name, item)| {
                (
                    format!("#.components.channels.{name}"),
                    format!("#/components/channels/{name}"),
                    name.clone(),
                    "#/components/operations",
                    item,
                    true,
                )
            }));
        for (at, source, channel_key, map, item, is_reusable) in places {
            // A channel that names another is converted as the two of
            // them together, so what it says itself is a destination
            // like anything else — unless the naming cannot be
            // followed, in which case nothing of it survives.
            let merged_item;
            let item = match &item.reference {
                None => item,
                Some(_) => match merged(item, &reusable) {
                    Some(item) => {
                        merged_item = item;
                        &merged_item
                    }
                    None => continue,
                },
            };
            for (action, operation) in [
                (v2_6::OperationKind::Publish, &item.publish),
                (v2_6::OperationKind::Subscribe, &item.subscribe),
            ] {
                let Some(operation) = operation else { continue };
                let offered = operation
                    .operation_id
                    .as_deref()
                    .map(sanitize)
                    .filter(|id| !id.is_empty());
                let taken = if is_reusable {
                    &mut reusable_taken
                } else {
                    &mut taken
                };
                let key = unique(
                    offered.unwrap_or_else(|| format!("{channel_key}_{}", action.as_str())),
                    taken,
                );
                if operation.operation_id.as_deref() != Some(key.as_str()) {
                    notes.push(Note {
                        at: format!("{at}.{}", action.as_str()),
                        kind: NoteKind::OperationKeyDerived {
                            from: operation.operation_id.clone(),
                            key: key.clone(),
                        },
                    });
                }
                operation_keys.insert((source.clone(), action.as_str()), format!("{map}/{key}"));
            }
        }

        // Messages last, and per channel: v3 files them under their
        // channel, so that is where their names have to be unique — and
        // a pointer at one has to know the name before anything moves.
        let mut message_keys = BTreeMap::new();
        for (source, item) in document
            .channels
            .iter()
            .map(|(address, item)| (format!("#/channels/{address}"), item))
            .chain(
                reusable
                    .iter()
                    .map(|(name, item)| (format!("#/components/channels/{name}"), item)),
            )
        {
            let merged_item;
            let item = match &item.reference {
                None => item,
                Some(_) => match merged(item, &reusable) {
                    Some(item) => {
                        merged_item = item;
                        &merged_item
                    }
                    None => continue,
                },
            };
            let mut taken = BTreeSet::new();
            for (action, operation) in [
                (v2_6::OperationKind::Publish, &item.publish),
                (v2_6::OperationKind::Subscribe, &item.subscribe),
            ] {
                let Some(operation) = operation else { continue };
                let at = format!("{source}/{}/message", action.as_str());
                for (within, message) in flatten(operation.message.clone()).entries {
                    let key = unique(
                        message_name(&message)
                            .as_deref()
                            .map(sanitize)
                            .filter(|key| !key.is_empty())
                            .unwrap_or_else(|| "message".to_owned()),
                        &mut taken,
                    );
                    message_keys.insert(format!("{at}{within}"), key);
                }
            }
        }
        // A v3 parameter keeps no schema, so every parameter that has
        // one is given a name a schema component could keep it under.
        // Whether the document ends up keeping it is settled at the
        // end, by whether anything named it.
        let mut taken: BTreeSet<String> = document
            .components
            .as_ref()
            .map(|components| components.schemas.keys().cloned().collect())
            .unwrap_or_default();
        let mut parameter_schemas = BTreeMap::new();
        for (source, channel_key, parameters) in document
            .channels
            .iter()
            .map(|(address, item)| {
                (
                    format!("#/channels/{address}"),
                    channel_keys[address].clone(),
                    item,
                )
            })
            .chain(
                reusable.iter().map(|(name, item)| {
                    (format!("#/components/channels/{name}"), name.clone(), item)
                }),
            )
            // A channel this conversion cannot follow stays a Reference
            // Object, parameters and all, so there is nothing of its to
            // keep anywhere.
            .filter(|(_, _, item)| item.reference.is_none() || merged(item, &reusable).is_some())
            .map(|(source, key, item)| (source, key, &item.parameters))
            // A reusable parameter has a schema a pointer can name just
            // as much as a channel's does.
            .chain(document.components.as_ref().map(|components| {
                (
                    "#/components".to_owned(),
                    "parameter".to_owned(),
                    &components.parameters,
                )
            }))
        {
            for (name, parameter) in parameters {
                // A Reference Object holds no schema, and neither does a
                // parameter that describes nothing: in 2.6 a pointer at
                // either named nothing to begin with.
                if parameter
                    .item()
                    .is_none_or(|parameter| parameter.schema.is_none())
                {
                    continue;
                }
                parameter_schemas.insert(
                    format!("{source}/parameters/{name}/schema"),
                    unique(sanitize(&format!("{channel_key}_{name}")), &mut taken),
                );
            }
        }

        Self {
            notes,
            channel_keys,
            reusable,
            operation_keys,
            message_keys,
            parameter_schemas,
            kept_schemas: BTreeMap::new(),
            used_schemas: BTreeSet::new(),
        }
    }

    /// Write in the parameter schemas that turned out to be named, and
    /// say what became of every parameter's schema either way.
    ///
    /// Converting a kept schema may itself rewrite a pointer at another
    /// parameter's schema, so this keeps going until nothing new is
    /// named.
    fn keep_schemas(&mut self, document: &mut v3_0::Document) {
        let mut kept = BTreeMap::new();
        let mut settled = BTreeSet::new();
        while let Some(key) = self
            .used_schemas
            .iter()
            .find(|key| !settled.contains(*key))
            .cloned()
        {
            settled.insert(key.clone());
            let Some(KeptSchema { at, schema, .. }) = self.kept_schemas.remove(&key) else {
                continue;
            };
            let Some(converted) = self.schema(&at, "schema", &schema, None) else {
                continue;
            };
            self.note(
                &at,
                NoteKind::ParameterSchemaMoved {
                    to: format!("#/components/schemas/{key}"),
                },
            );
            kept.insert(key, converted);
        }
        // Nothing names the rest, so they go the way they went before a
        // pointer gave one a reason to stay.
        for (_, KeptSchema { at, carried, .. }) in std::mem::take(&mut self.kept_schemas) {
            if !carried {
                self.note(&at, NoteKind::ParameterSchemaDropped);
            }
        }
        if !kept.is_empty() {
            document
                .components
                .get_or_insert_with(v3_0::Components::default)
                .schemas
                .extend(kept);
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
            let place = Place {
                source: format!("#/channels/{address}"),
                address: Some(&address),
                reference: format!("#/channels/{key}"),
                key: &key,
            };
            let (channel, channel_operations) = self.channel(&at, &place, item);
            channels.insert(key.clone(), channel);
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
            bindings: self.bindings(at, server.bindings),
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
            if requirement.0.len() > 1 {
                self.note(
                    at,
                    NoteKind::SecurityRequirementFlattened {
                        schemes: requirement.0.keys().cloned().collect(),
                    },
                );
            }
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

    /// v3 renamed an OAuth flow's `scopes` to `availableScopes`, and
    /// made it required — a scheme that crossed over as-is would leave
    /// an invalid document behind.
    fn security_scheme(&mut self, at: &str, scheme: v2_6::SecurityScheme) -> v3_0::SecurityScheme {
        let Ok(mut value) = serde_json::to_value(&scheme) else {
            self.note(
                at,
                NoteKind::NotConverted {
                    what: "securityScheme",
                },
            );
            return v3_0::SecurityScheme::default();
        };
        if let Some(flows) = value
            .get_mut("flows")
            .and_then(serde_json::Value::as_object_mut)
        {
            // Only the flows themselves: an `x-` member of `flows` is
            // the extension's business, whatever it spells its keys.
            for name in [
                "implicit",
                "password",
                "clientCredentials",
                "authorizationCode",
            ] {
                if let Some(flow) = flows
                    .get_mut(name)
                    .and_then(serde_json::Value::as_object_mut)
                    && let Some(scopes) = flow.remove("scopes")
                {
                    flow.insert("availableScopes".to_owned(), scopes);
                }
            }
        }
        match serde_json::from_value(value) {
            Ok(scheme) => scheme,
            Err(_) => {
                self.note(
                    at,
                    NoteKind::NotConverted {
                        what: "securityScheme",
                    },
                );
                v3_0::SecurityScheme::default()
            }
        }
    }

    /// Bindings need no conversion — both versions hold them as the
    /// protocol's own JSON — but a reference to one still has to be
    /// looked at, v3 having moved some of what they name.
    fn bindings<T>(&mut self, at: &str, bindings: Option<RefOr<T>>) -> Option<RefOr<T>> {
        bindings.map(|bindings| self.ref_or(at, bindings, |_, _, item| item))
    }

    /// An operation trait carries what an operation does, references
    /// and all — so it is converted the same way, rather than re-read
    /// as a whole and left holding pointers to where v3 is not.
    fn operation_trait(
        &mut self,
        at: &str,
        operation_trait: v2_6::OperationTrait,
    ) -> v3_0::OperationTrait {
        // v3 keys an operation by where it sits; a trait has no
        // `operationId` of its own to keep.
        if operation_trait.operation_id.is_some() {
            self.note(
                at,
                NoteKind::FieldsDropped {
                    fields: vec!["operationId".to_owned()],
                },
            );
        }
        v3_0::OperationTrait {
            title: None,
            summary: operation_trait.summary,
            description: operation_trait.description,
            security: self.security(at, operation_trait.security),
            tags: operation_trait
                .tags
                .into_iter()
                .map(|tag| RefOr::Item(self.tag(tag)))
                .collect(),
            external_docs: operation_trait
                .external_docs
                .map(|docs| RefOr::Item(external_documentation(docs))),
            bindings: self.bindings(at, operation_trait.bindings),
            extensions: operation_trait.extensions,
        }
    }

    /// The same for a message trait, which carries what a message does
    /// bar its payload.
    fn message_trait(&mut self, at: &str, message_trait: v2_6::MessageTrait) -> v3_0::MessageTrait {
        let dropped: Vec<String> = [
            message_trait.message_id.as_ref().map(|_| "messageId"),
            message_trait.schema_format.as_ref().map(|_| "schemaFormat"),
        ]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect();
        if !dropped.is_empty() {
            self.note(at, NoteKind::FieldsDropped { fields: dropped });
        }
        v3_0::MessageTrait {
            headers: message_trait
                .headers
                .and_then(|headers| self.schema(at, "headers", &headers, None)),
            correlation_id: message_trait.correlation_id.map(|correlation_id| {
                self.ref_or(at, correlation_id, |this, at, item| {
                    this.reinterpret(at, &item).unwrap_or_default()
                })
            }),
            content_type: message_trait.content_type,
            name: message_trait.name,
            title: message_trait.title,
            summary: message_trait.summary,
            description: message_trait.description,
            deprecated: message_trait.deprecated,
            tags: message_trait
                .tags
                .into_iter()
                .map(|tag| RefOr::Item(self.tag(tag)))
                .collect(),
            external_docs: message_trait
                .external_docs
                .map(|docs| RefOr::Item(external_documentation(docs))),
            bindings: self.bindings(at, message_trait.bindings),
            examples: message_trait
                .examples
                .into_iter()
                .filter_map(|example| self.reinterpret(at, &example))
                .collect(),
            extensions: message_trait.extensions,
        }
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
        place: &Place<'_>,
        item: v2_6::ChannelItem,
    ) -> (RefOr<v3_0::Channel>, Vec<(String, RefOr<v3_0::Operation>)>) {
        if let Some(reference) = &item.reference {
            // A v3 channel keeps its address, and a Reference Object
            // cannot carry one — so what the reference names is copied
            // in, where this conversion can reach it, together with
            // whatever the naming channel said itself.
            if let Some(named) = merged(&item, &self.reusable) {
                self.note(
                    at,
                    NoteKind::ChannelReferenceInlined {
                        reference: reference.clone(),
                    },
                );
                return self.channel(at, place, named);
            }
            // Out of reach, so a Reference Object it stays — and a
            // Reference Object is `$ref` alone.
            if item.publish.is_some()
                || item.subscribe.is_some()
                || item.description.is_some()
                || item.deprecated.is_some()
                || item.bindings.is_some()
                || item.extensions.is_some()
                || !item.parameters.is_empty()
                || !item.servers.is_empty()
            {
                self.note(at, NoteKind::ChannelReferenceSiblingsDropped);
            }
            let reference = self.rewrite(at, reference);
            if let Some(address) = place.address {
                self.note(
                    at,
                    NoteKind::ChannelAddressDropped {
                        address: address.to_owned(),
                    },
                );
            }
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
            let (name, converted) = self.operation(&at, place, action, operation, &mut messages);
            operations.push((name, RefOr::Item(converted)));
        }

        let channel = v3_0::Channel {
            address: place.address.map(|address| Some(address.to_owned())),
            messages,
            parameters: item
                .parameters
                .into_iter()
                .map(|(name, parameter)| {
                    let at = format!("{at}.parameters.{name}");
                    let source = format!("{}/parameters/{name}/schema", place.source);
                    (
                        name,
                        self.ref_or(&at, parameter, |this, at, item| {
                            this.parameter(at, &source, item)
                        }),
                    )
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
            bindings: self.bindings(at, item.bindings),
            extensions: item.extensions,
        };
        (RefOr::Item(channel), operations)
    }

    /// A 2.6 parameter describes its value with a schema; a 3.0 one is
    /// a string, with an enumeration and a default at most.
    fn parameter(&mut self, at: &str, source: &str, parameter: v2_6::Parameter) -> v3_0::Parameter {
        let mut converted = v3_0::Parameter {
            description: parameter.description,
            location: parameter.location,
            extensions: parameter.extensions,
            ..v3_0::Parameter::default()
        };
        let Some(schema) = parameter.schema else {
            return converted;
        };
        // Only what a v3 parameter can still say survives, and only
        // where the value itself survives with it: v3 enumerates
        // strings, so a number in an `enum` is as lost as a `pattern`.
        let carried = match serde_json::to_value(&schema) {
            Ok(serde_json::Value::Object(map)) => {
                converted.enum_values = strings(map.get("enum"));
                converted.examples = strings(map.get("examples"));
                converted.default = map
                    .get("default")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                let whole = |key: &str, kept: usize| match map.get(key) {
                    None => true,
                    Some(value) => value.as_array().is_some_and(|values| values.len() == kept),
                };
                map.len()
                    == usize::from(map.contains_key("enum"))
                        + usize::from(map.contains_key("examples"))
                        + usize::from(map.contains_key("default"))
                    && whole("enum", converted.enum_values.len())
                    && whole("examples", converted.examples.len())
                    && map.contains_key("default") == converted.default.is_some()
            }
            _ => false,
        };
        // The schema itself is set aside under the name it would take.
        // Whether it is kept — and so whether anything was lost here at
        // all — is not known until every pointer has been rewritten.
        if let Some(key) = self.parameter_schemas.get(source).cloned() {
            self.kept_schemas.insert(
                key,
                KeptSchema {
                    at: at.to_owned(),
                    schema,
                    carried,
                },
            );
            return converted;
        }
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
        place: &Place<'_>,
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

        // Everything was named before anything was converted, so that
        // a pointer at it could be rewritten. Deriving the name again
        // is the same answer, and a gentler one than a panic if that
        // ever stops being true.
        let key = self
            .operation_keys
            .get(&(place.source.clone(), action.as_str()))
            .and_then(|pointer| pointer.rsplit('/').next().map(str::to_owned))
            .unwrap_or_else(|| format!("{}_{}", place.key, action.as_str()));

        let references = self.messages(
            at,
            &format!("{}/{}/message", place.source, action.as_str()),
            place,
            operation.message,
            messages,
        );
        let converted = v3_0::Operation {
            action: converted_action,
            channel: Reference {
                reference: place.reference.clone(),
            },
            messages: references,
            reply: None,
            traits: operation
                .traits
                .into_iter()
                .map(|operation_trait| self.ref_or(at, operation_trait, Self::operation_trait))
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
            bindings: self.bindings(at, operation.bindings),
            extensions: operation.extensions,
        };
        (key, converted)
    }

    /// Move an operation's messages onto its channel, and name them
    /// there — which is how 3.0 says an operation carries a message.
    fn messages(
        &mut self,
        at: &str,
        source: &str,
        place: &Place<'_>,
        message: Option<v2_6::OperationMessage>,
        channel_messages: &mut BTreeMap<String, RefOr<v3_0::Message>>,
    ) -> Vec<Reference> {
        let found = flatten(message);
        // v3 has no `oneOf` object to hang anything off: the
        // alternatives become the channel's messages, and what the
        // container carried has nowhere to go.
        for (within, extensions) in found.containers {
            let fields: Vec<String> = extensions.into_keys().collect();
            if !fields.is_empty() {
                self.note(
                    &format!("{at}.message{within}"),
                    NoteKind::FieldsDropped { fields },
                );
            }
        }
        let mut references = Vec::new();
        for (within, message) in found.entries {
            let at = format!("{at}.message{within}");
            let named = message_name(&message);
            // The name was settled before anything moved, so that a
            // pointer at this message lands where it went; one copied
            // in from a channel that named another is named here.
            let key = self
                .message_keys
                .get(&format!("{source}{within}"))
                .cloned()
                .unwrap_or_else(|| {
                    named
                        .as_deref()
                        .map(sanitize)
                        .filter(|key| !key.is_empty())
                        .unwrap_or_else(|| "message".to_owned())
                });
            if named.as_deref() != Some(key.as_str()) {
                self.note(
                    &at,
                    NoteKind::MessageKeyDerived {
                        from: named,
                        key: key.clone(),
                    },
                );
            }
            let converted = match message {
                RefOr::Reference(reference) => {
                    let reference = self.rewrite(&at, &reference.reference);
                    RefOr::Reference(Reference { reference })
                }
                RefOr::Item(message) => RefOr::Item(self.message(&at, message, Some(&key))),
            };
            references.push(Reference {
                reference: format!("{}/messages/{key}", place.reference),
            });
            channel_messages.insert(key, converted);
        }
        references
    }

    /// `kept_as` is the name v3 files the message under, which is
    /// where its identity goes: v3 has no `messageId` of its own.
    fn message(
        &mut self,
        at: &str,
        message: v2_6::Message,
        kept_as: Option<&str>,
    ) -> v3_0::Message {
        if let Some(message_id) = &message.message_id
            && kept_as != Some(message_id.as_str())
        {
            self.note(
                at,
                NoteKind::FieldsDropped {
                    fields: vec!["messageId".to_owned()],
                },
            );
        }

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
            bindings: self.bindings(at, message.bindings),
            examples: message
                .examples
                .into_iter()
                .filter_map(|example| self.reinterpret(at, &example))
                .collect(),
            traits: message
                .traits
                .into_iter()
                .map(|message_trait| self.ref_or(at, message_trait, Self::message_trait))
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
        let mut value = serde_json::to_value(schema).ok()?;
        if crate::v2_6::message::payload_is_asyncapi_schema(schema_format) {
            // A schema in this document's own dialect names things the
            // way the document does, and v3 moved some of them. Another
            // dialect's `$ref` means whatever that dialect says.
            self.rewrite_in_schema(at, &mut value);
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
        let schemas: BTreeMap<String, RefOr<v3_0::SchemaOrMultiFormat>> = components
            .schemas
            .into_iter()
            .filter_map(|(name, schema)| {
                let at = format!("{at}.schemas.{name}");
                Some((name, self.schema(&at, "schema", &schema, None)?))
            })
            .collect();

        // A reusable channel has no address — in 2.6 the address is the
        // key of whatever names it — and its operations still have to
        // go somewhere, which in v3 is a map of their own.
        let mut reusable_channels = BTreeMap::new();
        let mut reusable_operations = BTreeMap::new();
        for (name, item) in components.channels {
            let at = format!("{at}.channels.{name}");
            let place = Place {
                source: format!("#/components/channels/{name}"),
                address: None,
                reference: format!("#/components/channels/{name}"),
                key: &name,
            };
            let (channel, operations) = self.channel(&at, &place, item);
            reusable_channels.insert(name.clone(), channel);
            reusable_operations.extend(operations);
        }

        v3_0::Components {
            schemas,
            servers: components
                .servers
                .into_iter()
                .map(|(name, server)| {
                    let at = format!("{at}.servers.{name}");
                    (name, self.ref_or(&at, server, Self::server))
                })
                .collect(),
            channels: reusable_channels,
            operations: reusable_operations,
            messages: components
                .messages
                .into_iter()
                .map(|(name, message)| {
                    let at = format!("{at}.messages.{name}");
                    let kept_as = name.clone();
                    (
                        name,
                        self.ref_or(&at, message, |this, at, item| {
                            this.message(at, item, Some(&kept_as))
                        }),
                    )
                })
                .collect(),
            security_schemes: components
                .security_schemes
                .into_iter()
                .map(|(name, scheme)| {
                    let at = format!("{at}.securitySchemes.{name}");
                    (name, self.ref_or(&at, scheme, Self::security_scheme))
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
                    let source = format!("#/components/parameters/{name}/schema");
                    (
                        name,
                        self.ref_or(&at, parameter, |this, at, item| {
                            this.parameter(at, &source, item)
                        }),
                    )
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
                        self.ref_or(&at, operation_trait, Self::operation_trait),
                    )
                })
                .collect(),
            message_traits: components
                .message_traits
                .into_iter()
                .map(|(name, message_trait)| {
                    let at = format!("{at}.messageTraits.{name}");
                    (name, self.ref_or(&at, message_trait, Self::message_trait))
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
    /// A channel keeps everything it had except its operations, which
    /// went to a map of their own — so a pointer at one of those
    /// follows it there, and a pointer at anything else follows the
    /// channel to its new key. What an operation *carried* is the one
    /// thing beyond reach: its message became the channel's, under a
    /// name this conversion invents while converting, not before.
    ///
    /// The pointer is read the way the rest of this crate reads one, so
    /// `%7E1` is the `~1` it decodes to.
    /// Where a parameter's schema is kept, saying as it answers that
    /// the document really does need it kept.
    fn kept_as(&mut self, source: &str) -> Option<String> {
        let key = self.parameter_schemas.get(source)?;
        let moved_to = format!("#/components/schemas/{key}");
        self.used_schemas.insert(key.clone());
        Some(moved_to)
    }

    fn rewrite(&mut self, at: &str, reference: &str) -> String {
        let Some(fragment) = reference.strip_prefix('#') else {
            // Another document's, and not this conversion's business.
            return reference.to_owned();
        };
        let Some(tokens) = pointer::tokens(fragment) else {
            return reference.to_owned();
        };
        // Which channel does this name, and what did it want inside it?
        let (source, moved_to, rest) = match tokens.as_slice() {
            [components, channels, name, rest @ ..]
                if components == "components" && channels == "channels" =>
            {
                // A reusable channel keeps its name; only its
                // operations move.
                (
                    format!("#/components/channels/{name}"),
                    format!("#/components/channels/{}", escape(name)),
                    rest,
                )
            }
            // A reusable parameter's schema went the same way a
            // channel parameter's did.
            [components, parameters, name, schema, rest @ ..]
                if components == "components"
                    && parameters == "parameters"
                    && schema == "schema" =>
            {
                let source = format!("#/components/parameters/{name}/schema");
                return match self.kept_as(&source) {
                    Some(moved_to) => join(&moved_to, rest.iter()),
                    None => reference.to_owned(),
                };
            }
            [channels, address, rest @ ..] if channels == "channels" => {
                let Some(key) = self.channel_keys.get(address) else {
                    self.note(
                        at,
                        NoteKind::ReferenceNotRewritten {
                            reference: reference.to_owned(),
                        },
                    );
                    return reference.to_owned();
                };
                (
                    format!("#/channels/{address}"),
                    format!("#/channels/{key}"),
                    rest,
                )
            }
            _ => return reference.to_owned(),
        };

        let [step, rest @ ..] = rest else {
            return moved_to;
        };
        // A parameter's schema was kept elsewhere, v3 parameters having
        // nowhere to hold one.
        if step == "parameters"
            && let [name, schema, rest @ ..] = rest
            && schema == "schema"
            && let Some(moved_to) = self.kept_as(&format!("{source}/parameters/{name}/schema"))
        {
            return join(&moved_to, rest.iter());
        }
        if !matches!(step.as_str(), "publish" | "subscribe") {
            // Parameters, bindings, a description: all still the
            // channel's, wherever the channel went.
            return join(&moved_to, std::iter::once(step).chain(rest));
        }
        let action = if step == "publish" {
            "publish"
        } else {
            "subscribe"
        };
        // A message did not follow its operation: it went to the
        // channel, under a name settled before any of this began.
        if let [message, tail @ ..] = rest
            && message == "message"
        {
            // A `oneOf` may hold another, and the pointer says so.
            let mut within = String::new();
            let mut rest = tail;
            while let [one_of, index, more @ ..] = rest {
                if one_of != "oneOf" || pointer::array_index(index).is_none() {
                    break;
                }
                within.push_str(&format!("/oneOf/{index}"));
                rest = more;
            }
            let named = format!("{source}/{action}/message{within}");
            return match self.message_keys.get(&named) {
                Some(key) => join(&format!("{moved_to}/messages/{key}"), rest.iter()),
                None => {
                    self.note(
                        at,
                        NoteKind::ReferenceNotRewritten {
                            reference: reference.to_owned(),
                        },
                    );
                    reference.to_owned()
                }
            };
        }
        match self.operation_keys.get(&(source, action)) {
            Some(operation) => join(&operation.clone(), rest.iter()),
            None => {
                self.note(
                    at,
                    NoteKind::ReferenceNotRewritten {
                        reference: reference.to_owned(),
                    },
                );
                reference.to_owned()
            }
        }
    }

    /// Rewrite the pointers a schema holds, and only those.
    ///
    /// A schema keeps schemas in a known set of places; everything else
    /// it holds is data — a `default`, an `enum`, an `x-` member — and
    /// a `$ref`-shaped value there means whatever the application says
    /// it means. This walks the first and leaves the second alone.
    fn rewrite_in_schema(&mut self, at: &str, value: &mut serde_json::Value) {
        let serde_json::Value::Object(map) = value else {
            return;
        };
        if let Some(serde_json::Value::String(reference)) = map.get("$ref") {
            let rewritten = self.rewrite(at, &reference.clone());
            map.insert("$ref".to_owned(), serde_json::Value::String(rewritten));
        }
        for (key, value) in map.iter_mut() {
            match key.as_str() {
                // A map of schemas.
                "properties" | "patternProperties" | "definitions" | "dependencies" => {
                    if let serde_json::Value::Object(schemas) = value {
                        for schema in schemas.values_mut() {
                            self.rewrite_in_schema(at, schema);
                        }
                    }
                }
                // A list of them.
                "allOf" | "anyOf" | "oneOf" => {
                    if let serde_json::Value::Array(schemas) = value {
                        for schema in schemas {
                            self.rewrite_in_schema(at, schema);
                        }
                    }
                }
                // Either, `items` being draft-07's both-ways keyword.
                "items" => match value {
                    serde_json::Value::Array(schemas) => {
                        for schema in schemas {
                            self.rewrite_in_schema(at, schema);
                        }
                    }
                    schema => self.rewrite_in_schema(at, schema),
                },
                // One schema.
                "additionalItems"
                | "additionalProperties"
                | "propertyNames"
                | "contains"
                | "not"
                | "if"
                | "then"
                | "else" => self.rewrite_in_schema(at, value),
                // Anything else is the application's own data.
                _ => {}
            }
        }
    }

    /// Re-read a value as its v3.0 counterpart, the two versions
    /// modelling it the same way.
    fn reinterpret<A: Serialize, B: DeserializeOwned + Serialize>(
        &mut self,
        at: &str,
        value: &A,
    ) -> Option<B> {
        let before = serde_json::to_value(value).ok();
        let converted: Option<B> = before
            .clone()
            .and_then(|value| serde_json::from_value(value).ok());
        // Serde drops what the destination does not model, so parsing
        // is no proof the value crossed whole. Compare what each side
        // spells, and say what did not make it.
        if let (Some(serde_json::Value::Object(before)), Some(converted)) =
            (before, converted.as_ref())
            && let Ok(serde_json::Value::Object(after)) = serde_json::to_value(converted)
        {
            let dropped: Vec<String> = before
                .keys()
                .filter(|key| !after.contains_key(*key))
                .cloned()
                .collect();
            if !dropped.is_empty() {
                self.note(at, NoteKind::FieldsDropped { fields: dropped });
            }
        }
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

/// A channel that names another, as the two of them together.
///
/// 2.6 leaves it undefined which wins where both say something, and
/// deprecates the whole arrangement — so this takes the naming channel
/// as the more particular of the two and lets it stand where it speaks.
/// Nothing is thrown away, which is what matters: a pointer at either
/// half still has somewhere to land.
fn merged(
    item: &v2_6::ChannelItem,
    reusable: &BTreeMap<String, v2_6::ChannelItem>,
) -> Option<v2_6::ChannelItem> {
    let mut named = named_channel(item.reference.as_deref()?, reusable)?;
    if item.description.is_some() {
        named.description = item.description.clone();
    }
    if !item.servers.is_empty() {
        named.servers = item.servers.clone();
    }
    if item.publish.is_some() {
        named.publish = item.publish.clone();
    }
    if item.subscribe.is_some() {
        named.subscribe = item.subscribe.clone();
    }
    if item.deprecated.is_some() {
        named.deprecated = item.deprecated;
    }
    if item.bindings.is_some() {
        named.bindings = item.bindings.clone();
    }
    named.parameters.extend(
        item.parameters
            .iter()
            .map(|(name, parameter)| (name.clone(), parameter.clone())),
    );
    if let Some(extensions) = &item.extensions {
        named
            .extensions
            .get_or_insert_with(BTreeMap::new)
            .extend(extensions.clone());
    }
    named.reference = None;
    Some(named)
}

/// The reusable channel a pointer names, following one that names
/// another. `None` for a pointer this conversion cannot follow.
fn named_channel(
    reference: &str,
    reusable: &BTreeMap<String, v2_6::ChannelItem>,
) -> Option<v2_6::ChannelItem> {
    let mut seen = BTreeSet::new();
    let mut reference = reference.to_owned();
    loop {
        let tokens = pointer::tokens(reference.strip_prefix('#')?)?;
        let [components, channels, name] = tokens.as_slice() else {
            return None;
        };
        if components != "components" || channels != "channels" || !seen.insert(name.clone()) {
            return None;
        }
        let named = reusable.get(name)?;
        match &named.reference {
            Some(next) => reference = next.clone(),
            None => return Some(named.clone()),
        }
    }
}

/// What a message offers to be called.
fn message_name(message: &RefOr<v2_6::Message>) -> Option<String> {
    match message {
        RefOr::Reference(reference) => reference.component_key("messages"),
        RefOr::Item(message) => message.message_id.clone().or_else(|| message.name.clone()),
    }
}

/// Every message an operation carries, however 2.6 spelled them, each
/// with what the pointer that names it says after `message`.
///
/// A `oneOf` may hold another, so the pointer is built as the walk goes
/// rather than counted off a flattened list: the second alternative of
/// the second is `/oneOf/1/oneOf/1`, and nothing else.
#[derive(Default)]
struct Messages {
    entries: Vec<(String, RefOr<v2_6::Message>)>,
    /// The `oneOf` objects walked through, and what each carried that
    /// v3 has no container to keep.
    containers: Vec<(String, BTreeMap<String, serde_json::Value>)>,
}

fn flatten(message: Option<v2_6::OperationMessage>) -> Messages {
    fn walk(within: &str, message: v2_6::OperationMessage, found: &mut Messages) {
        match message {
            v2_6::OperationMessage::Single(message) => {
                found.entries.push((within.to_owned(), *message));
            }
            v2_6::OperationMessage::OneOf(one_of) => {
                if let Some(extensions) = one_of.extensions {
                    found.containers.push((within.to_owned(), extensions));
                }
                for (index, message) in one_of.one_of.into_iter().enumerate() {
                    walk(&format!("{within}/oneOf/{index}"), message, found);
                }
            }
        }
    }
    let mut found = Messages::default();
    if let Some(message) = message {
        walk("", message, &mut found);
    }
    found
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

/// The RFC 6901 escapes a token carries inside a pointer.
fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// A pointer with more tokens on the end.
fn join<'t>(prefix: &str, tokens: impl Iterator<Item = &'t String>) -> String {
    let mut pointer = prefix.to_owned();
    for token in tokens {
        pointer.push('/');
        pointer.push_str(&escape(token));
    }
    pointer
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

        // One that points *inside* a channel is followed too: the
        // message went to the channel, under a name of its own.
        let (document, notes) = convert_json(minimal(json!({
            "a/b": { "publish": { "message": { "name": "M" } } },
            "alias": { "$ref": "#/channels/a~1b/publish/message" }
        })));
        assert!(matches!(
            &document.channels["alias"],
            RefOr::Reference(reference) if reference.reference == "#/channels/a_b/messages/M"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
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
                from: None,
                key: "k".to_owned(),
            },
            NoteKind::OperationKeyDerived {
                from: Some("read/orders".to_owned()),
                key: "read_orders".to_owned(),
            },
            NoteKind::MessageKeyDerived {
                from: None,
                key: "m".to_owned(),
            },
            NoteKind::MessageKeyDerived {
                from: Some("M/x".to_owned()),
                key: "M_x".to_owned(),
            },
            NoteKind::ChannelReferenceInlined {
                reference: "#/components/channels/shared".to_owned(),
            },
            NoteKind::ChannelAddressDropped {
                address: "orders".to_owned(),
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
    fn a_channel_that_names_another_is_given_what_it_names() {
        // Only the naming channel knows the address, so what it names
        // is copied in rather than referenced — operations and all.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "orders": { "$ref": "#/components/channels/shared" } },
            "components": {
                "channels": { "shared": { "publish": { "message": { "name": "M" } } } }
            }
        }));

        let channel = document.channels["orders"].item().expect("copied in");
        assert_eq!(
            channel.address.as_ref().and_then(Option::as_deref),
            Some("orders"),
        );
        let operation = document.operations["orders_publish"]
            .item()
            .expect("lifted");
        assert_eq!(operation.action, v3_0::OperationAction::Receive);
        assert_eq!(operation.channel.reference, "#/channels/orders");
        assert!(notes.iter().any(|note| note.contains("was copied in")));

        // The reusable channel has no address of its own, and its
        // operations went where v3 keeps reusable ones.
        let components = document.components.as_ref().expect("components");
        let shared = components.channels["shared"].item().expect("inline");
        assert_eq!(shared.address, None);
        let reusable = components.operations["shared_publish"]
            .item()
            .expect("lifted");
        assert_eq!(reusable.channel.reference, "#/components/channels/shared");
        document_is_valid(&document);
    }

    #[test]
    fn a_channel_reference_that_cannot_be_followed_says_what_it_costs() {
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": { "s": { "url": "b", "protocol": "kafka" } },
            "channels": {
                "c": {
                    "$ref": "./other.yaml#/c",
                    "servers": ["s"],
                    "deprecated": true,
                    "bindings": { "kafka": {} },
                    "x-note": 1
                }
            }
        }));
        assert!(matches!(&document.channels["c"], RefOr::Reference(_)));
        // Every sort of sibling counts, not only the obvious ones.
        assert!(
            notes
                .iter()
                .any(|note| note.contains("what sat beside it is dropped")),
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("address `c` is lost")),
            "got: {notes:?}"
        );
    }

    #[test]
    fn an_oauth_flow_keeps_its_scopes_under_the_name_v3_gave_them() {
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "components": {
                "securitySchemes": {
                    "oauth": {
                        "type": "oauth2",
                        "flows": {
                            "implicit": {
                                "authorizationUrl": "https://example.com/authorize",
                                "scopes": { "read": "read things" }
                            }
                        }
                    }
                }
            }
        }));
        // v3 requires `availableScopes`, so a scheme that crossed over
        // as-is would leave an invalid document behind.
        assert!(notes.is_empty(), "got: {notes:?}");
        document_is_valid(&document);
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["securitySchemes"]["oauth"]["flows"]["implicit"]["availableScopes"]
                ["read"],
            json!("read things"),
        );
    }

    #[test]
    fn a_pointer_follows_an_operation_to_where_v3_put_it() {
        let (document, notes) = convert_json(minimal(json!({
            "c": {
                "subscribe": { "bindings": { "kafka": {} } },
                "publish": { "bindings": { "$ref": "#/channels/c/subscribe/bindings" } }
            }
        })));
        let operation = document.operations["c_publish"].item().expect("inline");
        assert!(matches!(
            &operation.bindings,
            Some(RefOr::Reference(reference))
                if reference.reference == "#/operations/c_subscribe/bindings"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        document_is_valid(&document);

        // A message followed its channel rather than its operation,
        // and a pointer at one follows it there.
        let (document, notes) = convert_json(minimal(json!({
            "c": {
                "publish": { "message": { "name": "M" } },
                "subscribe": { "bindings": { "$ref": "#/channels/c/publish/message" } }
            }
        })));
        let send = document.operations["c_subscribe"].item().expect("inline");
        assert!(matches!(
            &send.bindings,
            Some(RefOr::Reference(reference))
                if reference.reference == "#/channels/c/messages/M"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
    }

    #[test]
    fn a_parameter_keeps_its_extensions_and_says_what_it_could_not_keep() {
        let (document, notes) = convert_json(minimal(json!({
            "c": {
                "parameters": {
                    "p": {
                        "x-owner": "me",
                        "schema": { "enum": ["a", 1], "examples": ["b", 2], "default": 3 }
                    }
                }
            }
        })));
        let channel = document.channels["c"].item().expect("inline");
        let parameter = channel.parameters["p"].item().expect("inline");
        // v3 parameters carry extensions, so these need not be lost.
        assert_eq!(
            parameter
                .extensions
                .as_ref()
                .and_then(|ext| ext.get("x-owner")),
            Some(&json!("me")),
        );
        // A v3 enumeration is of strings, so the number in each list —
        // and the numeric default — did not survive, and that is said.
        assert_eq!(parameter.enum_values, vec!["a"]);
        assert_eq!(parameter.examples, vec!["b"]);
        assert_eq!(parameter.default, None);
        assert!(
            notes
                .iter()
                .any(|note| note.contains("a v3 parameter is a string")),
            "got: {notes:?}"
        );
    }

    #[test]
    fn a_name_that_had_to_change_is_named_in_the_report() {
        let (document, notes) = convert_json(minimal(json!({
            "c": {
                "publish": {
                    "operationId": "read/orders",
                    "message": { "name": "M/x" }
                }
            }
        })));
        assert!(document.operations.contains_key("read_orders"));
        assert!(
            notes
                .iter()
                .any(|note| note
                    .contains("`read/orders` is not a usable key; keyed as `read_orders`")),
            "got: {notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("`M/x` is not a usable key; keyed as `M_x`")),
            "got: {notes:?}"
        );

        // A name taken twice is changed too, and said so.
        let (_, notes) = convert_json(minimal(json!({
            "a": { "publish": { "operationId": "shared" } },
            "b": { "publish": { "operationId": "shared" } }
        })));
        assert!(
            notes
                .iter()
                .any(|note| note.contains("`shared` is not a usable key; keyed as `shared_2`")),
            "got: {notes:?}"
        );
    }

    #[test]
    fn a_channel_that_names_a_channel_that_names_another_is_followed() {
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "orders": { "$ref": "#/components/channels/alias" } },
            "components": {
                "channels": {
                    "alias": { "$ref": "#/components/channels/shared" },
                    "shared": { "description": "the real one" }
                }
            }
        }));
        let channel = document.channels["orders"].item().expect("copied in");
        assert_eq!(channel.description.as_deref(), Some("the real one"));
        assert_eq!(
            channel.address.as_ref().and_then(Option::as_deref),
            Some("orders"),
        );

        // A chain that goes round in a circle is not followed at all.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "orders": { "$ref": "#/components/channels/a" } },
            "components": {
                "channels": {
                    "a": { "$ref": "#/components/channels/b" },
                    "b": { "$ref": "#/components/channels/a" }
                }
            }
        }));
        assert!(matches!(&document.channels["orders"], RefOr::Reference(_)));
        assert!(
            notes
                .iter()
                .any(|note| note.contains("address `orders` is lost"))
        );
    }

    #[test]
    fn a_pointer_at_a_whole_operation_follows_it_too() {
        let (document, notes) = convert_json(minimal(json!({
            "c": {
                "subscribe": { "operationId": "sendThings" },
                "publish": { "bindings": { "$ref": "#/channels/c/subscribe" } }
            }
        })));
        let operation = document.operations["c_publish"].item().expect("inline");
        assert!(matches!(
            &operation.bindings,
            Some(RefOr::Reference(reference))
                if reference.reference == "#/operations/sendThings"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
    }

    #[test]
    fn schemes_required_together_cannot_stay_that_way() {
        // A v2.6 requirement naming two schemes needs both; a v3 list
        // needs one of what it names, and there is no v3 way to say
        // otherwise — so it is said in the report instead.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "servers": {
                "s": {
                    "url": "b",
                    "protocol": "kafka",
                    "security": [ { "password": [], "certificate": [] } ]
                }
            },
            "components": {
                "securitySchemes": {
                    "password": { "type": "userPassword" },
                    "certificate": { "type": "X509" }
                }
            }
        }));
        assert!(
            notes.iter().any(|note| note
                .contains("[\"certificate\", \"password\"] were required together")),
            "got: {notes:?}"
        );
        let server = document.servers["s"].item().expect("inline");
        assert_eq!(server.security.len(), 2);

        // One scheme on its own is no weaker in v3 than it was.
        let (_, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "servers": {
                "s": { "url": "b", "protocol": "kafka", "security": [ { "password": [] } ] }
            },
            "components": { "securitySchemes": { "password": { "type": "userPassword" } } }
        }));
        assert!(notes.is_empty(), "got: {notes:?}");
    }

    #[test]
    fn a_reusable_channels_operation_can_be_followed_too() {
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "c": {
                    "publish": {
                        "bindings": { "$ref": "#/components/channels/shared/subscribe/bindings" }
                    }
                }
            },
            "components": {
                "channels": { "shared": { "subscribe": { "bindings": { "kafka": {} } } } }
            }
        }));
        let operation = document.operations["c_publish"].item().expect("inline");
        assert!(
            matches!(
                &operation.bindings,
                Some(RefOr::Reference(reference))
                    if reference.reference == "#/components/operations/shared_subscribe/bindings"
            ),
            "got {:?}",
            operation.bindings,
        );
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        document_is_valid(&document);
    }

    #[test]
    fn a_schema_names_things_too() {
        // A schema is carried as JSON, which is no reason for what it
        // names to be left behind.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "a/b": { "publish": { "bindings": { "kafka": {} } } } },
            "components": {
                "schemas": {
                    "s": { "properties": { "p": { "$ref": "#/channels/a~1b" } } }
                }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["properties"]["p"]["$ref"],
            json!("#/channels/a_b"),
        );
        assert!(!notes.iter().any(|note| note.contains("no longer names")));

        // Anything else a channel keeps, it keeps wherever it went.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "a/b": { "parameters": { "id": { "description": "the id" } } }
            },
            "components": {
                "schemas": {
                    "s": { "properties": { "p": { "$ref": "#/channels/a~1b/parameters/id" } } }
                }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["properties"]["p"]["$ref"],
            json!("#/channels/a_b/parameters/id"),
        );
        assert!(!notes.iter().any(|note| note.contains("no longer names")));

        // A fragment that is not a pointer is left exactly as it was.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "components": {
                "schemas": { "s": { "$ref": "#/channels/bad~2escape" } }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/channels/bad~2escape"),
        );

        // One that names what an operation carried is followed to the
        // channel the message went to, whichever form it took there.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "c": { "publish": { "message": { "name": "M", "payload": { "type": "object" } } } },
                "a/b": {
                    "publish": {
                        "message": {
                            "oneOf": [ { "name": "A" }, { "name": "B", "payload": { "type": "object" } } ]
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "s": {
                        "properties": {
                            "p": { "$ref": "#/channels/c/publish/message/payload" },
                            "q": { "$ref": "#/channels/a~1b/publish/message/oneOf/1/payload" }
                        }
                    }
                }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        let properties = &value["components"]["schemas"]["s"]["properties"];
        assert_eq!(
            properties["p"]["$ref"],
            json!("#/channels/c/messages/M/payload"),
        );
        assert_eq!(
            properties["q"]["$ref"],
            json!("#/channels/a_b/messages/B/payload"),
        );
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        document_is_valid(&document);
    }

    #[test]
    fn a_pointer_is_read_the_way_the_crate_reads_one() {
        // `%7E1` is the `~1` it decodes to, which is the `/` in `a/b`.
        let (document, notes) = convert_json(minimal(json!({
            "a/b": { "publish": {} },
            "alias": { "$ref": "#/channels/a%7E1b" }
        })));
        assert!(matches!(
            &document.channels["alias"],
            RefOr::Reference(reference) if reference.reference == "#/channels/a_b"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        document_is_valid(&document);
    }

    #[test]
    fn only_the_flows_v3_renamed_are_renamed() {
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "components": {
                "securitySchemes": {
                    "o": {
                        "type": "oauth2",
                        "flows": {
                            "implicit": {
                                "authorizationUrl": "https://example.com/authorize",
                                "scopes": { "r": "read" }
                            },
                            "x-config": { "scopes": { "custom": "thing" } }
                        }
                    }
                }
            }
        }));
        assert!(notes.is_empty(), "got: {notes:?}");
        let flows = &serde_json::to_value(&document).expect("serializable")["components"]["securitySchemes"]
            ["o"]["flows"];
        assert_eq!(flows["implicit"]["availableScopes"]["r"], json!("read"));
        // An extension of `flows` is the extension's business, whatever
        // it spells its keys.
        assert_eq!(flows["x-config"]["scopes"]["custom"], json!("thing"));
    }

    #[test]
    fn a_field_v3_has_no_home_for_is_named_in_the_report() {
        // Re-reading a value drops what the far side does not model, so
        // parsing is no proof it crossed whole.
        let (_, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "components": {
                "operationTraits": { "t": { "operationId": "op" } },
                "messageTraits": {
                    "m": {
                        "messageId": "mid",
                        "schemaFormat": "application/vnd.apache.avro;version=1.9.0"
                    }
                },
                "messages": { "msg": { "messageId": "other" } }
            }
        }));
        for expected in [
            "#.components.operationTraits.t: v3 has nowhere to keep [\"operationId\"]",
            "#.components.messageTraits.m: v3 has nowhere to keep [\"messageId\", \"schemaFormat\"]",
            "#.components.messages.msg: v3 has nowhere to keep [\"messageId\"]",
        ] {
            assert!(
                notes.iter().any(|note| note == expected),
                "{expected} — got: {notes:?}"
            );
        }

        // A message whose `messageId` becomes the name it is kept under
        // has lost nothing.
        let (_, notes) = convert_json(minimal(json!({
            "c": { "publish": { "message": { "messageId": "Placed" } } }
        })));
        assert!(
            !notes.iter().any(|note| note.contains("nowhere to keep")),
            "got: {notes:?}"
        );
    }

    #[test]
    fn a_traits_references_are_followed_like_an_operations() {
        // A trait carries what an operation does, references and all —
        // so re-reading it whole would leave it pointing where v3 is
        // not.
        let (document, notes) = convert_json(minimal(json!({
            "c": {
                "subscribe": { "operationId": "send", "bindings": { "kafka": {} } },
                "publish": {
                    "traits": [ { "bindings": { "$ref": "#/channels/c/subscribe/bindings" } } ]
                }
            }
        })));
        let operation = document.operations["c_publish"].item().expect("inline");
        let operation_trait = operation.traits[0].item().expect("inline");
        assert!(matches!(
            &operation_trait.bindings,
            Some(RefOr::Reference(reference))
                if reference.reference == "#/operations/send/bindings"
        ));
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        document_is_valid(&document);

        // A message trait's headers name things too.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "a/b": { "publish": {} } },
            "components": {
                "messageTraits": {
                    "t": { "headers": { "properties": { "p": { "$ref": "#/channels/a~1b" } } } }
                }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["messageTraits"]["t"]["headers"]["properties"]["p"]["$ref"],
            json!("#/channels/a_b"),
        );
    }

    #[test]
    fn another_dialect_is_left_exactly_as_it_was() {
        // A `$ref` in an Avro payload is Avro's business, and so is a
        // `$ref`-shaped value sitting in a schema's own data.
        let (document, _) = convert_json(minimal(json!({
            "a/b": {
                "publish": {
                    "message": {
                        "name": "M",
                        "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
                        "payload": { "type": "record", "ref": { "$ref": "#/channels/a~1b" } }
                    }
                }
            }
        })));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["channels"]["a_b"]["messages"]["M"]["payload"]["schema"]["ref"]["$ref"],
            json!("#/channels/a~1b"),
        );

        // The same for what a schema holds as data rather than as a
        // schema.
        let (document, _) = convert_json(minimal(json!({
            "a/b": {
                "publish": {
                    "message": {
                        "name": "M",
                        "payload": {
                            "type": "object",
                            "default": { "$ref": "#/channels/a~1b" },
                            "examples": [ { "$ref": "#/channels/a~1b" } ],
                            "x-sample": { "$ref": "#/channels/a~1b" },
                            "properties": { "p": { "$ref": "#/channels/a~1b" } }
                        }
                    }
                }
            }
        })));
        let value = serde_json::to_value(&document).expect("serializable");
        let payload = &value["channels"]["a_b"]["messages"]["M"]["payload"];
        for data in ["default", "x-sample"] {
            assert_eq!(payload[data]["$ref"], json!("#/channels/a~1b"), "{data}");
        }
        assert_eq!(payload["examples"][0]["$ref"], json!("#/channels/a~1b"));
        // …but a schema it holds as a schema is a schema.
        assert_eq!(payload["properties"]["p"]["$ref"], json!("#/channels/a_b"));
    }

    #[test]
    fn a_pointer_at_what_is_not_there_is_left_and_reported() {
        // Nothing to follow: the channel has no such operation, and no
        // such message.
        let (_, notes) = convert_json(minimal(json!({
            "c": {
                "subscribe": {
                    "bindings": { "$ref": "#/channels/c/publish/bindings" },
                    "message": { "$ref": "#/channels/c/publish/message" }
                }
            }
        })));
        for missing in [
            "`#/channels/c/publish/bindings` no longer names the same thing",
            "`#/channels/c/publish/message` no longer names the same thing",
        ] {
            assert!(
                notes.iter().any(|note| note.contains(missing)),
                "got: {notes:?}"
            );
        }
    }

    #[test]
    fn every_place_a_schema_keeps_schemas_is_walked() {
        let (document, _) = convert_json(minimal(json!({
            "a/b": {
                "publish": {
                    "message": {
                        "name": "M",
                        "payload": {
                            "allOf": [ { "$ref": "#/channels/a~1b" } ],
                            "items": [ { "$ref": "#/channels/a~1b" } ],
                            "additionalProperties": { "$ref": "#/channels/a~1b" },
                            "definitions": { "d": { "$ref": "#/channels/a~1b" } }
                        }
                    }
                }
            },
            "c": {
                "publish": {
                    "message": {
                        "name": "N",
                        "payload": { "items": { "$ref": "#/channels/a~1b" } }
                    }
                }
            }
        })));
        let value = serde_json::to_value(&document).expect("serializable");
        let payload = &value["channels"]["a_b"]["messages"]["M"]["payload"];
        assert_eq!(payload["allOf"][0]["$ref"], json!("#/channels/a_b"));
        assert_eq!(payload["items"][0]["$ref"], json!("#/channels/a_b"));
        assert_eq!(
            payload["additionalProperties"]["$ref"],
            json!("#/channels/a_b")
        );
        assert_eq!(payload["definitions"]["d"]["$ref"], json!("#/channels/a_b"));
        // `items` the other way round is a schema, not a list of them.
        assert_eq!(
            value["channels"]["c"]["messages"]["N"]["payload"]["items"]["$ref"],
            json!("#/channels/a_b"),
        );
    }

    #[test]
    fn a_message_trait_carries_a_correlation_id_across() {
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {},
            "components": {
                "messageTraits": {
                    "t": { "correlationId": { "location": "$message.header#/id" } }
                }
            }
        }));
        let components = document.components.expect("components");
        let message_trait = components.message_traits["t"].item().expect("inline");
        let correlation_id = message_trait
            .correlation_id
            .as_ref()
            .and_then(RefOr::item)
            .expect("carried across");
        assert_eq!(correlation_id.location, "$message.header#/id");
    }

    #[test]
    fn re_reading_a_value_says_what_it_could_not_bring() {
        // Nothing this conversion re-reads drops a field today — the
        // types that did are converted field by field — but the net
        // stays, so that adding one does not lose anything quietly.
        let source: v2_6::Document =
            serde_json::from_value(minimal(json!({}))).expect("a document");
        let mut conversion = Conversion::new(&source);
        let message = v2_6::Message {
            message_id: Some("mid".to_owned()),
            name: Some("M".to_owned()),
            ..v2_6::Message::default()
        };
        let converted: Option<v3_0::Message> = conversion.reinterpret("#.somewhere", &message);
        assert!(converted.is_some());
        assert_eq!(
            conversion
                .notes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["#.somewhere: v3 has nowhere to keep [\"messageId\"]"],
        );
    }

    #[test]
    fn a_oneof_inside_a_oneof_keeps_its_own_pointer() {
        // The alternatives all become the channel's messages, but the
        // pointer that names one says how it was nested to get there.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "source": {
                    "publish": {
                        "message": {
                            "oneOf": [
                                { "name": "A" },
                                {
                                    "oneOf": [
                                        { "name": "C" },
                                        { "name": "D", "payload": { "type": "object" } }
                                    ]
                                }
                            ]
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "s": { "$ref": "#/channels/source/publish/message/oneOf/1/oneOf/1/payload" }
                }
            }
        }));
        let channel = document.channels["source"].item().expect("inline");
        let mut keys: Vec<&String> = channel.messages.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["A", "C", "D"]);

        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/channels/source/messages/D/payload"),
        );
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        document_is_valid(&document);

        // A pointer that goes deeper into a message rather than into
        // another alternative stops counting alternatives.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "c": {
                    "publish": {
                        "message": {
                            "name": "M",
                            "payload": { "properties": { "p": { "type": "string" } } }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "s": { "$ref": "#/channels/c/publish/message/payload/properties" }
                }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/channels/c/messages/M/payload/properties"),
        );
    }

    #[test]
    fn a_channel_that_names_another_keeps_what_it_says_itself() {
        // 2.6 leaves it undefined which of the two wins, and v3 has no
        // Reference Object that can carry an address — so the two are
        // converted together, and a pointer at either half lands.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "alias": {
                    "$ref": "#/components/channels/shared",
                    "publish": { "message": { "name": "Own", "payload": { "type": "object" } } }
                }
            },
            "components": {
                "channels": {
                    "shared": { "description": "the real one", "deprecated": false }
                },
                "schemas": { "s": { "$ref": "#/channels/alias/publish/message/payload" } }
            }
        }));

        let channel = document.channels["alias"].item().expect("copied in");
        assert_eq!(channel.description.as_deref(), Some("the real one"));
        assert!(channel.messages.contains_key("Own"), "the sibling is kept");
        assert!(document.operations.contains_key("alias_publish"));

        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/channels/alias/messages/Own/payload"),
        );
        assert!(!notes.iter().any(|note| note.contains("no longer names")));
        assert!(!notes.iter().any(|note| note.contains("what sat beside it")));
        document_is_valid(&document);
    }

    #[test]
    fn the_channel_doing_the_naming_stands_where_it_speaks() {
        // Both halves say something in every field. The naming channel
        // is the more particular of the two, so it wins where it
        // speaks; where it is silent the named one is heard. The naming
        // goes through a channel that names a third, which is as far as
        // 2.6 lets one chain go before it stops being followable.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": {
                "mine": { "url": "amqp://mine", "protocol": "amqp" },
                "theirs": { "url": "amqp://theirs", "protocol": "amqp" }
            },
            "channels": {
                "things/{p}/{q}": {
                    "$ref": "#/components/channels/middle",
                    "description": "mine",
                    "servers": ["mine"],
                    "deprecated": true,
                    "bindings": { "amqp": { "is": "queue" } },
                    "parameters": { "p": { "description": "mine" } },
                    "publish": { "message": { "name": "Up" } },
                    "subscribe": { "message": { "name": "Down" } },
                    "x-mine": true
                }
            },
            "components": {
                "channels": {
                    "middle": { "$ref": "#/components/channels/shared" },
                    "shared": {
                        "description": "theirs",
                        "servers": ["theirs"],
                        "deprecated": false,
                        "bindings": { "amqp": { "is": "routingKey" } },
                        "parameters": { "q": { "description": "theirs" } },
                        "publish": { "message": { "name": "Other" } },
                        "x-theirs": true
                    }
                }
            }
        }));

        let key = "things__p___q_";
        let channel = document.channels[key].item().expect("copied in");
        assert_eq!(channel.description.as_deref(), Some("mine"));
        assert_eq!(channel.servers.len(), 1, "the naming channel's server");
        let value = serde_json::to_value(channel).expect("serializable");
        assert_eq!(value["bindings"]["amqp"]["is"], json!("queue"));
        assert!(
            channel.parameters.contains_key("p") && channel.parameters.contains_key("q"),
            "both halves' parameters: {:?}",
            channel.parameters.keys().collect::<Vec<_>>(),
        );
        let extensions = channel.extensions.as_ref().expect("extensions");
        assert!(extensions.contains_key("x-mine") && extensions.contains_key("x-theirs"));
        // The named channel's own `publish` gives way, so `Other` is not
        // carried twice — but `Down` has nothing to give way to.
        assert!(channel.messages.contains_key("Up") && channel.messages.contains_key("Down"));
        assert!(!channel.messages.contains_key("Other"));
        document_is_valid(&document);
    }

    #[test]
    fn a_naming_that_cannot_be_followed_costs_what_it_costs() {
        // Nothing to copy in, so a Reference Object it stays — and a
        // Reference Object is `$ref` alone.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "alias": {
                    "$ref": "./other.yaml#/channels/shared",
                    "publish": { "message": { "name": "Own" } },
                    "parameters": { "p": { "schema": { "type": "string" } } }
                }
            },
            "components": {
                "schemas": {
                    "s": { "$ref": "#/channels/alias/publish/message" },
                    "t": { "$ref": "#/channels/alias/parameters/p/schema" }
                }
            }
        }));
        assert!(matches!(&document.channels["alias"], RefOr::Reference(_)));
        // Its parameters went with it, so there is nothing of theirs to
        // keep anywhere — and no component invented that holds nothing.
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]
                .as_object()
                .expect("schemas")
                .len(),
            2,
            "no schema was invented: {:?}",
            value["components"]["schemas"],
        );
        for expected in [
            "what sat beside it is dropped",
            "address `alias` is lost",
            "`#/channels/alias/publish/message` no longer names the same thing",
        ] {
            assert!(
                notes.iter().any(|note| note.contains(expected)),
                "got: {notes:?}"
            );
        }
    }

    #[test]
    fn what_a_oneof_carried_itself_is_accounted_for() {
        // v3 has no `oneOf` object, so anything on one has nowhere to
        // go — which the report says rather than the conversion
        // swallowing it.
        let (_, notes) = convert_json(minimal(json!({
            "c": {
                "publish": {
                    "message": {
                        "x-selection-policy": "first",
                        "oneOf": [ { "name": "A" } ]
                    }
                }
            }
        })));
        assert!(
            notes.iter().any(|note| note
                == "#.channels.c.publish.message: v3 has nowhere to keep [\"x-selection-policy\"]"),
            "got: {notes:?}"
        );

        // A nested one is named where it sits.
        let (_, notes) = convert_json(minimal(json!({
            "c": {
                "publish": {
                    "message": {
                        "oneOf": [
                            { "name": "A" },
                            { "x-inner": true, "oneOf": [ { "name": "B" } ] }
                        ]
                    }
                }
            }
        })));
        assert!(
            notes.iter().any(|note| note
                == "#.channels.c.publish.message/oneOf/1: v3 has nowhere to keep [\"x-inner\"]"),
            "got: {notes:?}"
        );
    }

    #[test]
    fn a_reusable_channels_parameter_schema_is_kept_too() {
        // The reusable channels are converted after the document's own,
        // so what they set aside has to be gathered after both.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "a/{p}": { "$ref": "#/components/channels/shared" } },
            "components": {
                "channels": {
                    "shared": {
                        "parameters": { "p": { "schema": { "type": "string", "pattern": "^x" } } }
                    }
                },
                "schemas": {
                    "s": { "$ref": "#/components/channels/shared/parameters/p/schema" }
                }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/components/schemas/shared_p"),
        );
        assert_eq!(
            value["components"]["schemas"]["shared_p"],
            json!({ "type": "string", "pattern": "^x" }),
        );
        document_is_valid(&document);
    }

    #[test]
    fn a_reusable_parameters_schema_is_kept_too() {
        // `#/components/parameters/p/schema` is as nameable as a
        // channel parameter's, and v3 has no more room for it there.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "a/{p}": { "parameters": { "p": { "$ref": "#/components/parameters/p" } } } },
            "components": {
                "parameters": { "p": { "schema": { "type": "string", "pattern": "^x" } } },
                "schemas": { "s": { "$ref": "#/components/parameters/p/schema" } }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/components/schemas/parameter_p"),
        );
        assert_eq!(
            value["components"]["schemas"]["parameter_p"],
            json!({ "type": "string", "pattern": "^x" }),
        );
        document_is_valid(&document);
    }

    #[test]
    fn a_pointer_spelled_another_way_names_the_same_schema() {
        // `%7Bp%7D` is `{p}`, and a pointer is read the way the rest of
        // this crate reads one rather than matched as written.
        let (document, _) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "{p}": { "parameters": { "p": { "schema": { "type": "string", "pattern": "^x" } } } }
            },
            "components": {
                "schemas": { "s": { "$ref": "#/channels/%7Bp%7D/parameters/p/schema" } }
            }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["components"]["schemas"]["s"]["$ref"],
            json!("#/components/schemas/_p__p"),
        );
        assert_eq!(
            value["components"]["schemas"]["_p__p"],
            json!({ "type": "string", "pattern": "^x" }),
        );
        document_is_valid(&document);
    }

    #[test]
    fn a_ref_shaped_value_in_an_extension_moves_nothing() {
        // An extension is carried across as it was written, pointer and
        // all, so nothing may be moved on its account: what it names
        // has to still be there, and the parameter keeps the enum a v3
        // parameter can hold.
        let (document, notes) = convert_json(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "channels": {
                "a/{p}": {
                    "parameters": {
                        "p": { "schema": { "type": "string", "enum": ["x", "y"] } }
                    }
                }
            },
            "x-config": { "$ref": "#/channels/a~1{p}/parameters/p/schema" }
        }));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["channels"]["a__p_"]["parameters"]["p"]["enum"],
            json!(["x", "y"]),
        );
        assert_eq!(value["components"], json!(null), "nothing was moved");
        assert_eq!(
            value["x-config"]["$ref"],
            json!("#/channels/a~1{p}/parameters/p/schema"),
            "the extension is left as it was written",
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("`schema` is dropped")),
            "got: {notes:?}"
        );
        document_is_valid(&document);
    }

    #[test]
    fn a_parameter_schema_the_document_names_is_kept() {
        // A v3 parameter is a string and holds no schema, so one that
        // is pointed at is kept where v3 keeps schemas, and the pointer
        // follows it there.
        let (document, notes) = convert_json(minimal(json!({
            "{p}": {
                "parameters": { "p": { "schema": { "type": "string", "pattern": "^x" } } },
                "publish": {
                    "message": {
                        "name": "M",
                        "payload": {
                            "properties": {
                                "id": { "$ref": "#/channels/{p}/parameters/p/schema" }
                            }
                        }
                    }
                }
            }
        })));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["channels"]["_p_"]["messages"]["M"]["payload"]["properties"]["id"]["$ref"],
            json!("#/components/schemas/_p__p"),
        );
        assert_eq!(
            value["components"]["schemas"]["_p__p"],
            json!({ "type": "string", "pattern": "^x" }),
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("`schema` was kept at `#/components/schemas/_p__p`")),
            "got: {notes:?}"
        );
        document_is_valid(&document);

        // A pointer *into* one follows it too.
        let (document, _) = convert_json(minimal(json!({
            "{p}": {
                "parameters": {
                    "p": { "schema": { "properties": { "inner": { "type": "string" } } } }
                },
                "publish": {
                    "message": {
                        "name": "M",
                        "payload": {
                            "$ref": "#/channels/{p}/parameters/p/schema/properties/inner"
                        }
                    }
                }
            }
        })));
        let value = serde_json::to_value(&document).expect("serializable");
        assert_eq!(
            value["channels"]["_p_"]["messages"]["M"]["payload"]["$ref"],
            json!("#/components/schemas/_p__p/properties/inner"),
        );
        document_is_valid(&document);
    }

    #[test]
    fn a_parameter_schema_nothing_names_is_not_kept() {
        // Keeping every one of them would bulk out documents that never
        // name a single one.
        let (document, notes) = convert_json(minimal(json!({
            "{p}": { "parameters": { "p": { "schema": { "type": "string" } } } }
        })));
        assert!(document.components.is_none());
        assert!(
            notes
                .iter()
                .any(|note| note.contains("so `schema` is dropped")),
            "got: {notes:?}"
        );
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
