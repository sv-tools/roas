//! Arazzo v1.1 — see
//! <https://spec.openapis.org/arazzo/v1.1.0.html>.
//!
//! Authoritative JSON Schema:
//! <https://spec.openapis.org/arazzo/1.1/schema/2026-04-15>.
//!
//! Adds over v1.0: a root `$self`, AsyncAPI steps (`channelPath` /
//! `action` / `correlationId`), step `timeout` / `dependsOn`, the
//! [`Selector`] subsystem (used by parameter / output / replacement
//! values), `ExpressionType`-based criterion and selector types, action
//! `parameters`, and the `asyncapi` / `querystring` / `channel` enum
//! values. With the `v1_0` feature also enabled, an
//! `impl From<v1_0::Description> for Description` upconverts a v1.0
//! description.

pub mod components;
pub mod criterion;
pub mod description;
pub mod expression_type;
pub mod failure_action;
pub mod info;
pub mod parameter;
pub mod request_body;
pub mod selector;
pub mod source_description;
pub mod step;
pub mod success_action;
pub mod version;
pub mod workflow;

#[cfg(feature = "v1_0")]
mod from_v1_0;

pub use crate::common::reusable::{Reusable, ReusableOr};
pub use components::Components;
pub use criterion::{Criterion, CriterionKind, CriterionType};
pub use description::Description;
pub use expression_type::{ExpressionKind, ExpressionType};
pub use failure_action::{FailureAction, FailureActionType};
pub use info::Info;
pub use parameter::{Parameter, ParameterLocation};
pub use request_body::{PayloadReplacement, RequestBody};
pub use selector::{Selector, SelectorKind, SelectorType, ValueOrSelector};
pub use source_description::{SourceDescription, SourceType};
pub use step::{Step, StepAction};
pub use success_action::{SuccessAction, SuccessActionType};
pub use version::Version;
pub use workflow::Workflow;
