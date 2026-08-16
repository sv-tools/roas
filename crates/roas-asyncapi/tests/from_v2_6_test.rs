//! Converting the v2.6 example document to v3.0.

#![cfg(all(feature = "v2_6", feature = "v3_0"))]

use enumset::EnumSet;
use roas_asyncapi::v3_0::from_v2_6::{NoteKind, convert};
use roas_asyncapi::v3_0::{OperationAction, RefOr};
use roas_asyncapi::validation::Validate;
use roas_asyncapi::{v2_6, v3_0};

const COMMAND: &str = "smartylighting_streetlights__streetlightId__command_turn";
const MEASURED: &str = "smartylighting_streetlights__streetlightId__lighting_measured";

fn converted() -> (v3_0::Document, Vec<String>) {
    let text = std::fs::read_to_string("tests/v2_6_data/streetlights.yaml").expect("the fixture");
    let source: v2_6::Document = serde_yaml_ng::from_str(&text).expect("valid YAML");
    source
        .validate(EnumSet::empty())
        .expect("the fixture is valid 2.6");

    let (document, report) = convert(source);
    let notes = report.notes.iter().map(ToString::to_string).collect();
    (document, notes)
}

#[test]
fn the_converted_document_is_valid_v3_0() {
    let (document, _) = converted();
    document
        .validate(EnumSet::empty())
        .expect("the conversion produces a valid v3.0 document");
    assert_eq!(document.asyncapi.to_string(), "3.0.0");
}

#[test]
fn a_channel_keeps_its_address_and_gains_a_key() {
    let (document, notes) = converted();

    let channel = document.channels[MEASURED].item().expect("inline");
    assert_eq!(
        channel.address.as_ref().and_then(Option::as_deref),
        Some("smartylighting/streetlights/{streetlightId}/lighting/measured"),
    );
    assert!(
        notes
            .iter()
            .any(|note| note.contains("is not a usable key") && note.contains(MEASURED)),
        "got: {notes:?}"
    );
}

#[test]
fn an_operation_leaves_its_channel_and_changes_point_of_view() {
    let (document, notes) = converted();

    // v2.6 `publish` is what a client does, so the application
    // receives; `subscribe` is what a client consumes, so it sends.
    let receive = document.operations["receiveLightMeasurement"]
        .item()
        .expect("inline");
    assert_eq!(receive.action, OperationAction::Receive);
    assert_eq!(receive.channel.reference, format!("#/channels/{MEASURED}"));

    let send = document.operations["sendTurnCommand"]
        .item()
        .expect("inline");
    assert_eq!(send.action, OperationAction::Send);
    assert_eq!(send.channel.reference, format!("#/channels/{COMMAND}"));

    assert!(
        notes
            .iter()
            .any(|note| note.contains("`publish` is what the application does not do")),
        "got: {notes:?}"
    );
}

#[test]
fn an_operations_messages_move_onto_its_channel() {
    let (document, _) = converted();

    let channel = document.channels[COMMAND].item().expect("inline");
    assert_eq!(
        channel.messages.keys().collect::<Vec<_>>(),
        vec!["TurnOff", "TurnOn"],
    );

    let send = document.operations["sendTurnCommand"]
        .item()
        .expect("inline");
    assert_eq!(
        send.messages
            .iter()
            .map(|message| message.reference.as_str())
            .collect::<Vec<_>>(),
        vec![
            format!("#/channels/{COMMAND}/messages/TurnOn"),
            format!("#/channels/{COMMAND}/messages/TurnOff"),
        ],
    );
}

#[test]
fn the_documents_own_tags_and_docs_move_under_info() {
    let (document, _) = converted();

    assert!(matches!(
        document.info.tags.first(),
        Some(RefOr::Item(tag)) if tag.name == "telemetry"
    ));
    assert!(matches!(
        document.info.external_docs,
        Some(RefOr::Item(ref docs)) if docs.url == "https://example.com/docs"
    ));
}

#[test]
fn a_server_url_becomes_a_host() {
    let (document, _) = converted();

    let server = document.servers["production"].item().expect("inline");
    assert_eq!(server.host, "{stage}.broker.example.com:9092");
    assert_eq!(server.pathname, None);
    // A 2.6 requirement names a scheme; a 3.0 one references it.
    assert!(matches!(
        server.security.first(),
        Some(RefOr::Reference(reference))
            if reference.reference == "#/components/securitySchemes/saslScram"
    ));
}

#[test]
fn every_note_says_where_it_came_from() {
    let (_, notes) = converted();

    assert!(!notes.is_empty(), "this document cannot convert cleanly");
    for note in &notes {
        assert!(note.starts_with("#."), "{note} should name its source");
    }
    assert!(
        notes
            .iter()
            .any(|note| note.contains("a v3 parameter is a string")),
        "the fixture's parameter carries a schema: {notes:?}"
    );
}

#[test]
fn a_note_names_its_kind() {
    let text = std::fs::read_to_string("tests/v2_6_data/streetlights.yaml").expect("the fixture");
    let source: v2_6::Document = serde_yaml_ng::from_str(&text).expect("valid YAML");
    let (_, report) = convert(source);

    assert!(!report.is_clean());
    assert!(
        report
            .notes
            .iter()
            .any(|note| matches!(note.kind, NoteKind::ActionFlipped { .. })),
    );
}
