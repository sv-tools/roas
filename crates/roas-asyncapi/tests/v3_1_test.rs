//! Integration tests for AsyncAPI v3.1: load fixtures from
//! `tests/v3_1_data/`, parse them (JSON and YAML), and validate.
//!
//! The 3.1 object model is identical to 3.0's, so these mirror the 3.0
//! suite; what is genuinely new — the 3.1 `schemaFormat` values and the
//! upconversion from 3.0 — gets its own tests at the bottom.

#![cfg(feature = "v3_1")]

use enumset::EnumSet;
use roas_asyncapi::v3_1::{Document, OperationAction};
use roas_asyncapi::validation::{Validate, ValidationOptions};
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("v3_1_data")
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
}

#[test]
fn streetlights_yaml_parses_validates_and_round_trips() {
    let doc = load_yaml("streetlights.yaml");
    doc.validate(EnumSet::empty()).expect("must validate");

    assert_eq!(doc.channels.len(), 3);
    assert_eq!(doc.operations.len(), 2);

    let dynamic = doc.channels["turnOnOff"].item().expect("inline channel");
    assert_eq!(dynamic.address, Some(None));

    let receive = doc.operations["receiveLightMeasurement"]
        .item()
        .expect("inline operation");
    assert_eq!(receive.action, OperationAction::Receive);

    let json = serde_json::to_string(&doc).expect("serialize");
    let reparsed: Document = serde_json::from_str(&json).expect("reparse");
    assert_eq!(reparsed, doc);
}

#[test]
fn a_3_0_document_does_not_parse_as_3_1() {
    let v3_0 = read("../v3_0_data/minimal.json");
    assert!(
        serde_json::from_str::<Document>(&v3_0).is_err(),
        "the version newtype must reject 3.0.0",
    );
}

#[test]
fn broken_wiring_reports_every_dangling_reference() {
    let err = load_json("bad_wiring.json")
        .validate(EnumSet::empty())
        .unwrap_err();
    let errors: Vec<_> = err.errors.iter().map(ToString::to_string).collect();

    assert!(
        errors
            .iter()
            .any(|e| e.contains("server `#/servers/staging` names nothing in this document")),
        "got: {errors:?}",
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("belongs to channel `other`, not `userSignedUp`")),
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
}

#[test]
fn external_references_pass_by_default_and_fail_under_strictness() {
    let doc = load_yaml("external_refs.yaml");
    doc.validate(EnumSet::empty())
        .expect("external refs are not resolved by default");
    assert!(
        doc.validate(EnumSet::only(ValidationOptions::ErrorOnExternalReference))
            .is_err()
    );
}

#[test]
fn the_3_1_schema_format_is_accepted() {
    // The one genuinely new value in 3.1's payload dialect. 3.0's own
    // formats keep working alongside it.
    for format in [
        "application/vnd.aai.asyncapi;version=3.1.0",
        "application/vnd.aai.asyncapi+json;version=3.1.0",
        "application/vnd.aai.asyncapi+yaml;version=3.1.0",
        "application/vnd.aai.asyncapi;version=3.0.0",
        "application/vnd.apache.avro;version=1.9.0",
    ] {
        let doc: Document = serde_json::from_str(&format!(
            r##"{{
                "asyncapi": "3.1.0",
                "info": {{ "title": "T", "version": "1" }},
                "channels": {{
                    "c": {{
                        "address": "c",
                        "messages": {{
                            "m": {{ "payload": {{ "schemaFormat": "{format}", "schema": {{}} }} }}
                        }}
                    }}
                }}
            }}"##
        ))
        .unwrap_or_else(|e| panic!("{format}: {e}"));
        doc.validate(EnumSet::empty())
            .unwrap_or_else(|e| panic!("{format}: {e}"));
        assert!(
            roas_asyncapi::v3_1::is_supported_schema_format(format),
            "{format} should be one of the documented formats",
        );
    }
}

#[cfg(feature = "v3_0")]
mod upconversion {
    use super::{EnumSet, Validate, read};
    use roas_asyncapi::{v3_0, v3_1};

    #[test]
    fn a_full_3_0_document_upconverts_with_only_the_version_changed() {
        let source: serde_json::Value =
            serde_json::from_str(&read("full_v3_0.json")).expect("fixture parses");

        let from: v3_0::Document = serde_json::from_value(source.clone()).expect("parses as 3.0");
        from.validate(EnumSet::empty()).expect("source is valid");

        let converted: v3_1::Document = from.into();
        converted
            .validate(EnumSet::empty())
            .expect("result is valid");
        assert_eq!(converted.asyncapi, v3_1::Version::V3_1_0());

        // Every other byte is untouched: swapping the version string in
        // the source yields exactly the converted document. This covers
        // the whole model at once — servers, channels, operations with
        // replies, messages with a fully-populated draft-07 payload,
        // every security-scheme shape, and all 19 component maps.
        let mut expected = source;
        expected["asyncapi"] = serde_json::json!("3.1.0");
        assert_eq!(serde_json::to_value(&converted).unwrap(), expected);
    }
}
