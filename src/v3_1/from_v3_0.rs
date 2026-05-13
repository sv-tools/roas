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
//!      `{type: string, format: binary}` becomes the empty schema
//!      `{}` — the form the migration guide recommends, routed
//!      through the [`crate::v3_1::schema::EmptySchema`] variant so
//!      it round-trips byte-for-byte.
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

/// Position of the current node relative to schema boundaries.
/// Threading this through the recursive walker keeps the schema-only
/// rewrites from touching instance-valued payloads while still
/// reaching every real sub-schema.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pos {
    /// We haven't entered a schema yet. The walker still needs to
    /// recurse — schemas appear under known structural keys
    /// (`schema`, `schemas`, `allOf` / `anyOf` / `oneOf`, `not`, …).
    Generic,
    /// The current value is itself a Schema Object.
    Schema,
    /// The current value is a `BTreeMap<String, Schema>` (e.g.
    /// `properties`, `components.schemas`). Each entry's value is a
    /// schema.
    SchemaMap,
    /// The current value is a `Link` Object. Its `parameters` and
    /// `requestBody` fields hold arbitrary JSON (free-form
    /// runtime-expression maps and bodies) and must not be walked.
    Link,
    /// The current value is a `BTreeMap<String, Link>` (e.g.
    /// `components.links`, `Response.links`). Each entry's value is
    /// a Link.
    LinkMap,
}

/// Apply schema-shape rewrites — `nullable: true` → `type` array, the
/// boolean `exclusive*` modifier → numeric `exclusive*`, single
/// `example` → `examples: [example]`, and `format: base64` →
/// `contentEncoding: base64` — at every Schema Object reached via the
/// document's structural shape. Sub-schemas inside `properties`,
/// `items`, `allOf`, etc. are walked; instance-valued payloads
/// (`example`, `examples`, `default`, `enum`, `const`, `ExampleObject.value`)
/// are skipped so user-supplied JSON that happens to contain
/// schema-shaped keys (e.g. an example with `type` and `nullable`)
/// round-trips byte-for-byte.
fn transform_schemas_recursive(value: &mut Value) {
    walk(value, Pos::Generic);
}

fn walk(value: &mut Value, pos: Pos) {
    match value {
        Value::Object(obj) => match pos {
            Pos::Schema => walk_schema_object(obj),
            Pos::SchemaMap => {
                for (_, v) in obj.iter_mut() {
                    walk(v, Pos::Schema);
                }
            }
            Pos::Link => walk_link_object(obj),
            Pos::LinkMap => {
                for (_, v) in obj.iter_mut() {
                    walk(v, Pos::Link);
                }
            }
            Pos::Generic => walk_generic_object(obj),
        },
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                walk(v, pos);
            }
        }
        _ => {}
    }
}

/// Walk a Link Object's keys. Link's `parameters`
/// (`Map<String, runtime-expression>`) and `requestBody` (free-form
/// JSON / runtime expression) are opaque user payloads and must not
/// be touched — they may legally contain `schema`, `nullable`,
/// `content`, etc. without being OpenAPI schema or content shapes.
fn walk_link_object(obj: &mut Map<String, Value>) {
    for (k, v) in obj.iter_mut() {
        if is_extension_key(k) {
            continue;
        }
        match k.as_str() {
            "parameters" | "requestBody" => continue,
            // `server` and any future Link fields are walked
            // generically.
            _ => walk(v, Pos::Generic),
        }
    }
}

fn walk_schema_object(obj: &mut Map<String, Value>) {
    normalize_nullable(obj);
    normalize_exclusive_bound(obj, "exclusiveMinimum", "minimum");
    normalize_exclusive_bound(obj, "exclusiveMaximum", "maximum");
    normalize_example_to_examples(obj);
    normalize_base64_format(obj);
    for (k, v) in obj.iter_mut() {
        // Inside a Schema: instance-valued keywords carry user data;
        // sub-schema keywords lead to more schemas; everything else is
        // schema-adjacent metadata (xml, discriminator, externalDocs)
        // that we walk generically. `x-*` Specification Extensions
        // hold arbitrary JSON and must round-trip byte-for-byte.
        if is_extension_key(k) {
            continue;
        }
        match k.as_str() {
            "example" | "examples" | "default" | "enum" | "const" => continue,
            "items"
            | "not"
            | "additionalProperties"
            | "contains"
            | "propertyNames"
            | "if"
            | "then"
            | "else"
            | "unevaluatedItems"
            | "unevaluatedProperties" => walk(v, Pos::Schema),
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => walk(v, Pos::Schema),
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                walk(v, Pos::SchemaMap)
            }
            _ => walk(v, Pos::Generic),
        }
    }
}

