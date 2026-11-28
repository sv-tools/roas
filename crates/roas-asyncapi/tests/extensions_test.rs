//! Regression tests for `x-` extension and unclaimed-key handling on real documents.
//!
//! Both `x-`-prefixed keys and unknown non-`x-` keys are collected into
//! `extensions` so documents round-trip without loss and validation can
//! report unknown keys. These tests pin that behavior down against both
//! a streaming JSON deserializer and a YAML one, with unknown keys
//! placed before, inside, and after typed fields.

#![cfg(feature = "v3_1")]

#[path = "../src/test_yaml.rs"]
mod yaml;

use roas_asyncapi::v3_1::Document;

#[test]
fn unknown_non_x_keys_do_not_desync_a_streaming_json_deserializer() {
    let json = r#"{
        "asyncapi": "3.1.0",
        "unknownBefore": { "nested": [1, 2, { "deep": true }] },
        "info": { "title": "T", "unknownInside": "v", "version": "1" },
        "unknownAfter": [1, 2, 3],
        "x-kept": 7
    }"#;

    let doc: Document = serde_json::from_str(json).expect("parses");
    assert_eq!(doc.info.title, "T");
    assert_eq!(doc.info.version, "1");
    let ext = doc.extensions.as_ref().expect("extensions");
    assert_eq!(ext.get("x-kept"), Some(&serde_json::json!(7)));
    assert_eq!(
        ext.get("unknownBefore"),
        Some(&serde_json::json!({ "nested": [1, 2, { "deep": true }] })),
    );
    assert_eq!(ext.get("unknownAfter"), Some(&serde_json::json!([1, 2, 3])));
    assert_eq!(
        doc.info
            .extensions
            .as_ref()
            .and_then(|e| e.get("unknownInside")),
        Some(&serde_json::json!("v")),
    );
}

#[test]
fn unknown_non_x_keys_do_not_desync_a_yaml_deserializer() {
    let yaml = "\
asyncapi: 3.1.0
unknownBefore:
  nested: [1, 2]
info:
  title: T
  unknownInside: v
  version: '1'
unknownAfter: [1]
x-kept: 7
";

    let doc: Document = yaml::from_str(yaml).expect("parses");
    assert_eq!(doc.info.title, "T");
    assert_eq!(doc.info.version, "1");
    let ext = doc.extensions.as_ref().expect("extensions");
    assert_eq!(ext.get("x-kept"), Some(&serde_json::json!(7)));
    assert_eq!(
        ext.get("unknownBefore"),
        Some(&serde_json::json!({ "nested": [1, 2] })),
    );
    assert_eq!(ext.get("unknownAfter"), Some(&serde_json::json!([1])));
    assert_eq!(
        doc.info
            .extensions
            .as_ref()
            .and_then(|e| e.get("unknownInside")),
        Some(&serde_json::json!("v")),
    );
}
