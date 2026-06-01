//! OpenAPI Arazzo Specification — parser and validator.
//!
//! Implements the [Arazzo Specification](https://spec.openapis.org/arazzo/v1.0.1.html):
//! a document format that describes sequences of API calls (*workflows*)
//! and their dependencies, independent of the underlying OpenAPI
//! descriptions they orchestrate.
//!
//! ## Modules
//!
//! - [`common`] — version-agnostic helpers: the `x-` extensions serde
//!   helper.
//! - [`validation`] — [`Validate`](validation::Validate) trait,
//!   [`ValidationOptions`](validation::ValidationOptions) flag set,
//!   `Context` / `ValidationError` types.
//! - [`v1_0`] — Arazzo v1.0 document model + `Validate` impls.
//!
//! ## Parsing and validating
//!
//! ```rust
//! # // Gate the example on the v1_0 feature so it stays valid under any
//! # // feature combination (e.g. `--no-default-features --features v1_1`).
//! # // The hidden cfg block is removed entirely when v1_0 is off, so the
//! # // doctest compiles to an empty `fn main()` in that case.
//! # #[cfg(feature = "v1_0")] {
//! use enumset::EnumSet;
//! use roas_arazzo::v1_0::Description;
//! use roas_arazzo::validation::Validate;
//!
//! // Parse an Arazzo description (JSON or YAML).
//! let doc: Description = serde_json::from_str(r#"{
//!     "arazzo": "1.0.1",
//!     "info": { "title": "Example", "version": "1.0.0" },
//!     "sourceDescriptions": [
//!         { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
//!     ],
//!     "workflows": [
//!         {
//!             "workflowId": "getPet",
//!             "steps": [
//!                 {
//!                     "stepId": "findPet",
//!                     "operationId": "getPetById",
//!                     "successCriteria": [ { "condition": "$statusCode == 200" } ]
//!                 }
//!             ]
//!         }
//!     ]
//! }"#).unwrap();
//!
//! doc.validate(EnumSet::empty()).expect("description is well-formed");
//! assert_eq!(doc.workflows[0].workflow_id, "getPet");
//! # }
//! ```
//!
//! YAML descriptions work the same way — parse with `serde_yaml_ng` (or
//! any other YAML crate) into [`v1_0::Description`].
//!
//! ## Versions
//!
//! v1.0.x ([`v1_0`], default feature) and v1.1.x ([`v1_1`]) are both
//! implemented; enable whichever you need. With both features enabled,
//! an `impl From<v1_0::Description> for v1_1::Description` is available
//! for upconverting an existing v1.0 description.

pub mod common;
pub mod validation;

#[cfg(feature = "v1_0")]
pub mod v1_0;

#[cfg(feature = "v1_1")]
pub mod v1_1;
