//! Arazzo v1.1 `Workflow` object.
//!
//! Per [Workflow Object](https://spec.openapis.org/arazzo/v1.1.0.html#workflow-object).
//! Changed in v1.1: `outputs` values may be a `Selector`.

use crate::common::reusable::ReusableOr;
use crate::v1_1::failure_action::FailureAction;
use crate::v1_1::parameter::Parameter;
use crate::v1_1::selector::ValueOrSelector;
use crate::v1_1::step::Step;
use crate::v1_1::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Workflow {
    /// **Required** Unique string identifying the workflow.
    #[serde(rename = "workflowId")]
    pub workflow_id: String,

    /// A summary of the purpose or objective of the workflow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A description of the workflow. CommonMark MAY be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A JSON Schema 2020-12 object describing the workflow inputs. Kept
    /// as opaque JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<serde_json::Value>,

    /// Workflows that MUST complete before this one runs.
    #[serde(rename = "dependsOn", default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,

    /// **Required** Ordered, non-empty list of steps.
    pub steps: Vec<Step>,

    /// Success actions applicable to every step in the workflow.
    #[serde(
        rename = "successActions",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub success_actions: Vec<ReusableOr<SuccessAction>>,

    /// Failure actions applicable to every step in the workflow.
    #[serde(
        rename = "failureActions",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub failure_actions: Vec<ReusableOr<FailureAction>>,

    /// A map of friendly output names to runtime expressions or
    /// selectors. Keys must match `^[a-zA-Z0-9\.\-_]+$`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, ValueOrSelector>,

    /// Parameters applicable to every step in the workflow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ReusableOr<Parameter>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Workflow {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("workflowId", &self.workflow_id);

        if self.steps.is_empty() {
            ctx.error_field("steps", "must contain at least one entry");
        }

        let mut seen_step_ids = BTreeSet::new();
        for (i, step) in self.steps.iter().enumerate() {
            ctx.in_index("steps", i, |ctx| {
                step.validate_with_context(ctx);
                if !step.step_id.is_empty() && !seen_step_ids.insert(step.step_id.as_str()) {
                    ctx.error_field("stepId", format!("duplicate stepId `{}`", step.step_id));
                }
            });
        }

        for (i, parameter) in self.parameters.iter().enumerate() {
            ctx.in_index("parameters", i, |ctx| parameter.validate_with_context(ctx));
        }
        for (i, action) in self.success_actions.iter().enumerate() {
            ctx.in_index("successActions", i, |ctx| action.validate_with_context(ctx));
        }
        for (i, action) in self.failure_actions.iter().enumerate() {
            ctx.in_index("failureActions", i, |ctx| action.validate_with_context(ctx));
        }

        ctx.validate_map_keys("outputs", &self.outputs);
        for (name, output) in &self.outputs {
            ctx.in_key("outputs", name, |ctx| output.validate_with_context(ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(wf: &Workflow) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.workflows[0]");
        wf.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn minimal_workflow_round_trips() {
        let wf: Workflow = serde_json::from_value(json!({
            "workflowId": "getPet",
            "steps": [ { "stepId": "s1", "workflowId": "other" } ],
        }))
        .unwrap();
        assert_eq!(wf.workflow_id, "getPet");
        assert!(validate(&wf).is_empty());
    }

    #[test]
    fn duplicate_step_ids_are_rejected() {
        let wf: Workflow = serde_json::from_value(json!({
            "workflowId": "w",
            "steps": [
                { "stepId": "dup", "workflowId": "a" },
                { "stepId": "dup", "workflowId": "b" }
            ],
        }))
        .unwrap();
        assert!(
            validate(&wf)
                .iter()
                .any(|e| e == "#.workflows[0].steps[1].stepId: duplicate stepId `dup`")
        );
    }

    #[test]
    fn empty_id_and_action_lists_are_validated() {
        let wf: Workflow = serde_json::from_value(json!({
            "workflowId": "",
            "steps": [ { "stepId": "s", "workflowId": "x" } ],
            "parameters": [ { "reference": "" } ],
            "successActions": [ { "reference": "" } ],
            "failureActions": [ { "reference": "" } ],
        }))
        .unwrap();
        let errs = validate(&wf);
        assert!(
            errs.iter()
                .any(|e| e == "#.workflows[0].workflowId: must not be empty")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.workflows[0].parameters[0].reference: must not be empty")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.workflows[0].successActions[0].reference: must not be empty")
        );
        assert!(
            errs.iter()
                .any(|e| e == "#.workflows[0].failureActions[0].reference: must not be empty")
        );
    }

    #[test]
    fn output_selector_round_trips() {
        let wf: Workflow = serde_json::from_value(json!({
            "workflowId": "w",
            "steps": [ { "stepId": "s", "workflowId": "x" } ],
            "outputs": { "token": "$steps.s.outputs.token" },
        }))
        .unwrap();
        assert!(matches!(wf.outputs["token"], ValueOrSelector::Literal(_)));
        assert!(validate(&wf).is_empty());
    }
}
