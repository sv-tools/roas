use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    CONDITION_PROFILE, CriterionError, ExecutionError, ExpressionError, Options, Outcome,
    PreparationIssue, Progress, prepare, testing::Fake,
};
use serde_json::{Value, json};

fn document(steps: Value) -> Value {
    json!({ "arazzo": "1.1.0", "info": { "title": "Preparation", "version": "1" },
        "sourceDescriptions": [{ "name": "api", "url": "https://example.com/openapi.json", "type": "openapi" }],
        "workflows": [{ "workflowId": "w", "steps": steps }] })
}

fn options() -> Options {
    Options::new().source(
        "api",
        "https://example.com/openapi.json",
        json!({
            "openapi": "3.1.0", "servers": [{ "url": "https://example.com" }],
            "paths": { "/check": { "get": { "operationId": "check" } } }
        }),
    )
}

#[test]
fn static_criterion_errors_cannot_be_recovered_into_a_checked_success() {
    for criterion in [
        json!({ "condition": "$steps.typo.outputs.value" }),
        json!({ "condition": "true || $steps.typo.outputs.value" }),
        json!({ "condition": "false && $workflows.typo.outputs.value" }),
        json!({ "type": "regex", "condition": "ok" }),
        json!({ "type": "simple", "condition": "true" }),
    ] {
        let description: Description = serde_json::from_value(document(json!([
            { "stepId": "a", "operationId": "check", "successCriteria": [criterion],
              "onFailure": [{ "name": "recover", "type": "goto", "stepId": "b" }] },
            { "stepId": "b", "operationId": "check" }
        ])))
        .unwrap();
        let options = options();
        let error = prepare(&description, &options).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.path.contains("successCriteria[0]")
                    && diagnostic.is_error())
        );
        assert!(error.to_string().contains("nothing was run"));
    }
}

#[test]
fn preparation_collects_locations_and_offsets_before_a_later_step_can_send() {
    let description: Description = serde_json::from_value(document(json!([
        { "stepId": "a", "operationId": "check" },
        { "stepId": "b", "operationId": "absent", "parameters": [
            { "name": "bad", "in": "query", "value": "prefix {$statusCode.extra}" },
            { "name": "unknown", "in": "header", "value": "$steps.typo.outputs.value" }
        ], "successCriteria": [{ "condition": "true ||" }] }
    ])))
    .unwrap();
    let options = options();
    let error = prepare(&description, &options).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("operationId"), "{text}");
    assert!(text.contains("parameters[1].value"), "{text}");
    let diagnostic = error
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path.ends_with("parameters[0].value"))
        .unwrap();
    assert_eq!(diagnostic.workflow_id.as_deref(), Some("w"));
    assert_eq!(diagnostic.step_id.as_deref(), Some("b"));
    assert_eq!(diagnostic.offset, Some(19));
    assert_eq!(
        text,
        prepare(&description, &options).unwrap_err().to_string()
    );
}

#[test]
fn prepared_runs_are_independent_and_preserve_runtime_recovery() {
    let description: Description = serde_json::from_value(document(json!([
        { "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": "$response.body#/ready" }],
          "onFailure": [{ "name": "retry", "type": "retry", "retryLimit": 1 }],
          "outputs": { "value": "$inputs.value" } }
    ]))).unwrap();
    let options = options().input("value", 7);
    let plan = prepare(&description, &options).unwrap();
    assert_eq!(plan.condition_profile(), CONDITION_PROFILE);
    for _ in 0..2 {
        let report = plan
            .execute(
                &mut Fake::new()
                    .reply(200, &json!({}))
                    .reply(200, &json!({ "ready": true })),
            )
            .unwrap();
        assert!(report.is_success());
        assert_eq!(report.steps.len(), 2);
        assert_eq!(report.steps[0].attempt, 1);
        assert_eq!(report.steps[1].attempt, 2);
        assert!(report.steps[0].criteria[0].error.is_some());
        assert_eq!(report.steps[1].outputs["value"], json!(7));
    }
    let mut first = plan
        .start_with_inputs(serde_json::from_value(json!({ "value": 10 })).unwrap())
        .unwrap();
    let mut second = plan
        .start_with_inputs(serde_json::from_value(json!({ "value": 20 })).unwrap())
        .unwrap();
    for (run, expected) in [(&mut first, 10), (&mut second, 20)] {
        assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
        run.supply(roas_arazzo_executor::HttpResponse::json(
            200,
            &json!({ "ready": true }),
        ))
        .unwrap();
        let Progress::Done(report) = run.advance().unwrap() else {
            panic!("done")
        };
        assert_eq!(report.steps[0].outputs["value"], json!(expected));
    }
}

