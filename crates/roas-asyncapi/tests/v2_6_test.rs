//! Integration tests for AsyncAPI v2.6: load fixtures from
//! `tests/v2_6_data/`, parse them (JSON and YAML), and validate.

#![cfg(feature = "v2_6")]

use enumset::EnumSet;
use roas_asyncapi::v2_6::{Document, OperationKind, OperationMessage};
use roas_asyncapi::validation::{Validate, ValidationOptions};
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v2_6_data")
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
    assert_eq!(doc.channels.len(), 2);
    // 2.6 keeps tags and externalDocs at the root, not under `info`.
    assert_eq!(doc.tags.len(), 1);
    assert!(doc.external_docs.is_some());

    // A server is one `url`, not host + pathname.
    let server = doc.servers["production"].item().expect("inline server");
    assert_eq!(server.url, "{stage}.broker.example.com:9092");
    assert!(server.variables.contains_key("stage"));

    // Operations hang off the channel path, from the consumer's point
    // of view.
    let measured = &doc.channels["smartylighting/streetlights/{streetlightId}/lighting/measured"];
    assert!(measured.publish.is_some());
    assert!(measured.subscribe.is_none());
    assert_eq!(measured.servers, vec!["production"]);

    // A 2.6 parameter carries a full schema — the field v3 dropped.
    let parameter = measured.parameters["streetlightId"]
        .item()
        .expect("inline parameter");
    assert!(parameter.schema.is_some());

    // An operation's message may be a set of alternatives.
    let turn = &doc.channels["smartylighting/streetlights/{streetlightId}/command/turn"];
    let subscribe = turn.subscribe.as_ref().expect("subscribe operation");
    match subscribe.message.as_ref().expect("message") {
        OperationMessage::OneOf(one_of) => assert_eq!(one_of.one_of.len(), 2),
        other => panic!("expected the oneOf form, got {other:?}"),
    }

    // The message declares its own dialect rather than wrapping it.
    let message = doc.components.as_ref().unwrap().messages["lightMeasured"]
        .item()
        .expect("inline message");
    assert_eq!(
        message.schema_format.as_deref(),
        Some("application/vnd.aai.asyncapi;version=2.6.0")
    );
    assert!(message.payload.is_some());

    let json = serde_json::to_string(&doc).expect("serialize");
    let reparsed: Document = serde_json::from_str(&json).expect("reparse");
    assert_eq!(reparsed, doc);
}

#[test]
fn operations_enumerates_every_channel_half() {
    let doc = load_yaml("streetlights.yaml");
    let operations = doc.operations();
    assert_eq!(operations.len(), 2);

    let kinds: Vec<_> = operations.iter().map(|(_, kind, _)| *kind).collect();
    assert!(kinds.contains(&OperationKind::Publish));
    assert!(kinds.contains(&OperationKind::Subscribe));

    let ids: Vec<_> = operations
        .iter()
        .filter_map(|(_, _, op)| op.operation_id.as_deref())
        .collect();
    assert!(ids.contains(&"receiveLightMeasurement"));
    assert!(ids.contains(&"sendTurnCommand"));
}

#[test]
fn a_v3_document_does_not_parse_as_2_6() {
    let v3 = read("../v3_1_data/minimal.json");
    assert!(
        serde_json::from_str::<Document>(&v3).is_err(),
        "the version newtype must reject 3.1.0",
    );
}

#[test]
fn broken_wiring_reports_every_problem() {
    let err = load_json("bad_wiring.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    let errors: Vec<_> = err.errors.iter().map(ToString::to_string).collect();

    assert!(
        errors
            .iter()
            .any(|e| e.contains("server `staging` is not declared")),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("duplicate operationId `handleUser`")),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("missingScheme: is not declared in `components.securitySchemes`")),
        "got: {errors:?}",
    );
    // `scramSha512` takes no scopes.
    assert!(
        errors
            .iter()
            .any(|e| e.contains("must not list scopes: the `scramSha512` scheme type takes none")),
        "got: {errors:?}",
    );
}

#[test]
fn channel_parameters_must_match_the_path() {
    let err = load_json("bad_parameters.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    let errors: Vec<_> = err.errors.iter().map(ToString::to_string).collect();

    assert!(
        errors
            .iter()
            .any(|e| e.contains("`{metric}` in the channel path is not declared in `parameters`")),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`unused` is declared but never used in the channel path")),
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
    let err = load_json("bad_parameters.json")
        .validate(EnumSet::only(
            ValidationOptions::IgnoreUnusedChannelParameter,
        ))
        .unwrap_err();
    assert!(
        !err.errors
            .iter()
            .any(|e| e.contains("declared but never used")),
        "got: {err}",
    );
}
