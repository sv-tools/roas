//! Represents a possible design-time link for a response

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::v3_1::server::Server;
use crate::v3_1::spec::Spec;
use crate::validation::Options;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Decode one JSON Pointer reference token (`~1` → `/`, `~0` → `~`).
fn unescape_pointer_token(token: &str) -> String {
    // Per RFC 6901, `~1` decodes to `/` and `~0` decodes to `~`. Order
    // matters: a literal `~01` must round-trip to `~1`, so substitute
    // `~1` first then `~0`.
    token.replace("~1", "/").replace("~0", "~")
}

/// Outcome of attempting to resolve an internal `#/paths/...` operationRef.
enum OperationRefResolution {
    Ok,
    Err(String),
    /// The PathItem reached has a `$ref` that points outside this document;
    /// caller decides whether that's an error based on
    /// `IgnoreExternalReferences`.
    ExternalPathItemRef(String),
}

/// Resolve an internal operationRef against `Spec.paths` or
/// `Spec.components.pathItems`. Per OAS 3.1, an operationRef is a URI
/// Reference that "MAY point to any Operation Object in the OpenAPI
/// definition" — including Operations inside reusable Path Items in
/// `#/components/pathItems/<name>/<method>`.
///
/// Follows internal PathItem `$ref` chains (with cycle detection) so a
/// referencing PathItem with `{"$ref": "#/paths/~1canonical-pets"}` still
/// resolves correctly.
fn resolve_internal_operation_ref(spec: &Spec, reference: &str) -> OperationRefResolution {
    // Determine which container the ref points at.
    enum Container {
        Paths,
        ComponentPathItems,
    }
    let (container, after) = if let Some(rest) = reference.strip_prefix("#/paths/") {
        (Container::Paths, rest)
    } else if let Some(rest) = reference.strip_prefix("#/components/pathItems/") {
        (Container::ComponentPathItems, rest)
    } else {
        return OperationRefResolution::Err(format!(
            "must start with `#/paths/` or `#/components/pathItems/`, found `{reference}`"
        ));
    };

    // Per RFC 6901 the path is a single JSON Pointer reference token: `/`
    // inside the path MUST be escaped as `~1`. So between the prefix and
    // the method there must be exactly one `/` separator. Refs like
    // `#/paths//pets/get` (unescaped slash) are malformed and rejected.
    let slash_count = after.bytes().filter(|b| *b == b'/').count();
    let (path_token, method) = match (slash_count, after.split_once('/')) {
        (1, Some((p, m))) => (p, m),
        (0, _) => {
            return OperationRefResolution::Err(format!(
                "must point to `<container>/<encoded path>/<method>`, found `{reference}`"
            ));
        }
        _ => {
            return OperationRefResolution::Err(format!(
                "malformed JSON Pointer: the encoded path token must use `~1` for `/`, found `{reference}`"
            ));
        }
    };
    let path = unescape_pointer_token(path_token);

    let lookup = |key: &str| -> Option<&crate::v3_1::path_item::PathItem> {
        match container {
            Container::Paths => spec.paths.as_ref().and_then(|p| p.paths.get(key)),
            Container::ComponentPathItems => spec
                .components
                .as_ref()
                .and_then(|c| c.path_items.as_ref())
                .and_then(|m| m.get(key)),
        }
    };

    let Some(item) = lookup(&path) else {
        return OperationRefResolution::Err(format!(
            "path `{path}` not declared in the resolved container"
        ));
    };

    let mut seen = std::collections::BTreeSet::from([path.clone()]);
    let (target_path, target_item) = match resolve_path_item_ref_chain(spec, &path, item, &mut seen)
    {
        Ok(t) => t,
        Err(err) => return err,
    };

    let method_lower = method.to_lowercase();
    let exists = target_item
        .operations
        .as_ref()
        .is_some_and(|m| m.contains_key(&method_lower));
    if !exists {
        return OperationRefResolution::Err(format!(
            "method `{method}` not declared on path `{target_path}`"
        ));
    }
    OperationRefResolution::Ok
}

fn resolve_path_item_ref_chain<'a>(
    spec: &'a Spec,
    path: &str,
    item: &'a crate::v3_1::path_item::PathItem,
    seen: &mut std::collections::BTreeSet<String>,
) -> Result<(String, &'a crate::v3_1::path_item::PathItem), OperationRefResolution> {
    let Some(ref_str) = &item.reference else {
        return Ok((path.to_owned(), item));
    };

    if ref_str.is_empty() {
        return Err(OperationRefResolution::Err(format!(
            "path `{path}` carries an empty `$ref`"
        )));
    }

    // PathItem refs may target either container (paths or components.pathItems).
    let (target_path, target_item) = if let Some(after_paths) = ref_str.strip_prefix("#/paths/") {
        if after_paths.contains('/') {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to malformed JSON Pointer `{ref_str}`: the encoded path token must use `~1` for `/`"
            )));
        }
        let tp = unescape_pointer_token(after_paths);
        if !seen.insert(tp.clone()) {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` has a cyclic `$ref` chain through `{ref_str}`"
            )));
        }
        let Some(paths) = spec.paths.as_ref() else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` has a `$ref` to `{ref_str}` but spec has no `paths`"
            )));
        };
        let Some(t) = paths.paths.get(&tp) else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to `{ref_str}`, which is not declared in `#/paths`"
            )));
        };
        (tp, t)
    } else if let Some(after) = ref_str.strip_prefix("#/components/pathItems/") {
        if after.contains('/') {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to malformed JSON Pointer `{ref_str}`"
            )));
        }
        let tp = unescape_pointer_token(after);
        if !seen.insert(tp.clone()) {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` has a cyclic `$ref` chain through `{ref_str}`"
            )));
        }
        let Some(t) = spec
            .components
            .as_ref()
            .and_then(|c| c.path_items.as_ref())
            .and_then(|m| m.get(&tp))
        else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to `{ref_str}`, which is not declared in `#/components/pathItems`"
            )));
        };
        (tp, t)
    } else {
        return Err(OperationRefResolution::ExternalPathItemRef(ref_str.clone()));
    };

    resolve_path_item_ref_chain(spec, &target_path, target_item, seen)
}