#[test]
fn shared_actions_validate_without_manufacturing_step_dependencies() {
    let mut value = document(json!([
        { "stepId": "a", "operationId": "check" },
        { "stepId": "b", "operationId": "check", "outputs": { "ready": true } }
    ]));
    value["workflows"][0]["successActions"] = json!([{ "name": "end", "type": "end", "criteria": [
        { "condition": "$steps.a.outputs.ready && $steps.b.outputs.ready" }
    ] }]);
    let description = serde_json::from_value(value.clone()).unwrap();
    let options = options();
    let report = prepare(&description, &options)
        .unwrap()
        .execute(&mut Fake::new().reply(200, &json!({})).reply(200, &json!({})))
        .unwrap();
    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    value["workflows"][0]["successActions"][0]["criteria"][0]["condition"] =
        json!("$steps.typo.outputs.value");
    let description = serde_json::from_value(value).unwrap();
    assert!(
        prepare(&description, &options)
            .unwrap_err()
            .to_string()
            .contains("typo")
    );
}

#[test]
fn constant_patterns_are_checked_but_dynamic_patterns_remain_recoverable() {
    for kind in ["regex", "jsonpath"] {
        let mut value = document(
            json!([{ "stepId": "a", "operationId": "check", "successCriteria": [
            { "type": kind, "context": "$response.body", "condition": "[" }
        ] }]),
        );
        let options = options().input("pattern", "[");
        let description = serde_json::from_value(value.clone()).unwrap();
        assert!(prepare(&description, &options).is_err());
        value["workflows"][0]["steps"][0]["successCriteria"][0]["condition"] =
            json!("{$inputs.pattern}");
        let description = serde_json::from_value(value).unwrap();
        let report = prepare(&description, &options)
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})))
            .unwrap();
        assert_eq!(report.outcome, Outcome::Failed);
        assert!(report.steps[0].criteria[0].error.is_some());
    }
}

#[test]
fn preparation_follows_calls_and_recovery_targets_not_unrelated_operations() {
    let mut value = document(json!([{ "stepId": "a", "workflowId": "child" }]));
    value["workflows"].as_array_mut().unwrap().extend([
        json!({ "workflowId": "child", "steps": [{ "stepId": "read", "operationId": "check", "onFailure": [
            { "name": "recover", "type": "goto", "workflowId": "recovery" }
        ] }] }),
        json!({ "workflowId": "recovery", "steps": [{ "stepId": "fix", "operationId": "unknown" }] }),
        json!({ "workflowId": "unrelated", "steps": [{ "stepId": "unused", "operationId": "absentUnrelated" }] }),
    ]);
    let options = options();
    let description = serde_json::from_value(value.clone()).unwrap();
    let error = prepare(&description, &options).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.workflow_id.as_deref() == Some("recovery"))
    );
    assert!(
        !error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.workflow_id.as_deref() == Some("unrelated"))
    );
    value["workflows"][2]["steps"][0]["operationId"] = json!("check");
    assert!(prepare(&serde_json::from_value(value).unwrap(), &options).is_ok());
}

#[test]
fn portability_findings_are_optional_and_never_make_preparation_fail() {
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": "0" }] }]))).unwrap();
    let options = options();
    assert!(
        prepare(&description, &options)
            .unwrap()
            .diagnostics()
            .is_empty()
    );
    let options = options.portability_lints(true);
    let plan = prepare(&description, &options).unwrap();
    assert_eq!(plan.diagnostics().len(), 1);
    assert!(!plan.diagnostics()[0].is_error());
    assert!(matches!(
        plan.diagnostics()[0].issue,
        PreparationIssue::Portability(_)
    ));
    assert!(
        !plan
            .execute(&mut Fake::new().reply(200, &json!({})))
            .unwrap()
            .is_success()
    );
}

#[test]
fn checked_execution_keeps_partial_history_on_runtime_output_errors() {
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "outputs": { "missing": "$response.body#/absent" } }]))).unwrap();
    let options = options();
    let failure = prepare(&description, &options)
        .unwrap()
        .execute(&mut Fake::new().reply(200, &json!({})))
        .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::Select(_)));
    let report = failure.report.unwrap();
    assert_eq!(report.outcome, Outcome::Incomplete);
    assert_eq!(report.steps.len(), 1);
}

#[test]
fn source_requirements_distinguish_qualified_bare_and_literal_paths() {
    let mut value =
        document(json!([{ "stepId": "a", "operationId": "$sourceDescriptions.api.check" }]));
    value["sourceDescriptions"].as_array_mut().unwrap().extend([
        json!({ "name": "unrelated", "url": "https://example.com/absent.json", "type": "openapi" }),
        json!({ "name": "external", "url": "https://example.com/workflow.json", "type": "arazzo" }),
    ]);
    let options = options();
    let description = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        roas_arazzo_executor::required_sources(&description, &options)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["api"]
    );
    assert!(prepare(&description, &options).is_ok());
    value["workflows"][0]["steps"][0]["operationId"] = json!("check");
    let description = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        roas_arazzo_executor::required_sources(&description, &options)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["api", "unrelated"]
    );
    assert!(
        prepare(&description, &options)
            .unwrap_err()
            .to_string()
            .contains("unrelated")
    );
    value["workflows"][0]["steps"][0]
        .as_object_mut()
        .unwrap()
        .remove("operationId");
    for path in [
        "{$sourceDescriptions.api.url}#/paths/~1check/get",
        "https://example.com/openapi.json#/paths/~1check/get",
    ] {
        value["workflows"][0]["steps"][0]["operationPath"] = json!(path);
        let description = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            roas_arazzo_executor::required_sources(&description, &options)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["api"]
        );
        assert!(prepare(&description, &options).is_ok());
    }
}

