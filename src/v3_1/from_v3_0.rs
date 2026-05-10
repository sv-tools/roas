//! Forward conversion from OpenAPI v3.0 to OpenAPI v3.1.
//!
//! Converts a [`crate::v3_0::spec::Spec`] into a
//! [`crate::v3_1::spec::Spec`] by reshaping the document on-the-fly via
//! `serde_json::Value`. v3.0 and v3.1 share most of their JSON shape;
//! the rewrites here cover the breaking schema-shape changes that JSON
//! Schema 2020-12 introduced and the file-upload encoding migration.
//!
//! Following the official "Upgrading from 3.0 to 3.1" guide
//! (<https://learn.openapis.org/upgrading/v3.0-to-v3.1.html>):
//!
//! 1. `openapi: "3.0.x"` → `openapi: "3.1.2"`.
//! 2. Schema `nullable: true` is dropped and the parent `type: <T>` is
//!    promoted to `type: [<T>, "null"]` so the schema deserializes as a
//!    `MultiSchema`.
//! 3. Schema `exclusiveMinimum: true` (the draft-04 boolean modifier) +
//!    `minimum: <n>` collapses into `exclusiveMinimum: <n>`. Same for
//!    `exclusiveMaximum`.
//! 4. Schema `example: <v>` becomes `examples: [<v>]` (the JSON Schema
//!    keyword name and shape).
//! 5. File-upload media types in `content` maps are migrated:
//!    * Schema `format: binary` properties inside `multipart/*` content
//!      become `contentMediaType: application/octet-stream` (the
//!      `format` keyword is dropped).
//!    * Schema `type: string, format: base64` anywhere becomes
//!      `type: string, contentEncoding: base64`.
//!    * `application/octet-stream` body schema
//!      `{type: string, format: binary}` becomes the boolean schema
//!      `true` — JSON Schema 2020-12's "matches anything" idiom. The
//!      migration guide writes this as the empty schema `{}`; we emit
//!      `true` because `v3_1::Schema::Bool(bool)` round-trips cleanly,
//!      whereas an empty `{}` would normalise back to `{type: object}`
//!      via `ObjectSchema::default()`.
//!
//! Lossless edges:
//!
//! * v3.0's `webhooks`-shaped extension data (if any) sits in
//!   `extensions` already; we don't synthesise top-level `webhooks`.
//! * `jsonSchemaDialect` stays absent — v3.1's default (`base`) is fine.
//! * `paths` becomes optional in v3.1, but we always emit it because
//!   v3.0 always had it.
//!
//! The conversion serialises the v3.0 input with serde, runs the
//! transforms, and deserialises as a v3.1 spec. If the input is a
//! valid v3.0 document the output is a structurally valid v3.1
//! document; semantic regressions are surfaced by `Spec::validate`.

use crate::v3_0::spec::Spec as V30Spec;
use crate::v3_1::spec::Spec as V31Spec;
use serde_json::{Map, Value};

impl From<V30Spec> for V31Spec {
    fn from(v30: V30Spec) -> Self {
        let mut value = serde_json::to_value(v30).expect("v3_0::Spec serializes");
        transform_spec(&mut value);
        serde_json::from_value(value).expect("transformed spec deserializes as v3_1::Spec")
    }
}

fn transform_spec(spec: &mut Value) {
    let Value::Object(obj) = spec else {
        return;
    };
    obj.insert("openapi".into(), Value::String("3.1.2".to_owned()));

    // Walk the document with two passes: a schema-shape rewrite applied
    // at every Schema-Object-shaped node, and a content-map walker that
    // rewrites file-upload schemas in light of their owning media type.
    walk_content_aware(spec);
    transform_schemas_recursive(spec);
}

