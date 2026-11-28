//! Arazzo v1.1 `Failure Action` object.
//!
//! Per [Failure Action Object](https://spec.openapis.org/arazzo/v1.1.0.html#failure-action-object).
//! New in v1.1: `parameters` (requires `workflowId` when present).

use crate::common::reusable::ReusableOr;
use crate::v1_1::criterion::Criterion;
use crate::v1_1::parameter::Parameter;
use crate::v1_1::success_action::validate_goto_target;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The action taken on step failure.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FailureActionType {
    #[default]
    End,
    Goto,
    Retry,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct FailureAction {
    /// **Required** The name of the failure action.
    pub name: String,

    /// **Required** The type of action to take.
    #[serde(rename = "type")]
    pub type_: FailureActionType,

    /// The workflow to transfer to (required for `goto`, mutually
    /// exclusive with `stepId`).
    #[serde(rename = "workflowId", skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,

    /// The step to transfer to (required for `goto`, mutually exclusive
    /// with `workflowId`).
    #[serde(rename = "stepId", skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,

    /// Parameters passed to the workflow referenced by `workflowId`.
    /// Added in v1.1; requires `workflowId`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ReusableOr<Parameter>>,

    /// Non-negative seconds to wait before retrying.
    #[serde(rename = "retryAfter", skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<f64>,

    /// Non-negative number of retry attempts.
    #[serde(rename = "retryLimit", skip_serializing_if = "Option::is_none")]
    pub retry_limit: Option<u64>,

    /// Assertions determining whether this action runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<Criterion>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for FailureAction {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("name", &self.name);
        if let Some(wf) = &self.workflow_id {
            ctx.require_non_empty("workflowId", wf);
        }
        if let Some(step) = &self.step_id {
            ctx.require_non_empty("stepId", step);
        }
        validate_goto_target(
            self.type_ == FailureActionType::Goto,
            self.workflow_id.is_some(),
            self.step_id.is_some(),
            ctx,
        );
        match self.type_ {
            FailureActionType::End => {
                if self.workflow_id.is_some() {
                    ctx.error_field("workflowId", "is not valid when `type` is `end`");
                }
                if self.step_id.is_some() {
                    ctx.error_field("stepId", "is not valid when `type` is `end`");
                }
            }
            FailureActionType::Retry => {
                if self.workflow_id.is_some() && self.step_id.is_some() {
                    ctx.error("`workflowId` and `stepId` are mutually exclusive");
                }
            }
            FailureActionType::Goto => {}
        }
        if self.type_ != FailureActionType::Retry {
            if self.retry_after.is_some() {
                ctx.error_field("retryAfter", "is only valid when `type` is `retry`");
            }
            if self.retry_limit.is_some() {
                ctx.error_field("retryLimit", "is only valid when `type` is `retry`");
            }
        }
        if !self.parameters.is_empty() && self.workflow_id.is_none() {
            ctx.error_field("parameters", "are only valid when `workflowId` is set");
        }
        // Reject negatives, NaN, and infinities (the schema requires finite `minimum: 0`).
        if let Some(n) = self.retry_after {
            if n < 0.0 || n.is_nan() {
                ctx.error_field("retryAfter", "must not be negative");
            } else if !n.is_finite() {
                ctx.error_field("retryAfter", "must be finite");
            }
        }
        for (i, parameter) in self.parameters.iter().enumerate() {
            ctx.in_index("parameters", i, |ctx| parameter.validate_with_context(ctx));
        }
        for (i, criterion) in self.criteria.iter().enumerate() {
            ctx.in_index("criteria", i, |ctx| criterion.validate_with_context(ctx));
        }
        ctx.validate_unknown_fields(&self.extensions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(a: &FailureAction) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.a");
        a.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn retry_action_round_trips() {
        let a: FailureAction = serde_json::from_value(json!({
            "name": "retryStep", "type": "retry", "retryAfter": 1.5, "retryLimit": 3,
        }))
        .unwrap();
        assert_eq!(a.type_, FailureActionType::Retry);
        assert!(validate(&a).is_empty());
    }

    #[test]
    fn parameters_require_workflow_id() {
        let a = FailureAction {
            name: "g".into(),
            type_: FailureActionType::Goto,
            step_id: Some("next".into()),
            parameters: vec![ReusableOr::Reusable(Default::default())],
            ..Default::default()
        };
        assert!(
            validate(&a)
                .iter()
                .any(|e| e == "#.a.parameters: are only valid when `workflowId` is set")
        );
    }

    #[test]
    fn negative_retry_after_fails_validation() {
        let a = FailureAction {
            name: "n".into(),
            type_: FailureActionType::Retry,
            retry_after: Some(-1.0),
            ..Default::default()
        };
        assert!(
            validate(&a)
                .iter()
                .any(|e| e == "#.a.retryAfter: must not be negative")
        );
    }

    #[test]
    fn infinite_retry_after_fails_validation() {
        let a = FailureAction {
            name: "n".into(),
            type_: FailureActionType::Retry,
            retry_after: Some(f64::INFINITY),
            ..Default::default()
        };
        assert!(
            validate(&a)
                .iter()
                .any(|e| e == "#.a.retryAfter: must be finite")
        );
    }

    #[test]
    fn nan_retry_after_fails_validation() {
        let a = FailureAction {
            name: "n".into(),
            type_: FailureActionType::Retry,
            retry_after: Some(f64::NAN),
            ..Default::default()
        };
        assert!(
            validate(&a)
                .iter()
                .any(|e| e == "#.a.retryAfter: must not be negative")
        );
    }

    #[test]
    fn criteria_are_recursed() {
        let a = FailureAction {
            name: "n".into(),
            criteria: vec![Criterion::default()],
            ..Default::default()
        };
        assert!(
            validate(&a)
                .iter()
                .any(|e| e == "#.a.criteria[0].condition: must not be empty")
        );
    }

    #[test]
    fn cross_type_fields_and_empty_targets_are_rejected() {
        let end_bad = FailureAction {
            name: "n".into(),
            type_: FailureActionType::End,
            workflow_id: Some(String::new()),
            step_id: Some(String::new()),
            retry_after: Some(1.0),
            retry_limit: Some(2),
            ..Default::default()
        };
        let errs = validate(&end_bad);
        assert!(
            errs.iter()
                .any(|e| e == "#.a.workflowId: must not be empty")
        );
        assert!(errs.iter().any(|e| e == "#.a.stepId: must not be empty"));
        assert!(
            errs.iter()
                .any(|e| e == "#.a.workflowId: is not valid when `type` is `end`")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.a.stepId: is not valid when `type` is `end`")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.a.retryAfter: is only valid when `type` is `retry`")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.a.retryLimit: is only valid when `type` is `retry`")
        );

        let retry_both = FailureAction {
            name: "n".into(),
            type_: FailureActionType::Retry,
            workflow_id: Some("w".into()),
            step_id: Some("s".into()),
            ..Default::default()
        };
        assert!(
            validate(&retry_both)
                .iter()
                .any(|e| e == "#.a: `workflowId` and `stepId` are mutually exclusive")
        );
    }
}
