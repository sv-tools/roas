//! Runtime criterion failures follow workflow recovery; engine failures retain history.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    CriterionError, ExecutionError, ExecutionReport, ExpressionError, HttpResponse, Options,
    Outcome, Performed, Progress, Run, SelectError, execute, execute_async,
    execute_async_with_report, execute_with_report, testing::Fake,
};
use serde_json::{Value, json};

fn document(steps: Value) -> Value {
    json!({
        "arazzo": "1.1.0", "info": { "title": "Recovery", "version": "1" },
        "sourceDescriptions": [{ "name": "api", "url": "https://example.com/openapi.json", "type": "openapi" }],
        "workflows": [{ "workflowId": "w", "steps": steps }]
    })
}

fn description(steps: Value) -> Description {
    serde_json::from_value(document(steps)).unwrap()
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
fn missing_success_data_can_recover_with_a_retry() {
    let description = description(json!([{
        "stepId": "read", "operationId": "check",
        "successCriteria": [{ "condition": "$response.body.ready" }],
        "onFailure": [{ "name": "again", "type": "retry", "retryLimit": 1 }]
    }]));
    let mut client = Fake::new()
        .reply(200, &json!({}))
        .reply(200, &json!({ "ready": true }));
    let report = execute(&description, &options(), &mut client).unwrap();
    assert!(report.is_success());
    assert_eq!(client.sent().len(), 2);
    assert_eq!(report.steps.len(), 2);
    assert!(!report.steps[0].passed);
    assert_eq!(report.steps[0].attempt, 1);
    assert!(report.steps[1].passed);
    assert_eq!(report.steps[1].attempt, 2);
    assert!(matches!(
        report.steps[0].criteria[0].error,
        Some(CriterionError::Expression(ExpressionError::Missing { .. }))
    ));
    assert!(report.steps[1].criteria[0].error.is_none());
}

#[test]
fn an_errored_action_criterion_allows_the_next_action() {
    let description = description(json!([
        {
            "stepId": "read", "operationId": "check",
            "onFailure": [
                { "name": "broken", "type": "end", "criteria": [{ "condition": "$response.body.absent == null" }] },
                { "name": "recover", "type": "goto", "stepId": "recover" }
            ]
        },
        { "stepId": "recover", "operationId": "check" }
    ]));
    let mut client = Fake::new().reply(500, &json!({})).reply(200, &json!({}));
    let report = execute(&description, &options(), &mut client).unwrap();
    assert!(report.is_success());
    assert_eq!(report.steps.len(), 2);
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("goto step `recover`")
    );
    let actions = &report.steps[0].action_criteria;
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].name, "broken");
    assert!(!actions[0].passed);
    assert!(matches!(
        actions[0].criteria[0].error,
        Some(CriterionError::Expression(ExpressionError::Missing { .. }))
    ));
    assert!(actions[1].passed);
    assert!(report.to_string().contains("action `broken` criterion"));
}

#[test]
fn unsupported_criteria_are_terminal_even_with_recovery_available() {
    for criterion in [
        json!({ "condition": "/root", "type": "xpath", "context": "$response.body" }),
        json!({ "condition": "/root", "type": { "type": "xpath", "version": "xpath-31" }, "context": "$response.body" }),
        json!({ "condition": "$message.header.foo" }),
    ] {
        for in_action in [false, true] {
            let mut value = document(json!([{
                "stepId": "read", "operationId": "check",
                "onFailure": [{ "name": "again", "type": "retry" }]
            }]));
            if in_action {
                value["workflows"][0]["steps"][0]["onSuccess"] = json!([
                    { "name": "unsupported", "type": "end", "criteria": [criterion] },
                    { "name": "fallback", "type": "end" }
                ]);
            } else {
                value["workflows"][0]["steps"][0]["successCriteria"] = json!([criterion]);
            }
            let description = serde_json::from_value(value).unwrap();
            let mut client = Fake::new().reply(200, &json!({}));
            let failure = execute_with_report(&description, &options(), &mut client).unwrap_err();
            assert!(matches!(
                failure.error,
                ExecutionError::Criterion(
                    CriterionError::Unsupported(_)
                        | CriterionError::Expression(ExpressionError::Unsupported(_))
                )
            ));
            let report = failure.report.unwrap();
            assert_eq!(report.outcome, Outcome::Incomplete);
            assert!(!report.is_success());
            assert_eq!(report.steps.len(), 1);
            assert_eq!(client.sent().len(), 1);
            assert!(report.steps[0].action.is_none());
            if in_action {
                assert!(
                    report.steps[0].action_criteria[0].criteria[0]
                        .error
                        .is_some()
                );
            } else {
                assert!(report.steps[0].criteria[0].error.is_some());
            }
        }
    }
}