fn walk_generic_object(obj: &mut Map<String, Value>) {
    for (k, v) in obj.iter_mut() {
        // `x-*` Specification Extensions are opaque user payloads
        // that must round-trip unchanged.
        if is_extension_key(k) {
            continue;
        }
        match k.as_str() {
            // `schema` lives on Parameter / Header / MediaType; its
            // value is a Schema Object.
            "schema" => walk(v, Pos::Schema),
            // `schemas` is the components-level map of named schemas.
            "schemas" => walk(v, Pos::SchemaMap),
            // `links` is a map of named Link Objects. Both
            // `components.links` and `Response.links` use this key.
            "links" => walk(v, Pos::LinkMap),
            // ExampleObject's instance value, and the
            // example / examples carriers on MediaType / Parameter /
            // Header. Either an instance value, or a map of
            // ExampleObjects (which themselves carry instance values
            // — never schemas). Skip recursion entirely.
            "example" | "examples" | "value" => continue,
            _ => walk(v, Pos::Generic),
        }
    }
}

/// Per OAS / JSON Schema, fields with the `x-` prefix are
/// Specification Extensions: arbitrary user JSON the spec promises to
/// preserve untouched. The walkers skip recursion through these so
/// extension payloads round-trip byte-for-byte.
fn is_extension_key(k: &str) -> bool {
    k.starts_with("x-")
}

