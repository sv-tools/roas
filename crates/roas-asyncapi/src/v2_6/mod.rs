//! AsyncAPI v2.6 — see
//! <https://www.asyncapi.com/docs/reference/specification/v2.6.0>.
//!
//! Authoritative JSON Schema:
//! <https://asyncapi.com/schema-store/2.6.0.json>.
//!
//! This is the pre-v3 model, and it is a different document rather than
//! an earlier draft of the same one:
//!
//! - `channels` is required and keyed by the channel *path*, which
//!   carries the `{parameter}` placeholders. v3 gave a channel its own
//!   key plus a separate `address`.
//! - Operations live under a channel as `publish` / `subscribe`, named
//!   from the *consumer's* point of view. v3 hoisted them to a
//!   top-level map and inverted the naming to `send` / `receive`, from
//!   the application's own point of view.
//! - An operation's `message` may be a set of alternatives
//!   (`{ "oneOf": [...] }`); v3 replaced that with the channel's
//!   `messages` map.
//! - A message declares its payload dialect with `schemaFormat` and
//!   carries the payload directly, where v3 wraps both in a Multi
//!   Format Schema Object.
//! - A parameter carries a full `Schema`; v3 parameters are strings
//!   constrained by `enum` / `default` / `examples`.
//! - `security` is a list of OpenAPI-style requirement maps (name →
//!   scopes) rather than inline schemes, and an OAuth flow's scope map
//!   is spelled `scopes`, which v3 renamed to `availableScopes`.
//! - `tags` and `externalDocs` sit at the document root; v3 moved them
//!   under `info`.
//! - A server is addressed by a single `url`, which v3 split into
//!   `host` + `pathname`.
//!
//! Protocol bindings are held as raw JSON — see
//! [`Bindings`] for why.

pub mod channel_item;
pub mod components;
pub mod correlation_id;
pub mod document;
pub mod external_documentation;
pub mod info;
pub mod message;
pub mod operation;
pub mod parameter;
pub mod schema;
pub mod security_scheme;
pub mod server;
pub mod tag;
pub mod version;

pub use crate::common::bindings::Bindings;
pub use crate::common::reference::{RefOr, Reference};
pub use channel_item::ChannelItem;
pub use components::Components;
pub use correlation_id::CorrelationId;
pub use document::Document;
pub use external_documentation::ExternalDocumentation;
pub use info::{Contact, Info, License};
pub use message::{Message, MessageExample, MessageOneOf, MessageTrait, OperationMessage};
pub use operation::{Operation, OperationKind, OperationTrait};
pub use parameter::Parameter;
pub use schema::{Dependency, Items, Schema, SchemaType, SubSchema};
pub use security_scheme::{
    ApiKeyLocation, OAuthFlow, OAuthFlows, SecurityRequirement, SecurityScheme, SecuritySchemeType,
};
pub use server::{Server, ServerVariable};
pub use tag::Tag;
pub use version::Version;

// Every modelled type says what kind of object it is, so a reference to
// one can be judged wherever the model holds it.
crate::common::resolve::kinds! {
    server::Server => Some("servers"),
    server::ServerVariable => Some("serverVariables"),
    message::Message => Some("messages"),
    message::MessageTrait => Some("messageTraits"),
    operation::OperationTrait => Some("operationTraits"),
    security_scheme::SecurityScheme => Some("securitySchemes"),
    parameter::Parameter => Some("parameters"),
    correlation_id::CorrelationId => Some("correlationIds"),
}
