//! Integration tests for Overlay v1.1: load fixtures from
//! `tests/v1_1_data/`, parse them, validate, apply, and compare
//! against checked-in `*.expected.json` fixtures.

#![cfg(feature = "v1_1")]

use enumset::EnumSet;
use roas_overlay::apply::{Apply, ApplyErrorKind};
use roas_overlay::v1_1::Overlay;
use roas_overlay::validation::Validate;
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v1_1_data")
}

fn load_json(name: &str) -> serde_json::Value {
    let path = data_dir().join(name);
    let s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn load_yaml(name: &str) -> serde_json::Value {
    let path = data_dir().join(name);
    let s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml_ng::from_str(&s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn load_overlay_json(name: &str) -> Overlay {
    serde_json::from_value(load_json(name)).expect("overlay deserialize")
}

fn load_overlay_yaml(name: &str) -> Overlay {
    serde_json::from_value(load_yaml(name)).expect("overlay deserialize")
}

#[test]
fn copy_action_mirrors_source_under_destination() {
    let overlay = load_overlay_json("copy_overlay.json");
    overlay.validate(EnumSet::empty()).expect("validates");

    let mut target = load_json("base_with_source.json");
    let report = overlay
        .apply(&mut target, EnumSet::empty())
        .expect("apply succeeds");
    assert_eq!(report.actions.len(), 1);

    let expected = load_json("copy_overlay.expected.json");
    assert_eq!(target, expected);
}

#[test]
fn copy_then_update_pattern_lets_authors_compose() {
    let overlay = load_overlay_yaml("copy_and_extend.yaml");
    overlay.validate(EnumSet::empty()).expect("validates");

    let mut target = load_json("base_with_source.json");
    overlay
        .apply(&mut target, EnumSet::empty())
        .expect("apply succeeds");

    let expected = load_json("copy_and_extend.expected.json");
    assert_eq!(target, expected);
}

#[test]
fn conflicting_copy_and_update_fixture_fails_validation() {
    let overlay = load_overlay_json("bad_copy_and_update.json");
    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert!(
        err.errors.iter().any(|e| e.contains("`update` and `copy`")),
        "got: {err}",
    );
}

#[test]
fn invalid_copy_jsonpath_fixture_fails_validation_and_apply() {
    let overlay = load_overlay_json("bad_copy_invalid_jsonpath.json");
    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e.contains("invalid JSONPath") && e.contains(".copy")),
        "got: {err}",
    );

    let mut target = serde_json::json!({ "dest": {} });
    let snapshot = target.clone();
    let apply_err = overlay.apply(&mut target, EnumSet::empty()).unwrap_err();
    assert!(matches!(apply_err.kind, ApplyErrorKind::InvalidJsonPath(_)));
    assert_eq!(target, snapshot, "target must be unchanged on apply error");
}

#[cfg(feature = "v1_0")]
#[test]
fn v1_0_overlay_upconverts_to_v1_1() {
    use roas_overlay::v1_0;

    let v10_json = serde_json::json!({
        "overlay": "1.0.0",
        "info": { "title": "T", "version": "1.0.0" },
        "actions": [
            { "target": "$.info", "update": { "description": "Patched." } }
        ]
    });
    let src: v1_0::Overlay = serde_json::from_value(v10_json).unwrap();
    let dst: Overlay = src.into();

    // Apply the upconverted overlay and confirm it still works.
    let mut target = serde_json::json!({
        "openapi": "3.1.0",
        "info": { "title": "API", "version": "1.0.0" }
    });
    dst.apply(&mut target, EnumSet::empty()).unwrap();
    assert_eq!(target["info"]["description"], "Patched.");
}
