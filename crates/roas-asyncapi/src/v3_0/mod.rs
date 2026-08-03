//! AsyncAPI v3.0 — see
//! <https://www.asyncapi.com/docs/reference/specification/v3.0.0>.
//!
//! Authoritative JSON Schema:
//! <https://asyncapi.com/schema-store/3.0.0.json>.
//!
//! v3 reshaped the document against 2.x: channels carry an `address`
//! and the `messages` that travel over them, operations move to a
//! top-level map that `$ref`s a channel, and `publish` / `subscribe`
//! become `send` / `receive` stated from the application's point of
//! view.
//!
//! Protocol bindings are held as raw JSON — see
//! [`Bindings`] for why.

pub mod channel;
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
pub use channel::Channel;
pub use components::Components;
pub use correlation_id::CorrelationId;
pub use document::Document;
pub use external_documentation::ExternalDocumentation;
pub use info::{Contact, Info, License};
pub use message::{Message, MessageExample, MessageTrait};
pub use operation::{
    Operation, OperationAction, OperationReply, OperationReplyAddress, OperationTrait,
};
pub use parameter::Parameter;
pub use schema::{
    Dependency, Items, MultiFormatSchema, SUPPORTED_SCHEMA_FORMATS, Schema, SchemaOrMultiFormat,
    SchemaType, SubSchema, is_supported_schema_format,
};
pub use security_scheme::{
    ApiKeyLocation, OAuthFlow, OAuthFlows, SecurityScheme, SecuritySchemeType,
};
pub use server::{Server, ServerVariable};
pub use tag::Tag;
pub use version::Version;