/// `type: <T>` + `nullable: true` → `type: [<T>, "null"]`, and a bare
/// `nullable: true` paired with `type: [<T>, …]` adds `"null"` to
/// the array. `nullable: false` (or absent) is a no-op except that
/// the redundant `nullable` field is dropped — v3.1 has no such
/// keyword.
///
/// `nullable: true` on a schema with **no** `type` is a no-op
/// modulo the dropped keyword: in OAS 3.0 a schema without `type`
/// is already unconstrained (it allows any JSON value, including
/// null), so we don't synthesise a `type: ["null"]` — that would
/// flip the semantics from "matches anything" to "null only".
///
/// The input arrives via `serde_json::to_value(&v3_0::Spec)`, so
/// `nullable` is always either absent or a JSON boolean — anything
/// else would have failed v3.0 deserialization upstream.
fn normalize_nullable(obj: &mut Map<String, Value>) {
    let nullable = matches!(obj.remove("nullable"), Some(Value::Bool(true)));
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
            // No `type` to add `null` to. Drop `nullable` (already
            // removed above) and leave the schema typeless: a v3.0
            // schema with no `type` already allows any value.
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

/// Single `example` → `examples: [example]`. Schemas that already
/// have an `examples` array win; the deprecated `example` is dropped
/// to match v3.1's "examples is the source of truth" stance. The
/// caller is responsible for invoking this only on Schema Objects
/// (the position-aware walker handles that via [`Pos::Schema`]);
/// `Parameter`, `Header`, and `MediaType` keep their `example` field
/// in v3.1.
fn normalize_example_to_examples(obj: &mut Map<String, Value>) {
    let Some(example) = obj.remove("example") else {
        return;
    };
    if obj.contains_key("examples") {
        return;
    }
    obj.insert("examples".into(), Value::Array(vec![example]));
}

/// `type: string, format: base64` → `type: string,
/// contentEncoding: base64`. v3.1 follows JSON Schema 2020-12, which
/// dropped the OAS-only `format: base64` in favour of the standard
/// `contentEncoding` keyword.
///
/// Accepts both the bare-string `type: "string"` form and the array
/// form `type: ["string", "null"]` produced upstream by
/// [`normalize_nullable`] for nullable string schemas.
fn normalize_base64_format(obj: &mut Map<String, Value>) {
    if !type_includes_string(obj) {
        return;
    }
    if obj.get("format").and_then(|v| v.as_str()) != Some("base64") {
        return;
    }
    obj.remove("format");
    obj.insert("contentEncoding".into(), Value::String("base64".into()));
}

fn type_includes_string(obj: &Map<String, Value>) -> bool {
    match obj.get("type") {
        Some(Value::String(s)) => s == "string",
        Some(Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some("string")),
        _ => false,
    }
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
///   `{type: string, format: binary}` with the empty schema `{}`,
///   the form the migration guide recommends. `v3_1::Schema` carries
///   a first-class [`crate::v3_1::schema::EmptySchema`] variant that
///   round-trips cleanly: `{}` deserialises as `Schema::Empty`
///   (added in PR #117) instead of being normalised to
///   `{type: object}` by `ObjectSchema::default()`.
fn walk_content_aware(value: &mut Value) {
    walk_content_aware_with(value, /* in_link = */ false);
}

fn walk_content_aware_with(value: &mut Value, in_link: bool) {
    match value {
        Value::Object(obj) => {
            // The `content` key on a Link is a property name, not the
            // OAS Content Map; in fact Link doesn't define `content`
            // at all, but a free-form `Link.requestBody` could
            // contain one. Skip the rewrite entirely while inside a
            // Link.
            if !in_link && let Some(Value::Object(content)) = obj.get_mut("content") {
                rewrite_content_map(content);
            }
            for (k, v) in obj.iter_mut() {
                // `x-*` extensions are opaque — the spec promises they
                // round-trip byte-for-byte. Skip before any other
                // dispatch.
                if is_extension_key(k) {
                    continue;
                }
                if in_link {
                    // Link's `parameters` (map of arbitrary JSON) and
                    // `requestBody` (free-form) are opaque payloads.
                    if matches!(k.as_str(), "parameters" | "requestBody") {
                        continue;
                    }
                    // Other Link fields (server, description) are
                    // walked normally — no Link entries below them.
                    walk_content_aware_with(v, false);
                    continue;
                }
                // Skip instance-valued payloads — a user-supplied
                // example / default / enum / const that happens to
                // contain a `content`-shaped sub-object would
                // otherwise get its file-upload schemas rewritten.
                if matches!(
                    k.as_str(),
                    "example" | "examples" | "default" | "enum" | "const" | "value"
                ) {
                    continue;
                }
                // `links` is a map of named Link objects; transition
                // into Link context for the entries.
                if k == "links" {
                    if let Value::Object(map) = v {
                        for (_, entry) in map.iter_mut() {
                            walk_content_aware_with(entry, true);
                        }
                    }
                    continue;
                }
                walk_content_aware_with(v, false);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                walk_content_aware_with(v, in_link);
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
        // OAS media-type keys may carry parameters (`application/
        // octet-stream; charset=binary`); compare against just the
        // `type/subtype` head. Per RFC 7231 the type / subtype tokens
        // are case-insensitive (`Application/Octet-Stream` is the
        // same media type as `application/octet-stream`), so use
        // ASCII-case-insensitive comparisons throughout.
        let mime_main = mime_main_type(mime);
        if mime_main.eq_ignore_ascii_case("application/octet-stream") {
            // `{type: string, format: binary}` → `{}` (the empty
            // schema, JSON Schema 2020-12's "matches anything"
            // idiom). Routes through `Schema::Empty(EmptySchema)`
            // on the typed deserialisation.
            //
            // Only fires when the schema has nothing besides `type`
            // and `format` — preserving any additional annotations
            // (`description`, `title`, `nullable`, …) is safer than
            // silently dropping them. Schemas with extras stay in
            // their v3.0 form, which is still valid v3.1
            // (`format: binary` is just a JSON Schema annotation).
            if let Value::Object(s) = schema
                && is_string_binary(s)
                && s.len() == 2
            {
                *schema = Value::Object(Map::new());
            }
        } else if is_multipart_mime(mime_main)
            && let Value::Object(s) = schema
        {
            rewrite_string_binary_subschemas(s);
        }
    }
}

/// Return just the `type/subtype` portion of a media-type header
/// value, stripping any RFC-7231 parameters after the first `;`.
fn mime_main_type(mime: &str) -> &str {
    mime.split(';').next().unwrap_or(mime).trim()
}

fn is_multipart_mime(mime: &str) -> bool {
    // Type/subtype tokens are ASCII-case-insensitive per RFC 7231.
    // `mime` ultimately comes from arbitrary user JSON keys and may
    // contain non-ASCII / multi-byte UTF-8, so use the non-panicking
    // `str::get` form rather than slicing at an arbitrary byte
    // offset.
    let prefix = "multipart/";
    mime.get(..prefix.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(prefix))
}

fn is_string_binary(schema: &Map<String, Value>) -> bool {
    schema.get("type").and_then(|v| v.as_str()) == Some("string")
        && schema.get("format").and_then(|v| v.as_str()) == Some("binary")
}

/// Recursively walk a multipart schema tree and rewrite every
/// `{type: string, format: binary}` subschema to
/// `{type: string, contentMediaType: application/octet-stream}`.
///
/// v2/v3.0's `format: binary` annotation can sit anywhere inside a
/// multipart schema — directly on a property, on `items` of an array
/// property, on a nested `properties.<name>` schema, on `allOf`
/// branches, etc. The walker visits all of them; instance-valued
/// payloads (`example`, `default`, `enum`, …) are skipped so a
/// user-supplied example whose shape happens to mirror a string-binary
/// schema isn't mutated.
fn rewrite_string_binary_subschemas(schema: &mut Map<String, Value>) {
    if is_string_binary(schema) {
        schema.remove("format");
        schema.insert(
            "contentMediaType".into(),
            Value::String("application/octet-stream".into()),
        );
        // A string-binary schema is a leaf — no schema substructure
        // to recurse into.
        return;
    }
    for (k, v) in schema.iter_mut() {
        match k.as_str() {
            // Schema-level instance keys carry user data, never schemas.
            "example" | "examples" | "default" | "enum" | "const" => continue,
            // Sub-schema keys (single nested schema).
            "items"
            | "not"
            | "additionalProperties"
            | "contains"
            | "propertyNames"
            | "if"
            | "then"
            | "else"
            | "unevaluatedItems"
            | "unevaluatedProperties" => {
                if let Value::Object(s) = v {
                    rewrite_string_binary_subschemas(s);
                }
            }
            // Sub-schema arrays.
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Value::Array(arr) = v {
                    for entry in arr.iter_mut() {
                        if let Value::Object(s) = entry {
                            rewrite_string_binary_subschemas(s);
                        }
                    }
                }
            }
            // Schema-by-name maps.
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                if let Value::Object(map) = v {
                    for (_, entry) in map.iter_mut() {
                        if let Value::Object(s) = entry {
                            rewrite_string_binary_subschemas(s);
                        }
                    }
                }
            }
            _ => {}
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
    fn nullable_without_type_stays_typeless() {
        // OAS 3.0 `nullable: true` only adds `null` to an explicit
        // `type`. A schema with no `type` is already unconstrained
        // (allows any value including null), so the conversion must
        // drop `nullable` without synthesising a `type: ["null"]` —
        // that would change semantics from "any" to "null only".
        //
        // The typed `From<v3_0::Spec>` path can't actually deliver a
        // typeless schema to `normalize_nullable` (v3_0's
        // `ObjectSchema` re-serialises an explicit `type: object`),
        // so exercise the walker directly on hand-built JSON to pin
        // the defensive behaviour down.
        let mut v: Value = serde_json::json!({"nullable": true, "description": "free-form"});
        super::walk(&mut v, super::Pos::Schema);
        let free = &v;
        assert!(
            free.get("type").is_none(),
            "no `type` should be synthesised, got {free}"
        );
        assert!(free.get("nullable").is_none(), "nullable removed");
        assert_eq!(free["description"], "free-form");
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
    fn schema_example_payload_is_preserved_byte_for_byte() {
        // The example value is instance data — it can legitimately
        // contain keys like `nullable`, `type`, `exclusiveMinimum`
        // without being a schema. None of the schema rewrites should
        // touch it.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Cfg": {
                        "type": "object",
                        "example": {
                            "nullable": true,
                            "type": "string",
                            "exclusiveMinimum": true,
                            "minimum": 0,
                            "format": "base64"
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        // The example value migrates to an `examples` array (per the
        // schema-level `example → examples` rewrite) but its contents
        // round-trip verbatim.
        let payload = &value["components"]["schemas"]["Cfg"]["examples"][0];
        assert_eq!(payload["nullable"], true);
        assert_eq!(payload["type"], "string");
        assert_eq!(payload["exclusiveMinimum"], true);
        assert_eq!(payload["minimum"], 0);
        assert_eq!(payload["format"], "base64");
    }

    #[test]
    fn schema_default_payload_is_preserved_byte_for_byte() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Cfg": {
                        "type": "object",
                        "default": {"type": "string", "nullable": true}
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let default = &value["components"]["schemas"]["Cfg"]["default"];
        assert_eq!(default["type"], "string");
        assert_eq!(default["nullable"], true);
    }

    #[test]
    fn property_named_example_is_walked_as_a_subschema() {
        // The schema rewrites must reach a sub-schema whose property
        // name happens to be `example` (or any other instance-valued
        // keyword). The instance-key skip is gated on the parent
        // being a schema, so a `properties` map does NOT skip its
        // child entries.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Cfg": {
                        "type": "object",
                        "properties": {
                            "example": {"type": "string", "nullable": true},
                            "default": {"type": "integer", "nullable": true}
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let props = &value["components"]["schemas"]["Cfg"]["properties"];
        assert_eq!(
            props["example"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            props["default"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }

    #[test]
    fn media_type_examples_payload_is_preserved_byte_for_byte() {
        // `MediaType.examples` is a map of named ExampleObjects. The
        // ExampleObject's `value` field is instance data; recursing
        // through the `examples` key would mutate it.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"type": "object"},
                                        "examples": {
                                            "trap": {
                                                "value": {
                                                    "type": "string",
                                                    "nullable": true,
                                                    "format": "base64"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let trap = &value["paths"]["/x"]["get"]["responses"]["200"]["content"]["application/json"]
            ["examples"]["trap"]["value"];
        assert_eq!(trap["type"], "string");
        assert_eq!(trap["nullable"], true);
        assert_eq!(trap["format"], "base64");
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
    fn octet_stream_binary_schema_becomes_empty_schema() {
        // `{type: string, format: binary}` under
        // `application/octet-stream` is the v3.0 idiom for "raw bytes
        // body". v3.1's equivalent is the empty schema `{}` (the
        // form the official upgrade guide recommends), routed
        // through the `Schema::Empty(EmptySchema)` variant so it
        // round-trips byte-for-byte.
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
        // The result is the literal empty object `{}`, neither
        // `{type: object}` (the old ObjectSchema-default artefact)
        // nor `true` (the boolean-schema fallback).
        assert!(schema.is_object());
        assert!(
            schema.as_object().unwrap().is_empty(),
            "octet-stream schema should be `{{}}`, got {schema}"
        );
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
    fn multipart_binary_array_items_uses_content_media_type() {
        // `format: binary` can sit on `items` of an array property
        // (multipart upload of multiple files). v3.1 expects the
        // `contentMediaType` rewrite there too — not just on
        // top-level properties.
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
                                            "files": {
                                                "type": "array",
                                                "items": {"type": "string", "format": "binary"}
                                            }
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
        let items = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["multipart/form-data"]
            ["schema"]["properties"]["files"]["items"];
        assert_eq!(items["type"], "string");
        assert_eq!(items["contentMediaType"], "application/octet-stream");
        assert!(items.get("format").is_none(), "format dropped on items too");
    }

    #[test]
    fn multipart_nested_binary_property_uses_content_media_type() {
        // A binary deep inside a nested object schema is rewritten too.
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
                                            "envelope": {
                                                "type": "object",
                                                "properties": {
                                                    "blob": {"type": "string", "format": "binary"}
                                                }
                                            }
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
        let blob = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["multipart/form-data"]
            ["schema"]["properties"]["envelope"]["properties"]["blob"];
        assert_eq!(blob["contentMediaType"], "application/octet-stream");
        assert!(blob.get("format").is_none());
    }

    #[test]
    fn content_aware_walk_skips_example_payload() {
        // A schema whose `example` payload contains a `content` map
        // and binary schemas must NOT be rewritten — that's
        // user-supplied instance data, not an OAS Content map.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Doc": {
                        "type": "object",
                        "example": {
                            "content": {
                                "application/octet-stream": {
                                    "schema": {"type": "string", "format": "binary"}
                                },
                                "multipart/form-data": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "f": {"type": "string", "format": "binary"}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let payload = &value["components"]["schemas"]["Doc"]["examples"][0];
        // The example value's nested binary schema is preserved
        // verbatim — no octet-stream-empty rewrite, no
        // contentMediaType rewrite.
        let octet_schema = &payload["content"]["application/octet-stream"]["schema"];
        assert_eq!(octet_schema["type"], "string");
        assert_eq!(octet_schema["format"], "binary");
        let multipart_field =
            &payload["content"]["multipart/form-data"]["schema"]["properties"]["f"];
        assert_eq!(multipart_field["type"], "string");
        assert_eq!(multipart_field["format"], "binary");
        assert!(
            multipart_field.get("contentMediaType").is_none(),
            "example payload must not gain contentMediaType"
        );
    }

    #[test]
    fn non_ascii_media_type_key_does_not_panic() {
        // `is_multipart_mime` previously sliced the key at a
        // hard-coded byte offset (`mime[..prefix.len()]`). Map keys
        // can hold arbitrary UTF-8, so a non-ASCII / multi-byte head
        // would panic. Use `str::get` instead — the conversion must
        // tolerate (and pass through) arbitrary keys.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "🦀/binary": {
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
        // Key is preserved verbatim and not rewritten — neither
        // multipart nor octet-stream applies.
        let schema = &value["paths"]["/x"]["post"]["requestBody"]["content"]["🦀/binary"]["schema"];
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["format"], "binary");
    }

    #[test]
    fn media_type_match_is_case_insensitive() {
        // RFC 7231 type/subtype tokens are ASCII-case-insensitive,
        // so `Application/Octet-Stream` and `Multipart/Form-Data`
        // must trigger the same rewrites as their lowercase forms.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "Application/Octet-Stream": {
                                    "schema": {"type": "string", "format": "binary"}
                                },
                                "Multipart/Form-Data": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "f": {"type": "string", "format": "binary"}
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
        // Octet-stream with mixed-case key still becomes `{}`.
        let octet = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["Application/Octet-Stream"]
            ["schema"];
        assert!(octet.is_object());
        assert!(octet.as_object().unwrap().is_empty());
        // Multipart with mixed-case key still rewrites the binary field.
        let f = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["Multipart/Form-Data"]
            ["schema"]["properties"]["f"];
        assert_eq!(f["type"], "string");
        assert_eq!(f["contentMediaType"], "application/octet-stream");
        assert!(f.get("format").is_none());
    }

    #[test]
    fn octet_stream_with_media_type_parameters_is_rewritten() {
        // RFC 7231 lets a media-type carry parameters
        // (`application/octet-stream; charset=binary`). The rewrite
        // must compare against the `type/subtype` head, not the full
        // string, so parameterised keys still get the `{}` migration.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/octet-stream; charset=binary": {
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
        let schema = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["application/octet-stream; charset=binary"]
            ["schema"];
        assert!(schema.is_object());
        assert!(schema.as_object().unwrap().is_empty());
    }

    #[test]
    fn multipart_with_media_type_parameters_rewrites_binary_props() {
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "multipart/form-data; boundary=ABCD": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "file": {"type": "string", "format": "binary"}
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
        let file = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["multipart/form-data; boundary=ABCD"]
            ["schema"]["properties"]["file"];
        assert_eq!(file["type"], "string");
        assert_eq!(file["contentMediaType"], "application/octet-stream");
        assert!(file.get("format").is_none());
    }

    #[test]
    fn octet_stream_binary_with_extra_fields_is_preserved() {
        // A schema like `{type: string, format: binary, description}`
        // expresses more than a bare byte-stream — preserve it as-is
        // instead of dropping the description by replacing with `{}`.
        // The v3.0 form is still valid v3.1 (`format: binary` is a
        // JSON Schema annotation, not a constraint).
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/octet-stream": {
                                    "schema": {
                                        "type": "string",
                                        "format": "binary",
                                        "description": "the document bytes"
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
        let schema = &value["paths"]["/upload"]["post"]["requestBody"]["content"]["application/octet-stream"]
            ["schema"];
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["format"], "binary");
        assert_eq!(schema["description"], "the document bytes");
    }

    #[test]
    fn link_parameters_and_request_body_are_opaque_to_walkers() {
        // `Link.parameters` is a `BTreeMap<String, Value>` and
        // `Link.requestBody` is a `Value` — both hold arbitrary JSON
        // (runtime expressions, free-form payloads). Neither walker
        // should rewrite their contents even if those contents
        // contain schema-shaped or content-shaped JSON.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "operationId": "getX",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "links": {
                                    "trap": {
                                        "operationId": "getX",
                                        "parameters": {
                                            "p": {
                                                "schema": {
                                                    "type": "string",
                                                    "nullable": true,
                                                    "format": "base64"
                                                }
                                            }
                                        },
                                        "requestBody": {
                                            "content": {
                                                "application/octet-stream": {
                                                    "schema": {"type": "string", "format": "binary"}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let link = &value["paths"]["/x"]["get"]["responses"]["200"]["links"]["trap"];
        // Link.parameters payload is preserved verbatim (no
        // nullable→type-array, no format→contentEncoding).
        let p_schema = &link["parameters"]["p"]["schema"];
        assert_eq!(p_schema["type"], "string");
        assert_eq!(p_schema["nullable"], true);
        assert_eq!(p_schema["format"], "base64");
        assert!(p_schema.get("contentEncoding").is_none());
        // Link.requestBody payload is preserved verbatim (no
        // octet-stream binary→{}, no contentMediaType rewrite).
        let body_schema = &link["requestBody"]["content"]["application/octet-stream"]["schema"];
        assert_eq!(body_schema["type"], "string");
        assert_eq!(body_schema["format"], "binary");
    }

    #[test]
    fn x_extension_payloads_are_opaque_to_walkers() {
        // `x-*` Specification Extensions must round-trip byte-for-byte
        // even when they carry schema- or content-shaped sub-objects.
        // Both the schema rewrites (nullable, exclusive*, base64,
        // example→examples) and the content-map rewrite (octet-stream,
        // multipart binary) must skip recursion through extensions.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Cfg": {
                        "type": "object",
                        "x-json-schema": {
                            "type": "string",
                            "nullable": true,
                            "format": "base64",
                            "example": "abc"
                        },
                        "x-vendor-content": {
                            "content": {
                                "application/octet-stream": {
                                    "schema": {"type": "string", "format": "binary"}
                                },
                                "multipart/form-data": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "f": {"type": "string", "format": "binary"}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let cfg = &value["components"]["schemas"]["Cfg"];
        // x-json-schema's payload survives intact.
        let xjs = &cfg["x-json-schema"];
        assert_eq!(xjs["type"], "string");
        assert_eq!(xjs["nullable"], true);
        assert_eq!(xjs["format"], "base64");
        assert_eq!(xjs["example"], "abc");
        assert!(xjs.get("contentEncoding").is_none());
        assert!(xjs.get("examples").is_none());
        // x-vendor-content's nested binary schemas survive intact too.
        let xvc = &cfg["x-vendor-content"];
        let octet = &xvc["content"]["application/octet-stream"]["schema"];
        assert_eq!(octet["type"], "string");
        assert_eq!(octet["format"], "binary");
        let multipart_field = &xvc["content"]["multipart/form-data"]["schema"]["properties"]["f"];
        assert_eq!(multipart_field["type"], "string");
        assert_eq!(multipart_field["format"], "binary");
        assert!(multipart_field.get("contentMediaType").is_none());
    }

    #[test]
    fn nullable_string_with_base64_format_gets_content_encoding() {
        // `nullable: true` lifts `type: "string"` to `["string", "null"]`
        // before the base64 rewrite runs. The base64 rewrite must
        // accept that array form so the `format: base64` →
        // `contentEncoding: base64` migration applies consistently.
        let raw = r##"{
            "openapi": "3.0.4",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "components": {
                "schemas": {
                    "Token": {
                        "type": "string",
                        "format": "base64",
                        "nullable": true
                    }
                }
            }
        }"##;
        let value = convert(raw);
        let token = &value["components"]["schemas"]["Token"];
        assert_eq!(token["type"], serde_json::json!(["string", "null"]));
        assert_eq!(token["contentEncoding"], "base64");
        assert!(token.get("format").is_none());
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
            if let Err(e) = v31.validate(opts, None) {
                panic!("{name}: converted spec did not validate cleanly:\n{e}");
            }
        }
    }
}
