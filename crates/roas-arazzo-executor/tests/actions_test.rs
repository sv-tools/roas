//! The rest of what a description can say: the other parameter
//! locations, the options a caller sets, `goto` between workflows,
//! reusable actions, and the ways a description can be wrong.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{ExecutionError, Options, Outcome, execute, testing::Fake};
use serde_json::{Value, json};

fn petstore() -> Value {
    json!({
        "openapi": "3.0.3",
        "servers": [{ "url": "https://api.example.com/v1" }],
        "paths": {
            "/pets": { "get": { "operationId": "listPets" } },
            "/orders": { "post": { "operationId": "placeOrder" } }
        }
    })
}

fn description(workflows: Value, components: Option<Value>) -> Description {
    let mut document = json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": workflows,
    });
    if let Some(components) = components {
        document["components"] = components;
    }
    serde_json::from_value(document).expect("a v1.1 description")
}

fn one(steps: Value) -> Description {
    description(json!([{ "workflowId": "w", "steps": steps }]), None)
}

fn options() -> Options {
    Options::new().source(
        "petStore",
        "https://api.example.com/openapi.json",
        petstore(),
    )
}

#[test]
fn every_parameter_location_goes_where_it_belongs() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "parameters": [
            { "name": "limit", "in": "query", "value": 10 },
            { "name": "raw", "in": "querystring", "value": "a=1&b=2" },
            { "name": "session", "in": "cookie", "value": "s-1" },
            { "name": "other", "in": "cookie", "value": "o-2" },
            { "name": "X-Trace", "in": "header", "value": "t-1" }
        ]
    }]));
    let mut client = Fake::new().reply(200, &json!([]));

    execute(&description, &options(), &mut client).expect("the workflow runs");

    let sent = &client.sent()[0];
    assert_eq!(sent.url, "https://api.example.com/v1/pets?limit=10&a=1&b=2");
    assert_eq!(sent.header("cookie"), Some("session=s-1; other=o-2"));
    assert_eq!(sent.header("x-trace"), Some("t-1"));
}

#[test]
fn a_channel_parameter_says_it_belongs_to_a_step_this_crate_does_not_run() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "parameters": [{ "name": "topic", "in": "channel", "value": "pets" }]
    }]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error.to_string().contains("`channel` parameter"),
        "got: {error}"
    );
}

#[test]
fn the_callers_headers_fill_in_where_a_step_is_silent() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "parameters": [{ "name": "Accept", "in": "header", "value": "application/xml" }]
    }]));
    let options = options()
        .header("Authorization", "Bearer caller")
        .header("Accept", "application/json");
    let mut client = Fake::new().reply(200, &json!([]));

    execute(&description, &options, &mut client).expect("the workflow runs");

    let sent = &client.sent()[0];
    assert_eq!(sent.header("authorization"), Some("Bearer caller"));
    assert_eq!(
        sent.header("accept"),
        Some("application/xml"),
        "the step's own header is not overwritten"
    );
}

#[test]
fn inputs_can_be_given_all_at_once() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "parameters": [{ "name": "who", "in": "query", "value": "$inputs.who" }]
    }]));
    let options = options().inputs(json!({ "who": "ada" }));
    let mut client = Fake::new().reply(200, &json!([]));

    execute(&description, &options, &mut client).expect("the workflow runs");

    assert!(client.sent()[0].url.ends_with("?who=ada"));
}

#[test]
fn a_selector_picks_a_value_out_of_a_response() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "outputs": {
                "first": {
                    "context": "$response.body",
                    "selector": "$.pets[0].name",
                    "type": "jsonpath"
                }
            }
        },
        {
            "stepId": "orderPet",
            "operationId": "placeOrder",
            "requestBody": { "payload": { "name": "$steps.listPets.outputs.first" } }
        }
    ]));
    let mut client = Fake::new()
        .reply(
            200,
            &json!({ "pets": [{ "name": "fluffy" }, { "name": "rex" }] }),
        )
        .reply(201, &json!({}));

    execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "name": "fluffy" })
    );
}

