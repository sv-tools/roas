//! Upconversion from Arazzo v1.0 to v1.1.
//!
//! Available when both the `v1_0` and `v1_1` features are enabled.
//! Field-by-field `From` impls; the structural differences handled are:
//! the `arazzo` version string, the criterion `type` + `version` →
//! [`ExpressionType`](crate::v1_1::ExpressionType) folding, and wrapping
//! plain values / output strings as [`ValueOrSelector::Literal`].

use crate::common::extensions::retain_extensions_only;
use crate::common::reusable::ReusableOr;
use crate::{v1_0, v1_1};
use std::collections::BTreeMap;

/// Keep only `x-` Specification Extensions when upconverting from v1.0,
/// stripping unknown fields that v1.0 captured for round-tripping.
fn extensions_only(
    mut extensions: Option<BTreeMap<String, serde_json::Value>>,
) -> Option<BTreeMap<String, serde_json::Value>> {
    retain_extensions_only(&mut extensions);
    extensions
}

/// Map a `ReusableOr<A>` to a `ReusableOr<B>` (the `Reusable` arm is a
/// shared `common` type, so only the `Item` arm converts).
fn map_reusable<A, B: From<A>>(value: ReusableOr<A>) -> ReusableOr<B> {
    match value {
        ReusableOr::Reusable(r) => ReusableOr::Reusable(r),
        ReusableOr::Item(item) => ReusableOr::Item(item.into()),
    }
}

fn map_reusables<A, B: From<A>>(values: Vec<ReusableOr<A>>) -> Vec<ReusableOr<B>> {
    values.into_iter().map(map_reusable).collect()
}

impl From<v1_0::Description> for v1_1::Description {
    fn from(d: v1_0::Description) -> Self {
        v1_1::Description {
            // v1.0 documents upconvert to the minimum v1.1 version.
            arazzo: v1_1::Version::V1_1_0(),
            self_: None,
            info: d.info.into(),
            source_descriptions: d.source_descriptions.into_iter().map(Into::into).collect(),
            workflows: d.workflows.into_iter().map(Into::into).collect(),
            components: d.components.map(Into::into),
            extensions: extensions_only(d.extensions),
        }
    }
}

impl From<v1_0::Info> for v1_1::Info {
    fn from(i: v1_0::Info) -> Self {
        v1_1::Info {
            title: i.title,
            summary: i.summary,
            description: i.description,
            version: i.version,
            extensions: extensions_only(i.extensions),
        }
    }
}

impl From<v1_0::SourceType> for v1_1::SourceType {
    fn from(t: v1_0::SourceType) -> Self {
        match t {
            v1_0::SourceType::Arazzo => v1_1::SourceType::Arazzo,
            v1_0::SourceType::Openapi => v1_1::SourceType::Openapi,
        }
    }
}

impl From<v1_0::SourceDescription> for v1_1::SourceDescription {
    fn from(s: v1_0::SourceDescription) -> Self {
        v1_1::SourceDescription {
            name: s.name,
            url: s.url,
            type_: s.type_.map(Into::into),
            extensions: extensions_only(s.extensions),
        }
    }
}

impl From<v1_0::ParameterLocation> for v1_1::ParameterLocation {
    fn from(l: v1_0::ParameterLocation) -> Self {
        match l {
            v1_0::ParameterLocation::Path => v1_1::ParameterLocation::Path,
            v1_0::ParameterLocation::Query => v1_1::ParameterLocation::Query,
            v1_0::ParameterLocation::Header => v1_1::ParameterLocation::Header,
            v1_0::ParameterLocation::Cookie => v1_1::ParameterLocation::Cookie,
        }
    }
}

impl From<v1_0::Parameter> for v1_1::Parameter {
    fn from(p: v1_0::Parameter) -> Self {
        v1_1::Parameter {
            name: p.name,
            in_: p.in_.map(Into::into),
            value: v1_1::ValueOrSelector::Literal(p.value),
            extensions: extensions_only(p.extensions),
        }
    }
}

