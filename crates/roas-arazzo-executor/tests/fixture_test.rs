//! A whole description, as one would be written: YAML in, requests out.
//!
//! The description is Arazzo v1.0, so this also covers the upconversion
//! path — one interpreter, both versions.

#![cfg(feature = "v1_0")]

use roas_arazzo::v1_0::Description;
use roas_arazzo::validation::Validate;
use roas_arazzo_executor::testing::Fake;
use roas_arazzo_executor::{HttpResponse, Options, Outcome, Progress, Run, execute_v1_0};
use serde_json::{Value, json};
use std::time::Duration;

fn description() -> Description {
    let yaml = include_str!("data/buy_pet.arazzo.yaml");
    let description: Description =
        serde_yaml_ng::from_str(yaml).expect("the fixture is a v1.0 description");
    description
        .validate(enumset::EnumSet::empty())
        .expect("the fixture is valid");
    description
}

fn options() -> Options {
    let openapi: Value = serde_yaml_ng::from_str(include_str!("data/petstore.openapi.yaml"))
        .expect("the fixture is an OpenAPI document");
    Options::new()
        .source("petStore", "https://api.example.com/openapi.yaml", openapi)
        .input("petId", "7")
        .input("token", "abc")
}

#[test]
fn the_whole_description_runs_and_says_what_it_did() {
    let mut client = Fake::new()
        .reply(200, &json!({ "id": 7, "name": "fluffy", "tags": ["cat"] }))
        .reply(201, &json!({ "orderId": "o-1", "status": "placed" }));

    let report = execute_v1_0(&description(), &options(), &mut client).expect("the workflow runs");

    // The server variable's default filled the host in, the path
    // parameter the pet id, and the workflow parameter the header.
    assert_eq!(client.sent()[0].url, "https://api.example.com/v1/pets/7",);
    assert_eq!(client.sent()[0].header("authorization"), Some("Bearer abc"));

    // The second step took its id from the first step's output.
    assert_eq!(
        client.sent()[1].url,
        "https://api.example.com/v1/pets/7/order",
    );
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_ref().expect("a body"))
            .expect("json"),
        json!({ "petId": 7, "quantity": 1, "note": "a gift for 7" }),
        "the payload's expression is filled in and the replacement added its own"
    );

    assert_eq!(report.outcome, Outcome::Ended, "`end` finished it early");
    assert_eq!(report.outputs["petName"], json!("fluffy"));
    assert_eq!(report.outputs["orderId"], json!("o-1"));
    assert_eq!(client.sent().len(), 2, "the third step was never reached");

    let text = report.to_string();
    assert!(text.contains("workflow `buyPet` ended early"), "{text}");
    assert!(
        text.contains("findPet GET https://api.example.com/v1/pets/7 → 200"),
        "{text}"
    );
}

#[test]
fn the_store_being_busy_is_retried_the_way_the_description_asks() {
    // `Run` borrows the description it runs, so a v1.0 one is
    // upconverted first — which is what `execute_v1_0` does inside.
    let description = roas_arazzo::v1_1::Description::from(description());
    let options = options();
    let mut run = Run::start(&description, &options).expect("a run");

    let mut sent = 0;
    let mut waits = Vec::new();
    let report = loop {
        match run.advance().expect("progress") {
            Progress::Send(_) => {
                sent += 1;
                let response = match sent {
                    1 => HttpResponse::json(503, &json!({ "busy": true })),
                    2 => HttpResponse::json(200, &json!({ "id": 7, "name": "f", "tags": ["cat"] })),
                    _ => HttpResponse::json(201, &json!({ "orderId": "o-2", "status": "placed" })),
                };
                run.supply(response).expect("the response is understood");
            }
            Progress::Wait(duration) => waits.push(duration),
            Progress::Done(report) => break report,
        }
    };

    assert_eq!(waits, [Duration::from_millis(500)]);
    assert_eq!(sent, 3, "one failure, its retry, and the order");
    assert_eq!(report.outputs["orderId"], json!("o-2"));
    assert_eq!(report.steps[0].attempt, 1);
    assert_eq!(report.steps[0].action.as_deref(), Some("retry"));
    assert_eq!(report.steps[1].attempt, 2);
}

#[test]
fn a_pet_with_no_tags_fails_the_jsonpath_criterion() {
    let mut client = Fake::new().reply(200, &json!({ "id": 7, "name": "fluffy", "tags": [] }));

    let report = execute_v1_0(&description(), &options(), &mut client).expect("the run is fine");

    assert_eq!(report.outcome, Outcome::Failed);
    let criteria = &report.steps[0].criteria;
    assert_eq!(criteria.len(), 2);
    assert!(criteria[0].passed, "the status was fine");
    assert!(!criteria[1].passed, "but nothing matched `$.tags[*]`");
    assert_eq!(client.sent().len(), 1);
}
