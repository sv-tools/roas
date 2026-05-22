//! OpenAPI Overlay Specification — parser, validator, and applier.
//!
//! Implements the [OpenAPI Overlay Specification](https://spec.openapis.org/overlay/v1.0.0.html):
//! a sidecar document format that transforms OpenAPI documents through
//! an ordered list of [JSONPath](https://www.rfc-editor.org/rfc/rfc9535)
//! actions (`update`, `remove`, and v1.1's `copy`).
//!
//! ## Modules
//!
//! - [`common`] — version-agnostic helpers: `x-` extensions serde
//!   helpers, RFC 9535 JSONPath wrapper, the
//!   [§4.4.3.1](https://spec.openapis.org/overlay/v1.0.0.html#merging-rules)
//!   recursive merge.
//! - [`validation`] — [`Validate`](validation::Validate) trait,
//!   [`ValidationOptions`](validation::ValidationOptions) flag set,
//!   `Context` / `ValidationError` types.
//! - [`apply`] — [`Apply`](apply::Apply) trait, [`ApplyOptions`](apply::ApplyOptions),
//!   [`ApplyReport`](apply::ApplyReport), [`ApplyError`](apply::ApplyError).
//! - [`v1_0`] — Overlay v1.0 document model + `Validate` / `Apply` impls.
//!
//! ## Applying an overlay
//!
//! ```no_run
//! use enumset::EnumSet;
//! use roas_overlay::apply::Apply;
//! use roas_overlay::v1_0::Overlay;
//!
//! // Parse the overlay document (JSON or YAML).
//! let overlay: Overlay = serde_json::from_str(r#"{
//!     "overlay": "1.0.0",
//!     "info": { "title": "Example", "version": "1.0.0" },
//!     "actions": [
//!         { "target": "$.info", "update": { "description": "Patched." } }
//!     ]
//! }"#).unwrap();
//!
//! // Parse the target OpenAPI document as untyped JSON.
//! let mut target: serde_json::Value = serde_json::from_str(r#"{
//!     "openapi": "3.1.0",
//!     "info": { "title": "API", "version": "1.0.0" },
//!     "paths": {}
//! }"#).unwrap();
//!
//! // Apply the overlay in-place.
//! let report = overlay.apply(&mut target, EnumSet::empty()).unwrap();
//! assert_eq!(report.actions.len(), 1);
//! assert_eq!(target["info"]["description"], "Patched.");
//! ```

pub mod apply;
pub mod common;
pub mod validation;

#[cfg(feature = "v1_0")]
pub mod v1_0;

#[cfg(feature = "v1_1")]
pub mod v1_1;
