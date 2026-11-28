//! Keys an object does not define: kept on parsing, reported by validation unless
//! `ValidationOptions::IgnoreUnknownFields` is set, and stripped by the version
//! upconversions.

#![cfg(any(feature = "v1_0", feature = "v1_1", feature = "v1_2"))]

use enumset::EnumSet;
use roas_overlay::apply::Apply;
use roas_overlay::validation::{Validate, ValidationOptions};
use serde_json::json;

/// The sorted paths validation reports as unknown fields.
fn unknown_field_paths(
    overlay: &impl Validate,
    options: EnumSet<ValidationOptions>,
) -> Vec<String> {
    let Err(error) = overlay.validate(options) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = error
        .errors
        .iter()
        .filter(|error| {
            error.message == "not a field of this object; only `x-` extensions may be added"
        })
        .map(|error| error.path.clone())
        .collect();
    paths.sort();
    paths
}

#[cfg(feature = "v1_0")]
fn v1_0_document() -> serde_json::Value {
    json!({
        "overlay": "1.0.0",
        "info": {
            "title": "Petstore overlay",
            "version": "1.0.0",
            "x-owner": "api-team",
            "bogus": true
        },
        "actions": [
            {
                "target": "$.info",
                "update": { "description": "Updated" },
                "x-step": 1,
                "bogus": "action-extra"
            }
        ],
        "x-root": "kept",
        "bogus": 123
    })
}

#[cfg(feature = "v1_0")]
#[test]
fn v1_0_reports_unknown_fields_round_trips_and_applies() {
    let doc = v1_0_document();
    let overlay: roas_overlay::v1_0::Overlay = serde_json::from_value(doc.clone()).unwrap();

    assert_eq!(
        unknown_field_paths(&overlay, EnumSet::empty()),
        ["#.actions[0].bogus", "#.bogus", "#.info.bogus"],
    );
    assert!(
        overlay
            .validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .is_ok()
    );

    // Round-trip preserves both x- extensions and unknown keys.
    assert_eq!(serde_json::to_value(&overlay).unwrap(), doc);

    // Applying still succeeds cleanly when unknown keys exist in `extensions`.
    let mut target = json!({ "info": { "title": "API" } });
    overlay.apply(&mut target, EnumSet::empty()).unwrap();
    assert_eq!(target["info"]["description"], "Updated");
}

#[cfg(feature = "v1_1")]
fn v1_1_document() -> serde_json::Value {
    json!({
        "overlay": "1.1.0",
        "info": {
            "title": "Petstore overlay",
            "version": "1.1.0",
            "description": "Adds info",
            "x-owner": "api-team",
            "bogus": true
        },
        "actions": [
            {
                "target": "$.info",
                "update": { "description": "Updated" },
                "x-step": 1,
                "bogus": "action-extra"
            }
        ],
        "x-root": "kept",
        "bogus": 123
    })
}

#[cfg(feature = "v1_1")]
#[test]
fn v1_1_reports_unknown_fields_round_trips_and_applies() {
    let doc = v1_1_document();
    let overlay: roas_overlay::v1_1::Overlay = serde_json::from_value(doc.clone()).unwrap();

    assert_eq!(
        unknown_field_paths(&overlay, EnumSet::empty()),
        ["#.actions[0].bogus", "#.bogus", "#.info.bogus"],
    );
    assert!(
        overlay
            .validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .is_ok()
    );

    assert_eq!(serde_json::to_value(&overlay).unwrap(), doc);

    let mut target = json!({ "info": { "title": "API" } });
    overlay.apply(&mut target, EnumSet::empty()).unwrap();
    assert_eq!(target["info"]["description"], "Updated");
}

#[cfg(feature = "v1_2")]
fn v1_2_document() -> serde_json::Value {
    json!({
        "overlay": "1.2.0",
        "$self": "https://example.com/overlays/test.yaml",
        "info": {
            "title": "Petstore overlay",
            "version": "1.2.0",
            "description": "Adds info",
            "x-owner": "api-team",
            "bogus": "info"
        },
        "actions": [
            {
                "target": "$.info",
                "update": { "description": "Updated" },
                "x-inline": true,
                "bogus": "inline-action"
            },
            {
                "$ref": "#/components/actions/addTag",
                "target": "$.info",
                "x-ref": true,
                "bogus": "action-ref"
            }
        ],
        "components": {
            "actions": {
                "addTag": {
                    "description": "Reusable action",
                    "fields": {
                        "update": { "x-tagged": true },
                        "x-fields": true,
                        "bogus": "reusable-fields"
                    },
                    "x-reusable": true,
                    "bogus": "reusable-action"
                }
            },
            "x-components": true,
            "bogus": "components"
        },
        "x-root": "kept",
        "bogus": "root"
    })
}