#[test]
fn terminal_output_and_action_errors_keep_the_response_and_prior_steps() {
    for broken in [
        json!({ "outputs": { "missing": "$response.body#/missing" } }),
        json!({ "onSuccess": [{ "reference": "$components.successActions.missing" }] }),
        json!({ "onSuccess": [{ "name": "badTarget", "type": "goto", "stepId": "absent" }] }),
        json!({ "onSuccess": [{ "name": "badArgs", "type": "goto", "workflowId": "child", "parameters": [{ "name": "missing", "value": "$inputs.missing" }] }] }),
    ] {
        let mut value = document(json!([
            { "stepId": "before", "operationId": "check", "outputs": { "kept": 7 } },
            { "stepId": "broken", "operationId": "check", "successCriteria": [{ "condition": "true" }] }
        ]));
        value["workflows"][0]["steps"][1]
            .as_object_mut()
            .unwrap()
            .extend(broken.as_object().unwrap().clone());
        value["workflows"].as_array_mut().unwrap().push(
            json!({ "workflowId": "child", "steps": [{ "stepId": "c", "operationId": "check" }] }),
        );
        let description = serde_json::from_value(value).unwrap();
        let mut client = Fake::new().reply(200, &json!({})).reply(201, &json!({}));
        let failure = execute_with_report(&description, &options(), &mut client).unwrap_err();
        assert!(!matches!(failure.error, ExecutionError::Criterion(_)));
        let report = failure.report.unwrap();
        assert_eq!(report.outcome, Outcome::Incomplete);
        assert_eq!(report.steps.len(), 2);
        assert_eq!(report.steps[0].outputs["kept"], json!(7));
        assert_eq!(report.steps[1].status(), Some(201));
        assert_eq!(report.steps[1].criteria.len(), 1);
        assert!(report.steps[1].criteria[0].passed);
        if broken.get("outputs").is_some() {
            assert!(!report.steps[1].passed);
        }
        assert_eq!(client.sent().len(), 2);
    }
}

#[test]
fn failed_requests_and_unbuilt_steps_do_not_invent_completed_attempts() {
    for second in [
        json!({ "stepId": "next", "operationId": "check" }),
        json!({ "stepId": "next", "operationId": "notFound" }),
        json!({ "stepId": "next", "operationId": "check", "parameters": [{ "name": "missing", "in": "query", "value": "$inputs.missing" }] }),
        json!({ "stepId": "next", "channelPath": "{$sourceDescriptions.api.url}#/channels/events", "action": "send" }),
    ] {
        let description =
            description(json!([{ "stepId": "before", "operationId": "check" }, second]));
        let failure = execute_with_report(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({})),
        )
        .unwrap_err();
        assert!(matches!(
            failure.error,
            ExecutionError::Client(_)
                | ExecutionError::Operation(_)
                | ExecutionError::Select(_)
                | ExecutionError::Unsupported(_)
        ));
        let report = failure.report.unwrap();
        assert_eq!(report.steps.len(), 1);
        assert_eq!(report.steps[0].step_id, "before");
        assert!(!report.is_success());
    }
}

