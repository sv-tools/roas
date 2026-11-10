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
fn a_step_calling_an_unsupplied_workflow_document_says_why() {
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
        matches!(&error, ExecutionError::ExternalWorkflow { reference, reason }
            if reference == "$sourceDescriptions.other.someWorkflow"
                && reason.contains("declares no source `other`")),
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

/// The workflow ids of a report's steps, in the order they finished.
fn order(report: &roas_arazzo_executor::ExecutionReport) -> Vec<String> {
    report
        .steps
        .iter()
        .map(|step| format!("{}.{}", step.workflow_id, step.step_id))
        .collect()
}

#[test]
fn a_call_goto_or_retry_into_a_workflow_with_dependencies_runs_them_first_on_both_paths() {
    // The dependency reads the run's own inputs, not the arguments the
    // target was entered with.
    let dependency = json!({ "workflowId": "dependency",
        "steps": [{ "stepId": "dep", "operationId": "listPets",
            "outputs": { "token": "$inputs.token" } }] });
    let target = json!({ "workflowId": "target", "dependsOn": ["dependency"],
        "steps": [{ "stepId": "inner", "operationId": "listPets",
            "outputs": { "token": "$inputs.token" } }] });
    let argument = json!([{ "name": "token", "value": "argument" }]);
    for (caller, expected) in [
        (
            json!({ "workflowId": "caller", "steps": [
                { "stepId": "call", "workflowId": "target", "parameters": argument }] }),
            vec!["dependency.dep", "target.inner", "caller.call"],
        ),
        (
            json!({ "workflowId": "caller", "steps": [{
                "stepId": "first", "operationId": "listPets",
                "onSuccess": [{ "name": "away", "type": "goto", "workflowId": "target",
                    "parameters": argument }] }] }),
            vec!["caller.first", "dependency.dep", "target.inner"],
        ),
        (
            json!({ "workflowId": "caller", "steps": [{
                "stepId": "first", "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 201" }],
                "onFailure": [{ "name": "again", "type": "retry",
                    "workflowId": "target", "retryLimit": 1, "parameters": argument }] }] }),
            vec![
                "caller.first",
                "dependency.dep",
                "target.inner",
                "caller.first",
            ],
        ),
    ] {
        let description = description(
            json!([caller.clone(), dependency.clone(), target.clone()]),
            None,
        );
        let options = options().workflow("caller").input("token", "root");
        let replies = || (0..4).fold(Fake::new(), |client, _| client.reply(200, &json!([])));
        let lazy = execute(&description, &options, &mut replies()).expect("the lazy run");
        assert_eq!(order(&lazy), expected, "{caller}");
        let dep = lazy
            .steps
            .iter()
            .find(|step| step.step_id == "dep")
            .unwrap();
        assert_eq!(dep.outputs["token"], "root", "{caller}");
        let inner = lazy
            .steps
            .iter()
            .find(|step| step.step_id == "inner")
            .unwrap();
        assert_eq!(inner.outputs["token"], "argument", "{caller}");
        let plan = roas_arazzo_executor::prepare(&description, &options).expect("a plan");
        let checked = plan.execute(&mut replies()).expect("the prepared run");
        assert_eq!(order(&checked), expected, "{caller}");
    }
}