#[cfg(feature = "v1_2")]
#[test]
fn v1_2_reports_unknown_fields_on_every_object_round_trips_and_applies() {
    let doc = v1_2_document();
    let overlay: roas_overlay::v1_2::Overlay = serde_json::from_value(doc.clone()).unwrap();

    assert_eq!(
        unknown_field_paths(&overlay, EnumSet::empty()),
        [
            "#.actions[0].bogus",
            "#.actions[1].bogus",
            "#.bogus",
            "#.components.actions.addTag.bogus",
            "#.components.actions.addTag.fields.bogus",
            "#.components.bogus",
            "#.info.bogus",
        ],
    );
    assert!(
        overlay
            .validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .is_ok()
    );

    // Round-trip preserves every x- extension and every unknown field.
    assert_eq!(serde_json::to_value(&overlay).unwrap(), doc);

    // Applying still succeeds cleanly when unknown keys exist everywhere.
    let mut target = json!({ "info": { "title": "API" } });
    overlay.apply(&mut target, EnumSet::empty()).unwrap();
    assert_eq!(target["info"]["description"], "Updated");
    assert_eq!(target["info"]["x-tagged"], json!(true));
}

#[cfg(all(feature = "v1_0", feature = "v1_1"))]
#[test]
fn v1_0_to_v1_1_conversion_strips_unknown_fields_and_keeps_extensions() {
    // Include unknown fields that happen to have v1.1 field names (`description`
    // on v1.0 Info, `copy` on v1.0 Action) plus an action with ONLY an unknown
    // key so `extensions` becomes `None`.
    let src: roas_overlay::v1_0::Overlay = serde_json::from_value(json!({
        "overlay": "1.0.0",
        "info": {
            "title": "T",
            "version": "1.0.0",
            "description": 123,
            "x-info": "keep"
        },
        "actions": [
            {
                "target": "$.info",
                "update": { "summary": "ok" },
                "copy": 456,
                "x-action": "keep"
            },
            {
                "target": "$.paths",
                "remove": true,
                "only_unknown": true
            }
        ],
        "bogus": "root-unknown",
        "x-root": "keep"
    }))
    .unwrap();

    // Source validation catches all four unknown keys before conversion.
    assert_eq!(
        unknown_field_paths(&src, EnumSet::empty()),
        [
            "#.actions[0].copy",
            "#.actions[1].only_unknown",
            "#.bogus",
            "#.info.description",
        ],
    );

    let dst: roas_overlay::v1_1::Overlay = src.into();
    dst.validate(EnumSet::empty())
        .expect("converted v1.1 overlay must have no unknown fields");

    assert_eq!(
        serde_json::to_value(&dst).unwrap(),
        json!({
            "overlay": "1.1.0",
            "info": {
                "title": "T",
                "version": "1.0.0",
                "x-info": "keep"
            },
            "actions": [
                {
                    "target": "$.info",
                    "update": { "summary": "ok" },
                    "x-action": "keep"
                },
                {
                    "target": "$.paths",
                    "remove": true
                }
            ],
            "x-root": "keep"
        }),
    );
    assert!(dst.actions[1].extensions.is_none());
}

#[cfg(all(feature = "v1_1", feature = "v1_2"))]
#[test]
fn v1_1_to_v1_2_conversion_strips_unknown_fields_and_keeps_extensions() {
    // Include unknown fields that happen to have v1.2 field names (`$self`,
    // `components`) plus an object with ONLY an unknown key.
    let src: roas_overlay::v1_1::Overlay = serde_json::from_value(json!({
        "overlay": "1.1.0",
        "$self": 123,
        "components": "not-an-object",
        "info": {
            "title": "T",
            "version": "1.1.0",
            "only_unknown": true
        },
        "actions": [
            {
                "target": "$.info",
                "update": { "summary": "ok" },
                "bogus": "drop-me",
                "x-action": "keep"
            }
        ],
        "x-root": "keep"
    }))
    .unwrap();

    assert_eq!(
        unknown_field_paths(&src, EnumSet::empty()),
        [
            "#.$self",
            "#.actions[0].bogus",
            "#.components",
            "#.info.only_unknown",
        ],
    );

    let dst: roas_overlay::v1_2::Overlay = src.into();
    dst.validate(EnumSet::empty())
        .expect("converted v1.2 overlay must have no unknown fields");

    assert_eq!(
        serde_json::to_value(&dst).unwrap(),
        json!({
            "overlay": "1.2.0",
            "info": {
                "title": "T",
                "version": "1.1.0"
            },
            "actions": [
                {
                    "target": "$.info",
                    "update": { "summary": "ok" },
                    "x-action": "keep"
                }
            ],
            "x-root": "keep"
        }),
    );
    assert!(dst.info.extensions.is_none());
}
