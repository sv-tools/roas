//! Integration tests for Overlay v1.2: load fixtures from
//! `tests/v1_2_data/`, parse them, validate, apply, and compare
//! against checked-in `*.expected.json` fixtures.

#![cfg(feature = "v1_2")]

#[path = "support/yaml.rs"]
mod yaml;

use enumset::EnumSet;
use roas_overlay::apply::{ActionReferenceError, Apply, ApplyErrorKind};
use roas_overlay::v1_2::{ActionItem, Overlay};
use roas_overlay::validation::Validate;
use std::path::{Path, PathBuf};

fn data_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v1_2_data")
        .join(name)
}

fn read(name: &str) -> String {
    let path = data_path(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_json(name: &str) -> serde_json::Value {
    serde_json::from_str(&read(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn load_overlay_yaml(name: &str) -> Overlay {
    let value: serde_json::Value =
        yaml::from_str(&read(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"));
    serde_json::from_value(value).expect("overlay deserialize")
}

#[test]
fn reusable_actions_primitive_updates_and_removals_apply() {
    let overlay = load_overlay_yaml("reusable_actions.yaml");
    overlay.validate(EnumSet::empty()).expect("validates");
    assert_eq!(
        overlay.self_uri.as_deref(),
        Some("https://example.com/overlays/public.yaml")
    );
    assert!(matches!(overlay.actions[0], ActionItem::Reference(_)));

    let mut target = load_json("base.json");
    let report = overlay
        .apply(&mut target, EnumSet::empty())
        .expect("apply succeeds");
    let matched: Vec<usize> = report.actions.iter().map(|a| a.matched).collect();
    assert_eq!(matched, [2, 1, 1, 2]);

    assert_eq!(target, load_json("reusable_actions.expected.json"));
}

#[test]
fn unresolvable_reference_fails_validation_and_apply() {
    let overlay: Overlay = serde_json::from_value(load_json("bad_reference.json")).unwrap();

    let err = overlay.validate(EnumSet::empty()).unwrap_err();
    assert_eq!(err.errors.len(), 1);
    assert!(err.errors[0].contains("#.actions[0].$ref"));

    let mut target = load_json("base.json");
    let snapshot = target.clone();
    let err = overlay.apply(&mut target, EnumSet::empty()).unwrap_err();
    assert_eq!(
        err.kind,
        ApplyErrorKind::ActionReference(ActionReferenceError::NotFound(
            "#/components/actions/missing".into()
        )),
    );
    assert_eq!(target, snapshot);
}
