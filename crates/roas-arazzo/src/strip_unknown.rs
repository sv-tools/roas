//! Strip non-`x-` unknown fields captured in `extensions` maps across an
//! Arazzo document.
//!
//! Serialization emits every entry in `extensions` so documents round-trip
//! without loss. [`v1_0::Description::unknown_members`] and
//! [`v1_1::Description::unknown_members`] strip unknown fields from a clone
//! before serializing so [`crate::unknown::diff`] still reports every
//! unclaimed key alongside extra keys dropped by `Reusable`.
//!
//! Every struct is destructured exhaustively so adding a field to any model
//! type causes a compile error until this pass handles it.

use crate::common::reusable::{Reusable, ReusableOr};

fn strip_reusable_or<T>(entry: &mut ReusableOr<T>, strip: impl FnOnce(&mut T)) {
    match entry {
        ReusableOr::Reusable(Reusable {
            reference: _,
            value: _,
        }) => {}
        ReusableOr::Item(item) => strip(item),
    }
}

#[cfg(feature = "v1_0")]
pub(crate) mod v1_0 {
    use super::strip_reusable_or;
    use crate::common::extensions::retain_extensions_only;
    use crate::v1_0::{
        Components, Criterion, CriterionExpressionType, CriterionType, Description, FailureAction,
        Info, Parameter, PayloadReplacement, RequestBody, SourceDescription, Step, SuccessAction,
        Workflow,
    };

    pub(crate) fn strip_description(d: &mut Description) {
        let Description {
            arazzo: _,
            info,
            source_descriptions,
            workflows,
            components,
            extensions,
        } = d;
        retain_extensions_only(extensions);
        strip_info(info);
        for sd in source_descriptions {
            strip_source_description(sd);
        }
        for wf in workflows {
            strip_workflow(wf);
        }
        if let Some(c) = components {
            strip_components(c);
        }
    }

    fn strip_info(info: &mut Info) {
        let Info {
            title: _,
            summary: _,
            description: _,
            version: _,
            extensions,
        } = info;
        retain_extensions_only(extensions);
    }

    fn strip_source_description(sd: &mut SourceDescription) {
        let SourceDescription {
            name: _,
            url: _,
            type_: _,
            extensions,
        } = sd;
        retain_extensions_only(extensions);
    }

    fn strip_workflow(wf: &mut Workflow) {
        let Workflow {
            workflow_id: _,
            summary: _,
            description: _,
            inputs: _,
            depends_on: _,
            steps,
            success_actions,
            failure_actions,
            outputs: _,
            parameters,
            extensions,
        } = wf;
        retain_extensions_only(extensions);
        for step in steps {
            strip_step(step);
        }
        for action in success_actions {
            strip_reusable_or(action, strip_success_action);
        }
        for action in failure_actions {
            strip_reusable_or(action, strip_failure_action);
        }
        for param in parameters {
            strip_reusable_or(param, strip_parameter);
        }
    }

    fn strip_step(step: &mut Step) {
        let Step {
            step_id: _,
            description: _,
            operation_id: _,
            operation_path: _,
            workflow_id: _,
            parameters,
            request_body,
            success_criteria,
            on_success,
            on_failure,
            outputs: _,
            extensions,
        } = step;
        retain_extensions_only(extensions);
        for param in parameters {
            strip_reusable_or(param, strip_parameter);
        }
        if let Some(rb) = request_body {
            strip_request_body(rb);
        }
        for criterion in success_criteria {
            strip_criterion(criterion);
        }
        for action in on_success {
            strip_reusable_or(action, strip_success_action);
        }
        for action in on_failure {
            strip_reusable_or(action, strip_failure_action);
        }
    }

    fn strip_parameter(p: &mut Parameter) {
        let Parameter {
            name: _,
            in_: _,
            value: _,
            extensions,
        } = p;
        retain_extensions_only(extensions);
    }

    fn strip_request_body(rb: &mut RequestBody) {
        let RequestBody {
            content_type: _,
            payload: _,
            replacements,
            extensions,
        } = rb;
        retain_extensions_only(extensions);
        for r in replacements {
            strip_payload_replacement(r);
        }
    }

