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
//! - `v2_6` — AsyncAPI v2.6 document model + `Validate` impls, behind
//!   the `v2_6` feature.
//! - `v3_0` — AsyncAPI v3.0 document model + `Validate` impls, behind
//!   the `v3_0` feature.
//! - `v3_1` — AsyncAPI v3.1 document model + `Validate` impls, behind
//!   the `v3_1` feature (on by default).
//!
//! The version modules are named rather than linked above: a link to a
//! module the current feature set switched off is a broken intra-doc
//! link, which `cargo doc` reports and `RUSTDOCFLAGS="-D warnings"`
//! fails on. docs.rs builds this crate with every feature, so both
//! appear in the sidebar there.
//!
//! ## Parsing and validating
//!
//! ```rust
//! # // Gate the example on the v3_1 feature so it stays valid under any
//! # // feature combination. The hidden cfg block is removed entirely
//! # // when v3_1 is off, so the doctest compiles to an empty
//! # // `fn main()` in that case.
//! # #[cfg(feature = "v3_1")] {
//! use enumset::EnumSet;
//! use roas_asyncapi::v3_1::Document;
//! use roas_asyncapi::validation::Validate;
//!
//! // Parse an AsyncAPI document (JSON or YAML).
//! let doc: Document = serde_json::from_str(r##"{
//!     "asyncapi": "3.1.0",
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
//! any other YAML crate) into a version module's `Document`.
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
//! v2.6.0 (`v2_6`), v3.0.0 (`v3_0`), and v3.1.0 (`v3_1`, the default
//! feature) are all implemented; enable whichever you need. 2.6 is a
//! different document rather than an earlier draft of the same one —
//! channels keyed by path, `publish` / `subscribe` operations, and
//! parameters carrying full schemas. Each version's schema pins
//! its `asyncapi` field to exactly that string, so a document is parsed
//! by one module or rejected. With both features enabled, an
//! `impl From<v3_0::Document> for v3_1::Document` is available for
//! upconverting a 3.0 document — v3.1 left the object model untouched,
//! so nothing is dropped or approximated. A 2.6 → 3.0 conversion, which
//! has genuinely lossy cases, follows.

pub mod common;
pub mod validation;

#[cfg(feature = "v2_6")]
pub mod v2_6;

#[cfg(feature = "v3_0")]
pub mod v3_0;

#[cfg(feature = "v3_1")]
pub mod v3_1;
