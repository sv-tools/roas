//! OpenAPI Overlay Specification — parser, validator, and applier.
//!
//! Implements the OpenAPI Overlay Specification
//! ([v1.0](https://spec.openapis.org/overlay/v1.0.0.html) /
//! [v1.1](https://spec.openapis.org/overlay/v1.1.0.html)):
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
//! - [`v1_1`] — Overlay v1.1 document model + `Validate` / `Apply` impls.
//!
//! ## Applying an overlay
//!
//! ```no_run
//! # // Gate the example on the v1_1 feature so it stays valid under any
//! # // feature combination (e.g. `--no-default-features --features v1_0`).
//! # // The hidden cfg block is removed entirely when v1_1 is off, so the
//! # // doctest compiles to an empty `fn main()` in that case.
//! # #[cfg(feature = "v1_1")] {
//! use enumset::EnumSet;
//! use roas_overlay::apply::Apply;
//! use roas_overlay::v1_1::Overlay;
//!
//! // Parse the overlay document (JSON or YAML).
//! let overlay: Overlay = serde_json::from_str(r#"{
//!     "overlay": "1.1.0",
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
//! # }
//! ```
//!
//! ## Versions
//!
//! v1.0.x ([`v1_0`]) and v1.1.x ([`v1_1`], default feature) are both
//! implemented; enable whichever you need. With both features enabled,
//! an `impl From<v1_0::Overlay> for v1_1::Overlay` is available for
//! upconverting an existing v1.0 document.

// Everything internal here is reached through a versioned
// document: the validation impls, the walkers, the shared
// helpers. Build with no version feature at all and none of it
// is called — which is that configuration saying so, not code
// nobody uses. Every real configuration keeps the lint.
#![cfg_attr(not(any(feature = "v1_0", feature = "v1_1")), allow(dead_code))]

pub mod apply;
pub mod common;
pub mod validation;

#[cfg(feature = "v1_0")]
pub mod v1_0;

#[cfg(feature = "v1_1")]
pub mod v1_1;