    fn strip_payload_replacement(r: &mut PayloadReplacement) {
        let PayloadReplacement {
            target: _,
            value: _,
            extensions,
        } = r;
        retain_extensions_only(extensions);
    }

    fn strip_criterion(c: &mut Criterion) {
        let Criterion {
            context: _,
            condition: _,
            type_,
            version: _,
            extensions,
        } = c;
        retain_extensions_only(extensions);
        if let Some(t) = type_ {
            match t {
                CriterionType::Simple
                | CriterionType::Regex
                | CriterionType::Jsonpath
                | CriterionType::Xpath => {}
                CriterionType::Expression(et) => strip_criterion_expression_type(et),
            }
        }
    }

    fn strip_criterion_expression_type(et: &mut CriterionExpressionType) {
        let CriterionExpressionType {
            type_: _,
            version: _,
            extensions,
        } = et;
        retain_extensions_only(extensions);
    }

    fn strip_success_action(a: &mut SuccessAction) {
        let SuccessAction {
            name: _,
            type_: _,
            workflow_id: _,
            step_id: _,
            criteria,
            extensions,
        } = a;
        retain_extensions_only(extensions);
        for c in criteria {
            strip_criterion(c);
        }
    }

    fn strip_failure_action(a: &mut FailureAction) {
        let FailureAction {
            name: _,
            type_: _,
            workflow_id: _,
            step_id: _,
            retry_after: _,
            retry_limit: _,
            criteria,
            extensions,
        } = a;
        retain_extensions_only(extensions);
        for c in criteria {
            strip_criterion(c);
        }
    }

    fn strip_components(c: &mut Components) {
        let Components {
            inputs: _,
            parameters,
            success_actions,
            failure_actions,
            extensions,
        } = c;
        retain_extensions_only(extensions);
        for p in parameters.values_mut() {
            strip_parameter(p);
        }
        for a in success_actions.values_mut() {
            strip_success_action(a);
        }
        for a in failure_actions.values_mut() {
            strip_failure_action(a);
        }
    }
}

#[cfg(feature = "v1_1")]
pub(crate) mod v1_1 {
    use super::strip_reusable_or;
    use crate::common::extensions::retain_extensions_only;
    use crate::v1_1::{
        Components, Criterion, CriterionType, Description, ExpressionType, FailureAction, Info,
        Parameter, PayloadReplacement, RequestBody, Selector, SelectorType, SourceDescription,
        Step, SuccessAction, ValueOrSelector, Workflow,
    };

    pub(crate) fn strip_description(d: &mut Description) {
        let Description {
            arazzo: _,
            self_: _,
            info,
            source_descriptions,
            workflows,
            components,
            extensions,
        } = d;
        retain_extensions_only(extensions);
        strip_info(info);
        for sd in source_descriptions {
            strip_source_description(sd);
        }
        for wf in workflows {
            strip_workflow(wf);
        }
        if let Some(c) = components {
            strip_components(c);
        }
    }

    fn strip_info(info: &mut Info) {
        let Info {
            title: _,
            summary: _,
            description: _,
            version: _,
            extensions,
        } = info;
        retain_extensions_only(extensions);
    }

    fn strip_source_description(sd: &mut SourceDescription) {
        let SourceDescription {
            name: _,
            url: _,
            type_: _,
            extensions,
        } = sd;
        retain_extensions_only(extensions);
    }

    fn strip_workflow(wf: &mut Workflow) {
        let Workflow {
            workflow_id: _,
            summary: _,
            description: _,
            inputs: _,
            depends_on: _,
            steps,
            success_actions,
            failure_actions,
            outputs,
            parameters,
            extensions,
        } = wf;
        retain_extensions_only(extensions);
        for step in steps {
            strip_step(step);
        }
        for action in success_actions {
            strip_reusable_or(action, strip_success_action);
        }
        for action in failure_actions {
            strip_reusable_or(action, strip_failure_action);
        }
        for output in outputs.values_mut() {
            strip_value_or_selector(output);
        }
        for param in parameters {
            strip_reusable_or(param, strip_parameter);
        }
    }