#[test]
fn checked_capabilities_and_reference_errors_are_located_on_skipped_branches() {
    let cases = [
        (
            json!({ "condition": "true || $message.payload.id" }),
            "AsyncAPI",
        ),
        (
            json!({ "condition": "$sourceDescriptions.typo.url" }),
            "source description",
        ),
        (json!({ "condition": "$self" }), "does not set"),
        (
            json!({ "condition": "$components.parameters.typo" }),
            "component",
        ),
        (
            json!({ "type": "xpath", "context": "$response.body", "condition": "true()" }),
            "XPath",
        ),
        (
            json!({ "type": { "type": "xpath", "version": "xpath-31" }, "context": "$response.body", "condition": "true()" }),
            "XPath",
        ),
        (
            json!({ "type": { "type": "jsonpath", "version": "rfc9535" }, "context": "$response.body", "condition": "$[" }),
            "JSONPath",
        ),
        (
            json!({ "type": { "type": "jsonpointer", "version": "rfc6901" }, "context": "$response.body", "condition": "/~3" }),
            "JSON Pointer",
        ),
    ];
    let options = options();
    for (criterion, expected) in cases {
        let description = serde_json::from_value(document(
            json!([{ "stepId": "a", "operationId": "check", "onSuccess": [
            { "name": "done", "type": "end" },
            { "name": "unreachable", "type": "end", "criteria": [criterion] }
        ] }]),
        ))
        .unwrap();
        let error = prepare(&description, &options).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(
            error
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.path.contains("onSuccess[1].criteria[0]"))
        );
    }
}

#[test]
fn unknown_operations_workflows_targets_and_dependencies_are_static_errors() {
    for step in [
        json!({ "stepId": "a", "workflowId": "missing" }),
        json!({ "stepId": "a", "workflowId": "$sourceDescriptions.external.w" }),
        json!({ "stepId": "a", "operationId": "$inputs.operation" }),
        json!({ "stepId": "a", "operationId": "$sourceDescriptions.api" }),
        json!({ "stepId": "a", "operationId": "$sourceDescriptions.typo.check" }),
        json!({ "stepId": "a", "channelPath": "$sourceDescriptions.api.channel", "action": "send" }),
        json!({ "stepId": "a", "operationPath": "noFragment" }),
        json!({ "stepId": "a", "operationPath": "unknown.json#/paths/~1check/get" }),
        json!({ "stepId": "a", "operationId": "check", "dependsOn": ["unknown"] }),
        json!({ "stepId": "a", "operationId": "check", "dependsOn": ["a"] }),
        json!({ "stepId": "a", "operationId": "check", "onFailure": [{ "name": "go", "type": "goto", "stepId": "typo" }] }),
        json!({ "stepId": "a", "operationId": "check", "onSuccess": [{ "reference": "$components.successActions.absent" }] }),
        json!({ "stepId": "a", "operationId": "check", "onFailure": [{ "reference": "$components.failureActions.absent" }] }),
    ] {
        let description = serde_json::from_value(document(json!([step]))).unwrap();
        assert!(prepare(&description, &options()).is_err());
        assert!(roas_arazzo_executor::required_sources(&description, &options()).is_err());
    }
    let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
    value["workflows"][0]["dependsOn"] = json!(["w"]);
    assert!(
        prepare(&serde_json::from_value(value.clone()).unwrap(), &options())
            .unwrap_err()
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(
                &diagnostic.issue,
                PreparationIssue::Execution(ExecutionError::Circular(_))
            ))
    );
    value["workflows"][0]["dependsOn"] = json!(["missing"]);
    assert!(prepare(&serde_json::from_value(value).unwrap(), &options()).is_err());
}

#[test]
fn effective_reusable_overrides_keep_provenance_and_do_not_hide_sibling_errors() {
    let mut value = document(json!([
        { "stepId": "a", "operationId": "check", "parameters": [{ "reference": "$components.parameters.shared", "value": 7 }] }
    ]));
    value["components"] = json!({ "parameters": { "shared": { "name": "id", "in": "query", "value": "$statusCode.invalid" } } });
    value["workflows"][0]["parameters"] =
        json!([{ "name": "id", "in": "query", "value": "$steps.typo.outputs.value" }]);
    let options = options();
    let description = serde_json::from_value(value.clone()).unwrap();
    let report = prepare(&description, &options)
        .unwrap()
        .execute(&mut Fake::new().reply(200, &json!({})))
        .unwrap();
    assert!(report.is_success());
    value["workflows"][0]["steps"][0]["parameters"] = json!([
        { "reference": "$components.parameters.shared" },
        { "reference": "$components.parameters.missing" },
        { "name": "independent", "in": "query", "value": "$alsoInvalid" }
    ]);
    let description = serde_json::from_value(value).unwrap();
    let error = prepare(&description, &options).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| diagnostic.path
        == "#.components.parameters.shared.value"
        && diagnostic.step_id.as_deref() == Some("a")));
    assert!(error.to_string().contains("parameters[1]"));
    assert!(error.to_string().contains("parameters[2].value"));
}

