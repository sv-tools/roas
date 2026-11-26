//! Walks the raw description and says what the generator will not, or
//! cannot faithfully, handle.

use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity, push_token};
use serde_json::Value;
use url::Url;

/// Record what the 2.0 → 3.0 conversion is documented to drop, where it
/// touches something the generator would use.
pub(crate) fn v2_losses(raw: &Value, uri: &Url, out: &mut Vec<Diagnostic>) {
    walk(
        raw,
        String::new(),
        WalkContext::Normal,
        true,
        &mut |object, pointer| {
            if let Some(Value::String(format)) = object.get("collectionFormat")
                && format == "tsv"
            {
                let mut at = pointer.clone();
                push_token(&mut at, "collectionFormat");
                out.push(Diagnostic {
                severity: Severity::Warning,
                kind: DiagnosticKind::NormalizationLoss {
                    keyword: "collectionFormat".into(),
                },
                schema_id: SchemaId::new(uri.clone(), pointer.clone()),
                pointer: at,
                message: "`collectionFormat: tsv` has no 3.0 equivalent and is dropped by the 2.0 → 3.0 conversion".into(),
            });
            }
        },
    );
}

/// Report every reference the first release does not resolve.
///
/// Only a JSON Pointer into this document — `#/components/schemas/Pet`
/// and its kin — is followed. An external resource has its own version
/// and dialect and cannot yet be normalized on its own terms; a
/// plain-name anchor and an `$id` re-basing are not tracked by the
/// resolver. Each is an error on the affected schema, which is then
/// not generated, rather than a guess.
pub(crate) fn reference_restrictions(raw: &Value, uri: &Url, out: &mut Vec<Diagnostic>) {
    let is_v2 = raw.get("swagger").is_some();
    walk(
        raw,
        String::new(),
        WalkContext::Normal,
        is_v2,
        &mut |object, pointer| {
            if let Some(Value::String(reference)) = object.get("$ref") {
                let mut at = pointer.clone();
                push_token(&mut at, "$ref");
                let kind = if reference.starts_with("#/") || reference == "#" {
                    None
                } else if let Some(anchor) = reference.strip_prefix('#') {
                    Some((
                        DiagnosticKind::AnchorReference {
                            reference: reference.clone(),
                        },
                        format!(
                            "`$ref: {reference}` names the plain anchor `{anchor}` rather than a JSON Pointer; anchors are not resolved yet, so this schema is not generated"
                        ),
                    ))
                } else {
                    Some((
                        DiagnosticKind::ExternalReference {
                            reference: reference.clone(),
                        },
                        format!(
                            "`$ref: {reference}` points outside the document; external references are not resolved yet, so this schema is not generated"
                        ),
                    ))
                };
                if let Some((kind, message)) = kind {
                    out.push(Diagnostic {
                        severity: Severity::Error,
                        kind,
                        schema_id: SchemaId::new(uri.clone(), pointer.clone()),
                        pointer: at,
                        message,
                    });
                }
            }
            if let Some(Value::String(id)) = object.get("$id")
                && !pointer.is_empty()
            {
                let mut at = pointer.clone();
                push_token(&mut at, "$id");
                out.push(Diagnostic {
                severity: Severity::Error,
                kind: DiagnosticKind::IdRebasing { id: id.clone() },
                schema_id: SchemaId::new(uri.clone(), pointer.clone()),
                pointer: at,
                message: format!(
                    "`$id: {id}` re-bases the references beneath it; `$id` scopes are not tracked yet, so this schema is not generated"
                ),
            });
            }
        },
    );
}

/// Structural context of the JSON object currently visited by [`walk`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum WalkContext {
    /// Ordinary OpenAPI / JSON Schema object whose keys are specification keywords.
    Normal,
    /// User-keyed map of OpenAPI / JSON Schema objects (`properties`,
    /// `patternProperties`, `$defs`, `definitions`, `dependentSchemas`,
    /// `dependencies`, `responses`, `headers`, `webhooks`, `callbacks`,
    /// `content`, and `/components/*` registries): every key is a user name
    /// (such as `default`, `example`, or `x-request-id`), so all children are
    /// descended into as `Normal`.
    KeyedMap,
    /// Map of named OpenAPI 3.x Example Objects (`examples` when an object map).
    ExamplesMap,
    /// Inside one OpenAPI 3.x Example Object (its `value` / `dataValue` /
    /// `serializedValue` hold arbitrary payload JSON, not schemas).
    ExampleObject,
}