#[test]
fn snapshots_survive_completion_and_correctable_protocol_errors() {
    let description = description(json!([{ "stepId": "read", "operationId": "check" }]));
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert_eq!(run.partial_report().outcome, Outcome::Incomplete);
    assert!(run.partial_report().steps.is_empty());
    assert!(matches!(
        run.supply(HttpResponse::json(200, &json!({}))),
        Err(ExecutionError::NotWaiting)
    ));
    assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::Awaiting { .. })
    ));
    assert!(run.partial_report().steps.is_empty());
    run.supply(HttpResponse::json(200, &json!({}))).unwrap();
    assert_eq!(run.partial_report().steps.len(), 1);
    let Progress::Done(report) = run.advance().unwrap() else {
        panic!("done")
    };
    assert_eq!(run.partial_report(), *report);
    let Progress::Done(repeated) = run.advance().unwrap() else {
        panic!("still done")
    };
    assert_eq!(repeated, report);
}

#[test]
fn a_terminal_supply_error_stops_further_progress_without_losing_the_attempt() {
    let description = description(json!([{
        "stepId": "read", "operationId": "check", "outputs": { "missing": "$response.body#/missing" }
    }]));
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
    assert!(matches!(
        run.supply(HttpResponse::json(200, &json!({}))),
        Err(ExecutionError::Select(_))
    ));
    let report = run.partial_report();
    assert_eq!(report.steps.len(), 1);
    assert!(!report.steps[0].passed);
    assert!(!report.is_success());
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
    assert!(matches!(
        run.supply(HttpResponse::json(200, &json!({}))),
        Err(ExecutionError::Stopped)
    ));
    assert_eq!(run.partial_report(), report);
}

#[test]
fn a_workflow_output_error_after_its_last_step_cannot_turn_into_success() {
    let mut value = document(json!([{ "stepId": "read", "operationId": "check" }]));
    value["workflows"][0]["outputs"] = json!({ "missing": "$inputs.missing" });
    let description = serde_json::from_value(value).unwrap();
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
    run.supply(HttpResponse::json(200, &json!({}))).unwrap();
    assert!(matches!(run.advance(), Err(ExecutionError::Select(_))));
    let report = run.partial_report();
    assert_eq!(report.steps.len(), 1);
    assert!(report.steps[0].passed);
    assert_eq!(report.outcome, Outcome::Incomplete);
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
    assert_eq!(run.partial_report(), report);
}

#[test]
fn preparation_errors_have_no_fabricated_run_report() {
    let description = description(json!([{
        "stepId": "read", "operationId": "check", "successCriteria": [{ "condition": "$statusCode ==" }]
    }]));
    let mut client = Fake::new();
    let failure = execute_with_report(&description, &options(), &mut client).unwrap_err();
    assert!(matches!(
        failure.error,
        ExecutionError::Criterion(CriterionError::Syntax { .. })
    ));
    assert!(failure.report.is_none());
    assert!(client.sent().is_empty());
    assert_eq!(failure.to_string(), failure.error.to_string());
    assert!(std::error::Error::source(&failure).is_some());
}

#[test]
fn nested_criterion_failures_can_recover_in_the_caller() {
    let mut value = document(json!([
        { "stepId": "call", "workflowId": "child",
          "successCriteria": [{ "condition": "$steps.call.outputs.ready" }],
          "onFailure": [{ "name": "recover", "type": "goto", "stepId": "recover" }] },
        { "stepId": "recover", "operationId": "check" }
    ]));
    value["workflows"].as_array_mut().unwrap().push(json!({
        "workflowId": "child", "steps": [{ "stepId": "read", "operationId": "check",
            "successCriteria": [{ "condition": "$response.body.ready" }] }]
    }));
    let description = serde_json::from_value(value).unwrap();
    let report = execute_with_report(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
    )
    .unwrap();
    assert!(report.is_success());
    assert_eq!(report.steps.len(), 3);
    assert_eq!(report.steps[0].workflow_id, "child");
    assert!(report.steps[0].criteria[0].error.is_some());
    assert!(matches!(
        report.steps[1].performed,
        Performed::Workflow {
            outcome: Outcome::Failed,
            ..
        }
    ));
    assert!(report.steps[1].criteria[0].error.is_some());
    assert_eq!(report.steps[2].step_id, "recover");
}