#[test]
fn workflow_call_overrides_use_names_and_reusable_actions_use_the_callers_scope() {
    let mut value = document(
        json!([{ "stepId": "call", "workflowId": "child", "parameters": [{ "name": "id", "value": 7 }],
            "onSuccess": [{ "reference": "$components.successActions.done" }]
        }]),
    );
    value["workflows"][0]["parameters"] =
        json!([{ "name": "id", "in": "header", "value": "$steps.typo.outputs.value" }]);
    value["components"] = json!({ "successActions": { "done": { "name": "done", "type": "end", "criteria": [{ "condition": "$steps.call.outputs.id == 7" }] } } });
    value["workflows"].as_array_mut().unwrap().push(json!({ "workflowId": "child", "steps": [{ "stepId": "read", "operationId": "check" }], "outputs": { "id": "$inputs.id" } }));
    let description = serde_json::from_value(value.clone()).unwrap();
    assert!(
        prepare(&description, &options())
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})))
            .unwrap()
            .is_success()
    );
    value["components"]["successActions"]["done"]["criteria"][0]["condition"] =
        json!("$steps.read.outputs.id == 7");
    let error = prepare(&serde_json::from_value(value).unwrap(), &options()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("#.components.successActions.done.criteria[0].condition")
    );
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.workflow_id.as_deref() == Some("w")
                && diagnostic.step_id.as_deref() == Some("call"))
    );
}

#[test]
fn compiled_selectors_payloads_and_interpolations_keep_value_semantics() {
    let mut value = document(
        json!([{ "stepId": "a", "operationId": "check", "requestBody": {
        "payload": { "values": ["literal", "{$inputs.n}", "$inputs.n"], "append": null },
        "replacements": [{ "target": "/append", "value": { "type": "jsonpointer", "context": "$inputs", "selector": "/n" } }]
    }, "successCriteria": [
        { "type": "regex", "context": "$statusCode", "condition": "^200$" },
        { "type": { "type": "jsonpath", "version": "rfc9535" }, "context": "$response.body", "condition": "$.value" },
        { "type": { "type": "jsonpointer", "version": "rfc6901" }, "context": "$response.body", "condition": "/value" }
    ], "outputs": { "value": { "type": "jsonpath", "context": "$response.body", "selector": "$.value" } }
    } ]),
    );
    value["workflows"][0]["inputs"] = json!({ "type": "object" });
    let description = serde_json::from_value(value).unwrap();
    let options = options().input("n", 8);
    let plan = prepare(&description, &options).unwrap();
    assert_eq!(plan.workflow().inputs, Some(json!({ "type": "object" })));
    let mut client = Fake::new().reply(200, &json!({ "value": null }));
    let report = plan.execute(&mut client).unwrap();
    assert!(report.is_success());
    assert_eq!(report.steps[0].outputs["value"], Value::Null);
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[0].body.as_ref().unwrap()).unwrap(),
        json!({ "values": ["literal", "8", 8], "append": 8 })
    );
}

#[tokio::test]
async fn blocking_async_and_manual_drivers_share_the_checked_plan() {
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": "$statusCode == 200" }], "onFailure": [{ "name": "retry", "type": "retry", "retryAfter": 0.001, "retryLimit": 1 }] }]))).unwrap();
    let options = options();
    let plan = prepare(&description, &options).unwrap();
    let mut blocking = Fake::new().reply(503, &json!({})).reply(200, &json!({}));
    let mut asynchronous = Fake::new().reply(503, &json!({})).reply(200, &json!({}));
    let report = plan.execute(&mut blocking).unwrap();
    let other = plan.execute_async(&mut asynchronous).await.unwrap();
    assert!(report.is_success() && other.is_success());
    assert_eq!(blocking.sent().len(), asynchronous.sent().len());
    let mut run = plan.start().unwrap();
    assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
    run.supply(roas_arazzo_executor::HttpResponse::json(503, &json!({})))
        .unwrap();
    assert!(matches!(run.advance().unwrap(), Progress::Wait(_)));
    assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
    run.supply(roas_arazzo_executor::HttpResponse::json(200, &json!({})))
        .unwrap();
    assert_eq!(run.partial_report().steps.len(), report.steps.len());
    let failure = plan.execute_async(&mut Fake::new()).await.unwrap_err();
    assert!(matches!(failure.error, ExecutionError::Client(_)));
    assert!(failure.report.unwrap().steps.is_empty());
    let options = options.max_depth(0);
    let plan = prepare(&description, &options).unwrap();
    assert!(plan.execute(&mut Fake::new()).unwrap_err().report.is_none());
    assert!(
        plan.execute_async(&mut Fake::new())
            .await
            .unwrap_err()
            .report
            .is_none()
    );
}

