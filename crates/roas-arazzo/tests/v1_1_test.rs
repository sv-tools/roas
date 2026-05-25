//! Integration tests for Arazzo v1.1: load fixtures from
//! `tests/v1_1_data/`, parse them (JSON and YAML), and validate.

#![cfg(feature = "v1_1")]

use enumset::EnumSet;
use roas_arazzo::v1_1::Description;
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