#[test]
fn a_workflow_level_action_applies_to_every_step() {
    let description = description(
        json!([{
            "workflowId": "w",
            "failureActions": [{ "name": "stop", "type": "end" }],
            "steps": [
                {
                    "stepId": "listPets",
                    "operationId": "listPets",
                    "successCriteria": [{ "condition": "$statusCode == 200" }]
                },
                { "stepId": "orderPet", "operationId": "placeOrder" }
            ]
        }]),
        None,
    );
    let mut client = Fake::new().reply(500, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(report.steps[0].action.as_deref(), Some("ended, failed"));
    assert_eq!(client.sent().len(), 1);
}

#[test]
fn a_reusable_action_is_followed_into_the_components() {
    let description = description(
        json!([{
            "workflowId": "w",
            "steps": [{
                "stepId": "listPets",
                "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 200" }],
                "onFailure": [{ "reference": "$components.failureActions.retryOnce" }]
            }]
        }]),
        Some(json!({
            "failureActions": {
                "retryOnce": { "name": "retryOnce", "type": "retry", "retryLimit": 1 }
            }
        })),
    );
    let mut client = Fake::new().reply(503, &json!({})).reply(200, &json!([]));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(client.sent().len(), 2, "the failure action retried once");
    assert_eq!(report.outcome, Outcome::Succeeded);
}

#[test]
fn a_reusable_that_names_nothing_stops_the_run() {
    for reference in [
        "$components.failureActions.nope",
        "$components.parameters.nope",
        "$somethingElse.x",
    ] {
        let description = description(
            json!([{
                "workflowId": "w",
                "steps": [{
                    "stepId": "listPets",
                    "operationId": "listPets",
                    "parameters": [{ "reference": reference }],
                    "onFailure": [{ "reference": reference }]
                }]
            }]),
            None,
        );
        let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
        assert!(
            matches!(error, ExecutionError::Unsupported(_)),
            "`{reference}`: got {error}"
        );
    }
}

#[test]
fn a_goto_to_a_workflow_hands_over_and_ends_the_one_it_left() {
    let description = description(
        json!([
            {
                "workflowId": "first",
                "steps": [
                    {
                        "stepId": "listPets",
                        "operationId": "listPets",
                        "onSuccess": [{
                            "name": "handOver",
                            "type": "goto",
                            "workflowId": "second",
                            "parameters": [{ "name": "who", "value": "ada" }]
                        }]
                    },
                    { "stepId": "never", "operationId": "placeOrder" }
                ]
            },
            {
                "workflowId": "second",
                "steps": [{
                    "stepId": "order",
                    "operationId": "placeOrder",
                    "requestBody": { "payload": { "who": "$inputs.who" } }
                }]
            }
        ]),
        None,
    );
    let mut client = Fake::new().reply(200, &json!([])).reply(201, &json!({}));

    let report = execute(&description, &options().workflow("first"), &mut client)
        .expect("the workflow runs");

    assert_eq!(client.sent().len(), 2, "the step after the goto never ran");
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "who": "ada" }),
        "the action's parameters became the workflow's inputs"
    );
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("goto workflow `second`")
    );
    assert_eq!(report.outcome, Outcome::Succeeded);
}

#[test]
fn a_goto_that_names_nothing_stops_the_run() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "onSuccess": [{ "name": "go", "type": "goto", "stepId": "nope" }]
    }]));
    let mut client = Fake::new().reply(200, &json!([]));
    let error = execute(&description, &options(), &mut client).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownStep { ref step, .. } if step == "nope"),
        "got: {error}"
    );

    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "onSuccess": [{ "name": "go", "type": "goto", "workflowId": "nope" }]
    }]));
    let mut client = Fake::new().reply(200, &json!([]));
    let error = execute(&description, &options(), &mut client).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownWorkflow(ref id) if id == "nope"),
        "got: {error}"
    );
}

#[test]
fn a_step_calling_a_workflow_of_another_document_says_it_cannot() {
    let description = description(
        json!([{
            "workflowId": "w",
            "steps": [{
                "stepId": "call",
                "workflowId": "$sourceDescriptions.other.someWorkflow"
            }]
        }]),
        None,
    );
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only workflows of the description"),
        "got: {error}"
    );

    let description = one(json!([{ "stepId": "call", "workflowId": "nope" }]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownWorkflow(_)),
        "got: {error}"
    );
}

#[test]
fn workflows_that_depend_on_each_other_are_refused() {
    let description = description(
        json!([
            {
                "workflowId": "a",
                "dependsOn": ["b"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }]
            },
            {
                "workflowId": "b",
                "dependsOn": ["a"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }]
            }
        ]),
        None,
    );
    let error = execute(&description, &options().workflow("a"), &mut Fake::new()).unwrap_err();
    assert!(matches!(error, ExecutionError::Circular(_)), "got: {error}");
}