impl From<v1_0::Criterion> for v1_1::Criterion {
    fn from(c: v1_0::Criterion) -> Self {
        use v1_0::{CriterionExpressionKind as V0Expr, CriterionType as V0};
        use v1_1::{CriterionKind, CriterionType, ExpressionKind, ExpressionType};

        // v1.0 carried either a nested Criterion Expression Type Object or
        // the schema's flat `type` + `version`. Fold either form into a v1.1
        // ExpressionType when a version is present; otherwise keep the plain
        // string kind.
        let type_ = c.type_.map(|kind| match kind {
            V0::Expression(et) => CriterionType::Expression(ExpressionType {
                type_: match et.type_ {
                    V0Expr::Jsonpath => ExpressionKind::Jsonpath,
                    V0Expr::Xpath => ExpressionKind::Xpath,
                },
                version: et.version,
                extensions: extensions_only(et.extensions),
            }),
            V0::Jsonpath => match c.version {
                Some(version) => CriterionType::Expression(ExpressionType {
                    type_: ExpressionKind::Jsonpath,
                    version,
                    extensions: None,
                }),
                None => CriterionType::Simple(CriterionKind::Jsonpath),
            },
            V0::Xpath => match c.version {
                Some(version) => CriterionType::Expression(ExpressionType {
                    type_: ExpressionKind::Xpath,
                    version,
                    extensions: None,
                }),
                None => CriterionType::Simple(CriterionKind::Xpath),
            },
            V0::Simple => CriterionType::Simple(CriterionKind::Simple),
            V0::Regex => CriterionType::Simple(CriterionKind::Regex),
        });

        v1_1::Criterion {
            context: c.context,
            condition: c.condition,
            type_,
            extensions: extensions_only(c.extensions),
        }
    }
}

impl From<v1_0::PayloadReplacement> for v1_1::PayloadReplacement {
    fn from(r: v1_0::PayloadReplacement) -> Self {
        v1_1::PayloadReplacement {
            target: r.target,
            target_selector_type: None,
            value: v1_1::ValueOrSelector::literal(r.value),
            extensions: extensions_only(r.extensions),
        }
    }
}

impl From<v1_0::RequestBody> for v1_1::RequestBody {
    fn from(b: v1_0::RequestBody) -> Self {
        v1_1::RequestBody {
            content_type: b.content_type,
            payload: b.payload,
            replacements: b.replacements.into_iter().map(Into::into).collect(),
            extensions: extensions_only(b.extensions),
        }
    }
}

impl From<v1_0::SuccessActionType> for v1_1::SuccessActionType {
    fn from(t: v1_0::SuccessActionType) -> Self {
        match t {
            v1_0::SuccessActionType::End => v1_1::SuccessActionType::End,
            v1_0::SuccessActionType::Goto => v1_1::SuccessActionType::Goto,
        }
    }
}

impl From<v1_0::SuccessAction> for v1_1::SuccessAction {
    fn from(a: v1_0::SuccessAction) -> Self {
        v1_1::SuccessAction {
            name: a.name,
            type_: a.type_.into(),
            workflow_id: a.workflow_id,
            step_id: a.step_id,
            parameters: Vec::new(),
            criteria: a.criteria.into_iter().map(Into::into).collect(),
            extensions: extensions_only(a.extensions),
        }
    }
}

impl From<v1_0::FailureActionType> for v1_1::FailureActionType {
    fn from(t: v1_0::FailureActionType) -> Self {
        match t {
            v1_0::FailureActionType::End => v1_1::FailureActionType::End,
            v1_0::FailureActionType::Goto => v1_1::FailureActionType::Goto,
            v1_0::FailureActionType::Retry => v1_1::FailureActionType::Retry,
        }
    }
}

impl From<v1_0::FailureAction> for v1_1::FailureAction {
    fn from(a: v1_0::FailureAction) -> Self {
        v1_1::FailureAction {
            name: a.name,
            type_: a.type_.into(),
            workflow_id: a.workflow_id,
            step_id: a.step_id,
            parameters: Vec::new(),
            retry_after: a.retry_after,
            retry_limit: a.retry_limit,
            criteria: a.criteria.into_iter().map(Into::into).collect(),
            extensions: extensions_only(a.extensions),
        }
    }
}

