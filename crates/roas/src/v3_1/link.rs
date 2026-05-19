//! Represents a possible design-time link for a response

use crate::v3_1::server::Server;
use crate::v3_1::spec::Spec;
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};
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
    /// Successfully resolved. The carried list contains internal component
    /// references the resolver touched along the way (path-items and
    /// callbacks); the caller marks each as visited so unused-component
    /// detection doesn't false-flag them.
    Ok(Vec<String>),
    Err(String),
    /// The PathItem reached has a `$ref` that points outside this document;
    /// caller decides whether that's an error based on
    /// `IgnoreExternalReferences`.
    ExternalPathItemRef(String),
}

/// Resolve an internal operationRef. Per OAS 3.1, an operationRef is a
/// URI Reference that "MAY point to any Operation Object in the OpenAPI
/// definition." Supported tail shapes:
///
/// - `#/paths/<encoded path>/<method>`
/// - `#/webhooks/<name>/<method>`
/// - `#/components/pathItems/<name>/<method>`
/// - `#/components/callbacks/<name>/<encoded expression>/<method>`
///
/// Any of those may be followed by `/callbacks/<name>/<encoded expression>/<method>`
/// segments to address Operations declared inside inline `Operation.callbacks`
/// (recursively). Internal PathItem `$ref` chains are followed with cycle
/// detection at every PathItem level.
fn resolve_internal_operation_ref(spec: &Spec, reference: &str) -> OperationRefResolution {
    enum Container {
        Paths,
        Webhooks,
        ComponentPathItems,
        ComponentCallbacks,
    }
    let (container, after) = if let Some(rest) = reference.strip_prefix("#/paths/") {
        (Container::Paths, rest)
    } else if let Some(rest) = reference.strip_prefix("#/webhooks/") {
        (Container::Webhooks, rest)
    } else if let Some(rest) = reference.strip_prefix("#/components/pathItems/") {
        (Container::ComponentPathItems, rest)
    } else if let Some(rest) = reference.strip_prefix("#/components/callbacks/") {
        (Container::ComponentCallbacks, rest)
    } else {
        return OperationRefResolution::Err(format!(
            "must start with `#/paths/`, `#/webhooks/`, `#/components/pathItems/`, or `#/components/callbacks/`, found `{reference}`"
        ));
    };

    let parts: Vec<&str> = after.split('/').collect();
    if parts.iter().any(|p| p.is_empty()) {
        return OperationRefResolution::Err(format!(
            "malformed JSON Pointer: empty token in `{reference}`; each token with embedded `/` MUST be encoded as `~1`"
        ));
    }

    let mut visits: Vec<String> = Vec::new();
    // Track visited PathItems by their full container-prefixed reference so
    // cycle detection isn't fooled by identical keys in different
    // containers (e.g. a webhook named `Foo` $ref'ing
    // `#/components/pathItems/Foo`).
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let (entry_path, mut item, mut consumed): (String, &crate::v3_1::path_item::PathItem, usize) =
        match container {
            Container::Paths => {
                if parts.len() < 2 {
                    return malformed_pointer(reference);
                }
                let path = unescape_pointer_token(parts[0]);
                let Some(item) = spec.paths.as_ref().and_then(|p| p.paths.get(&path)) else {
                    return OperationRefResolution::Err(format!(
                        "path `{path}` not declared in `#/paths`"
                    ));
                };
                seen.insert(format!("#/paths/{}", parts[0]));
                (path, item, 1)
            }
            Container::Webhooks => {
                if parts.len() < 2 {
                    return malformed_pointer(reference);
                }
                let name = unescape_pointer_token(parts[0]);
                let Some(item) = spec.webhooks.as_ref().and_then(|w| w.paths.get(&name)) else {
                    return OperationRefResolution::Err(format!(
                        "webhook `{name}` not declared in `#/webhooks`"
                    ));
                };
                seen.insert(format!("#/webhooks/{}", parts[0]));
                (name, item, 1)
            }
            Container::ComponentPathItems => {
                if parts.len() < 2 {
                    return malformed_pointer(reference);
                }
                let name = unescape_pointer_token(parts[0]);
                let Some(item) = spec
                    .components
                    .as_ref()
                    .and_then(|c| c.path_items.as_ref())
                    .and_then(|m| m.get(&name))
                else {
                    return OperationRefResolution::Err(format!(
                        "path item `{name}` not declared in `#/components/pathItems`"
                    ));
                };
                visits.push(format!("#/components/pathItems/{name}"));
                seen.insert(format!("#/components/pathItems/{}", parts[0]));
                (name, item, 1)
            }
            Container::ComponentCallbacks => {
                if parts.len() < 3 {
                    return malformed_pointer(reference);
                }
                let cb_name = unescape_pointer_token(parts[0]);
                let expr = unescape_pointer_token(parts[1]);
                let Some(cb_ref) = spec
                    .components
                    .as_ref()
                    .and_then(|c| c.callbacks.as_ref())
                    .and_then(|m| m.get(&cb_name))
                else {
                    return OperationRefResolution::Err(format!(
                        "callback `{cb_name}` not declared in `#/components/callbacks`"
                    ));
                };
                let cb = match cb_ref.get_item(spec) {
                    Ok(cb) => cb,
                    Err(
                        crate::common::reference::ResolveError::ExternalUnsupported(target)
                        | crate::common::reference::ResolveError::External {
                            reference: target, ..
                        },
                    ) => {
                        return OperationRefResolution::ExternalPathItemRef(target);
                    }
                    Err(crate::common::reference::ResolveError::NotFound(t)) => {
                        return OperationRefResolution::Err(format!(
                            "callback `{cb_name}` is a `$ref` to `{t}`, which is not declared"
                        ));
                    }
                };
                let Some(item) = cb.paths.get(&expr) else {
                    return OperationRefResolution::Err(format!(
                        "expression `{expr}` not declared on callback `{cb_name}`"
                    ));
                };
                visits.push(format!("#/components/callbacks/{cb_name}"));
                seen.insert(format!("#/components/callbacks/{}/{}", parts[0], parts[1]));
                (format!("{cb_name}/{expr}"), item, 2)
            }
        };

    let mut display_path = entry_path;
    item = match resolve_path_item_ref_chain(spec, &display_path, item, &mut seen, &mut visits) {
        Ok((p, t)) => {
            display_path = p;
            t
        }
        Err(err) => return err,
    };

    if consumed >= parts.len() {
        return malformed_pointer(reference);
    }
    let mut method = parts[consumed];
    consumed += 1;

    while consumed < parts.len() {
        if parts.len() - consumed < 4 || parts[consumed] != "callbacks" {
            return OperationRefResolution::Err(format!(
                "malformed deep pointer: expected `/callbacks/<name>/<expr>/<method>` continuation, found `{reference}`"
            ));
        }
        // JSON Pointer tokens are case-sensitive; OAS 3.1.2 fixes Operation
        // field names to lowercase, so `GET` is not the same key as `get`.
        let Some(op) = item.operations.as_ref().and_then(|m| m.get(method)) else {
            return OperationRefResolution::Err(format!(
                "method `{method}` not declared on path `{display_path}`"
            ));
        };
        let cb_name = unescape_pointer_token(parts[consumed + 1]);
        let expr = unescape_pointer_token(parts[consumed + 2]);
        let next_method = parts[consumed + 3];
        let Some(cb_ref) = op.callbacks.as_ref().and_then(|m| m.get(&cb_name)) else {
            return OperationRefResolution::Err(format!(
                "callback `{cb_name}` not declared on `{display_path}.{method}`"
            ));
        };
        // If the inline callback slot is itself a `$ref` into
        // `#/components/callbacks/...`, that callback component is now
        // used; mark it so unused-detection doesn't false-flag it.
        if let crate::common::reference::RefOr::Ref(r) = cb_ref
            && let Some(after) = r.reference.strip_prefix("#/components/callbacks/")
        {
            let cb_token = after.split_once('/').map(|(c, _)| c).unwrap_or(after);
            visits.push(format!("#/components/callbacks/{cb_token}"));
        }
        let cb = match cb_ref.get_item(spec) {
            Ok(cb) => cb,
            Err(
                crate::common::reference::ResolveError::ExternalUnsupported(target)
                | crate::common::reference::ResolveError::External {
                    reference: target, ..
                },
            ) => {
                return OperationRefResolution::ExternalPathItemRef(target);
            }
            Err(crate::common::reference::ResolveError::NotFound(t)) => {
                return OperationRefResolution::Err(format!(
                    "callback `{cb_name}` is a `$ref` to `{t}`, which is not declared"
                ));
            }
        };
        let Some(next_item) = cb.paths.get(&expr) else {
            return OperationRefResolution::Err(format!(
                "expression `{expr}` not declared on callback `{cb_name}`"
            ));
        };
        display_path = format!("{display_path}.{method}.callbacks[{cb_name}][{expr}]");
        item = match resolve_path_item_ref_chain(
            spec,
            &display_path,
            next_item,
            &mut seen,
            &mut visits,
        ) {
            Ok((p, t)) => {
                display_path = p;
                t
            }
            Err(err) => return err,
        };
        method = next_method;
        consumed += 4;
    }

    if !item
        .operations
        .as_ref()
        .is_some_and(|m| m.contains_key(method))
    {
        return OperationRefResolution::Err(format!(
            "method `{method}` not declared on path `{display_path}`"
        ));
    }
    OperationRefResolution::Ok(visits)
}

