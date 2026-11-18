//! OpenAPI Overlay v1.2 — see
//! <https://spec.openapis.org/overlay/v1.2.0.html>.
//!
//! Authoritative JSON Schema:
//! <https://spec.openapis.org/overlay/1.2/schema/2026-08-08>.
//!
//! Over v1.1:
//! - `components.actions` holds reusable actions, and an `actions`
//!   entry may be a [`ReusableActionReference`] to one (`$ref` plus its
//!   own `target`). [`Apply`](crate::apply::Apply) resolves each
//!   reference into a plain [`Action`] before applying it.
//! - The root gains `$self`, a URI reference identifying the overlay;
//!   neither it nor `extends` may contain a fragment.
//! - An action's `target` may select primitive nodes: `update` and
//!   `copy` replace them with a primitive value, `remove` removes them.
//! - `overlay` version regex becomes `^1\.2\.\d+$`.

pub mod action;
pub mod components;
pub mod info;
pub mod overlay;
pub mod version;

pub use action::{Action, ActionItem, ReusableActionReference};
pub use components::{Components, ReusableAction, ReusableActionFields};
pub use info::Info;
pub use overlay::Overlay;
pub use version::Version;
