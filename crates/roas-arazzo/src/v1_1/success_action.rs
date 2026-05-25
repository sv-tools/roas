//! Arazzo v1.1 `Success Action` object.
//!
//! Per [Success Action Object](https://spec.openapis.org/arazzo/v1.1.0.html#success-action-object).
//! New in v1.1: `parameters` (requires `workflowId` when present).

use crate::common::reusable::ReusableOr;
use crate::v1_1::criterion::Criterion;
use crate::v1_1::parameter::Parameter;
use crate::validation::{Context, ValidateWithContext};
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

    /// Parameters passed to the workflow referenced by `workflowId`.
    /// Added in v1.1; requires `workflowId`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ReusableOr<Parameter>>,

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
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("name", &self.name);
        validate_goto_target(
            self.type_ == SuccessActionType::Goto,
            self.workflow_id.is_some(),
            self.step_id.is_some(),
            ctx,
        );
        if !self.parameters.is_empty() && self.workflow_id.is_none() {
            ctx.error_field("parameters", "are only valid when `workflowId` is set");
        }
        for (i, parameter) in self.parameters.iter().enumerate() {
            ctx.in_index("parameters", i, |ctx| parameter.validate_with_context(ctx));
        }
        for (i, criterion) in self.criteria.iter().enumerate() {
            ctx.in_index("criteria", i, |ctx| criterion.validate_with_context(ctx));
        }
    }
}

/// Shared `goto`-target rule used by both success and failure actions:
/// a `goto` requires exactly one of `workflowId` / `stepId`. Reported at
/// the action's current path.
pub(crate) fn validate_goto_target(
    is_goto: bool,
    has_workflow_id: bool,
    has_step_id: bool,
    ctx: &mut Context,
) {
    if is_goto && !(has_workflow_id ^ has_step_id) {
        ctx.error("`goto` requires exactly one of `workflowId` or `stepId`");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(a: &SuccessAction) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.a");
        a.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn end_action_round_trips() {
        let a: SuccessAction =
            serde_json::from_value(json!({ "name": "done", "type": "end" })).unwrap();
        assert_eq!(a.type_, SuccessActionType::End);
        assert!(validate(&a).is_empty());
    }

    #[test]
    fn parameters_require_workflow_id() {
        let a: SuccessAction = serde_json::from_value(json!({
            "name": "g",
            "type": "goto",
            "stepId": "next",
            "parameters": [ { "name": "p", "value": 1 } ],
        }))
        .unwrap();
        assert!(
            validate(&a)
                .iter()
                .any(|e| e == "#.a.parameters: are only valid when `workflowId` is set")
        );

        let ok: SuccessAction = serde_json::from_value(json!({
            "name": "g",
            "type": "goto",
            "workflowId": "wf",
            "parameters": [ { "name": "p", "value": 1 } ],
        }))
        .unwrap();
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn goto_requires_exactly_one_target() {
        let a = SuccessAction {
            name: "g".into(),
            type_: SuccessActionType::Goto,
            ..Default::default()
        };
        assert!(validate(&a).iter().any(|e| e.contains("goto")));
    }

    #[test]
    fn criteria_are_recursed() {
        let a = SuccessAction {
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
}
