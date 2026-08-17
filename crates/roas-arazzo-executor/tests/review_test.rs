//! What the review of the first cut turned up, pinned so it stays
//! fixed: how a called workflow finishes, what `retry` means, and the
//! places the specification is more particular than the first
//! implementation was.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    ExecutionError, HttpResponse, Options, Outcome, Performed, Progress, Run, execute,
    testing::Fake,
};
use serde_json::{Value, json};

fn petstore() -> Value {
    json!({
        "openapi": "3.0.3",
        "servers": [{ "url": "https://api.example.com/v1" }],
        "paths": {
            "/pets": { "get": { "operationId": "listPets" } },
            "/orders": { "post": { "operationId": "placeOrder" } },
            "/login": { "post": { "operationId": "login" } }
        }
    })
}

fn description(workflows: Value) -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "$self": "https://example.com/buy.arazzo.yaml",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": workflows,
    }))
    .expect("a v1.1 description")
}

fn one(steps: Value) -> Description {
    description(json!([{ "workflowId": "w", "steps": steps }]))
}

fn options() -> Options {
    Options::new().source(
        "petStore",
        "https://api.example.com/openapi.json",
        petstore(),
    )
}

// ---- 1: one request outstanding at a time ---------------------------

#[test]
fn advancing_twice_without_answering_is_refused() {
    let description = one(json!([
        { "stepId": "a", "operationId": "listPets" },
        { "stepId": "b", "operationId": "placeOrder" }
    ]));
    let options = options();
    let mut run = Run::start(&description, &options).expect("a run");

    let Progress::Send(first) = run.advance().expect("a request") else {
        panic!("expected a request");
    };
    // Asking again would send a second request while the first is
    // unanswered — and lose the exchange the first is judged by.
    let error = run.advance().unwrap_err();
    assert!(
        matches!(&error, ExecutionError::Awaiting { url, .. } if url == &first.url),
        "got: {error}"
    );

    // Answering it puts the run back in motion.
    run.supply(HttpResponse::json(200, &json!([])))
        .expect("the response is understood");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
}

// ---- 2: a called workflow finishes its caller's step ----------------

#[test]
fn a_calling_step_is_judged_by_its_own_criteria_and_outputs() {
    let description = description(json!([
        {
            "workflowId": "w",
            "steps": [
                {
                    "stepId": "call",
                    "workflowId": "login",
                    "successCriteria": [{
                        "context": "$steps.call.outputs.token",
                        "condition": "^t-",
                        "type": "regex"
                    }],
                    "outputs": { "shout": "TOKEN {$steps.call.outputs.token}" }
                },
                { "stepId": "after", "operationId": "placeOrder" }
            ]
        },
        {
            "workflowId": "login",
            "steps": [{
                "stepId": "post",
                "operationId": "login",
                "outputs": { "token": "$response.body#/token" }
            }],
            "outputs": { "token": "$steps.post.outputs.token" }
        }
    ]));
    let mut client = Fake::new()
        .reply(200, &json!({ "token": "t-1" }))
        .reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    let call = report
        .steps
        .iter()
        .find(|step| step.step_id == "call")
        .expect("the calling step has a record");
    assert!(call.passed);
    assert_eq!(call.criteria.len(), 1, "its own criteria were judged");
    assert_eq!(
        call.outputs["token"],
        json!("t-1"),
        "what it called gave it this"
    );
    assert_eq!(
        call.outputs["shout"],
        json!("TOKEN t-1"),
        "and its own outputs are named on top"
    );
    assert_eq!(report.outcome, Outcome::Succeeded);
    assert_eq!(client.sent().len(), 2);
}