#[test]
fn steps_that_depend_on_each_other_are_refused() {
    let description = one(json!([
        { "stepId": "a", "operationId": "listPets", "dependsOn": ["b"] },
        { "stepId": "b", "operationId": "listPets", "dependsOn": ["a"] }
    ]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(matches!(error, ExecutionError::Circular(_)), "got: {error}");
}

#[test]
fn a_step_depending_on_one_that_is_not_there_is_refused() {
    let description = one(json!([
        { "stepId": "a", "operationId": "listPets", "dependsOn": ["nope"] }
    ]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownStep { ref step, .. } if step == "nope"),
        "got: {error}"
    );
}

#[test]
fn a_workflow_depending_on_one_that_is_not_there_is_refused() {
    let description = description(
        json!([{
            "workflowId": "w",
            "dependsOn": ["nope"],
            "steps": [{ "stepId": "s", "operationId": "listPets" }]
        }]),
        None,
    );
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownWorkflow(_)),
        "got: {error}"
    );
}

#[test]
fn workflows_calling_each_other_stop_at_the_depth_limit() {
    let description = description(
        json!([
            {
                "workflowId": "a",
                "steps": [{ "stepId": "call", "workflowId": "b" }]
            },
            {
                "workflowId": "b",
                "steps": [{ "stepId": "call", "workflowId": "a" }]
            }
        ]),
        None,
    );
    let error = execute(
        &description,
        &options().workflow("a").max_depth(3),
        &mut Fake::new(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("workflow depth limit of 3"),
        "got: {error}"
    );
}

#[test]
fn the_callers_retry_limit_caps_what_the_description_asks_for() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "successCriteria": [{ "condition": "$statusCode == 200" }],
        "onFailure": [{ "name": "again", "type": "retry", "retryLimit": 100 }]
    }]));
    let mut client = Fake::new();
    for _ in 0..10 {
        client = client.reply(503, &json!({}));
    }

    let report =
        execute(&description, &options().max_retries(2), &mut client).expect("the run is fine");

    assert_eq!(client.sent().len(), 3, "the first try and two retries");
    assert_eq!(report.outcome, Outcome::Failed);
}

#[test]
fn a_description_with_no_workflows_says_so() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": []
    }))
    .expect("it parses, even though it would not validate");
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownWorkflow(_)),
        "got: {error}"
    );
}

#[test]
fn a_body_that_is_not_json_goes_as_it_was_written() {
    let description = one(json!([{
        "stepId": "orderPet",
        "operationId": "placeOrder",
        "requestBody": { "contentType": "text/plain", "payload": "pet {$inputs.id}, please" }
    }]));
    let mut client = Fake::new().reply(201, &json!({}));

    execute(&description, &options().input("id", 7), &mut client).expect("the workflow runs");

    let sent = &client.sent()[0];
    assert_eq!(sent.header("content-type"), Some("text/plain"));
    assert_eq!(sent.text(), "pet 7, please");
}

#[test]
fn a_replacement_that_points_nowhere_says_which_step_it_was_in() {
    let description = one(json!([{
        "stepId": "orderPet",
        "operationId": "placeOrder",
        "requestBody": {
            "payload": { "pet": { "id": 1 } },
            "replacements": [{ "target": "/nothing/here", "value": 1 }]
        }
    }]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("step `orderPet` cannot be turned into a request"),
        "got: {error}"
    );
}

#[test]
fn a_jsonpath_replacement_finds_where_to_write() {
    let description = one(json!([{
        "stepId": "orderPet",
        "operationId": "placeOrder",
        "requestBody": {
            "payload": { "pets": [{ "id": 1 }, { "id": 2 }] },
            "replacements": [{
                "target": "$.pets[1].id",
                "targetSelectorType": "jsonpath",
                "value": 9
            }]
        }
    }]));
    let mut client = Fake::new().reply(201, &json!({}));

    execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[0].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "pets": [{ "id": 1 }, { "id": 9 }] })
    );
}

#[test]
fn an_action_whose_criteria_do_not_hold_is_passed_over() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "onSuccess": [
                {
                    "name": "notThisOne",
                    "type": "end",
                    "criteria": [{ "condition": "$statusCode == 201" }]
                },
                { "name": "thisOne", "type": "goto", "stepId": "orderPet" }
            ]
        },
        { "stepId": "skipped", "operationId": "listPets" },
        { "stepId": "orderPet", "operationId": "placeOrder" }
    ]));
    let mut client = Fake::new().reply(200, &json!([])).reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(client.sent().len(), 2);
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("goto step `orderPet`"),
        "the first action's criteria did not hold, so the second one ran"
    );
}

