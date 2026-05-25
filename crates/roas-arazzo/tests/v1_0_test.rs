//! Integration tests for Arazzo v1.0: load fixtures from
//! `tests/v1_0_data/`, parse them (JSON and YAML), and validate.

#![cfg(feature = "v1_0")]

use enumset::EnumSet;
use roas_arazzo::v1_0::Description;
use roas_arazzo::validation::Validate;
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v1_0_data")
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
fn full_workflow_yaml_parses_and_validates() {
    let doc = load_yaml("full_workflow.yaml");
    doc.validate(EnumSet::empty()).expect("must validate");

    // Spot-check that the rich structure round-tripped.
    assert_eq!(doc.source_descriptions.len(), 2);
    let wf = &doc.workflows[0];
    assert_eq!(wf.steps.len(), 2);
    assert!(wf.steps[1].request_body.is_some());
    let components = doc.components.as_ref().expect("components present");
    assert!(components.parameters.contains_key("locale"));
    assert!(components.failure_actions.contains_key("notifyOps"));

    // Re-serialize to JSON and back; the document must survive a round trip.
    let json = serde_json::to_string(&doc).expect("serialize");
    let reparsed: Description = serde_json::from_str(&json).expect("reparse");
    assert_eq!(reparsed, doc);
}

#[test]
fn empty_workflows_fixture_fails_validation() {
    let err = load_json("bad_empty_workflows.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e == "#.workflows: must contain at least one entry"),
        "got: {err}",
    );
}

#[test]
fn step_without_operation_fixture_fails_validation() {
    let err = load_json("bad_step_no_operation.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    assert!(
        err.errors.iter().any(|e| e.contains("exactly one of")),
        "got: {err}",
    );
}

#[test]
fn duplicate_workflow_id_fixture_fails_validation() {
    let err = load_json("bad_duplicate_workflow_id.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e.contains("duplicate workflowId `dup`")),
        "got: {err}",
    );
}

#[test]
fn goto_without_target_fixture_fails_validation() {
    let err = load_json("bad_goto_missing_target.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    assert!(err.errors.iter().any(|e| e.contains("goto")), "got: {err}",);
}