#[test]
fn model_validation_is_global_and_its_existing_exceptions_remain_available() {
    let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
    value["info"]["title"] = json!("");
    let description = serde_json::from_value(value).unwrap();
    assert!(prepare(&description, &options()).unwrap_err().diagnostics.iter().any(|diagnostic| matches!(&diagnostic.issue, PreparationIssue::Model(error) if error.path == "#.info.title")));
    let options = options().validation_options(enumset::EnumSet::only(
        roas_arazzo::validation::ValidationOptions::IgnoreEmptyInfoTitle,
    ));
    assert!(prepare(&description, &options).is_ok());
    assert!(prepare(&description, &options.workflow("absent")).is_err());
    assert!(prepare(&Description::default(), &Options::new()).is_err());
}

#[test]
fn root_dependencies_share_fresh_inputs_and_nested_dependency_scheduling_is_explicit() {
    let mut value = document(
        json!([{ "stepId": "main", "operationId": "check", "outputs": { "value": "$inputs.value" } }]),
    );
    value["workflows"][0]["dependsOn"] = json!(["dependency"]);
    value["workflows"].as_array_mut().unwrap().push(json!({ "workflowId": "dependency", "steps": [{ "stepId": "dep", "operationId": "check", "outputs": { "value": "$inputs.value" } }] }));
    let options = options().input("value", 1);
    let description = serde_json::from_value(value.clone()).unwrap();
    let plan = prepare(&description, &options).unwrap();
    let mut run = plan
        .start_with_inputs(serde_json::from_value(json!({ "value": 2 })).unwrap())
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
        run.supply(roas_arazzo_executor::HttpResponse::json(200, &json!({})))
            .unwrap();
    }
    let Progress::Done(report) = run.advance().unwrap() else {
        panic!("done")
    };
    assert_eq!(report.steps[0].workflow_id, "dependency");
    assert_eq!(report.steps[1].workflow_id, "w");
    assert!(
        report
            .steps
            .iter()
            .all(|step| step.outputs["value"] == json!(2))
    );
    value["workflows"].as_array_mut().unwrap().push(
        json!({ "workflowId": "caller", "steps": [{ "stepId": "call", "workflowId": "w" }] }),
    );
    let description = serde_json::from_value(value).unwrap();
    assert!(
        prepare(&description, &options.workflow("caller"))
            .unwrap_err()
            .to_string()
            .contains("dependency scheduling for nested calls")
    );
}

#[test]
fn selector_and_replacement_capabilities_and_syntax_are_checked() {
    for selector in [
        json!({ "type": "xpath", "context": "$response.body", "selector": "/x" }),
        json!({ "type": "jsonpath", "context": "$response.body", "selector": "$[" }),
        json!({ "type": "jsonpointer", "context": "$response.body", "selector": "invalid" }),
        json!({ "type": { "type": "jsonpath", "version": "draft-goessner-dispatch-jsonpath-00" }, "context": "$response.body", "selector": "$.x" }),
    ] {
        let description = serde_json::from_value(document(
            json!([{ "stepId": "a", "operationId": "check", "outputs": { "selected": selector } }]),
        ))
        .unwrap();
        let error = prepare(&description, &options()).unwrap_err();
        assert!(error.to_string().contains("outputs.selected"));
    }
    for kind in [
        json!("xpath"),
        json!({ "type": "jsonpath", "version": "draft-goessner-dispatch-jsonpath-00" }),
    ] {
        let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "requestBody": { "payload": {}, "replacements": [
            { "target": "/x", "targetSelectorType": kind, "value": 1 }
        ] } }]))).unwrap();
        assert!(
            prepare(&description, &options())
                .unwrap_err()
                .to_string()
                .contains("targetSelectorType")
        );
    }
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "successCriteria": [
        { "type": { "type": "jsonpath", "version": "draft-goessner-dispatch-jsonpath-00" }, "context": "$response.body", "condition": "{$inputs.pattern}" }
    ] }]))).unwrap();
    assert!(
        prepare(&description, &options())
            .unwrap_err()
            .to_string()
            .contains("type.version")
    );
}

#[test]
fn parameter_locations_and_static_request_templates_fail_before_sending() {
    for parameter in [
        json!({ "name": "x", "in": "channel", "value": 1 }),
        json!({ "name": "x", "value": 1 }),
    ] {
        let mut value = document(
            json!([{ "stepId": "a", "operationId": "check", "parameters": [{ "reference": "$components.parameters.p" }] }]),
        );
        value["components"] = json!({ "parameters": { "p": parameter } });
        assert!(prepare(&serde_json::from_value(value).unwrap(), &options()).is_err());
    }
    let description =
        serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check" }])))
            .unwrap();
    assert!(
        prepare(&description, &options().base_url("api", "not a URL"))
            .unwrap_err()
            .to_string()
            .contains("not a URL")
    );
    let options = Options::new().source("api", "https://example.com/openapi.json", json!({ "openapi": "3.1.0", "servers": [{ "url": "https://example.com" }], "paths": { "/check/{id}": { "get": { "operationId": "check" } } } }));
    assert!(
        prepare(&description, &options)
            .unwrap_err()
            .to_string()
            .contains("no parameter filled in")
    );
}