#[test]
fn a_failure_can_go_somewhere_rather_than_end() {
    let description = one(json!([
        {
            "stepId": "listPets",
            "operationId": "listPets",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "onFailure": [{ "name": "fallback", "type": "goto", "stepId": "orderPet" }]
        },
        { "stepId": "orderPet", "operationId": "placeOrder" }
    ]));
    let mut client = Fake::new().reply(500, &json!({})).reply(201, &json!({}));

    let report = execute(&description, &options(), &mut client).expect("the run is fine");

    assert_eq!(client.sent().len(), 2);
    assert!(client.sent()[1].url.ends_with("/orders"));
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("goto step `orderPet`")
    );
}

#[test]
fn a_reusable_success_action_is_followed_into_the_components() {
    let stopping = description(
        json!([{
            "workflowId": "w",
            "steps": [
                {
                    "stepId": "listPets",
                    "operationId": "listPets",
                    "onSuccess": [{ "reference": "$components.successActions.stopHere" }]
                },
                { "stepId": "never", "operationId": "placeOrder" }
            ]
        }]),
        Some(json!({
            "successActions": { "stopHere": { "name": "stopHere", "type": "end" } }
        })),
    );
    let mut client = Fake::new().reply(200, &json!([]));

    let report = execute(&stopping, &options(), &mut client).expect("the workflow runs");

    assert_eq!(client.sent().len(), 1);
    assert_eq!(report.outcome, Outcome::Ended);

    // The same reference, with nothing behind it.
    let missing = description(
        json!([{
            "workflowId": "w",
            "steps": [{
                "stepId": "listPets",
                "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 200" }],
                "onFailure": [{ "reference": "$components.failureActions.nope" }]
            }]
        }]),
        None,
    );
    let mut client = Fake::new().reply(500, &json!({}));
    let error = execute(&missing, &options(), &mut client).unwrap_err();
    assert!(
        matches!(error, ExecutionError::Unsupported(_)),
        "got: {error}"
    );
}

#[test]
fn a_value_that_needs_encoding_is_encoded() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "parameters": [{ "name": "q", "in": "query", "value": "a b/c&d" }]
    }]));
    let mut client = Fake::new().reply(200, &json!([]));

    execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        client.sent()[0].url,
        "https://api.example.com/v1/pets?q=a%20b%2Fc%26d"
    );
}

#[test]
fn a_querystring_alone_becomes_the_whole_query() {
    let description = one(json!([{
        "stepId": "listPets",
        "operationId": "listPets",
        "parameters": [{ "name": "raw", "in": "querystring", "value": "a=1&b=2" }]
    }]));
    let mut client = Fake::new().reply(200, &json!([]));

    execute(&description, &options(), &mut client).expect("the workflow runs");

    assert_eq!(
        client.sent()[0].url,
        "https://api.example.com/v1/pets?a=1&b=2"
    );
}

#[test]
fn a_request_body_of_replacements_alone_starts_from_nothing() {
    let description = one(json!([{
        "stepId": "orderPet",
        "operationId": "placeOrder",
        "requestBody": {
            "replacements": [{ "target": "/note", "value": "hello" }]
        }
    }]));
    let error = execute(&description, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("points into nothing the payload has"),
        "a replacement needs a payload to write into: {error}"
    );
}

#[test]
fn a_workflow_depended_on_twice_runs_once() {
    let description = description(
        json!([
            {
                "workflowId": "w",
                "dependsOn": ["a", "b"],
                "steps": [{ "stepId": "s", "operationId": "placeOrder" }]
            },
            {
                "workflowId": "a",
                "dependsOn": ["shared"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }]
            },
            {
                "workflowId": "b",
                "dependsOn": ["shared"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }]
            },
            { "workflowId": "shared", "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let mut client = Fake::new();
    for _ in 0..4 {
        client = client.reply(200, &json!([]));
    }

    let report =
        execute(&description, &options().workflow("w"), &mut client).expect("the workflow runs");

    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step.workflow_id.as_str())
            .collect::<Vec<_>>(),
        ["shared", "a", "b", "w"],
        "the shared dependency ran once, before both that need it"
    );
}