#[test]
fn partial_reports_distinguish_interrupted_and_completed_workflow_calls() {
    for child_completed in [false, true] {
        let mut value = document(json!([{ "stepId": "call", "workflowId": "child" }]));
        value["workflows"].as_array_mut().unwrap().push(json!({
            "workflowId": "child", "steps": [{ "stepId": "read", "operationId": "check" }]
        }));
        if child_completed {
            value["workflows"][0]["steps"][0]["outputs"] = json!({ "missing": "$inputs.missing" });
        } else {
            value["workflows"][1]["outputs"] = json!({ "missing": "$inputs.missing" });
        }
        let description = serde_json::from_value(value).unwrap();
        let failure = execute_with_report(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({})),
        )
        .unwrap_err();
        assert!(matches!(failure.error, ExecutionError::Select(_)));
        let report = failure.report.unwrap();
        assert_eq!(report.workflow_id, "w");
        assert_eq!(report.outcome, Outcome::Incomplete);
        assert_eq!(report.steps.len(), if child_completed { 2 } else { 1 });
        assert_eq!(report.steps[0].workflow_id, "child");
        if child_completed {
            assert!(matches!(
                report.steps[1].performed,
                Performed::Workflow {
                    outcome: Outcome::Succeeded,
                    ..
                }
            ));
            assert!(!report.steps[1].passed, "the caller's outputs failed");
        }
    }
}

#[test]
fn dependency_history_survives_a_later_operation_error() {
    let mut value = document(json!([{ "stepId": "read", "operationId": "unknown" }]));
    value["workflows"][0]["dependsOn"] = json!(["setup"]);
    value["workflows"].as_array_mut().unwrap().push(json!({
        "workflowId": "setup", "steps": [{ "stepId": "prepare", "operationId": "check", "outputs": { "kept": 7 } }]
    }));
    let description = serde_json::from_value(value).unwrap();
    let failure = execute_with_report(
        &description,
        &options().workflow("w"),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::Operation(_)));
    let report = failure.report.unwrap();
    assert_eq!(report.workflow_id, "w");
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].workflow_id, "setup");
    assert_eq!(report.steps[0].outputs["kept"], json!(7));
    assert!(report.to_string().starts_with("workflow `w` incomplete"));
}

#[test]
fn recovery_respects_retry_budgets_and_preserves_history_at_engine_limits() {
    let description = description(json!([{
        "stepId": "read", "operationId": "check",
        "successCriteria": [{ "condition": "$response.body.missing" }],
        "onFailure": [{ "name": "again", "type": "retry", "retryLimit": 1 }]
    }]));
    let report = execute_with_report(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[1].attempt, 2);
    assert!(
        report
            .steps
            .iter()
            .all(|step| step.criteria[0].error.is_some())
    );
    let failure = execute_with_report(
        &description,
        &options().max_steps(1),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap_err();
    assert!(matches!(
        failure.error,
        ExecutionError::Limit {
            limit: "step",
            at: 1
        }
    ));
    let report = failure.report.unwrap();
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].attempt, 1);
    assert_eq!(report.outcome, Outcome::Incomplete);
}

#[tokio::test]
async fn async_drivers_preserve_recovery_waits_and_terminal_history() {
    let doc = description(json!([{
        "stepId": "read", "operationId": "check",
        "successCriteria": [{ "condition": "$response.body.ready" }],
        "onFailure": [{ "name": "again", "type": "retry", "retryAfter": 60, "retryLimit": 1 }]
    }]));
    let mut client = Fake::new()
        .reply(200, &json!({}))
        .reply(200, &json!({ "ready": true }));
    // Fake's async sleep completes immediately, even for this long retry delay.
    let report = execute_async_with_report(&doc, &options(), &mut client)
        .await
        .unwrap();
    assert!(report.is_success());
    assert_eq!(report.steps.len(), 2);
    let report = execute_async(
        &doc,
        &options().max_retries(0),
        &mut Fake::new().reply(200, &json!({})),
    )
    .await
    .unwrap();
    assert_eq!(report.outcome, Outcome::Failed);

    for second in [
        json!({ "stepId": "next", "operationId": "check" }),
        json!({ "stepId": "next", "operationId": "notFound" }),
        json!({ "stepId": "next", "operationId": "check", "outputs": { "bad": "$inputs.missing" } }),
    ] {
        let doc = description(json!([{ "stepId": "first", "operationId": "check" }, second]));
        let got_response = second.get("outputs").is_some();
        let client = Fake::new().reply(200, &json!({}));
        let mut client = if got_response {
            client.reply(200, &json!({}))
        } else {
            client
        };
        let failure = execute_async_with_report(&doc, &options(), &mut client)
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error,
            ExecutionError::Client(_) | ExecutionError::Operation(_) | ExecutionError::Select(_)
        ));
        let report = failure.report.unwrap();
        assert_eq!(report.outcome, Outcome::Incomplete);
        assert_eq!(report.steps.len(), if got_response { 2 } else { 1 });
    }
    let failure = execute_async_with_report(&doc, &options().workflow("absent"), &mut Fake::new())
        .await
        .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::UnknownWorkflow(_)));
    assert!(failure.report.is_none());
}