#[test]
fn source_kinds_and_missing_supplied_sources_are_reported_without_io() {
    let mut value =
        document(json!([{ "stepId": "a", "operationId": "$sourceDescriptions.api.check" }]));
    let description = serde_json::from_value(value.clone()).unwrap();
    assert!(prepare(&description, &Options::new()).is_err());
    for kind in ["arazzo", "asyncapi"] {
        value["sourceDescriptions"][0]["type"] = json!(kind);
        assert!(
            roas_arazzo_executor::required_sources(
                &serde_json::from_value(value.clone()).unwrap(),
                &Options::new()
            )
            .unwrap_err()
            .to_string()
            .contains("supported HTTP operations")
        );
    }
}

#[test]
fn reusable_failure_actions_validate_each_argument_and_preserve_overrides() {
    let mut value = document(
        json!([{ "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": "false" }] }]),
    );
    value["workflows"][0]["failureActions"] =
        json!([{ "reference": "$components.failureActions.recover" }]);
    value["components"] = json!({ "parameters": { "p": { "name": "x", "value": "$steps.typo.outputs.x" } },
        "failureActions": { "recover": { "name": "recover", "type": "goto", "workflowId": "child", "parameters": [{ "reference": "$components.parameters.p", "value": 7 }] } } });
    value["workflows"].as_array_mut().unwrap().push(json!({ "workflowId": "child", "steps": [{ "stepId": "b", "operationId": "check", "successCriteria": [{ "condition": "$inputs.x == 7" }] }] }));
    let description = serde_json::from_value(value.clone()).unwrap();
    assert!(
        prepare(&description, &options())
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})).reply(200, &json!({})))
            .unwrap()
            .is_success()
    );
    value["components"]["failureActions"]["recover"]["parameters"] = json!([{ "reference": "$components.parameters.missing" }, { "reference": "$components.parameters.p" }]);
    let error = prepare(&serde_json::from_value(value).unwrap(), &options()).unwrap_err();
    assert!(error.to_string().contains("parameters[0]"));
    assert!(
        error
            .to_string()
            .contains("#.components.parameters.p.value")
    );
}

#[test]
fn declared_components_and_self_are_static_symbols_not_runtime_dependencies() {
    let mut value = document(json!([{ "stepId": "a", "operationId": "check", "outputs": {
        "p": "$components.parameters.p", "s": "$components.successActions.s", "f": "$components.failureActions.f", "i": "$components.inputs.i", "self": "$self", "source": "$sourceDescriptions.api.url"
    } }]));
    value["$self"] = json!("urn:example:workflow");
    value["components"] = json!({ "parameters": { "p": { "name": "p", "in": "query", "value": 7 } },
        "successActions": { "s": { "name": "s", "type": "end" } }, "failureActions": { "f": { "name": "f", "type": "end" } }, "inputs": { "i": { "type": "object" } }
    });
    let description = serde_json::from_value(value.clone()).unwrap();
    assert!(
        prepare(&description, &options())
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})))
            .unwrap()
            .is_success()
    );
    for expression in [
        "$components.inputs.absent",
        "$components.successActions.absent",
        "$components.failureActions.absent",
    ] {
        value["workflows"][0]["steps"][0]["outputs"]["i"] = json!(expression);
        assert!(prepare(&serde_json::from_value(value.clone()).unwrap(), &options()).is_err());
    }
}

#[test]
fn profile_warnings_do_not_change_repeated_condition_evaluation_or_invalid_offsets() {
    let options = options().portability_lints(true);
    for condition in ["true", "false", "1 == 1", "!false"] {
        let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": condition }, { "condition": condition }] }]))).unwrap();
        assert!(
            prepare(&description, &options)
                .unwrap()
                .diagnostics()
                .is_empty()
        );
    }
    for condition in ["!0", "true && 1", "false || 'x'"] {
        let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": condition }] }]))).unwrap();
        assert!(
            !prepare(&description, &options)
                .unwrap()
                .diagnostics()
                .is_empty()
        );
    }
    let condition = "true || $steps.typo.outputs.x || $steps.typo.outputs.x";
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": condition }] }]))).unwrap();
    let error = prepare(&description, &options).unwrap_err();
    let offsets = error
        .diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.offset)
        .collect::<Vec<_>>();
    assert_eq!(offsets, [8, 33]);
}

