//! Arazzo v1.0 `Failure Action` object.
//!
//! Per [Failure Action Object](https://spec.openapis.org/arazzo/v1.0.1.html#failure-action-object):
//! what to do when a step fails — end, `goto`, or `retry`.

use crate::v1_0::criterion::Criterion;
use crate::v1_0::success_action::validate_goto_target;
use crate::validation::{Context, ValidateWithContext, validate_required_string};
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
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        validate_goto_target(
            self.type_ == FailureActionType::Goto,
            self.workflow_id.is_some(),
            self.step_id.is_some(),
            ctx,
            &path,
        );
        if self.retry_after.is_some_and(|n| n < 0.0) {
            ctx.error(format!("{path}.retryAfter"), "must not be negative");
        }
        for (i, criterion) in self.criteria.iter().enumerate() {
            criterion.validate_with_context(ctx, format!("{path}.criteria[{i}]"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(a: &FailureAction) -> Vec<String> {
        let mut ctx = Context::new(EnumSet::empty());
        a.validate_with_context(&mut ctx, "#.a".into());
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
}
