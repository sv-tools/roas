//! Represents a possible design-time link for a response

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::v3_0::server::Server;
use crate::v3_0::spec::Spec;
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
    /// The ref resolves to a defined Operation.
    Ok,
    /// The ref is structurally invalid or its target path/method is missing.
    Err(String),
    /// The PathItem reached has a `$ref` that points outside this document;
    /// we cannot inspect operations there. Caller decides whether that's an
    /// error based on `IgnoreExternalReferences`.
    ExternalPathItemRef(String),
}

/// Resolve an internal `#/paths/...` operationRef against `Spec.paths`.
///
/// Per OAS 3.0.4 a PathItem may itself be a `$ref` to another PathItem; in
/// that case `operations` on the referencing entry is empty and the methods
/// live on the target. We follow one hop of `PathItem.reference` for
/// internal refs so an operationRef like `#/paths/~1pets/get` still
/// validates when `/pets` is `{ "$ref": "#/paths/~1other" }`.
///
/// Multi-hop chains are not followed: spec paths cycles are unusual, and
/// the next hop's target would itself need to be a non-ref PathItem to
/// declare any operations.
fn resolve_internal_operation_ref(spec: &Spec, reference: &str) -> OperationRefResolution {
    let after = match reference.strip_prefix("#/paths/") {
        Some(rest) => rest,
        None => {
            return OperationRefResolution::Err(format!(
                "must start with `#/paths/`, found `{reference}`"
            ));
        }
    };
    let (path_token, method) = match after.rsplit_once('/') {
        Some((p, m)) => (p, m),
        None => {
            return OperationRefResolution::Err(format!(
                "must point to `#/paths/<encoded path>/<method>`, found `{reference}`"
            ));
        }
    };
    let path = unescape_pointer_token(path_token);
    let Some(item) = spec.paths.paths.get(&path) else {
        return OperationRefResolution::Err(format!("path `{path}` not declared in `#/paths`"));
    };

    // If the resolved PathItem is itself a `$ref`, follow it once.
    let target_item = if let Some(ref_str) = &item.reference {
        if let Some(after_paths) = ref_str.strip_prefix("#/paths/") {
            let target_path = unescape_pointer_token(after_paths);
            match spec.paths.paths.get(&target_path) {
                Some(t) => t,
                None => {
                    return OperationRefResolution::Err(format!(
                        "path `{path}` is a `$ref` to `{ref_str}`, which is not declared in `#/paths`"
                    ));
                }
            }
        } else if ref_str.is_empty() {
            return OperationRefResolution::Err(format!("path `{path}` carries an empty `$ref`"));
        } else {
            return OperationRefResolution::ExternalPathItemRef(ref_str.clone());
        }
    } else {
        item
    };

    let method_lower = method.to_lowercase();
    let exists = target_item
        .operations
        .as_ref()
        .is_some_and(|m| m.contains_key(&method_lower));
    if !exists {
        return OperationRefResolution::Err(format!(
            "method `{method}` not declared on path `{path}`"
        ));
    }
    OperationRefResolution::Ok
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

        // Validate operationRef points to an Operation.
        // Internal refs (start with `#/`) MUST resolve. External refs are
        // gated on `IgnoreExternalReferences`, mirroring `RefOr` behavior.
        // If the target PathItem itself is `$ref`-d to an external document,
        // we likewise gate on `IgnoreExternalReferences`.
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
    use crate::validation::Options;
    use serde_json::json;

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

    fn spec_with_pets_get() -> Spec {
        // Build a spec containing GET /pets so internal operationRef tests
        // have a valid target.
        use crate::v3_0::operation::Operation;
        use crate::v3_0::path_item::{PathItem, Paths};
        use crate::v3_0::reference::RefOr;
        use crate::v3_0::response::{Response, Responses};

        let op = Operation {
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
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
            paths,
            ..Default::default()
        }
    }

    #[test]
    fn server_validates() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            server: Some(crate::v3_0::server::Server {
                url: "".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("server.url")),
            "expected server.url error: {:?}",
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
            "expected unknown path: {:?}",
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
            ctx.errors
                .iter()
                .any(|e| e.contains(".operationRef") && e.contains("method `post`")),
            "expected unknown method: {:?}",
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
                .any(|e| e.contains(".operationRef") && e.contains("must start with `#/paths/`")),
            "expected bad-prefix error: {:?}",
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
            "expected empty-ref error: {:?}",
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
            "expected external-unsupported error: {:?}",
            ctx.errors
        );

        // Gating with IgnoreExternalReferences silences the error.
        let mut ctx = Context::new(&spec, Options::IgnoreExternalReferences.only());
        Link {
            operation_ref: Some("https://example.com/spec.yaml#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("external reference")),
            "with option, no external error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_follows_internal_path_item_ref() {
        // `/pets` is a PathItem `$ref` pointing at `/canonical-pets`, which
        // declares `get`. The Link's operationRef `#/paths/~1pets/get` must
        // resolve via the indirection.
        use crate::v3_0::operation::Operation;
        use crate::v3_0::path_item::{PathItem, Paths};
        use crate::v3_0::reference::RefOr;
        use crate::v3_0::response::{Response, Responses};

        let op = Operation {
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("get".to_owned(), op);
        let canonical_item = PathItem {
            operations: Some(ops),
            ..Default::default()
        };
        let alias_item = PathItem {
            reference: Some("#/paths/~1canonical-pets".into()),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths
            .paths
            .insert("/canonical-pets".to_owned(), canonical_item);
        paths.paths.insert("/pets".to_owned(), alias_item);
        let spec = Spec {
            paths,
            ..Default::default()
        };

        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains(".operationRef")),
            "ref-of-ref should resolve: {:?}",
            ctx.errors
        );

        // Method that doesn't exist on the canonical target still fails,
        // proving we resolved through the indirection (rather than just
        // skipping the check).
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/post".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("method `post`")),
            "expected unknown method on resolved target: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_target_missing() {
        // `/pets` is a `$ref` to `/missing`, which doesn't exist.
        use crate::v3_0::path_item::{PathItem, Paths};
        let alias_item = PathItem {
            reference: Some("#/paths/~1missing".into()),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert("/pets".to_owned(), alias_item);
        let spec = Spec {
            paths,
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("$ref") && e.contains("not declared")),
            "expected dangling-target error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_external_path_item_ref() {
        // `/pets` is a `$ref` to a path in another document. Without
        // IgnoreExternalReferences, we error; with the option set, we let
        // it pass.
        use crate::v3_0::path_item::{PathItem, Paths};
        let alias_item = PathItem {
            reference: Some("https://other.example/spec.yaml#/paths/~1pets".into()),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert("/pets".to_owned(), alias_item);
        let spec = Spec {
            paths,
            ..Default::default()
        };

        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("external document")),
            "expected external-PathItem-ref error: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&spec, Options::IgnoreExternalReferences.only());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains(".operationRef")),
            "with option, no .operationRef error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_tilde0_decodes_to_tilde() {
        // RFC 6901: `~01` round-trips to `~1`. Here we register a path that
        // contains a literal `~` and verify the decoder finds it.
        use crate::v3_0::operation::Operation;
        use crate::v3_0::path_item::{PathItem, Paths};
        use crate::v3_0::reference::RefOr;
        use crate::v3_0::response::{Response, Responses};
        let op = Operation {
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("get".to_owned(), op);
        let item = PathItem {
            operations: Some(ops),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert("/~weird".to_owned(), item);
        let spec = Spec {
            paths,
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1~0weird/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains(".operationRef")),
            "tilde-encoded ref should resolve: {:?}",
            ctx.errors
        );
    }
}