#[test]
fn a_failed_workflow_stops_the_one_that_called_it() {
    let description = description(json!([
        {
            "workflowId": "w",
            "steps": [
                { "stepId": "call", "workflowId": "login" },
                { "stepId": "after", "operationId": "placeOrder" }
            ]
        },
        {
            "workflowId": "login",
            "steps": [{
                "stepId": "post",
                "operationId": "login",
                "successCriteria": [{ "condition": "$statusCode == 200" }]
            }]
        }
    ]));
    let mut client = Fake::new().reply(500, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(client.sent().len(), 1, "the step after the call never ran");
    assert_eq!(report.outcome, Outcome::Failed);
    let call = report
        .steps
        .iter()
        .find(|step| step.step_id == "call")
        .expect("a record for the calling step");
    assert!(!call.passed);
    assert_eq!(
        call.performed,
        Performed::Workflow {
            workflow_id: "login".to_owned(),
            outcome: Outcome::Failed,
        }
    );
}

#[test]
fn a_calling_step_can_recover_from_the_workflow_it_called() {
    let description = description(json!([
        {
            "workflowId": "w",
            "steps": [
                {
                    "stepId": "call",
                    "workflowId": "login",
                    "onFailure": [{ "name": "carryOn", "type": "goto", "stepId": "after" }]
                },
                { "stepId": "after", "operationId": "placeOrder" }
            ]
        },
        {
            "workflowId": "login",
            "steps": [{
                "stepId": "post",
                "operationId": "login",
                "successCriteria": [{ "condition": "$statusCode == 200" }]
            }]
        }
    ]));
    let mut client = Fake::new().reply(500, &json!({})).reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(client.sent().len(), 2, "the caller's own action recovered");
    assert_eq!(report.outcome, Outcome::Succeeded);
}

#[test]
fn a_calling_steps_timeout_applies_to_the_whole_call() {
    let description = description(json!([
        {
            "workflowId": "w",
            "steps": [{ "stepId": "call", "workflowId": "login", "timeout": 0 }]
        },
        {
            "workflowId": "login",
            "steps": [{ "stepId": "post", "operationId": "login" }]
        }
    ]));
    let mut client = Fake::new().reply(200, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(
        report.outcome,
        Outcome::Failed,
        "no call completes within no time at all"
    );
}

// ---- 3: what `retry` means ------------------------------------------

#[test]
fn a_retry_without_a_limit_is_one_retry() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "successCriteria": [{ "condition": "$statusCode == 200" }],
        "onFailure": [{ "name": "again", "type": "retry" }]
    }]));
    let mut client = Fake::new()
        .reply(503, &json!({}))
        .reply(503, &json!({}))
        .reply(503, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(
        client.sent().len(),
        2,
        "the specification says a single retry when none is given"
    );
    assert_eq!(report.outcome, Outcome::Failed);
}

#[test]
fn an_exhausted_retry_gives_way_to_the_next_failure_action() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "onFailure": [
                { "name": "again", "type": "retry", "retryLimit": 2 },
                { "name": "fallback", "type": "goto", "stepId": "orderPet" }
            ]
        },
        { "stepId": "orderPet", "operationId": "placeOrder" }
    ]));
    let mut client = Fake::new()
        .reply(503, &json!({}))
        .reply(503, &json!({}))
        .reply(503, &json!({}))
        .reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(
        client.sent().len(),
        4,
        "one try, two retries, then the action that follows"
    );
    assert!(client.sent()[3].url.ends_with("/orders"));
    assert_eq!(
        report.steps.last().expect("a last step").step_id,
        "orderPet"
    );
    assert_eq!(report.outcome, Outcome::Succeeded);
}

#[test]
fn a_retry_can_run_another_step_first_and_come_back() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "onFailure": [{
                "name": "refresh",
                "type": "retry",
                "stepId": "login",
                "retryLimit": 1
            }],
            // Ends here once it works, so the step the detour ran is
            // not also reached in the ordinary course of the workflow.
            "onSuccess": [{ "name": "done", "type": "end" }]
        },
        { "stepId": "login", "operationId": "login" }
    ]));
    let mut client = Fake::new()
        .reply(401, &json!({}))
        .reply(200, &json!({ "token": "t" }))
        .reply(200, &json!([]));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    let asked: Vec<&str> = client
        .sent()
        .iter()
        .map(|request| request.url.rsplit('/').next().expect("a path"))
        .collect();
    assert_eq!(
        asked,
        ["pets", "login", "pets"],
        "the named step runs, then the step that failed is tried again"
    );
    assert_eq!(report.outcome, Outcome::Ended);
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("retry via step `login`")
    );
}

#[test]
fn a_retry_can_run_another_workflow_first_and_come_back() {
    let description = description(json!([
        {
            "workflowId": "w",
            "steps": [{
                "stepId": "listPets",
                "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 200" }],
                "onFailure": [{
                    "name": "refresh",
                    "type": "retry",
                    "workflowId": "login",
                    "parameters": [{ "name": "who", "value": "ada" }],
                    "retryLimit": 1
                }]
            }]
        },
        {
            "workflowId": "login",
            "steps": [{
                "stepId": "post",
                "operationId": "login",
                "requestBody": { "payload": { "who": "$inputs.who" } }
            }]
        }
    ]));
    let mut client = Fake::new()
        .reply(401, &json!({}))
        .reply(200, &json!({ "token": "t" }))
        .reply(200, &json!([]));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(client.sent().len(), 3);
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "who": "ada" }),
        "the action's parameters reached the workflow it named"
    );
    assert!(client.sent()[2].url.ends_with("/pets"), "then back again");
    assert_eq!(report.outcome, Outcome::Succeeded);
}