/// Apply schema-shape rewrites at every node that looks like a Schema.
/// `nullable: true` collapses into a `type` array, the boolean
/// `exclusive*` keyword paired with `minimum`/`maximum` collapses into
/// a numeric `exclusive*`, single `example` becomes
/// `examples: [example]`, and `format: base64` becomes
/// `contentEncoding: base64`.
fn transform_schemas_recursive(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            normalize_nullable(obj);
            normalize_exclusive_bound(obj, "exclusiveMinimum", "minimum");
            normalize_exclusive_bound(obj, "exclusiveMaximum", "maximum");
            normalize_example_to_examples(obj);
            normalize_base64_format(obj);
            for (_, v) in obj.iter_mut() {
                transform_schemas_recursive(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                transform_schemas_recursive(v);
            }
        }
        _ => {}
    }
}

/// `type: <T>` + `nullable: true` → `type: [<T>, "null"]`, and a bare
/// `nullable: true` with no `type` becomes `type: ["null"]`. A
/// `nullable: false` (or absent) is left unchanged but the redundant
/// `nullable` field is dropped — v3.1 has no such keyword.
fn normalize_nullable(obj: &mut Map<String, Value>) {
    let nullable = match obj.remove("nullable") {
        Some(Value::Bool(b)) => b,
        Some(other) => {
            // Non-bool value at `nullable` — restore and bail; the v3.1
            // schema deserializer will surface it as an unknown field.
            obj.insert("nullable".into(), other);
            return;
        }
        None => return,
    };
    if !nullable {
        return;
    }
    match obj.remove("type") {
        Some(Value::String(t)) if t != "null" => {
            obj.insert(
                "type".into(),
                Value::Array(vec![Value::String(t), Value::String("null".into())]),
            );
        }
        Some(Value::Array(mut arr)) => {
            if !arr.iter().any(|v| v.as_str() == Some("null")) {
                arr.push(Value::String("null".into()));
            }
            obj.insert("type".into(), Value::Array(arr));
        }
        Some(other) => {
            // Restore unrecognised type values verbatim.
            obj.insert("type".into(), other);
        }
        None => {
            obj.insert(
                "type".into(),
                Value::Array(vec![Value::String("null".into())]),
            );
        }
    }
}

/// Collapse the v3.0 `exclusive<bound>: true` + `<bound>: <n>` pair
/// into v3.1's numeric `exclusive<bound>: <n>`, dropping the
/// now-redundant inclusive bound. `exclusive<bound>: false` is just
/// removed — v3.1 has no boolean form. If the value is already a
/// number (already-3.1-shaped or weird input), leave it alone.
fn normalize_exclusive_bound(
    obj: &mut Map<String, Value>,
    exclusive_key: &str,
    inclusive_key: &str,
) {
    match obj.get(exclusive_key) {
        Some(Value::Bool(true)) => {
            obj.remove(exclusive_key);
            if let Some(bound) = obj.remove(inclusive_key) {
                obj.insert(exclusive_key.to_owned(), bound);
            }
        }
        Some(Value::Bool(false)) => {
            obj.remove(exclusive_key);
        }
        _ => {}
    }
}

/// Single `example` → `examples: [example]`. Schemas that already have
/// an `examples` array win; the deprecated `example` is dropped to
/// match v3.1's "examples is the source of truth" stance. Only fires
/// when the surrounding object looks like a Schema (declares `type` /
/// composition keywords / `properties` / `items`); `Parameter`,
/// `Header`, and `MediaType` keep their `example` field in v3.1.
fn normalize_example_to_examples(obj: &mut Map<String, Value>) {
    if !looks_like_schema(obj) {
        return;
    }
    let Some(example) = obj.remove("example") else {
        return;
    };
    if obj.contains_key("examples") {
        return;
    }
    obj.insert("examples".into(), Value::Array(vec![example]));
}

/// Heuristic for "this object is a Schema Object". A schema declares
/// at least one of: `type`, `allOf`/`anyOf`/`oneOf`/`not`, `$ref`
/// (handled elsewhere), `properties`, `items`, `enum`, or
/// `additionalProperties`. The check exists to keep the schema-only
/// rewrites from firing on `Parameter`/`Header`/`MediaType` where
/// `example` keeps its v3.0 single-value form.
fn looks_like_schema(obj: &Map<String, Value>) -> bool {
    const SCHEMA_KEYWORDS: &[&str] = &[
        "type",
        "allOf",
        "anyOf",
        "oneOf",
        "not",
        "properties",
        "items",
        "enum",
        "additionalProperties",
        "$ref",
    ];
    SCHEMA_KEYWORDS.iter().any(|k| obj.contains_key(*k))
}

