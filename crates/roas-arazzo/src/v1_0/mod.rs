//! Arazzo v1.0 — see
//! <https://spec.openapis.org/arazzo/v1.0.1.html>.
//!
//! Authoritative JSON Schema:
//! <https://spec.openapis.org/arazzo/1.0/schema/2025-10-15>.

pub mod components;
pub mod criterion;
pub mod description;
pub mod failure_action;
pub mod info;
pub mod parameter;
pub mod request_body;
pub mod source_description;
pub mod step;
pub mod success_action;
pub mod version;
pub mod workflow;

pub use crate::common::reusable::{Reusable, ReusableOr};
pub use components::Components;
pub use criterion::{Criterion, CriterionType};
pub use description::Description;
pub use failure_action::{FailureAction, FailureActionType};
pub use info::Info;
pub use parameter::{Parameter, ParameterLocation};
pub use request_body::{PayloadReplacement, RequestBody};
pub use source_description::{SourceDescription, SourceType};
pub use step::Step;
pub use success_action::{SuccessAction, SuccessActionType};
pub use version::Version;
pub use workflow::Workflow;
