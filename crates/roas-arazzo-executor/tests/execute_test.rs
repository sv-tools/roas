//! Running whole workflows, with a client that answers from a script.
//!
//! Every test here goes through the public API — `execute`, or `Run`
//! where the point is what the engine *asked* for rather than what it
//! did with the answer.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    ExecutionError, HttpResponse, Options, Outcome, Progress, Run, execute, testing::Fake,
};
use serde_json::{Value, json};

/// The OpenAPI description every workflow here points at.
fn petstore() -> Value {
    json!({
        "openapi": "3.0.3",
        "servers": [{ "url": "https://api.example.com/v1" }],
        "paths": {
            "/pets/{petId}": { "get": { "operationId": "getPetById" } },
            "/pets": { "get": { "operationId": "listPets" } },
            "/orders": { "post": { "operationId": "placeOrder" } },
            "/login": { "post": { "operationId": "login" } }
        }
    })
}

fn description(value: Value) -> Description {
    serde_json::from_value(value).expect("a v1.1 description")
}

/// A description with one workflow, spelled out around `workflow`.
fn around(workflow: Value) -> Description {
    description(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [workflow],
    }))
}

fn options() -> Options {
    Options::new().source(
        "petStore",
        "https://api.example.com/openapi.json",
        petstore(),
    )
}

#[test]
fn a_step_sends_what_its_parameters_say_and_keeps_what_it_named() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{
            "stepId": "findPet",
            "operationId": "getPetById",
            "parameters": [
                { "name": "petId", "in": "path", "value": "$inputs.petId" },
                { "name": "detail", "in": "query", "value": "full" },
                { "name": "Authorization", "in": "header", "value": "Bearer {$inputs.token}" }
            ],
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "outputs": { "name": "$response.body#/name" }
        }],
        "outputs": { "pet": "$steps.findPet.outputs.name" }
    }));
    let mut client = Fake::new().reply(200, &json!({ "id": 7, "name": "fluffy" }));

    let report = execute(
        &description,
        &options().input("petId", 7).input("token", "abc"),
        &mut client,
    )
    .expect("the workflow runs");

    let sent = &client.sent()[0];
    assert_eq!(sent.method, "GET");
    assert_eq!(sent.url, "https://api.example.com/v1/pets/7?detail=full");
    assert_eq!(sent.header("authorization"), Some("Bearer abc"));

    assert_eq!(report.outcome, Outcome::Succeeded);
    assert_eq!(report.outputs["pet"], json!("fluffy"));
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].step_id, "findPet");
    assert_eq!(report.steps[0].status, Some(200));
    assert!(report.steps[0].passed);
    assert_eq!(report.steps[0].outputs["name"], json!("fluffy"));
    assert!(report.is_success());
}

#[test]
fn a_step_reads_what_an_earlier_step_produced() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [
            {
                "stepId": "findPet",
                "operationId": "getPetById",
                "parameters": [{ "name": "petId", "in": "path", "value": "1" }],
                "outputs": { "id": "$response.body#/id" }
            },
            {
                "stepId": "orderPet",
                "operationId": "placeOrder",
                "requestBody": {
                    "contentType": "application/json",
                    "payload": { "petId": "$steps.findPet.outputs.id", "quantity": 1 },
                    "replacements": [
                        { "target": "/quantity", "value": 2 },
                        { "target": "/note", "value": "gift" }
                    ]
                }
            }
        ]
    }));
    let mut client = Fake::new()
        .reply(200, &json!({ "id": 7 }))
        .reply(201, &json!({ "orderId": "o-1" }));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    let order = &client.sent()[1];
    assert_eq!(order.method, "POST");
    assert_eq!(order.url, "https://api.example.com/v1/orders");
    assert_eq!(order.header("content-type"), Some("application/json"));
    assert_eq!(
        serde_json::from_slice::<Value>(order.body.as_ref().expect("a body")).expect("json"),
        json!({ "petId": 7, "quantity": 2, "note": "gift" }),
        "the payload is filled in and both replacements applied"
    );
    assert_eq!(report.outcome, Outcome::Succeeded);
    assert_eq!(report.steps.len(), 2);
}