impl From<v1_0::Step> for v1_1::Step {
    fn from(s: v1_0::Step) -> Self {
        v1_1::Step {
            step_id: s.step_id,
            description: s.description,
            timeout: None,
            depends_on: Vec::new(),
            operation_id: s.operation_id,
            operation_path: s.operation_path,
            channel_path: None,
            correlation_id: None,
            action: None,
            workflow_id: s.workflow_id,
            parameters: map_reusables(s.parameters),
            request_body: s.request_body.map(Into::into),
            success_criteria: s.success_criteria.into_iter().map(Into::into).collect(),
            on_success: map_reusables(s.on_success),
            on_failure: map_reusables(s.on_failure),
            outputs: s
                .outputs
                .into_iter()
                .map(|(k, v)| (k, v1_1::ValueOrSelector::literal(v)))
                .collect(),
            extensions: extensions_only(s.extensions),
        }
    }
}

impl From<v1_0::Workflow> for v1_1::Workflow {
    fn from(w: v1_0::Workflow) -> Self {
        v1_1::Workflow {
            workflow_id: w.workflow_id,
            summary: w.summary,
            description: w.description,
            inputs: w.inputs,
            depends_on: w.depends_on,
            steps: w.steps.into_iter().map(Into::into).collect(),
            success_actions: map_reusables(w.success_actions),
            failure_actions: map_reusables(w.failure_actions),
            outputs: w
                .outputs
                .into_iter()
                .map(|(k, v)| (k, v1_1::ValueOrSelector::literal(v)))
                .collect(),
            parameters: map_reusables(w.parameters),
            extensions: extensions_only(w.extensions),
        }
    }
}

