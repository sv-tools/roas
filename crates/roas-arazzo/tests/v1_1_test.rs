//! Integration tests for Arazzo v1.1: load fixtures from
//! `tests/v1_1_data/`, parse them (JSON and YAML), and validate.

#![cfg(feature = "v1_1")]

use enumset::EnumSet;
use roas_arazzo::v1_1::{Description, SourceType, StepAction, ValueOrSelector};
use roas_arazzo::validation::Validate;
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v1_1_data")
}

fn read(name: &str) -> String {
    let path = data_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_json(name: &str) -> Description {
    serde_json::from_str(&read(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn load_yaml(name: &str) -> Description {
    serde_yaml_ng::from_str(&read(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

#[test]
fn minimal_description_parses_and_validates() {
    let doc = load_json("minimal.json");
    doc.validate(EnumSet::empty()).expect("must validate");
    assert_eq!(doc.workflows[0].workflow_id, "getPet");
}

#[test]
fn full_async_yaml_parses_and_validates() {
    let doc = load_yaml("full_async.yaml");
    doc.validate(EnumSet::empty()).expect("must validate");

    assert_eq!(doc.self_.as_deref(), Some("urn:example:arazzo:pets"));
    assert_eq!(doc.source_descriptions.len(), 2);
    // The AsyncAPI receive step.
    let await_step = &doc.workflows[0].steps[1];
    assert_eq!(await_step.step_id, "await");
    assert!(await_step.channel_path.is_some());
    assert!(await_step.correlation_id.is_some());

    // Round-trips through JSON.
    let json = serde_json::to_string(&doc).expect("serialize");
    let reparsed: Description = serde_json::from_str(&json).expect("reparse");
    assert_eq!(reparsed, doc);
}

#[test]
fn the_specifications_own_mixed_example_parses_and_validates() {
    // The one published description that mixes an OpenAPI source with
    // an AsyncAPI one — see the fixture's header for what had to be
    // corrected in it, and why.
    let doc = load_yaml("spec_example.yaml");
    doc.validate(EnumSet::empty()).expect("must validate");

    assert_eq!(
        doc.self_.as_deref(),
        Some("https://api.example.com/workflows/pet-purchase.arazzo.yaml")
    );
    let kinds: Vec<_> = doc
        .source_descriptions
        .iter()
        .map(|source| (source.name.as_str(), source.type_))
        .collect();
    assert_eq!(
        kinds,
        [
            ("petStoreDescription", Some(SourceType::Openapi)),
            ("asyncOrderApiDescription", Some(SourceType::Asyncapi)),
        ]
    );

    // Two HTTP steps, named the two ways a step may name an operation.
    let steps = &doc.workflows[0].steps;
    assert_eq!(steps.len(), 4);
    assert_eq!(
        steps[0].operation_id.as_deref(),
        Some("$sourceDescriptions.petStoreDescription.loginUser")
    );
    assert_eq!(
        steps[1].operation_path.as_deref(),
        Some("{$sourceDescriptions.petStoreDescription.url}#/paths/~1pet~1findByStatus/get")
    );

    // Then a send to a channel, and a correlated wait for the reply.
    assert_eq!(steps[2].action, Some(StepAction::Send));
    assert_eq!(
        steps[2].operation_id.as_deref(),
        Some("$sourceDescriptions.asyncOrderApiDescription.placeOrder")
    );
    assert_eq!(steps[3].action, Some(StepAction::Receive));
    assert_eq!(
        steps[3].correlation_id,
        Some(serde_json::json!("$inputs.orderCorrelationId"))
    );
    assert_eq!(steps[3].timeout, Some(6000));
    assert_eq!(
        steps[3].outputs["orderId"],
        ValueOrSelector::literal("$message.payload.orderId")
    );
}

#[test]
fn an_async_step_may_not_name_an_operation_by_path() {
    // What the published example got wrong: the schema's
    // `asyncapi-step-object` is `operationId` or `channelPath`.
    let mut document: serde_json::Value =
        serde_yaml_ng::from_str(&read("spec_example.yaml")).expect("the fixture");
    let step = &mut document["workflows"][0]["steps"][2];
    let named = step["operationId"].take();
    step.as_object_mut().expect("a step").remove("operationId");
    step["operationPath"] = named;

    let doc: Description = serde_json::from_value(document).expect("it still parses");
    let errors = doc
        .validate(EnumSet::empty())
        .expect_err("but it does not validate");
    assert!(
        errors.errors.iter().any(|error| error
            .message
            .contains("`operationPath` is not valid on an AsyncAPI step")),
        "got: {errors:?}"
    );
}

#[test]
fn asyncapi_step_without_action_fails_validation() {
    let err = load_json("bad_asyncapi_no_action.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    assert!(
        err.errors.iter().any(|e| e.contains("AsyncAPI step")),
        "got: {err}",
    );
}

#[test]
fn self_with_fragment_fails_validation() {
    let err = load_json("bad_self_fragment.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e == "#.$self: must not contain a fragment (`#`)"),
        "got: {err}",
    );
}