/// The Link object represents a possible design-time link for a response.
/// The presence of a link does not guarantee the caller’s ability to successfully invoke it,
/// rather it provides a known relationship and traversal mechanism between responses and other operations.
///
/// Unlike dynamic links (i.e. links provided in the response payload),
/// the OAS linking mechanism does not require link information in the runtime response.
//
/// For computing links, and providing instructions to execute them,
/// a runtime expression is used for accessing values in an operation and using them as parameters
/// while invoking the linked operation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Link {
    /// A relative or absolute URI reference to an OAS operation.
    /// This field is mutually exclusive of the operationId field, and MUST point to an Operation Object.
    /// Relative operationRef values MAY be used to locate an existing Operation Object in the OpenAPI definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationRef")]
    pub operation_ref: Option<String>,

    /// The name of an existing, resolvable OAS operation,
    /// as defined with a unique operationId.
    /// This field is mutually exclusive of the operationRef field.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,

    /// A map representing parameters to pass to an operation as specified with operationId
    /// or identified via operationRef.
    /// The key is the parameter name to be used, whereas the value can be a constant
    /// or an expression to be evaluated and passed to the linked operation.
    /// The parameter name can be qualified using the parameter location [{in}.]{name} for operations
    /// that use the same parameter name in different locations (e.g. path.id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, serde_json::Value>>,

    /// A literal value or {expression} to use as a request body when calling the target operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "requestBody")]
    pub request_body: Option<serde_json::Value>,

    /// A description of the link.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A server object to be used by the target operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<Server>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Link {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        // Spec: a Link MUST identify the linked operation via operationRef
        // or operationId, and the two are mutually exclusive.
        match (&self.operation_ref, &self.operation_id) {
            (Some(_), Some(_)) => ctx.error(
                path.clone(),
                "operationRef and operationId are mutually exclusive",
            ),
            (None, None) => ctx.error(
                path.clone(),
                "must specify exactly one of operationRef or operationId",
            ),
            _ => {}
        }

        if let Some(operation_id) = &self.operation_id
            && !ctx
                .visited
                .contains(format!("#/paths/operations/{operation_id}").as_str())
        {
            ctx.error(
                path.clone(),
                format_args!(".operationId: missing operation with id `{operation_id}`"),
            );
        }

        if let Some(operation_ref) = &self.operation_ref {
            if operation_ref.is_empty() {
                ctx.error(path.clone(), ".operationRef: must not be empty");
            } else if operation_ref.starts_with("#/") {
                match resolve_internal_operation_ref(ctx.spec, operation_ref) {
                    OperationRefResolution::Ok => {}
                    OperationRefResolution::Err(msg) => {
                        ctx.error(path.clone(), format_args!(".operationRef: {msg}"));
                    }
                    OperationRefResolution::ExternalPathItemRef(target)
                        if !ctx.is_option(Options::IgnoreExternalReferences) =>
                    {
                        ctx.error(
                            path.clone(),
                            format_args!(
                                ".operationRef: target PathItem is a `$ref` to external document `{target}`, which is not supported"
                            ),
                        );
                    }
                    OperationRefResolution::ExternalPathItemRef(_) => {}
                }
            } else if !ctx.is_option(Options::IgnoreExternalReferences) {
                ctx.error(
                    path.clone(),
                    format_args!(
                        ".operationRef: external reference `{operation_ref}` is not supported"
                    ),
                );
            }
        }

        if let Some(server) = &self.server {
            server.validate_with_context(ctx, format!("{path}.server"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::common::reference::RefOr;
    use crate::v3_1::operation::Operation;
    use crate::v3_1::path_item::{PathItem, Paths};
    use crate::v3_1::response::{Response, Responses};
    use serde_json::json;

    fn spec_with_pets_get() -> Spec {
        let op = Operation {
            responses: Some(Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("get".to_owned(), op);
        let item = PathItem {
            operations: Some(ops),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert("/pets".to_owned(), item);
        Spec {
            paths: Some(paths),
            ..Default::default()
        }
    }

    #[test]
    fn round_trip_full() {
        let v = json!({
            "operationId": "getPet",
            "parameters": {"id": "$response.body#/id"},
            "requestBody": {"name": "fluffy"},
            "description": "Linked",
            "server": {"url": "https://example.com"},
            "x-internal": true
        });
        let l: Link = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&l).unwrap(), v);
    }

    #[test]
    fn xor_both_present_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("ref".into()),
            operation_id: Some("id".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("mutually exclusive")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn xor_neither_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link::default().validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must specify exactly one")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn missing_operation_id_reported() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_id: Some("nonexistent".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("missing operation with id `nonexistent`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_internal_resolves() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains(".operationRef")),
            "valid ref should not error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_unknown_path_errors() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1users/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".operationRef") && e.contains("`/users` not declared")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_unknown_method_errors() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/post".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("method `post`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_bad_prefix_errors() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/schemas/Foo".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must start with `#/paths/`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_unescaped_slash_malformed() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths//pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("malformed JSON Pointer")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_external_unsupported() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("https://example.com/spec.yaml#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("external reference") && e.contains("not supported")),
            "errors: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&spec, Options::IgnoreExternalReferences.only());
        Link {
            operation_ref: Some("https://example.com/spec.yaml#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("external reference")),
            "with option: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_empty_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".operationRef") && e.contains("must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
    }
}
