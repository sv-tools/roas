//! Regression tests for `x-` extension handling on real documents.
//!
//! The extensions visitor skips non-`x-` keys without consuming their
//! values, which would desync a raw `MapAccess`. It is only ever
//! reached through `#[serde(flatten)]`, and serde's flatten buffers
//! every leftover entry (key *and* value) before handing them over — so
//! the skip is safe. These tests pin that down against both a streaming
//! JSON deserializer and a YAML one, with unknown keys placed before,
//! inside, and after typed fields.

#![cfg(feature = "v3_0")]

use roas_asyncapi::v3_0::Document;

#[test]
fn unknown_non_x_keys_do_not_desync_a_streaming_json_deserializer() {
    let json = r#"{
        "asyncapi": "3.0.0",
        "unknownBefore": { "nested": [1, 2, { "deep": true }] },
        "info": { "title": "T", "unknownInside": "v", "version": "1" },
        "unknownAfter": [1, 2, 3],
        "x-kept": 7
    }"#;

    let doc: Document = serde_json::from_str(json).expect("parses");
    assert_eq!(doc.info.title, "T");
    assert_eq!(doc.info.version, "1");
    assert_eq!(
        doc.extensions.as_ref().and_then(|e| e.get("x-kept")),
        Some(&serde_json::json!(7)),
    );
    // Unknown non-`x-` keys are dropped rather than preserved.
    assert!(
        doc.extensions
            .as_ref()
            .is_none_or(|e| !e.contains_key("unknownBefore")),
    );
}

#[test]
fn unknown_non_x_keys_do_not_desync_a_yaml_deserializer() {
    let yaml = "\
asyncapi: 3.0.0
unknownBefore:
  nested: [1, 2]
info:
  title: T
  unknownInside: v
  version: '1'
unknownAfter: [1]
x-kept: 7
";

    let doc: Document = serde_yaml_ng::from_str(yaml).expect("parses");
    assert_eq!(doc.info.title, "T");
    assert_eq!(doc.info.version, "1");
    assert_eq!(
        doc.extensions.as_ref().and_then(|e| e.get("x-kept")),
        Some(&serde_json::json!(7)),
    );
}