#[test]
fn a_failing_step_with_nothing_to_say_about_it_fails_the_workflow() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [
            {
                "stepId": "findPet",
                "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 200" }]
            },
            { "stepId": "orderPet", "operationId": "placeOrder" }
        ]
    }));
    let mut client = Fake::new().reply(404, &json!({ "error": "gone" }));

    let report = execute(&description, &options(), &mut client).expect("the run itself is fine");

    assert_eq!(report.outcome, Outcome::Failed);
    assert!(!report.is_success());
    assert_eq!(client.sent().len(), 1, "the second step never ran");
    assert!(!report.steps[0].passed);
    assert_eq!(report.steps[0].criteria[0].condition, "$statusCode == 200");
    assert!(!report.steps[0].criteria[0].passed);
}

#[test]
fn without_criteria_a_step_is_judged_by_its_status() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{ "stepId": "listPets", "operationId": "listPets" }]
    }));
    for (status, outcome) in [(204, Outcome::Succeeded), (500, Outcome::Failed)] {
        let mut client = Fake::new().reply(status, &json!(null));
        let report = execute(&description, &options(), &mut client).expect("the run is fine");
        assert_eq!(report.outcome, outcome, "status {status}");
    }
}

#[test]
fn a_success_action_can_skip_ahead() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [
            {
                "stepId": "findPet",
                "operationId": "listPets",
                "onSuccess": [{ "name": "skip", "type": "goto", "stepId": "orderPet" }]
            },
            { "stepId": "never", "operationId": "getPetById",
              "parameters": [{ "name": "petId", "in": "path", "value": "1" }] },
            { "stepId": "orderPet", "operationId": "placeOrder" }
        ]
    }));
    let mut client = Fake::new()
        .reply(200, &json!([]))
        .reply(201, &json!({ "orderId": "o-1" }));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(client.sent().len(), 2);
    assert!(client.sent()[1].url.ends_with("/orders"));
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("goto step `orderPet`")
    );
    assert_eq!(report.outcome, Outcome::Succeeded);
}

#[test]
fn a_failure_action_ends_the_workflow_where_it_says() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [
            {
                "stepId": "findPet",
                "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 200" }],
                "onFailure": [{
                    "name": "giveUp",
                    "type": "end",
                    "criteria": [{ "condition": "$statusCode == 404" }]
                }]
            },
            { "stepId": "orderPet", "operationId": "placeOrder" }
        ]
    }));
    let mut client = Fake::new().reply(404, &json!(null));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(report.steps[0].action.as_deref(), Some("ended, failed"));
    assert_eq!(client.sent().len(), 1);
}

#[test]
fn a_retry_tries_again_and_asks_for_the_delay_it_wants() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{
            "stepId": "findPet",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "onFailure": [{
                "name": "again",
                "type": "retry",
                "retryAfter": 1.5,
                "retryLimit": 3,
                "criteria": [{ "condition": "$statusCode == 503" }]
            }]
        }]
    }));
    let options = options();
    let mut run = Run::start(&description, &options).expect("a run");

    // Driving the engine directly: the delay is asserted, not spent.
    let mut sent = 0;
    let mut waits = Vec::new();
    let report = loop {
        match run.advance().expect("progress") {
            Progress::Send(_) => {
                sent += 1;
                let status = if sent < 3 { 503 } else { 200 };
                run.supply(HttpResponse::json(status, &json!({ "ok": true })))
                    .expect("the response is understood");
            }
            Progress::Wait(duration) => waits.push(duration),
            Progress::Done(report) => break report,
        }
    };

    assert_eq!(sent, 3, "two failures and the try that worked");
    assert_eq!(waits, [std::time::Duration::from_millis(1500); 2]);
    assert_eq!(report.outcome, Outcome::Succeeded);
    assert_eq!(report.steps.len(), 3, "every attempt is in the report");
    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step.attempt)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn a_retry_that_never_works_stops_at_its_limit() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{
            "stepId": "findPet",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "onFailure": [{ "name": "again", "type": "retry", "retryLimit": 2 }]
        }]
    }));
    let mut client = Fake::new()
        .reply(503, &json!(null))
        .reply(503, &json!(null))
        .reply(503, &json!(null));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(client.sent().len(), 3, "the first try and two retries");
    assert_eq!(report.outcome, Outcome::Failed);
}

