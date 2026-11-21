//! Keys an object does not define: kept on parsing, reported by validation unless
//! `Options::IgnoreUnknownFields` is set, and left out by the version conversions.

#![cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2"))]

use roas::validation::{Options, Validate};
use serde_json::json;

/// The paths validation reports as unknown fields.
fn unknown_field_paths(spec: &impl Validate, options: enumset::EnumSet<Options>) -> Vec<String> {
    let Err(error) = spec.validate(options, None) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = error
        .errors
        .iter()
        .filter(|error| error.message.starts_with("not a field of this object"))
        .map(|error| error.path.clone())
        .collect();
    paths.sort();
    paths
}

/// A v3.x document with a key OpenAPI does not define (`embed`) on several objects, an `x-`
/// extension beside it, and a schema keyword the model does not know.
#[cfg(any(feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
fn v3_document(version: &str) -> serde_json::Value {
    json!({
        "openapi": version,
        "info": {"title": "t", "version": "1", "embed": true, "x-keep": 1},
        "paths": {"/x": {"post": {
            "embed": true,
            "parameters": [{"name": "q", "in": "query", "schema": {"type": "string"}, "embed": true}],
            "requestBody": {"embed": true, "content": {"application/json": {
                "schema": {"$ref": "#/components/schemas/Cat"}, "embed": true
            }}},
            "responses": {"200": {"description": "ok", "embed": true}}
        }}},
        "components": {"schemas": {"Cat": {"type": "object", "embed": true}}},
        "embed": true
    })
}

/// Where `v3_document` puts `embed` outside its schema.
#[cfg(any(feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
const V3_UNKNOWN_FIELDS: [&str; 7] = [
    "#.embed",
    "#.info.embed",
    "#.paths[/x].post.embed",
    "#.paths[/x].post.parameters[0].embed",
    "#.paths[/x].post.requestBody.content[application/json].embed",
    "#.paths[/x].post.requestBody.embed",
    "#.paths[/x].post.responses.200.embed",
];

#[cfg(feature = "v3_0")]
#[test]
fn v3_0_reports_every_unknown_field_but_not_extensions_or_schema_keywords() {
    let spec: roas::v3_0::spec::Spec = serde_json::from_value(v3_document("3.0.4")).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        V3_UNKNOWN_FIELDS
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

#[cfg(feature = "v3_1")]
#[test]
fn v3_1_reports_every_unknown_field_but_not_extensions_or_schema_keywords() {
    let spec: roas::v3_1::spec::Spec = serde_json::from_value(v3_document("3.1.2")).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        V3_UNKNOWN_FIELDS
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

#[cfg(feature = "v3_2")]
#[test]
fn v3_2_reports_every_unknown_field_but_not_extensions_or_schema_keywords() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(v3_document("3.2.0")).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        V3_UNKNOWN_FIELDS
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

#[cfg(feature = "v3_2")]
#[test]
fn unknown_fields_round_trip() {
    let document = v3_document("3.2.0");
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(spec).unwrap(), document);
}

/// A v2.0 document with `embed` on several objects, its schema among them: a v2.0 schema is a
/// fixed subset of JSON Schema, so an unknown keyword there is reported too.
#[cfg(feature = "v2")]
#[test]
fn v2_reports_every_unknown_field_including_schemas() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1", "embed": true, "x-keep": 1},
        "paths": {"/x": {"post": {
            "embed": true,
            "parameters": [{"name": "q", "in": "query", "type": "string", "embed": true}],
            "responses": {"200": {
                "description": "ok", "schema": {"type": "object", "embed": true}, "embed": true
            }}
        }}},
        "embed": true
    });
    let spec: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.embed",
            "#.info.embed",
            "#.paths[/x].post.embed",
            "#.paths[/x].post.parameters[0].embed",
            "#.paths[/x].post.responses.200.embed",
            "#.paths[/x].post.responses.200.schema.embed",
        ]
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

/// `summary` is not a v3.0 Info field and is a string in v3.1: a number there would not fit,
/// so carrying it would fail the conversion.
#[cfg(all(feature = "v3_0", feature = "v3_1"))]
#[test]
fn v3_0_to_v3_1_leaves_out_unknown_fields_and_keeps_extensions_and_schema_keywords() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1", "summary": 5, "embed": true, "x-keep": 1},
        "paths": {},
        "components": {"schemas": {"Choice": {"type": "string", "const": "a"}}}
    });
    let v30: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    let v31 = roas::v3_1::spec::Spec::try_from(v30).unwrap();
    let converted = serde_json::to_value(v31).unwrap();
    assert_eq!(
        converted["info"],
        json!({"title": "t", "version": "1", "x-keep": 1})
    );
    assert_eq!(converted["components"]["schemas"]["Choice"]["const"], "a");
}

/// `kind` is not a v3.1 Tag field and is a string in v3.2.
#[cfg(all(feature = "v3_1", feature = "v3_2"))]
#[test]
fn v3_1_to_v3_2_leaves_out_unknown_fields_and_keeps_extensions() {
    let document = json!({
        "openapi": "3.1.2",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "tags": [{"name": "t", "kind": 5, "x-keep": 1}]
    });
    let v31: roas::v3_1::spec::Spec = serde_json::from_value(document).unwrap();
    let v32 = roas::v3_2::spec::Spec::try_from(v31).unwrap();
    let converted = serde_json::to_value(v32).unwrap();
    assert_eq!(converted["tags"], json!([{"name": "t", "x-keep": 1}]));
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
#[test]
fn v2_to_v3_0_leaves_out_unknown_fields_and_keeps_extensions() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1", "embed": true, "x-keep": 1},
        "paths": {}
    });
    let v2: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    let v30 = roas::v3_0::spec::Spec::try_from(v2).unwrap();
    let converted = serde_json::to_value(v30).unwrap();
    assert_eq!(
        converted["info"],
        json!({"title": "t", "version": "1", "x-keep": 1})
    );
}