#[test]
#[cfg(feature = "v1_0")]
fn v1_0_upconversion_uses_the_same_recovery_and_partial_report_policy() {
    let mut value = document(json!([{
        "stepId": "read", "operationId": "check", "successCriteria": [{ "condition": "$response.body.ready" }],
        "onFailure": [{ "name": "again", "type": "retry", "retryLimit": 1 }]
    }]));
    value["arazzo"] = json!("1.0.1");
    let description = serde_json::from_value(value).unwrap();
    let report = roas_arazzo_executor::execute_v1_0_with_report(
        &description,
        &options(),
        &mut Fake::new()
            .reply(200, &json!({}))
            .reply(200, &json!({ "ready": true })),
    )
    .unwrap();
    assert!(report.is_success());
    assert!(report.steps[0].criteria[0].error.is_some());
    let failure = roas_arazzo_executor::execute_v1_0_with_report(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::Client(_)));
    assert_eq!(failure.report.unwrap().steps.len(), 1);
}

fn judge(criterion: Value, body: &Value) -> ExecutionReport {
    let description = description(json!([{
        "stepId": "read", "operationId": "check", "successCriteria": [criterion]
    }]));
    execute(&description, &options(), &mut Fake::new().reply(200, body)).unwrap()
}

#[test]
fn missing_null_and_false_remain_distinct_in_criterion_reports() {
    for criterion in [
        json!({ "condition": "$response.body.value == null" }),
        json!({ "condition": "^null$", "type": "regex", "context": "$response.body#/value" }),
        json!({ "condition": "$", "type": "jsonpath", "context": "$response.body#/value" }),
        json!({ "condition": "$", "type": { "type": "jsonpath", "version": "rfc9535" }, "context": "$response.body#/value" }),
    ] {
        let missing = judge(criterion.clone(), &json!({}));
        assert_eq!(missing.outcome, Outcome::Failed);
        assert!(matches!(
            missing.steps[0].criteria[0].error,
            Some(CriterionError::Expression(ExpressionError::Missing { .. }))
        ));
        let null = judge(criterion.clone(), &json!({ "value": null }));
        assert_eq!(
            null.is_success(),
            criterion.get("type").is_none(),
            "{criterion}"
        );
        assert!(null.steps[0].criteria[0].error.is_none());
    }
    let ordinary_false = judge(json!({ "condition": "false" }), &json!({}));
    assert_eq!(ordinary_false.outcome, Outcome::Failed);
    assert!(ordinary_false.steps[0].criteria[0].error.is_none());
    for value in [json!(false), json!(null), json!([]), json!({})] {
        let selected = judge(
            json!({ "condition": "$.value", "type": "jsonpath", "context": "$response.body" }),
            &json!({ "value": value }),
        );
        assert!(
            selected.is_success(),
            "a selected node is not bare simple truthiness"
        );
    }
}

