//! Integration tests for Overlay v1.0: load fixtures from
//! `tests/v1_0_data/`, parse them, validate, apply, and compare
//! against checked-in `*.expected.json` fixtures.

#![cfg(feature = "v1_0")]

use enumset::EnumSet;
use roas_overlay::apply::{Apply, ApplyErrorKind, ApplyOptions};
use roas_overlay::v1_0::Overlay;
use roas_overlay::validation::{Validate, ValidationOptions};
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v1_0_data")
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
fn targeted_overlay_applies_and_validates() {
    let overlay = load_overlay_json("targeted_overlay.json");
    overlay
        .validate(EnumSet::empty())
        .expect("overlay must validate");

    let mut target = load_json("base_petstore.json");
    let report = overlay
        .apply(&mut target, EnumSet::empty())
        .expect("apply must succeed");
    assert_eq!(report.actions.len(), 2);
    assert!(report.actions.iter().all(|a| a.matched == 1));

    let expected = load_json("targeted_overlay.expected.json");
    assert_eq!(target, expected);
}

#[test]
fn remove_action_drops_internal_path() {
    let overlay = load_overlay_json("remove_internal.json");
    overlay.validate(EnumSet::empty()).expect("validates");

    let mut target = load_json("base_petstore.json");
    let report = overlay
        .apply(&mut target, EnumSet::empty())
        .expect("apply succeeds");
    assert_eq!(report.actions[0].matched, 1);

    let expected = load_json("remove_internal.expected.json");
    assert_eq!(target, expected);
}

#[test]
fn sequential_yaml_overlay_composes_actions_in_order() {
    let overlay = load_overlay_yaml("sequential_actions.yaml");
    overlay.validate(EnumSet::empty()).expect("validates");

    let mut target = load_json("base_petstore.json");
    overlay
        .apply(&mut target, EnumSet::empty())
        .expect("apply succeeds");

    let expected = load_json("sequential_actions.expected.json");
    assert_eq!(target, expected);
}

#[test]
fn empty_actions_fixture_fails_validation_with_helpful_path() {
    let overlay = load_overlay_json("bad_empty_actions.json");
    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e == "#.actions: must contain at least one entry"),
        "got: {err}",
    );
}

#[test]
fn invalid_jsonpath_fixture_fails_validation_and_apply() {
    let overlay = load_overlay_json("bad_invalid_jsonpath.json");
    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert!(
        err.errors.iter().any(|e| e.contains("invalid JSONPath")),
        "got: {err}",
    );

    let mut target = serde_json::json!({});
    let snapshot = target.clone();
    let apply_err = overlay.apply(&mut target, EnumSet::empty()).unwrap_err();
    assert!(matches!(apply_err.kind, ApplyErrorKind::InvalidJsonPath(_)));
    assert_eq!(target, snapshot, "target must be unchanged on apply error");
}

#[test]
fn conflicting_remove_and_update_fixture_fails_validation() {
    let overlay = load_overlay_json("bad_remove_and_update.json");
    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert!(
        err.errors.iter().any(|e| e.contains("mutually exclusive")),
        "got: {err}",
    );
}

#[test]
fn error_on_zero_match_aborts_and_rolls_back() {
    let overlay = load_overlay_json("targeted_overlay.json");
    let mut target = serde_json::json!({ "openapi": "3.1.0", "info": {} });
    let snapshot = target.clone();
    let err = overlay
        .apply(&mut target, ApplyOptions::ErrorOnZeroMatch.into())
        .unwrap_err();
    assert!(matches!(err.kind, ApplyErrorKind::ZeroMatch));
    assert_eq!(target, snapshot);
}

#[test]
fn ignore_info_options_suppress_diagnostics() {
    let mut overlay = load_overlay_json("targeted_overlay.json");
    overlay.info.title.clear();
    overlay.info.version.clear();

    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e == "#.info.title: must not be empty")
    );
    assert!(
        err.errors
            .iter()
            .any(|e| e == "#.info.version: must not be empty")
    );

    let opts = ValidationOptions::IgnoreEmptyInfoTitle | ValidationOptions::IgnoreEmptyInfoVersion;
    overlay.validate(opts).expect("validates with options");
}