#[test]
fn checked_implicit_dependencies_match_legacy_order_and_report_cycles() {
    let mut value = document(json!([
        { "stepId": "b", "operationId": "check", "successCriteria": [{ "condition": "$steps.a.outputs.ready == true" }],
          "parameters": [{ "name": "value", "in": "query", "value": "$steps.a.outputs.ready" }] },
        { "stepId": "a", "operationId": "check", "outputs": { "ready": true } }
    ]));
    let description = serde_json::from_value(value.clone()).unwrap();
    let options = options();
    let prepared = prepare(&description, &options)
        .unwrap()
        .execute(&mut Fake::new().reply(200, &json!({})).reply(200, &json!({})))
        .unwrap();
    let legacy = roas_arazzo_executor::execute(
        &description,
        &options,
        &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
    )
    .unwrap();
    assert!(prepared.is_success() && legacy.is_success());
    assert_eq!(
        prepared
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        prepared
            .steps
            .iter()
            .map(|step| &step.step_id)
            .collect::<Vec<_>>(),
        legacy
            .steps
            .iter()
            .map(|step| &step.step_id)
            .collect::<Vec<_>>()
    );
    value["workflows"][0]["steps"][1]["successCriteria"] =
        json!([{ "condition": "$steps.b.outputs.value" }]);
    let error = prepare(&serde_json::from_value(value).unwrap(), &options).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.issue,
        PreparationIssue::Execution(ExecutionError::Circular(_))
    )));
}

#[test]
fn unrepresentable_retry_delays_are_preparation_errors() {
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "onFailure": [{ "name": "retry", "type": "retry", "retryAfter": 1e308 }] }]))).unwrap();
    assert!(
        prepare(&description, &options())
            .unwrap_err()
            .to_string()
            .contains("retryAfter")
    );
}

#[test]
fn unterminated_interpolation_is_literal_in_both_execution_paths() {
    let description = serde_json::from_value(document(json!([{ "stepId": "a", "operationId": "check", "parameters": [{ "name": "x", "in": "query", "value": "literal {$inputs.unclosed" }] }]))).unwrap();
    let options = options();
    let mut checked = Fake::new().reply(200, &json!({}));
    let mut legacy = Fake::new().reply(200, &json!({}));
    prepare(&description, &options)
        .unwrap()
        .execute(&mut checked)
        .unwrap();
    roas_arazzo_executor::execute(&description, &options, &mut legacy).unwrap();
    assert_eq!(checked.sent()[0].url, legacy.sent()[0].url);
}

#[test]
fn preparation_preserves_send_and_sync_for_plans_and_runs() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<roas_arazzo_executor::PreparedWorkflow<'static>>();
    assert_send_sync::<roas_arazzo_executor::Run<'static>>();
}

#[test]
fn condition_offsets_ignore_quoted_lookalikes_and_survive_ast_reuse() {
    let condition = "'é $steps.typo.outputs.x' == 'x' || $steps.typo.outputs.x";
    let description = serde_json::from_value(document(json!([
        { "stepId": "a", "operationId": "check", "successCriteria": [
            { "condition": condition }, { "condition": condition }
        ] }
    ])))
    .unwrap();
    let error = prepare(&description, &options()).unwrap_err();
    let offsets = error
        .diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.offset)
        .collect::<Vec<_>>();
    let expected = condition.rfind("$steps.typo").unwrap();
    assert_eq!(offsets, [expected, expected]);
}

#[test]
fn expression_references_use_lookup_errors_not_goto_errors() {
    for expression in ["$steps.typo.outputs.value", "$workflows.typo.outputs.value"] {
        let description = serde_json::from_value(document(json!([
            { "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": expression }] }
        ]))).unwrap();
        let options = options();
        let error = prepare(&description, &options).unwrap_err();
        let PreparationIssue::Execution(ExecutionError::Expression(prepared)) =
            &error.diagnostics[0].issue
        else {
            panic!("expected an expression lookup error: {error}");
        };
        assert!(
            matches!(prepared, ExpressionError::Missing { expression: actual, .. } if actual == expression)
        );
        assert!(!error.to_string().contains("to go to"));
        let lazy = roas_arazzo_executor::execute_with_report(
            &description,
            &options,
            &mut Fake::new().reply(200, &json!({})),
        )
        .unwrap();
        let Some(CriterionError::Expression(runtime)) = &lazy.steps[0].criteria[0].error else {
            panic!("expected a runtime lookup error");
        };
        assert_eq!(prepared, runtime);
    }
    let description = serde_json::from_value(document(json!([
        { "stepId": "a", "operationId": "check", "onSuccess": [{ "name": "jump", "type": "goto", "stepId": "typo" }] }
    ]))).unwrap();
    assert!(prepare(&description, &options()).unwrap_err().diagnostics.iter().any(|diagnostic|
        matches!(&diagnostic.issue, PreparationIssue::Execution(ExecutionError::UnknownStep { step, .. }) if step == "typo")
    ));
}