#[test]
fn a_step_can_call_another_workflow_and_take_its_outputs() {
    let description = description(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [
            {
                "workflowId": "buyPet",
                "steps": [
                    {
                        "stepId": "authenticate",
                        "workflowId": "login",
                        "parameters": [{ "name": "user", "value": "$inputs.user" }]
                    },
                    {
                        "stepId": "findPet",
                        "operationId": "getPetById",
                        "parameters": [
                            { "name": "petId", "in": "path", "value": "1" },
                            { "name": "Authorization", "in": "header",
                              "value": "Bearer {$steps.authenticate.outputs.token}" }
                        ]
                    }
                ],
                "outputs": { "token": "$steps.authenticate.outputs.token" }
            },
            {
                "workflowId": "login",
                "steps": [{
                    "stepId": "post",
                    "operationId": "login",
                    "requestBody": { "payload": { "user": "$inputs.user" } },
                    "outputs": { "token": "$response.body#/token" }
                }],
                "outputs": { "token": "$steps.post.outputs.token" }
            }
        ]
    }));
    let mut client = Fake::new()
        .reply(200, &json!({ "token": "t-1" }))
        .reply(200, &json!({ "id": 1 }));

    let report = execute(
        &description,
        &options().workflow("buyPet").input("user", "ada"),
        &mut client,
    )
    .expect("the workflow runs");

    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[0].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "user": "ada" }),
        "the step's parameters became the sub-workflow's inputs"
    );
    assert_eq!(client.sent()[1].header("authorization"), Some("Bearer t-1"));
    assert_eq!(report.outputs["token"], json!("t-1"));
    assert_eq!(report.steps.len(), 2);
    assert_eq!(
        report.steps[0].workflow_id, "login",
        "the called workflow's step"
    );
    assert_eq!(report.steps[1].workflow_id, "buyPet");
}

#[test]
fn a_workflow_runs_what_it_depends_on_first() {
    let description = description(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [
            {
                "workflowId": "buyPet",
                "dependsOn": ["authenticate"],
                "steps": [{
                    "stepId": "findPet",
                    "operationId": "getPetById",
                    "parameters": [
                        { "name": "petId", "in": "path", "value": "1" },
                        { "name": "Authorization", "in": "header",
                          "value": "Bearer {$workflows.authenticate.outputs.token}" }
                    ]
                }]
            },
            {
                "workflowId": "authenticate",
                "steps": [{
                    "stepId": "post",
                    "operationId": "login",
                    "outputs": { "token": "$response.body#/token" }
                }],
                "outputs": { "token": "$steps.post.outputs.token" }
            }
        ]
    }));
    let mut client = Fake::new()
        .reply(200, &json!({ "token": "t-2" }))
        .reply(200, &json!({ "id": 1 }));

    let report = execute(&description, &options().workflow("buyPet"), &mut client)
        .expect("the workflow runs");

    assert!(client.sent()[0].url.ends_with("/login"));
    assert_eq!(client.sent()[1].header("authorization"), Some("Bearer t-2"));
    assert_eq!(report.workflow_id, "buyPet");
    assert_eq!(report.steps.len(), 2);
}

