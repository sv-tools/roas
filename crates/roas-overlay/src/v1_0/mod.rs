//! OpenAPI Overlay v1.0 — see
//! <https://spec.openapis.org/overlay/v1.0.0.html>.
//!
//! Authoritative JSON Schema:
//! <https://spec.openapis.org/overlay/1.0/schema/2026-04-01>.

pub mod action;
pub mod info;
pub mod overlay;
pub mod version;

pub use action::Action;
pub use info::Info;
pub use overlay::Overlay;
pub use version::Version;
