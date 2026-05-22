//! OpenAPI Overlay v1.1 — see
//! <https://spec.openapis.org/overlay/v1.1.0.html>.
//!
//! Authoritative JSON Schema:
//! <https://spec.openapis.org/overlay/1.1/schema/2026-04-01>.
//!
//! Additive over v1.0:
//! - `Info` gains an optional `description` field.
//! - `Action` gains an optional `copy` field — a JSONPath selecting a
//!   single source node in the working document whose value is then
//!   merged into each `target` node (mutually exclusive with `update`
//!   and `remove: true`).
//! - `overlay` version regex becomes `^1\.1\.\d+$`.

pub mod action;
pub mod info;
pub mod overlay;
pub mod version;

pub use action::Action;
pub use info::Info;
pub use overlay::Overlay;
pub use version::Version;