// ---- 4: a condition may be written with expressions inside it -------

#[test]
fn a_regex_or_path_condition_is_filled_in_before_it_is_read() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "successCriteria": [
            {
                "context": "$response.body#/name",
                "condition": "^{$inputs.expected}$",
                "type": "regex"
            },
            {
                "context": "$response.body",
                "condition": "$.{$inputs.field}",
                "type": "jsonpath"
            }
        ]
    }]));
    let options = options().input("expected", "fluffy").input("field", "name");
    let mut client = Fake::new().reply(200, &json!({ "name": "fluffy" }));

    let report = execute(&description, &options, &mut client).expect("the run is fine");

    assert_eq!(report.outcome, Outcome::Succeeded);
    assert!(
        report.steps[0]
            .criteria
            .iter()
            .all(|outcome| outcome.passed)
    );
}

// ---- 5: an early stop hides only what never ran ---------------------

#[test]
fn a_stopped_workflow_still_reports_a_broken_output() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }]
        },
        { "stepId": "orderPet", "operationId": "placeOrder" }
    ]));
    let mut description = description;
    description.workflows[0].outputs.insert(
        "broken".to_owned(),
        serde_json::from_value(json!({
            "context": "$inputs",
            "selector": "/x",
            "type": "xpath"
        }))
        .expect("a selector"),
    );
    let mut client = Fake::new().reply(500, &json!({}));

    let error = execute(&description, &options(), &mut client).unwrap_err();
    assert!(
        error.to_string().contains("XPath"),
        "a failed workflow is no reason to keep quiet about a broken output: {error}"
    );
}

#[test]
fn a_stopped_workflow_skips_the_outputs_of_steps_that_never_ran() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "outputs": { "pets": "$response.body" }
        },
        {
            "stepId": "orderPet",
            "operationId": "placeOrder",
            "outputs": { "orderId": "$response.body#/orderId" }
        }
    ]));
    let mut description = description;
    description.workflows[0].outputs = serde_json::from_value(json!({
        "order": "$steps.orderPet.outputs.orderId",
        "self": "$self"
    }))
    .expect("outputs");
    let mut client = Fake::new().reply(500, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(report.outcome, Outcome::Failed);
    assert!(
        !report.outputs.contains_key("order"),
        "the step it names never ran"
    );
    assert_eq!(
        report.outputs["self"],
        json!("https://example.com/buy.arazzo.yaml"),
        "what could still be named was"
    );
}

// ---- 6: what a criterion counts as true -----------------------------

#[test]
fn a_jsonpath_that_finds_false_has_still_found_something() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "successCriteria": [{
            "context": "$response.body",
            "condition": "$.done",
            "type": "jsonpath"
        }]
    }]));
    let mut client = Fake::new().reply(200, &json!({ "done": false }));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(
        report.outcome,
        Outcome::Succeeded,
        "a non-empty nodelist passes, whatever the node holds"
    );
}

#[test]
fn strings_compare_without_regard_to_case() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "successCriteria": [
            { "condition": "$response.body#/status == 'placed'" },
            { "condition": "$response.body#/status != 'shipped'" }
        ]
    }]));
    let mut client = Fake::new().reply(200, &json!({ "status": "PLACED" }));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(report.outcome, Outcome::Succeeded);
}

// ---- 8: a step that reads another runs after it ---------------------

#[test]
fn a_step_reading_another_runs_after_it_without_being_told_to() {
    let description = one(json!([
        {
            "stepId": "orderPet",
            "operationId": "placeOrder",
            "requestBody": { "payload": { "pets": "$steps.listPets.outputs.pets" } }
        },
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "outputs": { "pets": "$response.body" }
        }
    ]));
    let mut client = Fake::new()
        .reply(200, &json!(["fluffy"]))
        .reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        ["listPets", "orderPet"],
        "the step that is read runs first, though the document lists it second"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "pets": ["fluffy"] })
    );
}

#[test]
fn steps_that_read_each_other_are_refused() {
    let description = one(json!([
        {
            "stepId": "a",
            "operationId": "listPets",
            "outputs": { "x": "$steps.b.outputs.y" }
        },
        {
            "stepId": "b",
            "operationId": "listPets",
            "outputs": { "y": "$steps.a.outputs.x" }
        }
    ]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(matches!(error, ExecutionError::Circular(_)), "got: {error}");
}
