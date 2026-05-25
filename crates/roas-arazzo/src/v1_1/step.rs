//! Arazzo v1.1 `Step` object.
//!
//! Per [Step Object](https://spec.openapis.org/arazzo/v1.1.0.html#step-object).
//! In v1.1 a step is one of three shapes — **OpenAPI**
//! (`operationId`/`operationPath`), **AsyncAPI** (`operationId`/
//! `channelPath` + `action`), or **Workflow** (`workflowId`) — over a
//! shared base. Modeled as one flat struct (the shapes overlap on
//! `operationId`, so a serde-untagged enum can't reliably dispatch);
//! the shape is enforced in validation. New base fields: `timeout`,
//! `dependsOn`.

use crate::common::reusable::ReusableOr;
use crate::v1_1::criterion::Criterion;
use crate::v1_1::failure_action::FailureAction;
use crate::v1_1::parameter::Parameter;
use crate::v1_1::request_body::RequestBody;
use crate::v1_1::selector::ValueOrSelector;
use crate::v1_1::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The direction of an AsyncAPI step.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum StepAction {
    #[default]
    Send,
    Receive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Step {
    /// **Required** Unique string identifying the step within its
    /// workflow.
    #[serde(rename = "stepId")]
    pub step_id: String,

    /// A description of the step. CommonMark MAY be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Milliseconds to wait before timing out the step. Added in v1.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i64>,

    /// Step identifiers that must complete before this step. Added in
    /// v1.1.
    #[serde(rename = "dependsOn", default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,

    /// The `operationId` of an OpenAPI or AsyncAPI operation.
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,

    /// A source reference + JSON Pointer to an OpenAPI operation.
    #[serde(rename = "operationPath", skip_serializing_if = "Option::is_none")]
    pub operation_path: Option<String>,

    /// A source reference + JSON Pointer to an async channel. Added in
    /// v1.1 (AsyncAPI step).
    #[serde(rename = "channelPath", skip_serializing_if = "Option::is_none")]
    pub channel_path: Option<String>,

    /// Correlation id for an async `receive`. Added in v1.1.
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<serde_json::Value>,

    /// The async channel action. Required on an AsyncAPI step. Added in
    /// v1.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<StepAction>,

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

    /// A map of friendly output names to runtime expressions or
    /// selectors. Keys must match `^[a-zA-Z0-9\.\-_]+$`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, ValueOrSelector>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Step {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("stepId", &self.step_id);

        let has_op_id = self.operation_id.is_some();
        let has_op_path = self.operation_path.is_some();
        let has_channel = self.channel_path.is_some();
        let has_workflow = self.workflow_id.is_some();
        let is_async = has_channel || self.action.is_some() || self.correlation_id.is_some();

        // Determine the step shape and enforce its constraints. An
        // operation step (OpenAPI/AsyncAPI) requires `in` on each
        // operation parameter; a workflow step does not.
        let is_operation = if has_workflow {
            if has_op_id || has_op_path || has_channel || self.action.is_some() {
                ctx.error(
                    "a workflow step (`workflowId`) must not set `operationId`, `operationPath`, `channelPath`, or `action`",
                );
            }
            false
        } else if is_async {
            if self.action.is_none() {
                ctx.error_field("action", "is required for an AsyncAPI step");
            }
            match (has_op_id, has_channel) {
                (true, true) => ctx.error("`operationId` and `channelPath` are mutually exclusive"),
                (false, false) => {
                    ctx.error("an AsyncAPI step must set `operationId` or `channelPath`")
                }
                _ => {}
            }
            if has_op_path {
                ctx.error("`operationPath` is not valid on an AsyncAPI step");
            }
            if self.correlation_id.is_some() && self.action != Some(StepAction::Receive) {
                ctx.error_field("correlationId", "is only valid when `action` is `receive`");
            }
            true
        } else {
            match (has_op_id, has_op_path) {
                (true, true) => {
                    ctx.error("`operationId` and `operationPath` are mutually exclusive")
                }
                (false, false) => ctx.error(
                    "must set exactly one of `operationId`, `operationPath`, `channelPath`, or `workflowId`",
                ),
                _ => {}
            }
            true
        };

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

    fn validate(step: &Step) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.steps[0]");
        step.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn openapi_step_round_trips() {
        let step: Step = serde_json::from_value(json!({
            "stepId": "findPet",
            "operationId": "getPetById",
            "timeout": 5000,
            "parameters": [ { "name": "petId", "in": "path", "value": "$inputs.petId" } ],
            "successCriteria": [ { "condition": "$statusCode == 200" } ],
        }))
        .unwrap();
        assert_eq!(step.timeout, Some(5000));
        assert!(validate(&step).is_empty());
    }

    #[test]
    fn asyncapi_step_round_trips() {
        let step: Step = serde_json::from_value(json!({
            "stepId": "listen",
            "channelPath": "$sourceDescriptions.events#/channels/pets",
            "action": "receive",
            "correlationId": "$message.headers.id",
        }))
        .unwrap();
        assert_eq!(step.action, Some(StepAction::Receive));
        assert!(validate(&step).is_empty());
    }

    #[test]
    fn asyncapi_step_requires_action() {
        let step = Step {
            step_id: "s".into(),
            channel_path: Some("$src#/c".into()),
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0].action: is required for an AsyncAPI step")
        );
    }

    #[test]
    fn correlation_id_requires_receive() {
        let step = Step {
            step_id: "s".into(),
            channel_path: Some("$src#/c".into()),
            action: Some(StepAction::Send),
            correlation_id: Some(json!("x")),
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0].correlationId: is only valid when `action` is `receive`")
        );
    }

    #[test]
    fn workflow_step_rejects_operation_fields() {
        let step = Step {
            step_id: "s".into(),
            workflow_id: Some("wf".into()),
            operation_id: Some("op".into()),
            ..Default::default()
        };
        assert!(validate(&step).iter().any(|e| e.contains("must not set")));
    }

    #[test]
    fn step_with_no_target_is_rejected() {
        let step = Step {
            step_id: "s".into(),
            ..Default::default()
        };
        assert!(validate(&step).iter().any(|e| e.contains("exactly one of")));
    }

    #[test]
    fn openapi_operation_parameter_requires_in() {
        let step = Step {
            step_id: "s".into(),
            operation_id: Some("op".into()),
            parameters: vec![ReusableOr::Item(Parameter {
                name: "p".into(),
                value: ValueOrSelector::literal("v"),
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
    fn async_operation_id_and_channel_path_are_mutually_exclusive() {
        let step = Step {
            step_id: "s".into(),
            operation_id: Some("op".into()),
            channel_path: Some("$src#/c".into()),
            action: Some(StepAction::Send),
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0]: `operationId` and `channelPath` are mutually exclusive")
        );
    }

    #[test]
    fn async_step_with_action_but_no_target_is_rejected() {
        let step = Step {
            step_id: "s".into(),
            action: Some(StepAction::Send),
            ..Default::default()
        };
        assert!(
            validate(&step).iter().any(
                |e| e == "#.steps[0]: an AsyncAPI step must set `operationId` or `channelPath`"
            )
        );
    }

    #[test]
    fn operation_path_on_async_step_is_rejected() {
        let step = Step {
            step_id: "s".into(),
            channel_path: Some("$src#/c".into()),
            operation_path: Some("$src#/op".into()),
            action: Some(StepAction::Send),
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0]: `operationPath` is not valid on an AsyncAPI step")
        );
    }

    #[test]
    fn openapi_operation_id_and_path_are_mutually_exclusive() {
        let step = Step {
            step_id: "s".into(),
            operation_id: Some("op".into()),
            operation_path: Some("$src#/op".into()),
            ..Default::default()
        };
        assert!(
            validate(&step).iter().any(
                |e| e == "#.steps[0]: `operationId` and `operationPath` are mutually exclusive"
            )
        );
    }

    #[test]
    fn on_failure_actions_are_recursed() {
        let step = Step {
            step_id: "s".into(),
            workflow_id: Some("wf".into()),
            on_failure: vec![ReusableOr::Reusable(
                crate::common::reusable::Reusable::default(),
            )],
            ..Default::default()
        };
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0].onFailure[0].reference: must not be empty")
        );
    }

    #[test]
    fn output_selector_is_validated() {
        let step: Step = serde_json::from_value(json!({
            "stepId": "s",
            "workflowId": "wf",
            "outputs": { "id": { "context": "$response.body", "selector": "", "type": "jsonpath" } },
        }))
        .unwrap();
        assert!(
            validate(&step)
                .iter()
                .any(|e| e == "#.steps[0].outputs.id.selector: must not be empty")
        );
    }
}
