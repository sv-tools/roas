//! AsyncAPI v3.1 — see
//! <https://www.asyncapi.com/docs/reference/specification/v3.1.0>.
//!
//! Authoritative JSON Schema:
//! <https://asyncapi.com/schema-store/3.1.0.json>.
//!
//! v3 reshaped the document against 2.x: channels carry an `address`
//! and the `messages` that travel over them, operations move to a
//! top-level map that `$ref`s a channel, and `publish` / `subscribe`
//! become `send` / `receive` stated from the application's point of
//! view.
//!
//! v3.1 changes almost nothing in that object model — the only
//! additions are this version's own `schemaFormat` media types and the
//! `ros2` protocol bindings, which cost nothing here because bindings
//! are untyped. Its schema even `$ref`s the 3.0.0-namespaced Schema and
//! Reference definitions. With the `v3_0` feature also enabled, an
//! `impl From<v3_0::Document> for Document` upconverts a 3.0 document.
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

#[cfg(feature = "v3_0")]
mod from_v3_0;

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