#[test]
fn a_dependency_runs_once_however_many_entries_need_it() {
    // The root depends on `shared`, and calls two workflows that do too,
    // one of them twice: `shared` runs once, first.
    let description = description(
        json!([
            { "workflowId": "w", "dependsOn": ["shared"], "steps": [
                { "stepId": "one", "workflowId": "a" },
                { "stepId": "two", "workflowId": "b" },
                { "stepId": "three", "workflowId": "a" }] },
            { "workflowId": "a", "dependsOn": ["shared"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "b", "dependsOn": ["shared"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "shared", "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let mut client = (0..4).fold(Fake::new(), |client, _| client.reply(200, &json!([])));
    let report = execute(&description, &options().workflow("w"), &mut client).unwrap();
    assert_eq!(
        order(&report),
        ["shared.s", "a.s", "w.one", "b.s", "w.two", "a.s", "w.three"]
    );
}

#[test]
fn a_root_dependency_a_nested_entry_ran_first_is_not_run_again() {
    // `w` depends on `x` then `y`; `x` calls `t`, which needs `y` — so `y`
    // runs inside `x`, and the root queue does not run it a second time.
    let description = description(
        json!([
            { "workflowId": "w", "dependsOn": ["x", "y"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "x", "steps": [{ "stepId": "call", "workflowId": "t" }] },
            { "workflowId": "y", "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "t", "dependsOn": ["y"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let mut client = (0..3).fold(Fake::new(), |client, _| client.reply(200, &json!([])));
    let report = execute(&description, &options().workflow("w"), &mut client).unwrap();
    assert_eq!(order(&report), ["y.s", "t.s", "x.call", "w.s"]);
    assert_eq!(client.sent().len(), 3);
}

#[test]
fn a_dependency_a_deeper_entry_ran_meanwhile_is_skipped() {
    // `t` waits for `x` then `y`; `x` itself calls `u`, which needs `y`,
    // so `y` runs inside `x` and `t` does not run it again.
    let description = description(
        json!([
            { "workflowId": "w", "steps": [{ "stepId": "call", "workflowId": "t" }] },
            { "workflowId": "t", "dependsOn": ["x", "y"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "x", "steps": [{ "stepId": "call", "workflowId": "u" }] },
            { "workflowId": "u", "dependsOn": ["y"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "y", "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let mut client = (0..3).fold(Fake::new(), |client, _| client.reply(200, &json!([])));
    let report = execute(&description, &options().workflow("w"), &mut client).unwrap();
    assert_eq!(order(&report), ["y.s", "u.s", "x.call", "t.s", "w.call"]);
}

#[test]
fn a_calling_steps_timeout_covers_the_dependencies_of_what_it_calls() {
    use roas_arazzo_executor::{HttpResponse, Progress, Run};
    let description = description(
        json!([
            { "workflowId": "w", "steps": [
                { "stepId": "call", "workflowId": "t", "timeout": 30 }] },
            { "workflowId": "t", "dependsOn": ["slow"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] },
            { "workflowId": "slow", "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let options = options().workflow("w");
    let mut run = Run::start(&description, &options).unwrap();
    for wait in [60, 0] {
        assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
        // `slow` answers after the calling step's whole budget.
        std::thread::sleep(std::time::Duration::from_millis(wait));
        run.supply(HttpResponse::json(200, &json!([]))).unwrap();
    }
    let Progress::Done(report) = run.advance().unwrap() else {
        panic!("done")
    };
    assert_eq!(report.outcome, Outcome::Failed, "{report:?}");
}

#[test]
fn entering_a_workflow_that_depends_on_one_still_running_is_a_cycle() {
    // `w` calls `t`, and `t` depends on `w`, which cannot finish first.
    let description = description(
        json!([
            { "workflowId": "w", "steps": [
                { "stepId": "first", "operationId": "listPets" },
                { "stepId": "call", "workflowId": "t" }] },
            { "workflowId": "t", "dependsOn": ["w"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let options = options().workflow("w");
    let mut client = Fake::new().reply(200, &json!([]));
    let error = execute(&description, &options, &mut client).unwrap_err();
    assert!(
        matches!(&error, ExecutionError::Circular(id) if id == "w"),
        "{error}"
    );
    // Nothing of `t` was sent.
    assert_eq!(client.sent().len(), 1);
    let plan = roas_arazzo_executor::prepare(&description, &options).expect("a plan");
    let failure = plan
        .execute(&mut Fake::new().reply(200, &json!([])))
        .unwrap_err();
    assert_eq!(failure.error.to_string(), error.to_string());
}

#[test]
fn a_dependency_on_a_workflow_running_as_an_ordinary_call_is_a_cycle() {
    // `w` calls `a`, `a` calls `b`, and `b` depends on `a`: the `a` that is
    // running waits for `b`, so a second `a` is not started.
    let description = description(
        json!([
            { "workflowId": "w", "steps": [{ "stepId": "call", "workflowId": "a" }] },
            { "workflowId": "a", "steps": [
                { "stepId": "first", "operationId": "listPets" },
                { "stepId": "call", "workflowId": "b" }] },
            { "workflowId": "b", "dependsOn": ["a"],
                "steps": [{ "stepId": "s", "operationId": "listPets" }] }
        ]),
        None,
    );
    let mut client = (0..3).fold(Fake::new(), |client, _| client.reply(200, &json!([])));
    let error = execute(&description, &options().workflow("w"), &mut client).unwrap_err();
    assert!(
        matches!(&error, ExecutionError::Circular(id) if id == "a"),
        "{error}"
    );
    assert_eq!(client.sent().len(), 1);
}