impl From<v1_0::Components> for v1_1::Components {
    fn from(c: v1_0::Components) -> Self {
        v1_1::Components {
            inputs: c.inputs,
            parameters: c
                .parameters
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            success_actions: c
                .success_actions
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            failure_actions: c
                .failure_actions
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            extensions: extensions_only(c.extensions),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::validation::Validate;
    use crate::{v1_0, v1_1};
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn a_replacement_value_keeps_its_type_across_the_upconversion() {
        let replacement = v1_0::PayloadReplacement {
            target: "/quantity".to_owned(),
            value: json!(2),
            extensions: None,
        };
        assert_eq!(
            v1_1::PayloadReplacement::from(replacement).value,
            v1_1::ValueOrSelector::Literal(json!(2)),
            "a number stays a number, rather than becoming `\"2\"`"
        );
    }

    #[test]
    fn upconverts_and_validates() {
        let v0: v1_0::Description = serde_json::from_value(json!({
            "arazzo": "1.0.1",
            "info": { "title": "T", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "src", "url": "openapi.yaml", "type": "openapi" } ],
            "workflows": [
                {
                    "workflowId": "wf",
                    "steps": [
                        {
                            "stepId": "s",
                            "operationId": "op",
                            "parameters": [ { "name": "p", "in": "query", "value": 1 } ],
                            "successCriteria": [
                                { "context": "$response.body", "condition": "$.ok",
                                  "type": "jsonpath", "version": "draft-goessner-dispatch-jsonpath-00" }
                            ],
                            "outputs": { "id": "$response.body.id" }
                        }
                    ]
                }
            ]
        }))
        .unwrap();

        let v1: v1_1::Description = v0.into();
        assert_eq!(v1.arazzo, v1_1::Version::V1_1_0());
        v1.validate(EnumSet::empty())
            .expect("upconverted doc validates");

        // The criterion's type+version folded into an ExpressionType.
        let criterion = &v1.workflows[0].steps[0].success_criteria[0];
        assert!(matches!(
            criterion.type_,
            Some(v1_1::CriterionType::Expression(_))
        ));

        // The output string became a literal value-or-selector.
        assert!(matches!(
            v1.workflows[0].steps[0].outputs["id"],
            v1_1::ValueOrSelector::Literal(_)
        ));
    }

    #[test]
    fn simple_criterion_type_stays_simple() {
        let v0: v1_0::Criterion =
            serde_json::from_value(json!({ "condition": "$x", "context": "$y", "type": "regex" }))
                .unwrap();
        let v1: v1_1::Criterion = v0.into();
        assert_eq!(
            v1.type_,
            Some(v1_1::CriterionType::Simple(v1_1::CriterionKind::Regex))
        );

        let v0_nested_jp: v1_0::Criterion = serde_json::from_value(json!({
            "condition": "$.x",
            "context": "$response.body",
            "type": {
                "type": "jsonpath",
                "version": "draft-goessner-dispatch-jsonpath-00",
                "x-note": true
            }
        }))
        .unwrap();
        let v1_jp: v1_1::Criterion = v0_nested_jp.into();
        match v1_jp.type_ {
            Some(v1_1::CriterionType::Expression(et)) => {
                assert_eq!(et.type_, v1_1::ExpressionKind::Jsonpath);
                assert_eq!(et.version, "draft-goessner-dispatch-jsonpath-00");
                assert_eq!(et.extensions.unwrap().get("x-note"), Some(&json!(true)));
            }
            other => panic!("expected jsonpath ExpressionType, got {other:?}"),
        }

        let v0_nested_xp: v1_0::Criterion = serde_json::from_value(json!({
            "condition": "//x",
            "context": "$response.body",
            "type": { "type": "xpath", "version": "xpath-20" }
        }))
        .unwrap();
        let v1_xp: v1_1::Criterion = v0_nested_xp.into();
        match v1_xp.type_ {
            Some(v1_1::CriterionType::Expression(et)) => {
                assert_eq!(et.type_, v1_1::ExpressionKind::Xpath);
                assert_eq!(et.version, "xpath-20");
            }
            other => panic!("expected xpath ExpressionType, got {other:?}"),
        }

        let v0_bare_jp: v1_0::Criterion = serde_json::from_value(json!({
            "condition": "$.x",
            "context": "$response.body",
            "type": "jsonpath"
        }))
        .unwrap();
        assert_eq!(
            v1_1::Criterion::from(v0_bare_jp).type_,
            Some(v1_1::CriterionType::Simple(v1_1::CriterionKind::Jsonpath))
        );

        let v0_bare_xp: v1_0::Criterion = serde_json::from_value(json!({
            "condition": "//x",
            "context": "$response.body",
            "type": "xpath"
        }))
        .unwrap();
        assert_eq!(
            v1_1::Criterion::from(v0_bare_xp).type_,
            Some(v1_1::CriterionType::Simple(v1_1::CriterionKind::Xpath))
        );

        let v0_simple: v1_0::Criterion = serde_json::from_value(json!({
            "condition": "$statusCode == 200",
            "context": "$statusCode",
            "type": "simple"
        }))
        .unwrap();
        assert_eq!(
            v1_1::Criterion::from(v0_simple).type_,
            Some(v1_1::CriterionType::Simple(v1_1::CriterionKind::Simple))
        );
    }

    /// Exercises every `From` impl: both source types, all v1.0
    /// parameter locations, the xpath-with-version criterion branch, a
    /// reusable + an inline action, a request body with a replacement,
    /// every components map, a workflow step, and outputs.
    #[test]
    fn upconverts_every_object() {
        let v0: v1_0::Description = serde_json::from_value(json!({
            "arazzo": "1.0.1",
            "info": { "title": "T", "summary": "S", "description": "D", "version": "1.0.0", "x-a": 1 },
            "sourceDescriptions": [
                { "name": "arazzoSrc", "url": "a.yaml", "type": "arazzo" },
                { "name": "openapiSrc", "url": "o.yaml", "type": "openapi" }
            ],
            "workflows": [
                {
                    "workflowId": "wf",
                    "dependsOn": ["other"],
                    "parameters": [
                        { "name": "h", "in": "header", "value": "v" },
                        { "reference": "$components.parameters.locale" }
                    ],
                    "successActions": [
                        { "name": "sa", "type": "end" },
                        { "reference": "$components.successActions.done" }
                    ],
                    "failureActions": [
                        { "name": "fa", "type": "retry", "retryAfter": 1.0, "retryLimit": 2 }
                    ],
                    "steps": [
                        {
                            "stepId": "s",
                            "operationId": "op",
                            "parameters": [
                                { "name": "p1", "in": "path", "value": "x" },
                                { "name": "q1", "in": "query", "value": "y" },
                                { "name": "c1", "in": "cookie", "value": "z" }
                            ],
                            "requestBody": {
                                "contentType": "application/json",
                                "payload": { "k": "v" },
                                "replacements": [ { "target": "/a", "value": "b" } ]
                            },
                            "successCriteria": [
                                { "context": "$r", "condition": "$.x", "type": "xpath", "version": "xpath-10" },
                                { "condition": "ok" }
                            ],
                            "onSuccess": [ { "name": "g", "type": "goto", "stepId": "s2" } ],
                            "onFailure": [ { "name": "f", "type": "end" } ],
                            "outputs": { "o1": "$response.body" }
                        },
                        { "stepId": "s2", "workflowId": "wf2" }
                    ],
                    "outputs": { "wo": "$x" }
                }
            ],
            "components": {
                "inputs": { "in1": { "type": "string" } },
                "parameters": { "locale": { "name": "locale", "in": "header", "value": "en" } },
                "successActions": { "done": { "name": "done", "type": "end" } },
                "failureActions": { "abort": { "name": "abort", "type": "end" } }
            }
        }))
        .unwrap();

        let v1: v1_1::Description = v0.into();
        v1.validate(EnumSet::empty())
            .expect("upconverted doc validates");

        // Source types mapped.
        assert_eq!(
            v1.source_descriptions[0].type_,
            Some(v1_1::SourceType::Arazzo)
        );
        assert_eq!(
            v1.source_descriptions[1].type_,
            Some(v1_1::SourceType::Openapi)
        );

        let wf = &v1.workflows[0];
        // Reusable arm preserved; inline arm converted.
        assert!(matches!(wf.parameters[1], v1_1::ReusableOr::Reusable(_)));
        assert!(matches!(
            wf.success_actions[1],
            v1_1::ReusableOr::Reusable(_)
        ));

        let step = &wf.steps[0];
        // Parameter locations mapped across the board.
        let locs: Vec<_> = step
            .parameters
            .iter()
            .filter_map(|p| match p {
                v1_1::ReusableOr::Item(p) => p.in_,
                v1_1::ReusableOr::Reusable(_) => None,
            })
            .collect();
        assert_eq!(
            locs,
            vec![
                v1_1::ParameterLocation::Path,
                v1_1::ParameterLocation::Query,
                v1_1::ParameterLocation::Cookie,
            ]
        );

        // xpath + version folded into an ExpressionType; the bare
        // condition kept no type.
        match &step.success_criteria[0].type_ {
            Some(v1_1::CriterionType::Expression(et)) => {
                assert_eq!(et.type_, v1_1::ExpressionKind::Xpath);
                assert_eq!(et.version, "xpath-10");
            }
            other => panic!("expected xpath expression type, got {other:?}"),
        }
        assert!(step.success_criteria[1].type_.is_none());

        // Request body + replacement converted; value wrapped as literal.
        let replacement = &step.request_body.as_ref().unwrap().replacements[0];
        assert_eq!(replacement.value, v1_1::ValueOrSelector::literal("b"));
        assert!(replacement.target_selector_type.is_none());

        // Second step is a workflow step.
        assert_eq!(v1.workflows[0].steps[1].workflow_id.as_deref(), Some("wf2"));

        // Every components map carried over.
        let components = v1.components.as_ref().unwrap();
        assert!(components.inputs.contains_key("in1"));
        assert!(components.parameters.contains_key("locale"));
        assert!(components.success_actions.contains_key("done"));
        assert!(components.failure_actions.contains_key("abort"));

        // Root info / extensions preserved.
        assert_eq!(v1.info.summary.as_deref(), Some("S"));
        assert!(v1.info.extensions.is_some());
    }
}