/// Visit every specification object in `value`, depth first, with its JSON
/// Pointer, skipping arbitrary data containers (`example`, `default`, `const`,
/// `enum`, `x-*`, schema `examples` arrays, Swagger 2.0 response `examples`
/// maps, and OpenAPI 3.x Example Object payload values).
fn walk(
    value: &Value,
    pointer: String,
    ctx: WalkContext,
    is_v2: bool,
    visit: &mut dyn FnMut(&serde_json::Map<String, Value>, &String),
) {
    match value {
        Value::Object(object) => {
            if !matches!(ctx, WalkContext::KeyedMap | WalkContext::ExamplesMap) {
                visit(object, &pointer);
            }
            for (key, child) in object {
                let next_ctx = match ctx {
                    WalkContext::KeyedMap => WalkContext::Normal,
                    WalkContext::ExamplesMap => WalkContext::ExampleObject,
                    WalkContext::ExampleObject => {
                        if matches!(key.as_str(), "value" | "dataValue" | "serializedValue")
                            || key.starts_with("x-")
                        {
                            continue;
                        }
                        WalkContext::Normal
                    }
                    WalkContext::Normal => {
                        if key.starts_with("x-")
                            || matches!(key.as_str(), "example" | "default" | "const" | "enum")
                        {
                            continue;
                        }
                        if key == "examples" {
                            if is_v2 || !child.is_object() {
                                continue;
                            }
                            WalkContext::ExamplesMap
                        } else if matches!(
                            key.as_str(),
                            "properties"
                                | "patternProperties"
                                | "$defs"
                                | "definitions"
                                | "dependentSchemas"
                                | "dependencies"
                                | "responses"
                                | "headers"
                                | "webhooks"
                                | "callbacks"
                                | "content"
                        ) || (pointer == "/components"
                            && matches!(
                                key.as_str(),
                                "schemas"
                                    | "parameters"
                                    | "responses"
                                    | "headers"
                                    | "requestBodies"
                                    | "links"
                                    | "callbacks"
                                    | "pathItems"
                                    | "mediaTypes"
                                    | "securitySchemes"
                            ))
                        {
                            WalkContext::KeyedMap
                        } else {
                            WalkContext::Normal
                        }
                    }
                };
                let mut next = pointer.clone();
                push_token(&mut next, key);
                walk(child, next, next_ctx, is_v2, visit);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let mut next = pointer.clone();
                push_token(&mut next, &i.to_string());
                walk(item, next, WalkContext::Normal, is_v2, visit);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn uri() -> Url {
        Url::parse("file:///d.json").unwrap()
    }

    #[test]
    fn a_discriminator_is_not_a_loss() {
        // 3.x keeps it on a plain object schema as on a composition.
        let raw = json!({
            "definitions": {
                "Pet": {"type": "object", "discriminator": "petType"},
                "Cat": {"allOf": [{"$ref": "#/definitions/Pet"}], "discriminator": "petType"}
            }
        });
        let mut out = Vec::new();
        v2_losses(&raw, &uri(), &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn tsv_collection_format_is_a_loss() {
        let raw =
            json!({"parameters": [{"name": "ids", "in": "query", "collectionFormat": "tsv"}]});
        let mut out = Vec::new();
        v2_losses(&raw, &uri(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pointer, "/parameters/0/collectionFormat");
        let csv = json!({"parameters": [{"collectionFormat": "csv"}]});
        out.clear();
        v2_losses(&csv, &uri(), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn references_are_classified() {
        let raw = json!({
            "components": {"schemas": {
                "A": {"$ref": "#/components/schemas/B"},
                "B": {"$ref": "other.json#/components/schemas/C"},
                "C": {"$ref": "#Pet"},
                "D": {"$id": "https://example.com/d", "type": "object"},
                "E": {"properties": {"x": {"$ref": "https://example.com/x.json"}}}
            }}
        });
        let mut out = Vec::new();
        reference_restrictions(&raw, &uri(), &mut out);
        let kinds: Vec<(&str, &DiagnosticKind)> =
            out.iter().map(|d| (d.pointer.as_str(), &d.kind)).collect();
        assert_eq!(out.len(), 4, "{kinds:?}");
        assert!(matches!(
            &out[0].kind,
            DiagnosticKind::ExternalReference { reference } if reference == "other.json#/components/schemas/C"
        ));
        assert_eq!(out[0].pointer, "/components/schemas/B/$ref");
        assert!(matches!(
            &out[1].kind,
            DiagnosticKind::AnchorReference { .. }
        ));
        assert!(
            matches!(&out[2].kind, DiagnosticKind::IdRebasing { id } if id == "https://example.com/d")
        );
        assert_eq!(out[3].pointer, "/components/schemas/E/properties/x/$ref");
        assert!(out.iter().all(|d| d.severity == Severity::Error));
    }

    #[test]
    fn a_root_id_is_not_a_rebasing() {
        let raw = json!({"$id": "https://example.com/root", "openapi": "3.1.0"});
        let mut out = Vec::new();
        reference_restrictions(&raw, &uri(), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn examples_defaults_extensions_do_not_trigger_false_positives_while_named_properties_do() {
        let raw = json!({
            "openapi": "3.1.0",
            "paths": {
                "/pets": {
                    "get": {
                        "responses": {
                            "default": {
                                "$ref": "https://example.com/default-resp.json"
                            },
                            "200": {
                                "headers": {
                                    "x-request-id": {
                                        "$ref": "https://example.com/hdr.json"
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "examples": {
                    "Sample": {
                        "summary": "example summary",
                        "value": {"$ref": "https://example.com/data.json", "$id": "not-a-schema"},
                        "x-meta": {"$ref": "https://example.com/ext.json"}
                    }
                },
                "schemas": {
                    "IgnoredData": {
                        "type": "object",
                        "example": {"$ref": "https://example.com/example.json", "$id": "bad"},
                        "examples": [{"$ref": "https://example.com/arr.json"}],
                        "default": {"$ref": "https://example.com/default.json"},
                        "const": {"$ref": "https://example.com/const.json"},
                        "enum": [{"$ref": "https://example.com/enum.json"}],
                        "x-vendor": {"$ref": "https://example.com/vendor.json", "$id": "v"}
                    },
                    "RealProps": {
                        "type": "object",
                        "dependentSchemas": {
                            "default": {"$ref": "https://example.com/dep.json"}
                        },
                        "properties": {
                            "example": {"$ref": "https://example.com/real.json"},
                            "default": {"$id": "https://example.com/prop-id"}
                        }
                    }
                }
            }
        });
        let mut out = Vec::new();
        reference_restrictions(&raw, &uri(), &mut out);
        let pointers: Vec<&str> = out.iter().map(|d| d.pointer.as_str()).collect();
        assert_eq!(
            pointers,
            [
                "/components/schemas/RealProps/dependentSchemas/default/$ref",
                "/components/schemas/RealProps/properties/default/$id",
                "/components/schemas/RealProps/properties/example/$ref",
                "/paths/~1pets/get/responses/200/headers/x-request-id/$ref",
                "/paths/~1pets/get/responses/default/$ref",
            ]
        );

        // Swagger 2.0 response `examples` maps MIME types to raw payloads and is skipped.
        let v2_raw = json!({
            "swagger": "2.0",
            "paths": {
                "/pets": {
                    "get": {
                        "responses": {
                            "200": {
                                "examples": {
                                    "application/json": {
                                        "$ref": "https://example.com/not-a-ref.json",
                                        "nested": {"$id": "not-an-id", "collectionFormat": "tsv"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let mut v2_out = Vec::new();
        reference_restrictions(&v2_raw, &uri(), &mut v2_out);
        v2_losses(&v2_raw, &uri(), &mut v2_out);
        assert!(v2_out.is_empty(), "{v2_out:?}");
    }
}
