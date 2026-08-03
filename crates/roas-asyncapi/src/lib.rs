//! AsyncAPI Specification — parser and validator.
//!
//! Implements the [AsyncAPI Specification](https://www.asyncapi.com/docs/reference/specification/v3.0.0):
//! a document format describing event-driven APIs — the channels an
//! application publishes to and consumes from, the messages that travel
//! over them, and the servers that carry them.
//!
//! ## Modules
//!
//! - [`common`] — version-agnostic helpers: the `x-` extensions serde
//!   helper, the `$ref` wrapper, untyped protocol bindings, and the
//!   runtime-expression grammar.
//! - [`validation`] — [`Validate`](validation::Validate) trait,
//!   [`ValidationOptions`](validation::ValidationOptions) flag set,
//!   `Context` / `ValidationError` types.
//! - [`v3_0`] — AsyncAPI v3.0 document model + `Validate` impls.
//!
//! ## Parsing and validating
//!
//! ```rust
//! # // Gate the example on the v3_0 feature so it stays valid under any
//! # // feature combination. The hidden cfg block is removed entirely
//! # // when v3_0 is off, so the doctest compiles to an empty
//! # // `fn main()` in that case.
//! # #[cfg(feature = "v3_0")] {
//! use enumset::EnumSet;
//! use roas_asyncapi::v3_0::Document;
//! use roas_asyncapi::validation::Validate;
//!
//! // Parse an AsyncAPI document (JSON or YAML).
//! let doc: Document = serde_json::from_str(r##"{
//!     "asyncapi": "3.0.0",
//!     "info": { "title": "Streetlights", "version": "1.0.0" },
//!     "servers": {
//!         "production": { "host": "broker.example.com:9092", "protocol": "kafka" }
//!     },
//!     "channels": {
//!         "lightMeasured": {
//!             "address": "smartylighting/streetlights/{streetlightId}/lighting/measured",
//!             "parameters": { "streetlightId": { "description": "The streetlight id" } },
//!             "messages": { "lightMeasured": { "name": "LightMeasured" } }
//!         }
//!     },
//!     "operations": {
//!         "receiveLightMeasurement": {
//!             "action": "receive",
//!             "channel": { "$ref": "#/channels/lightMeasured" },
//!             "messages": [ { "$ref": "#/channels/lightMeasured/messages/lightMeasured" } ]
//!         }
//!     }
//! }"##).unwrap();
//!
//! doc.validate(EnumSet::empty()).expect("document is well-formed");
//! assert_eq!(doc.channels.len(), 1);
//! # }
//! ```
//!
//! YAML documents work the same way — parse with `serde_yaml_ng` (or
//! any other YAML crate) into [`v3_0::Document`].
//!
//! ## Scope
//!
//! The document model and its validators are the whole surface: this
//! crate does not resolve `$ref`s across files, apply message /
//! operation traits, or type protocol bindings. Cross-reference checks
//! therefore run on document-local pointers only — see
//! [`ValidationOptions`](validation::ValidationOptions) to require a
//! self-contained document instead.
//!
//! ## Versions
//!
//! v3.0.x ([`v3_0`], default feature) is implemented; v3.1 and 2.6
//! follow.

pub mod common;
pub mod validation;

#[cfg(feature = "v3_0")]
pub mod v3_0;
