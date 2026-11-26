//! Arazzo v1.0 `Failure Action` object.
//!
//! Per [Failure Action Object](https://spec.openapis.org/arazzo/v1.0.1.html#failure-action-object):
//! what to do when a step fails — end, `goto`, or `retry`.

use crate::v1_0::criterion::Criterion;
use crate::v1_0::success_action::validate_goto_target;
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
        // Reject negatives, NaN, and infinities (the schema requires finite `minimum: 0`).
        if let Some(n) = self.retry_after {
            if n < 0.0 || n.is_nan() {
                ctx.error_field("retryAfter", "must not be negative");
            } else if !n.is_finite() {
                ctx.error_field("retryAfter", "must be finite");
            }
        }
        for (i, criterion) in self.criteria.iter().enumerate() {
            ctx.in_index("criteria", i, |ctx| criterion.validate_with_context(ctx));
        }
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
            "name": "retryStep",
            "type": "retry",
            "retryAfter": 1.5,
            "retryLimit": 3,
        }))
        .unwrap();
        assert_eq!(a.type_, FailureActionType::Retry);
        assert_eq!(a.retry_after, Some(1.5));
        assert_eq!(a.retry_limit, Some(3));
        assert!(validate(&a).is_empty());
    }

    #[test]
    fn goto_requires_exactly_one_target() {
        let neither = FailureAction {
            name: "g".into(),
            type_: FailureActionType::Goto,
            ..Default::default()
        };
        assert!(validate(&neither).iter().any(|e| e.contains("goto")));

        let ok = FailureAction {
            step_id: Some("s".into()),
            ..neither
        };
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn negative_retry_limit_is_rejected_on_parse() {
        // `retryLimit` is a u64 at the type level, so a negative value
        // fails to deserialize outright.
        let err = serde_json::from_value::<FailureAction>(
            json!({ "name": "n", "type": "retry", "retryLimit": -1 }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("u64") || err.to_string().contains("invalid"));
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

        let goto_retry_fields = FailureAction {
            name: "n".into(),
            type_: FailureActionType::Goto,
            step_id: Some("s".into()),
            retry_after: Some(1.0),
            retry_limit: Some(2),
            ..Default::default()
        };
        let errs = validate(&goto_retry_fields);
        assert!(
            errs.iter()
                .any(|e| e == "#.a.retryAfter: is only valid when `type` is `retry`")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.a.retryLimit: is only valid when `type` is `retry`")
        );
    }
}
