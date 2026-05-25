//! Arazzo v1.0 `Workflow` object.
//!
//! Per [Workflow Object](https://spec.openapis.org/arazzo/v1.0.1.html#workflow-object):
//! an ordered list of steps achieving an objective across one or more
//! APIs.

use crate::v1_0::failure_action::FailureAction;
use crate::v1_0::parameter::Parameter;
use crate::v1_0::reusable::ReusableOr;
use crate::v1_0::step::Step;
use crate::v1_0::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

    /// A map of friendly output names to runtime expressions. Keys must
    /// match `^[a-zA-Z0-9\.\-_]+$`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, String>,

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

        let mut seen_step_ids = std::collections::BTreeSet::new();
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
        assert_eq!(wf.steps.len(), 1);
        assert!(validate(&wf).is_empty());

        let v = serde_json::to_value(&wf).unwrap();
        assert_eq!(v["workflowId"], json!("getPet"));
        assert!(v.get("parameters").is_none());
    }

    #[test]
    fn empty_steps_is_rejected() {
        let wf = Workflow {
            workflow_id: "w".into(),
            ..Default::default()
        };
        assert!(
            validate(&wf)
                .iter()
                .any(|e| e == "#.workflows[0].steps: must contain at least one entry")
        );
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
}
