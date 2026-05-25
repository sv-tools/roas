//! Arazzo v1.0 `Step` object.
//!
//! Per [Step Object](https://spec.openapis.org/arazzo/v1.0.1.html#step-object):
//! a single call to an API operation or another workflow. Exactly one
//! of `operationId`, `operationPath`, or `workflowId` must be set.

use crate::v1_0::criterion::Criterion;
use crate::v1_0::failure_action::FailureAction;
use crate::v1_0::parameter::Parameter;
use crate::v1_0::request_body::RequestBody;
use crate::v1_0::reusable::ReusableOr;
use crate::v1_0::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Step {
    /// **Required** Unique string identifying the step within its
    /// workflow.
    #[serde(rename = "stepId")]
    pub step_id: String,

    /// A description of the step. CommonMark MAY be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The `operationId` of an operation in one of the source
    /// descriptions.
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,

    /// A source reference combined with a JSON Pointer to an operation.
    #[serde(rename = "operationPath", skip_serializing_if = "Option::is_none")]
    pub operation_path: Option<String>,

    /// The `workflowId` of another workflow to invoke.
    #[serde(rename = "workflowId", skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,

    /// Parameters passed to the operation or workflow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ReusableOr<Parameter>>,

    /// The request body to pass to the operation.
    #[serde(rename = "requestBody", skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBody>,

    /// Assertions determining the success of the step.
    #[serde(
        rename = "successCriteria",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub success_criteria: Vec<Criterion>,

    /// Actions to take when the step succeeds.
    #[serde(rename = "onSuccess", default, skip_serializing_if = "Vec::is_empty")]
    pub on_success: Vec<ReusableOr<SuccessAction>>,

    /// Actions to take when the step fails.
    #[serde(rename = "onFailure", default, skip_serializing_if = "Vec::is_empty")]
    pub on_failure: Vec<ReusableOr<FailureAction>>,

    /// A map of friendly output names to runtime expressions. Keys must
    /// match `^[a-zA-Z0-9\.\-_]+$`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Step {
    /// `true` when the step targets an operation (rather than another
    /// workflow); operation parameters must then set `in`.
    fn is_operation(&self) -> bool {
        self.operation_id.is_some() || self.operation_path.is_some()
    }
}

impl ValidateWithContext for Step {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("stepId", &self.step_id);

        // Exactly one of operationId / operationPath / workflowId.
        let targets = [
            self.operation_id.is_some(),
            self.operation_path.is_some(),
            self.workflow_id.is_some(),
        ];
        match targets.iter().filter(|set| **set).count() {
            1 => {}
            0 => {
                ctx.error("must set exactly one of `operationId`, `operationPath`, or `workflowId`")
            }
            _ => {
                ctx.error("`operationId`, `operationPath`, and `workflowId` are mutually exclusive")
            }
        }

        let is_operation = self.is_operation();
        for (i, parameter) in self.parameters.iter().enumerate() {
            ctx.in_index("parameters", i, |ctx| {
                parameter.validate_with_context(ctx);
                if is_operation
                    && let ReusableOr::Item(p) = parameter
                    && p.in_.is_none()
                {
                    ctx.error_field("in", "is required for operation steps");
                }
            });
        }

        if let Some(request_body) = &self.request_body {
            ctx.in_field("requestBody", |ctx| request_body.validate_with_context(ctx));
        }
        for (i, criterion) in self.success_criteria.iter().enumerate() {
            ctx.in_index("successCriteria", i, |ctx| {
                criterion.validate_with_context(ctx)
            });
        }
        for (i, action) in self.on_success.iter().enumerate() {
            ctx.in_index("onSuccess", i, |ctx| action.validate_with_context(ctx));
        }
        for (i, action) in self.on_failure.iter().enumerate() {
            ctx.in_index("onFailure", i, |ctx| action.validate_with_context(ctx));
        }
        ctx.validate_map_keys("outputs", &self.outputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(step: &Step) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.steps[0]");
        step.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn operation_step_round_trips() {
        let step: Step = serde_json::from_value(json!({
            "stepId": "findPet",
            "operationId": "getPetById",
            "parameters": [ { "name": "petId", "in": "path", "value": "$inputs.petId" } ],
            "successCriteria": [ { "condition": "$statusCode == 200" } ],
            "outputs": { "pet": "$response.body" },
        }))
        .unwrap();
        assert_eq!(step.step_id, "findPet");
        assert_eq!(step.parameters.len(), 1);
        assert!(validate(&step).is_empty());
    }

    #[test]
    fn missing_operation_target_is_rejected() {
        let step = Step {
            step_id: "s".into(),
            ..Default::default()
        };
        assert!(validate(&step).iter().any(|e| e.contains("exactly one of")));
    }

    #[test]
    fn multiple_operation_targets_are_rejected() {
        let step = Step {
            step_id: "s".into(),
            operation_id: Some("op".into()),
            workflow_id: Some("wf".into()),
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e.contains("mutually exclusive"))
        );
    }

    #[test]
    fn operation_parameter_requires_in() {
        let step = Step {
            step_id: "s".into(),
            operation_id: Some("op".into()),
            parameters: vec![ReusableOr::Item(Parameter {
                name: "p".into(),
                value: json!("v"),
                ..Default::default()
            })],
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0].parameters[0].in: is required for operation steps")
        );
    }

    #[test]
    fn workflow_step_parameter_does_not_require_in() {
        let step = Step {
            step_id: "s".into(),
            workflow_id: Some("wf".into()),
            parameters: vec![ReusableOr::Item(Parameter {
                name: "p".into(),
                value: json!("v"),
                ..Default::default()
            })],
            ..Default::default()
        };
        assert!(validate(&step).is_empty());
    }

    #[test]
    fn reusable_parameter_skips_in_check() {
        let step: Step = serde_json::from_value(json!({
            "stepId": "s",
            "operationId": "op",
            "parameters": [ { "reference": "$components.parameters.petId" } ],
        }))
        .unwrap();
        assert!(validate(&step).is_empty());
    }

    #[test]
    fn bad_output_key_is_rejected() {
        let step: Step = serde_json::from_value(json!({
            "stepId": "s",
            "workflowId": "wf",
            "outputs": { "bad key": "$x" },
        }))
        .unwrap();
        assert!(validate(&step).iter().any(|e| e.contains("key must match")));
    }
}