    fn strip_step(step: &mut Step) {
        let Step {
            step_id: _,
            description: _,
            timeout: _,
            depends_on: _,
            operation_id: _,
            operation_path: _,
            channel_path: _,
            correlation_id: _,
            action: _,
            workflow_id: _,
            parameters,
            request_body,
            success_criteria,
            on_success,
            on_failure,
            outputs,
            extensions,
        } = step;
        retain_extensions_only(extensions);
        for param in parameters {
            strip_reusable_or(param, strip_parameter);
        }
        if let Some(rb) = request_body {
            strip_request_body(rb);
        }
        for criterion in success_criteria {
            strip_criterion(criterion);
        }
        for action in on_success {
            strip_reusable_or(action, strip_success_action);
        }
        for action in on_failure {
            strip_reusable_or(action, strip_failure_action);
        }
        for output in outputs.values_mut() {
            strip_value_or_selector(output);
        }
    }

    fn strip_parameter(p: &mut Parameter) {
        let Parameter {
            name: _,
            in_: _,
            value,
            extensions,
        } = p;
        retain_extensions_only(extensions);
        strip_value_or_selector(value);
    }

    fn strip_request_body(rb: &mut RequestBody) {
        let RequestBody {
            content_type: _,
            payload: _,
            replacements,
            extensions,
        } = rb;
        retain_extensions_only(extensions);
        for r in replacements {
            strip_payload_replacement(r);
        }
    }

    fn strip_payload_replacement(r: &mut PayloadReplacement) {
        let PayloadReplacement {
            target: _,
            target_selector_type,
            value,
            extensions,
        } = r;
        retain_extensions_only(extensions);
        if let Some(st) = target_selector_type {
            strip_selector_type(st);
        }
        strip_value_or_selector(value);
    }

    fn strip_expression_type(et: &mut ExpressionType) {
        let ExpressionType {
            type_: _,
            version: _,
            extensions,
        } = et;
        retain_extensions_only(extensions);
    }

    fn strip_selector_type(st: &mut SelectorType) {
        match st {
            SelectorType::Simple(_) => {}
            SelectorType::Expression(et) => strip_expression_type(et),
        }
    }

    fn strip_selector(s: &mut Selector) {
        let Selector {
            context: _,
            selector: _,
            type_,
            extensions,
        } = s;
        retain_extensions_only(extensions);
        strip_selector_type(type_);
    }

    fn strip_value_or_selector(v: &mut ValueOrSelector) {
        match v {
            ValueOrSelector::Selector(s) => strip_selector(s),
            ValueOrSelector::Literal(_) => {}
        }
    }

    fn strip_criterion(c: &mut Criterion) {
        let Criterion {
            context: _,
            condition: _,
            type_,
            extensions,
        } = c;
        retain_extensions_only(extensions);
        if let Some(t) = type_ {
            match t {
                CriterionType::Simple(_) => {}
                CriterionType::Expression(et) => strip_expression_type(et),
            }
        }
    }

    fn strip_success_action(a: &mut SuccessAction) {
        let SuccessAction {
            name: _,
            type_: _,
            workflow_id: _,
            step_id: _,
            parameters,
            criteria,
            extensions,
        } = a;
        retain_extensions_only(extensions);
        for p in parameters {
            strip_reusable_or(p, strip_parameter);
        }
        for c in criteria {
            strip_criterion(c);
        }
    }

    fn strip_failure_action(a: &mut FailureAction) {
        let FailureAction {
            name: _,
            type_: _,
            workflow_id: _,
            step_id: _,
            parameters,
            retry_after: _,
            retry_limit: _,
            criteria,
            extensions,
        } = a;
        retain_extensions_only(extensions);
        for p in parameters {
            strip_reusable_or(p, strip_parameter);
        }
        for c in criteria {
            strip_criterion(c);
        }
    }

    fn strip_components(c: &mut Components) {
        let Components {
            inputs: _,
            parameters,
            success_actions,
            failure_actions,
            extensions,
        } = c;
        retain_extensions_only(extensions);
        for p in parameters.values_mut() {
            strip_parameter(p);
        }
        for a in success_actions.values_mut() {
            strip_success_action(a);
        }
        for a in failure_actions.values_mut() {
            strip_failure_action(a);
        }
    }
}