#[test]
fn runtime_generated_patterns_fail_with_diagnostics_and_recover() {
    for kind in [
        json!("regex"),
        json!("jsonpath"),
        json!({ "type": "jsonpath", "version": "rfc9535" }),
    ] {
        let description = description(json!([
            { "stepId": "read", "operationId": "check",
              "successCriteria": [{ "type": kind, "context": "$response.body", "condition": "{$inputs.pattern}" }],
              "onFailure": [{ "name": "recover", "type": "goto", "stepId": "recover" }] },
            { "stepId": "recover", "operationId": "check" }
        ]));
        let report = execute(
            &description,
            &options().input("pattern", "["),
            &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
        )
        .unwrap();
        assert!(report.is_success());
        assert_eq!(report.steps.len(), 2);
        let criterion = &report.steps[0].criteria[0];
        assert_eq!(criterion.condition, "{$inputs.pattern}");
        assert!(matches!(
            criterion.error,
            Some(
                CriterionError::Regex { .. }
                    | CriterionError::Select(SelectError::Malformed { .. })
            )
        ));
        assert!(
            report
                .to_string()
                .contains("criterion `{$inputs.pattern}`:")
        );
    }
}

#[test]
fn every_success_criterion_is_recorded_but_unused_operands_are_not_evaluated() {
    let doc = description(json!([{
        "stepId": "read", "operationId": "check", "successCriteria": [
            { "condition": "$response.body.missing" },
            { "condition": "true || $response.body.other" },
            { "condition": "$response.body[0]" },
            { "condition": "true" }
        ]
    }]));
    let report = execute(&doc, &options(), &mut Fake::new().reply(200, &json!({}))).unwrap();
    assert_eq!(report.outcome, Outcome::Failed);
    let criteria = &report.steps[0].criteria;
    assert_eq!(criteria.len(), 4);
    assert!(matches!(
        criteria[0].error,
        Some(CriterionError::Expression(ExpressionError::Missing { .. }))
    ));
    assert!(criteria[1].passed && criteria[1].error.is_none());
    assert!(matches!(
        criteria[2].error,
        Some(CriterionError::Expression(
            ExpressionError::Navigation { .. }
        ))
    ));
    assert!(criteria[3].passed && criteria[3].error.is_none());

    let doc = description(json!([{
        "stepId": "read", "operationId": "check",
        "successCriteria": [{ "condition": "true || $response.body.absent" }],
        "onFailure": [{ "name": "mustNotRun", "type": "retry" }]
    }]));
    let report = execute(&doc, &options(), &mut Fake::new().reply(200, &json!({}))).unwrap();
    assert!(report.is_success());
    assert!(report.steps[0].action_criteria.is_empty());
    assert!(report.steps[0].criteria[0].error.is_none());
}

#[test]
fn local_and_shared_action_criteria_fall_through_on_both_outcomes() {
    for (step_field, field, status) in [
        ("onSuccess", "successActions", 200),
        ("onFailure", "failureActions", 500),
    ] {
        for shared in [false, true] {
            let mut value = document(json!([
                { "stepId": "read", "operationId": "check" },
                { "stepId": "recover", "operationId": "check", "onSuccess": [{ "name": "stop", "type": "end" }] }
            ]));
            let actions = json!([
                { "reference": format!("$components.{field}.broken") },
                { "name": "recover", "type": "goto", "stepId": "recover" },
                { "reference": format!("$components.{field}.unreachable") }
            ]);
            value["components"] = json!({ field: { "broken": {
                "name": "broken", "type": "end", "criteria": [
                    { "condition": "$response.body.missing" },
                    { "condition": "/unsupported", "type": "xpath", "context": "$response.body" }
                ]
            } } });
            if shared {
                value["workflows"][0][field] = actions;
            } else {
                value["workflows"][0]["steps"][0][step_field] = actions;
            }
            let description = serde_json::from_value(value).unwrap();
            let report = execute(
                &description,
                &options(),
                &mut Fake::new().reply(status, &json!({})).reply(200, &json!({})),
            )
            .unwrap();
            assert!(report.is_success());
            assert_eq!(report.steps[0].action_criteria.len(), 2);
            assert_eq!(report.steps[0].action_criteria[0].criteria.len(), 1);
            assert!(
                report.steps[0].action_criteria[0].criteria[0]
                    .error
                    .is_some()
            );
        }
    }
}
