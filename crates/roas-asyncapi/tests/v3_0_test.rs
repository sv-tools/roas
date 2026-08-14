//! Integration tests for AsyncAPI v3.0: load fixtures from
//! `tests/v3_0_data/`, parse them (JSON and YAML), and validate.

#![cfg(feature = "v3_0")]

use enumset::EnumSet;
use roas_asyncapi::v3_0::{Document, OperationAction, SchemaOrMultiFormat};
use roas_asyncapi::validation::{Validate, ValidationOptions};
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v3_0_data")
}

fn read(name: &str) -> String {
    let path = data_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_json(name: &str) -> Document {
    serde_json::from_str(&read(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

fn load_yaml(name: &str) -> Document {
    serde_yaml_ng::from_str(&read(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

#[test]
fn minimal_document_parses_and_validates() {
    let doc = load_json("minimal.json");
    doc.validate(EnumSet::empty()).expect("must validate");
    assert_eq!(doc.info.title, "Streetlights");
    assert!(doc.channels.is_empty());
}

#[test]
fn streetlights_yaml_parses_validates_and_round_trips() {
    let doc = load_yaml("streetlights.yaml");
    doc.validate(EnumSet::empty()).expect("must validate");

    assert_eq!(doc.id.as_deref(), Some("urn:example:streetlights"));
    assert_eq!(
        doc.default_content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(doc.channels.len(), 3);
    assert_eq!(doc.operations.len(), 2);

    // The server's `{stage}` placeholder is declared as a variable.
    let server = doc.servers["production"].item().expect("inline server");
    assert_eq!(server.protocol, "kafka-secure");
    assert!(server.variables.contains_key("stage"));
    assert_eq!(
        server
            .bindings
            .as_ref()
            .and_then(|b| b.item())
            .and_then(|b| b.binding_version("kafka")),
        Some("0.5.0")
    );

    // `address: null` is preserved as an explicit unknown address,
    // distinct from an absent one.
    let dynamic = doc.channels["turnOnOff"].item().expect("inline channel");
    assert_eq!(dynamic.address, Some(None));
    assert_eq!(dynamic.address(), None);

    // Actions read from the application's point of view.
    let receive = doc.operations["receiveLightMeasurement"]
        .item()
        .expect("inline operation");
    assert_eq!(receive.action, OperationAction::Receive);
    let send = doc.operations["sendTurnOn"]
        .item()
        .expect("inline operation");
    assert_eq!(send.action, OperationAction::Send);
    assert!(send.reply.is_some());

    // The component message carries a typed default-dialect payload.
    let message = doc.components.as_ref().unwrap().messages["lightMeasured"]
        .item()
        .expect("inline message");
    match message.payload.as_ref() {
        Some(SchemaOrMultiFormat::Schema(schema)) => {
            assert_eq!(schema.required, vec!["lumens"]);
            assert!(schema.properties.contains_key("streetlightId"));
        }
        other => panic!("expected a typed payload schema, got {other:?}"),
    }

    // Round-trips through JSON.
    let json = serde_json::to_string(&doc).expect("serialize");
    let reparsed: Document = serde_json::from_str(&json).expect("reparse");
    assert_eq!(reparsed, doc);
}

#[test]
fn broken_wiring_reports_every_dangling_reference() {
    let err = load_json("bad_wiring.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    let errors: Vec<_> = err.errors.iter().map(ToString::to_string).collect();

    assert!(
        errors.iter().any(|e| e
            == "#.channels.userSignedUp.servers[0].$ref: server `#/servers/staging` names nothing in this document"),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains(
                "message `#/channels/other/messages/ping` must point at a message of `#/channels/userSignedUp`"
            )),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("channel `#/channels/missing` names nothing in this document")),
        "got: {errors:?}",
    );
}

#[test]
fn channel_parameters_must_match_the_address() {
    let err = load_json("bad_parameters.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    let errors: Vec<_> = err.errors.iter().map(ToString::to_string).collect();

    assert!(
        errors
            .iter()
            .any(|e| e.contains("`{metric}` in `address` is not declared in `parameters`")),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`unused` is declared but never used in `address`")),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("is not a valid runtime expression")),
        "got: {errors:?}",
    );
}

#[test]
fn unused_channel_parameters_can_be_allowed() {
    let errors = load_json("bad_parameters.json")
        .validate(EnumSet::only(
            ValidationOptions::IgnoreUnusedChannelParameter,
        ))
        .unwrap_err();
    assert!(
        !errors
            .errors
            .iter()
            .any(|e| e.contains("declared but never used")),
        "got: {errors}",
    );
}

#[test]
fn external_references_pass_by_default_and_fail_under_strictness() {
    let doc = load_yaml("external_refs.yaml");
    doc.validate(EnumSet::empty())
        .expect("external refs are not resolved by default");

    let err = doc
        .validate(EnumSet::only(ValidationOptions::ErrorOnExternalReference))
        .unwrap_err();
    assert!(
        err.errors
            .iter()
            .filter(|e| e.contains("external reference"))
            .count()
            >= 2,
        "got: {err}",
    );
}