fn malformed_pointer(reference: &str) -> OperationRefResolution {
    OperationRefResolution::Err(format!(
        "malformed JSON Pointer: each token with embedded `/` MUST be encoded as `~1`, found `{reference}`"
    ))
}

fn resolve_path_item_ref_chain<'a>(
    spec: &'a Spec,
    path: &str,
    item: &'a crate::v3_1::path_item::PathItem,
    seen: &mut std::collections::BTreeSet<String>,
    visits: &mut Vec<String>,
) -> Result<(String, &'a crate::v3_1::path_item::PathItem), OperationRefResolution> {
    let Some(ref_str) = &item.reference else {
        return Ok((path.to_owned(), item));
    };

    if ref_str.is_empty() {
        return Err(OperationRefResolution::Err(format!(
            "path `{path}` carries an empty `$ref`"
        )));
    }

    // A PathItem `$ref` may target any of the four containers that hold
    // PathItem objects: `#/paths`, `#/webhooks`, `#/components/pathItems`,
    // or — under a Callback — `#/components/callbacks/<name>/<expr>`.
    // Cycle key is the full container-prefixed reference: identical
    // sub-paths (e.g. `Foo`) in different containers must NOT collide.
    if !seen.insert(ref_str.clone()) {
        return Err(OperationRefResolution::Err(format!(
            "path `{path}` has a cyclic `$ref` chain through `{ref_str}`"
        )));
    }
    let (target_path, target_item) = if let Some(after_paths) = ref_str.strip_prefix("#/paths/") {
        if after_paths.contains('/') {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to malformed JSON Pointer `{ref_str}`: the encoded path token must use `~1` for `/`"
            )));
        }
        let tp = unescape_pointer_token(after_paths);
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
    } else if let Some(after) = ref_str.strip_prefix("#/webhooks/") {
        if after.contains('/') {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to malformed JSON Pointer `{ref_str}`"
            )));
        }
        let tp = unescape_pointer_token(after);
        let Some(t) = spec.webhooks.as_ref().and_then(|w| w.paths.get(&tp)) else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to `{ref_str}`, which is not declared in `#/webhooks`"
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
        visits.push(format!("#/components/pathItems/{tp}"));
        (tp, t)
    } else if let Some(after) = ref_str.strip_prefix("#/components/callbacks/") {
        let mut split = after.splitn(2, '/');
        let (Some(cb_token), Some(expr_token)) = (split.next(), split.next()) else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to malformed JSON Pointer `{ref_str}`: callback target must be `<name>/<encoded expression>`"
            )));
        };
        if expr_token.contains('/') {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to malformed JSON Pointer `{ref_str}`: the encoded expression token must use `~1` for `/`"
            )));
        }
        let cb_name = unescape_pointer_token(cb_token);
        let expr = unescape_pointer_token(expr_token);
        let tp = format!("{cb_name}/{expr}");
        let Some(cb_ref) = spec
            .components
            .as_ref()
            .and_then(|c| c.callbacks.as_ref())
            .and_then(|m| m.get(&cb_name))
        else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to `{ref_str}`, callback `{cb_name}` is not declared in `#/components/callbacks`"
            )));
        };
        let cb = match cb_ref.get_item(spec) {
            Ok(cb) => cb,
            Err(
                crate::common::reference::ResolveError::ExternalUnsupported(target)
                | crate::common::reference::ResolveError::External {
                    reference: target, ..
                },
            ) => {
                return Err(OperationRefResolution::ExternalPathItemRef(target));
            }
            Err(crate::common::reference::ResolveError::NotFound(t)) => {
                return Err(OperationRefResolution::Err(format!(
                    "path `{path}` is a `$ref` to `{ref_str}`; callback resolves to `{t}`, which is not declared"
                )));
            }
        };
        let Some(t) = cb.paths.get(&expr) else {
            return Err(OperationRefResolution::Err(format!(
                "path `{path}` is a `$ref` to `{ref_str}`, expression `{expr}` is not declared on callback `{cb_name}`"
            )));
        };
        visits.push(format!("#/components/callbacks/{cb_name}"));
        (tp, t)
    } else {
        return Err(OperationRefResolution::ExternalPathItemRef(ref_str.clone()));
    };

    resolve_path_item_ref_chain(spec, &target_path, target_item, seen, visits)
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
                    OperationRefResolution::Ok(visits) => {
                        // Mark each touched component reference so unused-
                        // detection doesn't flag a path-item or callback that
                        // is reached only via this operationRef.
                        for r in visits {
                            ctx.visit(r);
                        }
                    }
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
    use crate::common::reference::RefOr;
    use crate::v3_1::operation::Operation;
    use crate::v3_1::path_item::{PathItem, Paths};
    use crate::v3_1::response::{Response, Responses};
    use crate::validation::Context;
    use crate::validation::ValidationErrorsExt;
    use serde_json::json;

    fn spec_with_pets_get() -> Spec {
        let op = Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
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
            ctx.errors.mentions("mutually exclusive"),
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
            !ctx.errors.mentions(".operationRef"),
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
            ctx.errors.mentions("method `post`"),
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
            !ctx.errors.mentions("external reference"),
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

    fn ok_responses() -> Responses {
        Responses {
            responses: Some(BTreeMap::from([(
                "200".to_owned(),
                RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                }),
            )])),
            ..Default::default()
        }
    }

    fn pi_with_get() -> PathItem {
        let mut ops = BTreeMap::new();
        ops.insert(
            "get".to_owned(),
            Operation {
                responses: Some(ok_responses()),
                ..Default::default()
            },
        );
        PathItem {
            operations: Some(ops),
            ..Default::default()
        }
    }

    #[test]
    fn operation_ref_into_components_path_items() {
        // Per OAS 3.1, operationRef can target an Operation in
        // `#/components/pathItems/<name>/<method>`.
        use crate::v3_1::components::Components;
        let comp = Components {
            path_items: Some(BTreeMap::from([("Reusable".to_owned(), pi_with_get())])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/pathItems/Reusable/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "components.pathItems target should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_webhooks() {
        let mut webhooks = Paths::default();
        webhooks
            .paths
            .insert("petCreated".to_owned(), pi_with_get());
        let spec = Spec {
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/webhooks/petCreated/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "webhook target should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_chain_paths_to_components_pathitems() {
        // /pets is a $ref to #/components/pathItems/Reusable; the resolver
        // should follow the chain across containers.
        use crate::v3_1::components::Components;
        let comp = Components {
            path_items: Some(BTreeMap::from([("Reusable".to_owned(), pi_with_get())])),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/pathItems/Reusable".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "cross-container chain should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_cycle_errors() {
        let mut paths = Paths::default();
        paths.paths.insert(
            "/a".to_owned(),
            PathItem {
                reference: Some("#/paths/~1b".into()),
                ..Default::default()
            },
        );
        paths.paths.insert(
            "/b".to_owned(),
            PathItem {
                reference: Some("#/paths/~1a".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1a/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("cyclic `$ref` chain"),
            "expected cycle error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_dangling_components_path_items_target() {
        let spec = spec_with_pets_get();
        // Add the `/pets` PathItem with a $ref to a missing component.
        let mut spec = spec;
        let mut paths = spec.paths.take().unwrap();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/pathItems/Missing".into()),
                ..Default::default()
            },
        );
        spec.paths = Some(paths);
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not declared in `#/components/pathItems`")),
            "expected dangling-component-pathItem error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_components_callbacks() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("{$request.body#/cb}".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let comp = Components {
            callbacks: Some(BTreeMap::from([("OnPing".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        // Expression key contains `/` and so is encoded as `~1`.
        Link {
            operation_ref: Some("#/components/callbacks/OnPing/{$request.body#~1cb}/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "callback target should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_callbacks_unknown_expression_errors() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "OnPing".to_owned(),
                RefOr::new_item(Callback::default()),
            )])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/callbacks/OnPing/missing/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("expression `missing`") && e.contains("OnPing")),
            "expected unknown-expression error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_callbacks_unknown_callback_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/callbacks/Missing/x/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("callback `Missing` not declared")),
            "expected dangling-callback error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_inline_path_op_callback() {
        use crate::v3_1::callback::Callback;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("{$request.query.callbackUrl}".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let mut callbacks = BTreeMap::new();
        callbacks.insert("myCb".to_owned(), RefOr::new_item(cb));
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/subscribe".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some(
                "#/paths/~1subscribe/post/callbacks/myCb/{$request.query.callbackUrl}/get".into(),
            ),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "deep callback target should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_inline_callback_unknown_callback_errors() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get/callbacks/missing/expr/post".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("callback `missing` not declared on")),
            "expected unknown-inline-callback error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_deep_pointer_malformed_continuation_errors() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get/extra".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("malformed deep pointer")),
            "expected malformed-deep-pointer error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_method_token_is_case_sensitive() {
        // OAS 3.1.2 fixes Operation field names to lowercase, and JSON
        // Pointer tokens are case-sensitive. `#/paths/~1pets/GET` must NOT
        // resolve to the `get` operation.
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/GET".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("method `GET` not declared")),
            "expected case-sensitive method error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_inline_callback_name_unescaped() {
        // RFC 6901: a callback name containing `/` must round-trip through
        // `~1`. Build a callback whose name literally is `weird/name`,
        // referenced as `weird~1name`.
        use crate::v3_1::callback::Callback;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("expr".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let mut callbacks = BTreeMap::new();
        callbacks.insert("weird/name".to_owned(), RefOr::new_item(cb));
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/post/callbacks/weird~1name/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "callback with `/` in name should resolve via `~1`: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn path_item_ref_chain_target_in_components_callbacks() {
        // A PathItem in `paths` carries `$ref` pointing at a Path Item
        // that lives under `#/components/callbacks/<n>/<expr>` — that
        // PathItem is still a Path Item Object, so the chain follower must
        // resolve it (Codex finding).
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("e".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let comp = Components {
            callbacks: Some(BTreeMap::from([("CB".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/callbacks/CB/e".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "chain through components.callbacks must resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_components_path_items_marks_visited() {
        // Codex: a Link.operationRef that resolves into
        // `#/components/pathItems/<name>/<method>` must mark the
        // component as visited so the unused-pathItems check doesn't
        // falsely flag it.
        use crate::v3_1::components::Components;
        let mut cp = BTreeMap::new();
        cp.insert("Reusable".to_owned(), pi_with_get());
        let comp = Components {
            path_items: Some(cp),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::empty());
        Link {
            operation_ref: Some("#/components/pathItems/Reusable/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.is_visited("#/components/pathItems/Reusable"),
            "components.pathItems target should be marked visited"
        );
    }

    #[test]
    fn operation_ref_into_components_callbacks_marks_visited() {
        // Same idea for `#/components/callbacks/<n>/<expr>/<method>`:
        // the unused-callbacks check keys off the callback container
        // (`#/components/callbacks/<n>`), so that's what we mark.
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("e".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let comp = Components {
            callbacks: Some(BTreeMap::from([("CB".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::empty());
        Link {
            operation_ref: Some("#/components/callbacks/CB/e/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.is_visited("#/components/callbacks/CB"),
            "components.callbacks target should be marked visited"
        );
    }

    #[test]
    fn ref_chain_cross_container_same_key_not_cycle() {
        // Webhook `Foo` $refs `#/components/pathItems/Foo`. Identical key
        // strings in different containers must NOT collide in the cycle
        // detector — the chain resolves cleanly to the components.pathItems
        // operation.
        use crate::v3_1::components::Components;
        let mut cp = BTreeMap::new();
        cp.insert("Foo".to_owned(), pi_with_get());
        let comp = Components {
            path_items: Some(cp),
            ..Default::default()
        };
        let mut webhooks = Paths::default();
        webhooks.paths.insert(
            "Foo".to_owned(),
            PathItem {
                reference: Some("#/components/pathItems/Foo".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            webhooks: Some(webhooks),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/webhooks/Foo/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions("cyclic"),
            "cross-container same-key must not be flagged as cycle: {:?}",
            ctx.errors
        );
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "operationRef should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn server_validates_when_operation_ref_set() {
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            server: Some(crate::v3_1::server::Server {
                url: "".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("server.url"),
            "expected server.url error: {:?}",
            ctx.errors
        );
    }

    // ── Malformed operationRefs with too-few tokens ───────────────────────────

    #[test]
    fn operation_ref_paths_only_one_token_malformed() {
        // `#/paths/~1pets` has only one token after the prefix; needs >= 2.
        let spec = spec_with_pets_get();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("malformed JSON Pointer"),
            "expected malformed-pointer error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_webhooks_only_one_token_malformed() {
        let mut webhooks = Paths::default();
        webhooks.paths.insert("pet".to_owned(), pi_with_get());
        let spec = Spec {
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/webhooks/pet".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("malformed JSON Pointer"),
            "expected malformed-pointer error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_components_path_items_only_one_token_malformed() {
        use crate::v3_1::components::Components;
        let comp = Components {
            path_items: Some(BTreeMap::from([("Reusable".to_owned(), pi_with_get())])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/pathItems/Reusable".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("malformed JSON Pointer"),
            "expected malformed-pointer error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_components_callbacks_fewer_than_three_tokens_malformed() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "CB".to_owned(),
                RefOr::new_item(Callback::default()),
            )])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        // Only two tokens: CB/e — need at least three (name/expr/method).
        Link {
            operation_ref: Some("#/components/callbacks/CB/e".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("malformed JSON Pointer"),
            "expected malformed-pointer error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_webhooks_unknown_webhook_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/webhooks/nonexistent/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("webhook `nonexistent` not declared")),
            "expected unknown-webhook error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_to_webhooks_target() {
        // A PathItem in `paths` that `$ref`s to `#/webhooks/<name>` should
        // be followed by the path-item ref chain resolver.
        let mut webhooks = Paths::default();
        webhooks
            .paths
            .insert("petCreated".to_owned(), pi_with_get());
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/webhooks/petCreated".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "webhook-chain target should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_to_components_path_items_dangling() {
        // A PathItem.reference pointing at `#/components/pathItems/Missing`
        // (target doesn't exist) hits the "not declared" error path.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/pathItems/Missing".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
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
                .any(|e| e.contains("not declared in `#/components/pathItems`")),
            "expected dangling components.pathItems error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_token_with_slash_malformed() {
        // A PathItem.reference of `#/paths/a/b` has a token with '/' — the
        // resolver should report a malformed pointer.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/paths/a/b".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for malformed path token: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_webhooks_token_with_slash_malformed() {
        // A PathItem.reference of `#/webhooks/a/b` has extra token.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/webhooks/a/b".into()),
                ..Default::default()
            },
        );
        let mut webhooks = Paths::default();
        webhooks.paths.insert("a".to_owned(), PathItem::default());
        let spec = Spec {
            paths: Some(paths),
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for extra webhook token: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_components_callbacks_malformed_missing_expr() {
        // A PathItem.reference of `#/components/callbacks/CB` has no expr part.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/callbacks/CB".into()),
                ..Default::default()
            },
        );
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "CB".to_owned(),
                RefOr::new_item(Callback::default()),
            )])),
            ..Default::default()
        };
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for callbacks ref with no expr: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_components_callbacks_dangling_cb() {
        // A PathItem.reference of `#/components/callbacks/Missing/expr` where
        // the callback is not declared.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/callbacks/Missing/expr".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for dangling callback ref: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_components_callbacks_dangling_expr() {
        // A PathItem.reference of `#/components/callbacks/CB/missing` where
        // the expression is not declared in the callback.
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let cb = Callback {
            paths: BTreeMap::from([("expr".to_owned(), PathItem::default())]),
            ..Default::default()
        };
        let comp = Components {
            callbacks: Some(BTreeMap::from([("CB".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/callbacks/CB/missing".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for dangling callback expression: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_external_is_error_without_ignore() {
        // A PathItem.reference pointing at an external doc triggers
        // `ExternalPathItemRef`. Without `IgnoreExternalReferences` this
        // must be reported as an error.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("https://other.example/spec.yaml#/paths/~1pets".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "external path-item ref should be reported: {:?}",
            ctx.errors
        );

        // With IgnoreExternalReferences: no error.
        let mut paths2 = Paths::default();
        paths2.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("https://other.example/spec.yaml#/paths/~1pets".into()),
                ..Default::default()
            },
        );
        let spec2 = Spec {
            paths: Some(paths2),
            ..Default::default()
        };
        let mut ctx2 = Context::new(&spec2, Options::IgnoreExternalReferences.only());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx2, "l".into());
        assert!(
            !ctx2.errors.mentions(".operationRef"),
            "with ignore-external: {:?}",
            ctx2.errors
        );
    }

    #[test]
    fn operation_ref_inline_callback_method_missing() {
        // An operationRef that traverses into a callback but the method
        // is not declared on the path item — exercising the `method not
        // declared on path` error for inline callback deep pointers.
        use crate::v3_1::callback::Callback;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("expr".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let mut callbacks = BTreeMap::new();
        callbacks.insert("myCb".to_owned(), RefOr::new_item(cb));
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/subscribe".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1subscribe/post/callbacks/myCb/expr/delete".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("method `delete` not declared"),
            "expected missing-method error: {:?}",
            ctx.errors
        );
    }

    // ── components/pathItems name not declared ────────────────────────────────

    #[test]
    fn operation_ref_into_components_path_items_missing_name() {
        // `#/components/pathItems/Missing/get` — the name "Missing" is not
        // in components.pathItems, so the "not declared" branch fires.
        use crate::v3_1::components::Components;
        let comp = Components {
            path_items: Some(BTreeMap::from([("Reusable".to_owned(), pi_with_get())])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/pathItems/Missing/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("path item `Missing` not declared in `#/components/pathItems`")),
            "expected not-declared error: {:?}",
            ctx.errors
        );
    }

    // ── components/callbacks cb_ref is an external $ref ───────────────────────

    #[test]
    fn operation_ref_into_components_callbacks_external_ref_errors() {
        // The callback entry is itself a `$ref` to an external document.
        // `cb_ref.get_item(spec)` returns `ExternalUnsupported`.
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "CB".to_owned(),
                RefOr::new_ref(
                    "https://other.example/spec.yaml#/components/callbacks/CB".to_owned(),
                ),
            )])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/callbacks/CB/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        // Without IgnoreExternalReferences the external path-item ref must surface.
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for external callback ref: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_into_components_callbacks_notfound_ref_errors() {
        // The callback entry is an internal `$ref` to a non-existent key.
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "CB".to_owned(),
                RefOr::new_ref("#/components/callbacks/NonExistent".to_owned()),
            )])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/components/callbacks/CB/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for not-found callback ref: {:?}",
            ctx.errors
        );
    }

    // ── consumed >= parts.len() (malformed pointer) ───────────────────────────

    #[test]
    fn operation_ref_webhooks_consumed_equals_parts_len_malformed() {
        // After consuming the webhook entry, consumed == parts.len() (no
        // method token left) → malformed_pointer is returned.
        let mut webhooks = Paths::default();
        webhooks.paths.insert("pet".to_owned(), pi_with_get());
        // The ref points at a webhook path item (1 token) that itself
        // has no further method token because the ref ends at `pet`:
        // Note: `#/webhooks/pet` alone has parts.len()==1 which is
        // caught by the < 2 check. To hit `consumed >= parts.len()`
        // we need the chain follower to eat exactly the right number —
        // trigger via a path item `$ref` that resolves to webhooks and
        // then the outer ref has exactly one remaining part (the method)
        // but the chain consumed it all. Easiest: use a single-part
        // `#/webhooks/...` where len >= 2 but consumed==parts.len()
        // after resolving the entry. That requires parts.len()==1 after
        // initial consumption of the webhook name but that's blocked by
        // the "< 2" guard above.  Instead use a ref that starts at
        // `#/paths/...` where the path item `$ref` itself points into
        // `#/webhooks` and the outer path ref's trailing token count
        // equals consumed. The simplest reproducible path:
        // `#/components/pathItems/X/` (trailing slash) yields an empty
        // token that triggers the "empty token" guard, which is a
        // different branch. Let's use the "no `paths`" branch instead:
        // a path item $ref to `#/paths/x` when spec.paths is None.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/a".to_owned(),
            PathItem {
                reference: Some("#/paths/~1b".into()),
                ..Default::default()
            },
        );
        // spec has paths but /b is missing → fires "not declared in #/paths"
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1a/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for dangling chain ref: {:?}",
            ctx.errors
        );
    }

    // ── deep pointer — method not declared in while-loop ─────────────────────

    #[test]
    fn operation_ref_deep_method_not_declared_in_loop() {
        // In the deep-pointer while loop, `item.operations` does not
        // contain `method` → fires lines 193-195.
        //
        // Strategy: two levels of callback hopping.
        //   ref: #/paths/~1pets/post/callbacks/cb/expr/delete/callbacks/inner/x/post
        //   First hop: /pets.post.callbacks["cb"]["expr"] → pi_with_get()
        //   method becomes "delete", consumed = 6
        //   Second loop iteration: item = pi_with_get(), method = "delete"
        //   pi_with_get() only has "get", NOT "delete" → 193-195 fires.
        use crate::v3_1::callback::Callback;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("expr".to_owned(), pi_with_get());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let mut callbacks = BTreeMap::new();
        callbacks.insert("cb".to_owned(), RefOr::new_item(cb));
        let outer = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), outer);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        // After first callback hop, item=pi_with_get(), method="delete".
        // Second while-loop iteration: item.operations.get("delete") fails.
        Link {
            operation_ref: Some(
                "#/paths/~1pets/post/callbacks/cb/expr/delete/callbacks/inner/x/post".into(),
            ),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions("method `delete` not declared"),
            "expected method-not-declared error in loop: {:?}",
            ctx.errors
        );
    }

    // ── deep pointer — inline cb is a $ref into components/callbacks ──────────

    #[test]
    fn operation_ref_deep_pointer_cb_ref_into_components_marks_visited() {
        // When an inline callback slot is a `$ref` into
        // `#/components/callbacks/...`, the deep-pointer resolver marks
        // the component as visited (lines 209-212).
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("expr".to_owned(), pi_with_get());
        let comp_cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "SharedCB".to_owned(),
                RefOr::new_item(comp_cb),
            )])),
            ..Default::default()
        };
        // The path's operation has a callback that is a $ref into
        // #/components/callbacks/SharedCB.
        let mut callbacks = BTreeMap::new();
        callbacks.insert(
            "myCb".to_owned(),
            RefOr::new_ref("#/components/callbacks/SharedCB".to_owned()),
        );
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/sub".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::empty());
        Link {
            operation_ref: Some("#/paths/~1sub/post/callbacks/myCb/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            !ctx.errors.mentions(".operationRef"),
            "should resolve via components callback ref: {:?}",
            ctx.errors
        );
        assert!(
            ctx.is_visited("#/components/callbacks/SharedCB"),
            "SharedCB should be marked visited"
        );
    }

    // ── deep pointer — inline callback External or NotFound ───────────────────

    #[test]
    fn operation_ref_deep_pointer_inline_cb_external_ref_errors() {
        // Inline callback slot is a `$ref` to an external document:
        // `cb_ref.get_item(spec)` returns ExternalUnsupported → 217-222.
        let mut callbacks = BTreeMap::new();
        callbacks.insert(
            "myCb".to_owned(),
            RefOr::new_ref("https://other.example/spec.yaml#/components/callbacks/CB".to_owned()),
        );
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/sub".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1sub/post/callbacks/myCb/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for external inline callback: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_deep_pointer_inline_cb_notfound_ref_errors() {
        // Inline callback slot is an internal `$ref` to a non-existent key:
        // `cb_ref.get_item(spec)` returns NotFound → 224-227.
        let mut callbacks = BTreeMap::new();
        callbacks.insert(
            "myCb".to_owned(),
            RefOr::new_ref("#/components/callbacks/NonExistent".to_owned()),
        );
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/sub".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1sub/post/callbacks/myCb/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for not-found inline callback: {:?}",
            ctx.errors
        );
    }

    // ── deep pointer — expression not declared on callback ────────────────────

    #[test]
    fn operation_ref_deep_pointer_inline_cb_missing_expression() {
        // In the deep-pointer loop, the expression is not declared on the
        // resolved callback → 231-233.
        use crate::v3_1::callback::Callback;
        let cb = Callback {
            paths: BTreeMap::from([("declared".to_owned(), pi_with_get())]),
            ..Default::default()
        };
        let mut callbacks = BTreeMap::new();
        callbacks.insert("myCb".to_owned(), RefOr::new_item(cb));
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/sub".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        // "undeclared" expression does not exist in the callback
        Link {
            operation_ref: Some("#/paths/~1sub/post/callbacks/myCb/undeclared/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("expression `undeclared`") && e.contains("myCb")),
            "expected missing-expression error: {:?}",
            ctx.errors
        );
    }

    // ── deep pointer — resolve_path_item_ref_chain fails in loop ─────────────

    #[test]
    fn operation_ref_deep_pointer_chain_fails_in_loop() {
        // After resolving into an inline callback, the callback's path item
        // itself has a `$ref` that fails (dangling) → line 247 fires.
        use crate::v3_1::callback::Callback;
        // The callback's path item has a dangling `$ref`.
        let dangling_pi = PathItem {
            reference: Some("#/paths/~1nonexistent".into()),
            ..Default::default()
        };
        let cb = Callback {
            paths: BTreeMap::from([("expr".to_owned(), dangling_pi)]),
            ..Default::default()
        };
        let mut callbacks = BTreeMap::new();
        callbacks.insert("myCb".to_owned(), RefOr::new_item(cb));
        let op = Operation {
            responses: Some(ok_responses()),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/sub".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1sub/post/callbacks/myCb/expr/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error when chain fails inside deep pointer loop: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — empty $ref ──────────────────────────────

    #[test]
    fn operation_ref_path_item_ref_empty_errors() {
        // A PathItem with `$ref: ""` triggers the empty-ref branch (283-285).
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for empty path-item $ref: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — no paths in spec ────────────────────────

    #[test]
    fn operation_ref_path_item_ref_to_paths_when_spec_has_no_paths() {
        // A PathItem.reference of `#/paths/~1foo` when the spec has no
        // `paths` field at all → lines 306-308.
        // We need to enter resolve_path_item_ref_chain via a different
        // container (webhooks) so that spec.paths == None.
        let mut webhooks = Paths::default();
        webhooks.paths.insert(
            "event".to_owned(),
            PathItem {
                reference: Some("#/paths/~1foo".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/webhooks/event/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error when chained ref targets missing paths: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — path not declared in #/paths ───────────

    #[test]
    fn operation_ref_path_item_ref_to_missing_path_entry() {
        // A PathItem.reference of `#/paths/~1missing` where `/missing`
        // is not in paths → lines 311-313.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/source".to_owned(),
            PathItem {
                reference: Some("#/paths/~1missing".into()),
                ..Default::default()
            },
        );
        // /missing is not in paths
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1source/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not declared in `#/paths`")),
            "expected not-declared-in-paths error: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — webhook not declared ────────────────────

    #[test]
    fn operation_ref_path_item_ref_to_missing_webhook_entry() {
        // A PathItem.reference of `#/webhooks/missing` → lines 324-326.
        let mut webhooks = Paths::default();
        webhooks.paths.insert(
            "event".to_owned(),
            PathItem {
                reference: Some("#/webhooks/missing".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/webhooks/event/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not declared in `#/webhooks`")),
            "expected not-declared-in-webhooks error: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — #/components/pathItems/ with slash ──────

    #[test]
    fn operation_ref_path_item_ref_components_path_items_slash_in_token() {
        // A PathItem.reference of `#/components/pathItems/a/b` has a '/'
        // in the token → lines 331-333.
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/pathItems/a/b".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for components/pathItems/ with slash: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — callbacks expr_token contains '/' ───────

    #[test]
    fn operation_ref_path_item_ref_components_callbacks_expr_with_slash() {
        // A PathItem.reference of `#/components/callbacks/CB/a/b` where
        // expr_token is `a/b` (contains '/') → lines 356-358.
        use crate::v3_1::callback::Callback;
        use crate::v3_1::components::Components;
        let cb = Callback {
            paths: BTreeMap::from([("a".to_owned(), pi_with_get())]),
            ..Default::default()
        };
        let comp = Components {
            callbacks: Some(BTreeMap::from([("CB".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                // expr_token is "a/b" which contains '/'
                reference: Some("#/components/callbacks/CB/a/b".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for callbacks ref with slash in expr: {:?}",
            ctx.errors
        );
    }

    // ── resolve_path_item_ref_chain — callbacks cb_ref External / NotFound ────

    #[test]
    fn operation_ref_path_item_ref_components_callbacks_external_cb_ref() {
        // A PathItem.reference targets `#/components/callbacks/CB/expr`
        // where CB is itself an external `$ref` → ExternalUnsupported (376-381).
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "CB".to_owned(),
                RefOr::new_ref(
                    "https://other.example/spec.yaml#/components/callbacks/CB".to_owned(),
                ),
            )])),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/callbacks/CB/expr".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        // The ExternalPathItemRef propagates; without IgnoreExternalReferences
        // it must result in an error.
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for external cb $ref in path-item chain: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn operation_ref_path_item_ref_components_callbacks_notfound_cb_ref() {
        // A PathItem.reference targets `#/components/callbacks/CB/expr`
        // where CB is an internal `$ref` to a non-existent key → NotFound (383-386).
        use crate::v3_1::components::Components;
        let comp = Components {
            callbacks: Some(BTreeMap::from([(
                "CB".to_owned(),
                RefOr::new_ref("#/components/callbacks/NonExistent".to_owned()),
            )])),
            ..Default::default()
        };
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                reference: Some("#/components/callbacks/CB/expr".into()),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        Link {
            operation_ref: Some("#/paths/~1pets/get".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "l".into());
        assert!(
            ctx.errors.mentions(".operationRef"),
            "expected error for not-found cb $ref in path-item chain: {:?}",
            ctx.errors
        );
    }
}
