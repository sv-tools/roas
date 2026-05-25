//! Arazzo v1.0 `Success Action` object.
//!
//! Per [Success Action Object](https://spec.openapis.org/arazzo/v1.0.1.html#success-action-object):
//! what to do when a step succeeds — end the workflow or `goto` another
//! step / workflow.

use crate::v1_0::criterion::Criterion;
use crate::validation::{Context, ValidateWithContext, validate_required_string};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The action taken on step success.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SuccessActionType {
    #[default]
    End,
    Goto,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct SuccessAction {
    /// **Required** The name of the success action.
    pub name: String,

    /// **Required** The type of action to take.
    #[serde(rename = "type")]
    pub type_: SuccessActionType,

    /// The workflow to transfer to (required for `goto`, mutually
    /// exclusive with `stepId`).
    #[serde(rename = "workflowId", skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,

    /// The step to transfer to (required for `goto`, mutually exclusive
    /// with `workflowId`).
    #[serde(rename = "stepId", skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,

    /// Assertions determining whether this action runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<Criterion>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for SuccessAction {
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        validate_goto_target(
            self.type_ == SuccessActionType::Goto,
            self.workflow_id.is_some(),
            self.step_id.is_some(),
            ctx,
            &path,
        );
        for (i, criterion) in self.criteria.iter().enumerate() {
            criterion.validate_with_context(ctx, format!("{path}.criteria[{i}]"));
        }
    }
}

/// Shared `goto`-target rule used by both success and failure actions:
/// a `goto` requires exactly one of `workflowId` / `stepId`.
pub(crate) fn validate_goto_target(
    is_goto: bool,
    has_workflow_id: bool,
    has_step_id: bool,
    ctx: &mut Context,
    path: &str,
) {
    if is_goto && !(has_workflow_id ^ has_step_id) {
        ctx.error(
            path.to_owned(),
            "`goto` requires exactly one of `workflowId` or `stepId`",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(a: &SuccessAction) -> Vec<String> {
        let mut ctx = Context::new(EnumSet::empty());
        a.validate_with_context(&mut ctx, "#.a".into());
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn end_action_round_trips() {
        let a: SuccessAction =
            serde_json::from_value(json!({ "name": "done", "type": "end" })).unwrap();
        assert_eq!(a.type_, SuccessActionType::End);
        assert!(validate(&a).is_empty());

        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v, json!({ "name": "done", "type": "end" }));
    }

    #[test]
    fn goto_requires_exactly_one_target() {
        let neither = SuccessAction {
            name: "g".into(),
            type_: SuccessActionType::Goto,
            ..Default::default()
        };
        assert!(validate(&neither).iter().any(|e| e.contains("goto")));

        let both = SuccessAction {
            workflow_id: Some("w".into()),
            step_id: Some("s".into()),
            ..neither.clone()
        };
        assert!(validate(&both).iter().any(|e| e.contains("goto")));

        let ok = SuccessAction {
            workflow_id: Some("w".into()),
            ..neither
        };
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn validate_recurses_into_criteria() {
        let a = SuccessAction {
            name: "n".into(),
            criteria: vec![Criterion::default()],
            ..Default::default()
        };
        assert!(validate(&a).iter().any(|e| e.contains("condition")));
    }
}