#[test]
fn syntax_offsets_are_structured_and_displayed_once() {
    for condition in ["'é' == 'é' && (", "'é' == 'é' && $steps."] {
        let description = serde_json::from_value(document(json!([
            { "stepId": "a", "operationId": "check", "successCriteria": [{ "condition": condition }] }
        ]))).unwrap();
        let options = options();
        let error = prepare(&description, &options).unwrap_err();
        let diagnostic = &error.diagnostics[0];
        let PreparationIssue::Execution(ExecutionError::Criterion(CriterionError::Syntax {
            offset,
            message,
            ..
        })) = &diagnostic.issue
        else {
            panic!("expected condition syntax error: {error}");
        };
        assert_eq!(diagnostic.offset, Some(*offset));
        assert_eq!(*offset, condition.len());
        assert!(!message.contains("at byte"));
        assert_eq!(diagnostic.to_string().matches("at byte").count(), 1);
        assert_eq!(diagnostic.issue.to_string().matches("at byte").count(), 1);
        let lazy =
            roas_arazzo_executor::execute(&description, &options, &mut Fake::new()).unwrap_err();
        let ExecutionError::Criterion(CriterionError::Syntax {
            offset: lazy_offset,
            ..
        }) = lazy
        else {
            panic!("expected lazy condition syntax error");
        };
        assert_eq!(*offset, lazy_offset);
    }
    let value = "é {$statusCode.extra}";
    let description = serde_json::from_value(document(json!([
        { "stepId": "a", "operationId": "check", "parameters": [{ "name": "x", "in": "query", "value": value }] }
    ]))).unwrap();
    let error = prepare(&description, &options()).unwrap_err();
    let diagnostic = &error.diagnostics[0];
    let PreparationIssue::Execution(ExecutionError::Expression(ExpressionError::Syntax {
        offset,
        ..
    })) = &diagnostic.issue
    else {
        panic!("expected runtime-expression syntax error");
    };
    assert_eq!(diagnostic.offset, Some(value.find('.').unwrap()));
    assert_ne!(
        diagnostic.offset,
        Some(*offset),
        "field and embedded-expression offsets differ"
    );
    assert_eq!(diagnostic.to_string().matches("at byte").count(), 1);
    assert!(
        diagnostic
            .to_string()
            .contains(&format!("at byte {}:", value.find('.').unwrap()))
    );
    assert_eq!(diagnostic.issue.to_string().matches("at byte").count(), 1);
}

#[test]
fn shared_cached_conditions_are_checked_in_each_workflow_scope() {
    let condition = "true || $steps.only_in_a.outputs.value";
    let mut value = document(json!([
        { "stepId": "only_in_a", "operationId": "check", "outputs": { "value": true } },
        { "stepId": "call", "workflowId": "b" }
    ]));
    value["components"] = json!({ "successActions": { "done": {
        "name": "done", "type": "end", "criteria": [{ "condition": condition }]
    } } });
    value["workflows"][0]["successActions"] =
        json!([{ "reference": "$components.successActions.done" }]);
    value["workflows"].as_array_mut().unwrap().push(json!({
        "workflowId": "b", "steps": [{ "stepId": "only_in_b", "operationId": "check" }],
        "successActions": [{ "reference": "$components.successActions.done" }]
    }));
    let error = prepare(&serde_json::from_value(value.clone()).unwrap(), &options()).unwrap_err();
    assert_eq!(error.diagnostics.len(), 1);
    assert_eq!(error.diagnostics[0].workflow_id.as_deref(), Some("b"));
    assert_eq!(error.diagnostics[0].step_id, None);
    assert_eq!(
        error.diagnostics[0].path,
        "#.components.successActions.done.criteria[0].condition"
    );
    value["workflows"][1]["steps"][0]["stepId"] = json!("only_in_a");
    let description = serde_json::from_value(value).unwrap();
    assert!(
        prepare(&description, &options())
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),)
            .unwrap()
            .is_success()
    );
}

#[test]
fn diagnostic_paths_sort_array_indices_numerically() {
    let steps = (0..12).map(|step| json!({
        "stepId": format!("step{step}"), "operationId": "check",
        "successCriteria": (0..12).map(|_| json!({ "condition": "$steps.typo.outputs.value" })).collect::<Vec<_>>()
    })).collect::<Vec<_>>();
    let error = prepare(
        &serde_json::from_value(document(json!(steps))).unwrap(),
        &options(),
    )
    .unwrap_err();
    let paths = error
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.path.clone())
        .collect::<Vec<_>>();
    let expected = (0..12)
        .flat_map(|step| {
            (0..12).map(move |criterion| {
                format!("#.workflows[0].steps[{step}].successCriteria[{criterion}].condition")
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(paths, expected);
}

#[test]
#[cfg(feature = "v1_0")]
fn preparation_accepts_v1_0_after_upconversion() {
    let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
    value["arazzo"] = json!("1.0.1");
    let legacy: roas_arazzo::v1_0::Description = serde_json::from_value(value).unwrap();
    let description = Description::from(legacy);
    assert!(
        prepare(&description, &options())
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})))
            .unwrap()
            .is_success()
    );
}