/// `type: string, format: base64` → `type: string,
/// contentEncoding: base64`. v3.1 follows JSON Schema 2020-12, which
/// dropped the OAS-only `format: base64` in favour of the standard
/// `contentEncoding` keyword.
fn normalize_base64_format(obj: &mut Map<String, Value>) {
    if obj.get("type").and_then(|v| v.as_str()) != Some("string") {
        return;
    }
    if obj.get("format").and_then(|v| v.as_str()) != Some("base64") {
        return;
    }
    obj.remove("format");
    obj.insert("contentEncoding".into(), Value::String("base64".into()));
}

/// Walk `content: { <mime>: { schema, … } }` maps and apply the
/// content-aware file-upload rewrites.
///
/// * Inside any `multipart/*` media type, rewrite each property whose
///   schema is `{type: string, format: binary}` into
///   `{type: string, contentMediaType: application/octet-stream}` —
///   `format: binary` was deprecated in 3.1 in favour of the standard
///   `contentMediaType` keyword.
/// * For `application/octet-stream`, replace
///   `{type: string, format: binary}` with the boolean schema `true`,
///   the JSON Schema 2020-12 "matches anything" idiom. The migration
///   guide presents the equivalent empty schema `{}`; we use `true`
///   because `v3_1::Schema` carries a first-class `Bool(bool)` variant
///   that round-trips cleanly, whereas `{}` would normalise back to
///   `{type: object}` via `ObjectSchema::default()`.
fn walk_content_aware(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            if let Some(Value::Object(content)) = obj.get_mut("content") {
                rewrite_content_map(content);
            }
            for (_, v) in obj.iter_mut() {
                walk_content_aware(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                walk_content_aware(v);
            }
        }
        _ => {}
    }
}

fn rewrite_content_map(content: &mut Map<String, Value>) {
    for (mime, media_type) in content.iter_mut() {
        let Value::Object(media) = media_type else {
            continue;
        };
        let Some(schema) = media.get_mut("schema") else {
            continue;
        };
        if mime == "application/octet-stream" {
            // `{type: string, format: binary}` → `true` (the boolean
            // "any value" schema). Anything else stays as-is — the
            // user may have declared a typed schema like base64 text.
            if let Value::Object(s) = schema
                && is_string_binary(s)
            {
                *schema = Value::Bool(true);
            }
        } else if is_multipart_mime(mime)
            && let Value::Object(s) = schema
        {
            rewrite_multipart_properties(s);
        }
    }
}

fn is_multipart_mime(mime: &str) -> bool {
    mime.starts_with("multipart/")
}

fn is_string_binary(schema: &Map<String, Value>) -> bool {
    schema.get("type").and_then(|v| v.as_str()) == Some("string")
        && schema.get("format").and_then(|v| v.as_str()) == Some("binary")
}