#[test]
fn steps_run_in_the_order_depends_on_asks_for() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [
            { "stepId": "second", "operationId": "placeOrder", "dependsOn": ["first"] },
            { "stepId": "first", "operationId": "listPets" }
        ]
    }));
    let mut client = Fake::new().reply(200, &json!([])).reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn a_reusable_parameter_is_followed_into_the_components() {
    let description = description(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [{
            "workflowId": "buyPet",
            "steps": [{
                "stepId": "listPets",
                "operationId": "listPets",
                "parameters": [
                    { "reference": "$components.parameters.locale" },
                    { "reference": "$components.parameters.locale", "value": "de-DE" }
                ]
            }]
        }],
        "components": {
            "parameters": {
                "locale": { "name": "locale", "in": "query", "value": "en-GB" }
            }
        }
    }));
    let mut client = Fake::new().reply(200, &json!([]));

    execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        client.sent()[0].url,
        "https://api.example.com/v1/pets?locale=de-DE",
        "the later parameter, with its override, wins"
    );
}

#[tokio::test]
async fn the_async_entry_point_runs_the_same_workflow() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{
            "stepId": "listPets",
            "operationId": "listPets",
            "outputs": { "count": "$response.body#/count" }
        }],
        "outputs": { "count": "$steps.listPets.outputs.count" }
    }));
    let mut client = Fake::new().reply(200, &json!({ "count": 3 }));

    let report = roas_arazzo_executor::execute_async(&description, &options(), &mut client)
        .await
        .expect("the workflow runs");

    assert_eq!(report.outputs["count"], json!(3));
}

// ---- what stops a run -----------------------------------------------

#[test]
fn asking_for_a_workflow_that_is_not_there_says_so() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{ "stepId": "listPets", "operationId": "listPets" }]
    }));
    let error = execute(&description, &options().workflow("nope"), &mut Fake::new()).unwrap_err();
    assert!(matches!(error, ExecutionError::UnknownWorkflow(id) if id == "nope"));
}

#[test]
fn an_operation_no_description_holds_stops_the_run() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{ "stepId": "listPets", "operationId": "nope" }]
    }));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("is in none of the source descriptions"),
        "got: {error}"
    );
}

#[test]
fn an_async_step_says_it_is_not_run_rather_than_being_skipped() {
    let description = around(json!({
        "workflowId": "watchPets",
        "steps": [{
            "stepId": "watch",
            "channelPath": "{$sourceDescriptions.events.url}#/channels/pets",
            "action": "receive"
        }]
    }));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(error.to_string().contains("AsyncAPI step"), "got: {error}");
}

#[test]
fn a_goto_that_loops_forever_stops_at_the_step_limit() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{
            "stepId": "listPets",
            "operationId": "listPets",
            "onSuccess": [{ "name": "again", "type": "goto", "stepId": "listPets" }]
        }]
    }));
    let mut client = Fake::new();
    for _ in 0..10 {
        client = client.reply(200, &json!([]));
    }
    let error = execute(&description, &options().max_steps(5), &mut client).unwrap_err();
    assert!(
        error.to_string().contains("step limit of 5"),
        "got: {error}"
    );
}

#[test]
fn a_parameter_that_names_nothing_stops_the_run_where_it_is() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{
            "stepId": "findPet",
            "operationId": "getPetById",
            "parameters": [{ "name": "petId", "in": "path", "value": "$inputs.petId" }]
        }]
    }));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("`$inputs.petId` names an input named `petId`"),
        "got: {error}"
    );
}

#[test]
fn a_path_parameter_left_unfilled_is_refused_rather_than_sent() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{ "stepId": "findPet", "operationId": "getPetById" }]
    }));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error.to_string().contains("which no parameter filled in"),
        "got: {error}"
    );
}

#[test]
fn a_response_supplied_when_none_was_asked_for_says_so() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{ "stepId": "listPets", "operationId": "listPets" }]
    }));
    let options = options();
    let mut run = Run::start(&description, &options).expect("a run");
    let error = run.supply(HttpResponse::json(200, &json!([]))).unwrap_err();
    assert!(matches!(error, ExecutionError::NotWaiting));
}

#[test]
fn a_client_failure_ends_the_run_saying_what_the_client_said() {
    let description = around(json!({
        "workflowId": "buyPet",
        "steps": [{ "stepId": "listPets", "operationId": "listPets" }]
    }));
    // A fake with no replies fails on the first request.
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error.to_string().contains("the request could not be sent"),
        "got: {error}"
    );
}
