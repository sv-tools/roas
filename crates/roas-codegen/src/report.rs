//! Walks the raw description and says what the generator will not, or
//! cannot faithfully, handle.

use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity, push_token};
use serde_json::Value;
use url::Url;

/// Record what the 2.0 → 3.0 conversion is documented to drop, where it
/// touches something the generator would use.
pub(crate) fn v2_losses(raw: &Value, uri: &Url, out: &mut Vec<Diagnostic>) {
    walk(raw, String::new(), &mut |object, pointer| {
        if let Some(Value::String(name)) = object.get("discriminator")
            && !object.contains_key("allOf")
        {
            let mut at = pointer.clone();
            push_token(&mut at, "discriminator");
            out.push(Diagnostic {
                severity: Severity::Warning,
                kind: DiagnosticKind::NormalizationLoss {
                    keyword: "discriminator".into(),
                },
                schema_id: SchemaId::new(uri.clone(), pointer.clone()),
                pointer: at,
                message: format!(
                    "`discriminator: {name}` on a plain object schema is dropped by the 2.0 → 3.0 conversion, which carries a discriminator only on `allOf` / `oneOf` / `anyOf`; the union will be generated untagged"
                ),
            });
        }
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
    });
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
    walk(raw, String::new(), &mut |object, pointer| {
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
    });
}

/// Visit every object in `value`, depth first, with its JSON Pointer.
fn walk(
    value: &Value,
    pointer: String,
    visit: &mut dyn FnMut(&serde_json::Map<String, Value>, &String),
) {
    match value {
        Value::Object(object) => {
            visit(object, &pointer);
            for (key, child) in object {
                let mut next = pointer.clone();
                push_token(&mut next, key);
                walk(child, next, visit);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let mut next = pointer.clone();
                push_token(&mut next, &i.to_string());
                walk(item, next, visit);
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
    fn plain_object_discriminator_is_a_loss_but_composed_is_not() {
        let raw = json!({
            "definitions": {
                "Pet": {"type": "object", "discriminator": "petType"},
                "Cat": {"allOf": [{"$ref": "#/definitions/Pet"}], "discriminator": "petType"}
            }
        });
        let mut out = Vec::new();
        v2_losses(&raw, &uri(), &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].pointer, "/definitions/Pet/discriminator");
        assert_eq!(out[0].schema_id.pointer, "/definitions/Pet");
        assert_eq!(out[0].severity, Severity::Warning);
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
}