fn rewrite_multipart_properties(schema: &mut Map<String, Value>) {
    let Some(Value::Object(properties)) = schema.get_mut("properties") else {
        return;
    };
    for (_, prop) in properties.iter_mut() {
        let Value::Object(p) = prop else { continue };
        if is_string_binary(p) {
            p.remove("format");
            p.insert(
                "contentMediaType".into(),
                Value::String("application/octet-stream".into()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_0::spec::Spec as V30Spec;
    use crate::v3_1::spec::Spec as V31Spec;
    use crate::validation::{IGNORE_UNUSED, Options, Validate};

    fn v30_from_json(s: &str) -> V30Spec {
        serde_json::from_str(s).expect("v3.0 spec parses")
    }

    fn convert(raw: &str) -> Value {
        let v30: V30Spec = v30_from_json(raw);
        let v31: V31Spec = v30.into();
        serde_json::to_value(&v31).unwrap()
    }

    #[test]
    fn openapi_version_lifted() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {}
        }"##;
        let value = convert(raw);
        assert_eq!(value["openapi"], "3.1.2");
    }

    #[test]
    fn nullable_promotes_type_into_array() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "MaybeName": {"type": "string", "nullable": true},
                    "MaybeBool": {"type": "boolean"}
                }
            }
        }"##;
        let value = convert(raw);
        let maybe_name = &value["components"]["schemas"]["MaybeName"];
        assert_eq!(maybe_name["type"], serde_json::json!(["string", "null"]));
        assert!(
            maybe_name.get("nullable").is_none(),
            "nullable keyword removed"
        );
        // Non-nullable schema is untouched.
        let maybe_bool = &value["components"]["schemas"]["MaybeBool"];
        assert_eq!(maybe_bool["type"], "boolean");
    }

    #[test]
    fn nullable_string_with_constraints_round_trips_via_extensions() {
        // A nullable string with constraints (`minLength`, `pattern`,
        // `enum`) becomes a `MultiSchema` whose first-class fields
        // are very limited; the type-specific keywords are preserved
        // through the schema extensions catch-all so they round-trip
        // unchanged at the JSON level. Pin this down so the limitation
        // is visible — adding first-class fields to MultiSchema is a
        // separate piece of work.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Slug": {
                        "type": "string",
                        "nullable": true,
                        "minLength": 3,
                        "maxLength": 32,
                        "pattern": "^[a-z][a-z0-9-]*$",
                        "enum": ["alpha", "beta"]
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let slug = &value["components"]["schemas"]["Slug"];
        assert_eq!(slug["type"], serde_json::json!(["string", "null"]));
        assert_eq!(slug["minLength"], 3);
        assert_eq!(slug["maxLength"], 32);
        assert_eq!(slug["pattern"], "^[a-z][a-z0-9-]*$");
        assert_eq!(slug["enum"], serde_json::json!(["alpha", "beta"]));
    }

    #[test]
    fn nullable_object_with_properties_round_trips_via_extensions() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "nullable": true,
                        "required": ["id"],
                        "properties": {
                            "id": {"type": "integer"},
                            "name": {"type": "string", "nullable": true}
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let pet = &value["components"]["schemas"]["Pet"];
        assert_eq!(pet["type"], serde_json::json!(["object", "null"]));
        assert_eq!(pet["required"], serde_json::json!(["id"]));
        assert_eq!(pet["properties"]["id"]["type"], "integer");
        // Recursive `nullable` rewrite reaches nested properties too.
        assert_eq!(
            pet["properties"]["name"]["type"],
            serde_json::json!(["string", "null"])
        );
    }

    #[test]
    fn nullable_array_with_items_round_trips_via_extensions() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Tags": {
                        "type": "array",
                        "nullable": true,
                        "minItems": 1,
                        "uniqueItems": true,
                        "items": {"type": "string"}
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let tags = &value["components"]["schemas"]["Tags"];
        assert_eq!(tags["type"], serde_json::json!(["array", "null"]));
        assert_eq!(tags["minItems"], 1);
        assert_eq!(tags["uniqueItems"], true);
        assert_eq!(tags["items"]["type"], "string");
    }

    #[test]
    fn exclusive_bound_collapses_to_number() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Pos": {
                        "type": "integer",
                        "minimum": 0,
                        "exclusiveMinimum": true,
                        "maximum": 100,
                        "exclusiveMaximum": true
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let pos = &value["components"]["schemas"]["Pos"];
        assert_eq!(pos["exclusiveMinimum"], 0);
        assert_eq!(pos["exclusiveMaximum"], 100);
        assert!(pos.get("minimum").is_none(), "redundant minimum dropped");
        assert!(pos.get("maximum").is_none(), "redundant maximum dropped");
    }

    #[test]
    fn exclusive_bound_false_is_just_removed() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "InclOnly": {
                        "type": "integer",
                        "minimum": 0,
                        "exclusiveMinimum": false
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let s = &value["components"]["schemas"]["InclOnly"];
        assert_eq!(s["minimum"], 0);
        assert!(s.get("exclusiveMinimum").is_none());
    }

    #[test]
    fn schema_example_becomes_examples_array() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Pet": {"type": "string", "example": "fedora"}
                }
            }
        }"##;
        let value = convert(raw);
        let pet = &value["components"]["schemas"]["Pet"];
        assert_eq!(pet["examples"], serde_json::json!(["fedora"]));
        assert!(pet.get("example").is_none());
    }

    #[test]
    fn parameter_example_is_kept_as_is() {
        // `Parameter` keeps its `example` field in 3.1; only Schema's
        // `example` migrates to `examples`.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "limit",
                            "schema": {"type": "integer"},
                            "example": 10
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let p = &value["paths"]["/items"]["get"]["parameters"][0];
        assert_eq!(p["example"], 10);
    }

    #[test]
    fn octet_stream_binary_schema_becomes_boolean_true() {
        // `{type: string, format: binary}` under
        // `application/octet-stream` is the v3.0 idiom for "raw bytes
        // body". v3.1's equivalent is the JSON Schema 2020-12 "matches
        // anything" boolean schema — emitted as `true` so it survives
        // the v3.1 typed round-trip via `Schema::Bool(true)` (an empty
        // `{}` would normalise back to `{type: object}` instead).
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/octet-stream": {
                                    "schema": {"type": "string", "format": "binary"}
                                }
                            }
                        },
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let schema = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["application/octet-stream"]
            ["schema"];
        assert_eq!(*schema, Value::Bool(true));
    }

    #[test]
    fn octet_stream_with_non_binary_schema_is_kept() {
        // A typed schema (e.g. base64 text) under
        // `application/octet-stream` is not the binary idiom — keep it
        // as-is (only `format: base64` flips to `contentEncoding`).
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/octet-stream": {
                                    "schema": {"type": "string", "format": "base64"}
                                }
                            }
                        },
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let schema = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["application/octet-stream"]
            ["schema"];
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["contentEncoding"], "base64");
    }

    #[test]
    fn multipart_binary_property_uses_content_media_type() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "multipart/form-data": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "file": {"type": "string", "format": "binary"},
                                            "name": {"type": "string"}
                                        }
                                    }
                                }
                            }
                        },
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let props = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["multipart/form-data"]
            ["schema"]["properties"];
        let file = &props["file"];
        assert_eq!(file["type"], "string");
        assert_eq!(file["contentMediaType"], "application/octet-stream");
        assert!(file.get("format").is_none(), "format dropped");
        // Non-binary properties stay as-is.
        assert_eq!(props["name"]["type"], "string");
    }

    #[test]
    fn base64_format_becomes_content_encoding() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Token": {"type": "string", "format": "base64"}
                }
            }
        }"##;
        let value = convert(raw);
        let token = &value["components"]["schemas"]["Token"];
        assert_eq!(token["type"], "string");
        assert_eq!(token["contentEncoding"], "base64");
        assert!(token.get("format").is_none());
    }

    /// Sweep every checked-in v3.0 fixture; each should convert and
    /// validate clean as v3.1 with the lenient validator options used
    /// by the v2→v3.0 fixture sweep.
    #[test]
    fn all_v3_0_fixtures_convert_to_valid_v3_1() {
        let fixtures: &[(&str, &str)] = &[
            (
                "petstore",
                include_str!("../../tests/v3_0_data/petstore.json"),
            ),
            (
                "petstore-expanded",
                include_str!("../../tests/v3_0_data/petstore-expanded.json"),
            ),
            (
                "api-with-examples",
                include_str!("../../tests/v3_0_data/api-with-examples.json"),
            ),
            (
                "callback-example",
                include_str!("../../tests/v3_0_data/callback-example.json"),
            ),
            (
                "link-example",
                include_str!("../../tests/v3_0_data/link-example.json"),
            ),
        ];
        let opts = Options::new() | Options::IgnoreMissingTags | IGNORE_UNUSED;
        for (name, raw) in fixtures {
            let v30: V30Spec =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{name}: parse: {e}"));
            let v31: V31Spec = v30.into();
            assert_eq!(v31.openapi.as_str(), "3.1.2", "{name} openapi version");
            if let Err(e) = v31.validate(opts) {
                panic!("{name}: converted spec did not validate cleanly:\n{e}");
            }
        }
    }
}
