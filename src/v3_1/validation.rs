//! Cross-cutting v3.1 validation rules that span multiple objects.
//!
//! Each helper is invoked from `Spec::validate` to enforce rules from the
//! OpenAPI 3.1.2 spec that don't naturally fit on a single struct's
//! `ValidateWithContext` impl:
//!
//! * Parameter `(name, in)` uniqueness with operation-overrides-pathItem semantics
//! * Path template `{var}` ↔ `in: path` parameter correspondence
//! * Equivalent templated paths (e.g. `/pets/{id}` vs `/pets/{name}`) collisions
//! * Tag-name uniqueness in `Spec.tags`
//! * Security requirement validation (top-level + operation-level). Per
//!   OAS 3.1, only `oauth2` requirements need their scopes resolved
//!   against the scheme's flow scopes; for `apiKey` / `http` /
//!   `mutualTLS` / `openIdConnect` the array MAY contain free-form role
//!   names that aren't otherwise defined in the document (relaxed from
//!   3.0's "must be empty for non-oauth" rule).
//! * Operation ID uniqueness across `Spec.paths` *and* `Spec.webhooks`
//!   (3.1 added webhooks; previous code only walked paths).

use lazy_regex::regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::common::reference::{RefOr, ResolveReference};
use crate::v3_1::parameter::{InCookie, InHeader, InPath, InQuery, Parameter};
use crate::v3_1::path_item::PathItem;
use crate::v3_1::security_scheme::SecurityScheme;
use crate::v3_1::spec::Spec;
use crate::v3_1::tag::Tag;
use crate::validation::Options;
use crate::validation::{Context, PushError};

/// Result of attempting to resolve a `RefOr<Parameter>` for cross-cutting
/// validation. The internal/external distinction matters: when
/// `IgnoreExternalReferences` is set, we suppress the
/// "template var has no matching `in: path` parameter" error because the
/// missing param may legitimately live in the external document we agreed
/// not to follow.
enum ResolvedParam<'a> {
    Item(&'a Parameter),
    UnresolvedInternal,
    UnresolvedExternal,
}

fn resolve_parameter<'a>(spec: &'a Spec, p: &'a RefOr<Parameter>) -> ResolvedParam<'a> {
    match p {
        RefOr::Item(p) => ResolvedParam::Item(p),
        RefOr::Ref(r) => {
            if r.reference.starts_with("#/") {
                match <Spec as ResolveReference<Parameter>>::resolve_reference(spec, &r.reference) {
                    Some(p) => ResolvedParam::Item(p),
                    None => ResolvedParam::UnresolvedInternal,
                }
            } else {
                ResolvedParam::UnresolvedExternal
            }
        }
    }
}

fn parameter_identity(p: &Parameter) -> (&str, &'static str) {
    match p {
        Parameter::Path(p) => (in_path_name(p), "path"),
        Parameter::Query(q) => (in_query_name(q), "query"),
        Parameter::Header(h) => (in_header_name(h), "header"),
        Parameter::Cookie(c) => (in_cookie_name(c), "cookie"),
    }
}

fn in_path_name(p: &InPath) -> &str {
    &p.name
}
fn in_query_name(q: &InQuery) -> &str {
    &q.name
}
fn in_header_name(h: &InHeader) -> &str {
    &h.name
}
fn in_cookie_name(c: &InCookie) -> &str {
    &c.name
}

fn path_template_variables(template: &str) -> BTreeSet<String> {
    let re = regex!(r"\{([^}]+)\}");
    re.captures_iter(template)
        .map(|c| c.get(1).unwrap().as_str().to_owned())
        .collect()
}

fn canonical_path(template: &str) -> String {
    regex!(r"\{[^}]+\}")
        .replace_all(template, "{}")
        .into_owned()
}

/// Validate operation-level parameter rules:
/// * `(name, in)` duplicates within the same level are flagged.
/// * Operation-level entries override path-item entries with the same key.
/// * Each path template `{var}` has a matching `in: path` parameter.
/// * Each `in: path` parameter is referenced by the path template.
pub fn validate_operation_parameters(
    ctx: &mut Context<Spec>,
    op_path: &str,
    template: &str,
    path_item_params: Option<&[RefOr<Parameter>]>,
    op_params: Option<&[RefOr<Parameter>]>,
) {
    let template_vars = path_template_variables(template);

    let ignore_external = ctx.is_option(Options::IgnoreExternalReferences);
    let mut has_unresolved_external = false;

    fn dup_pass(
        ctx: &mut Context<Spec>,
        op_path: &str,
        params: &[RefOr<Parameter>],
        origin: &str,
        ignore_external: bool,
        has_unresolved_external: &mut bool,
    ) {
        let mut seen: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
        for (i, raw) in params.iter().enumerate() {
            let r = resolve_parameter(ctx.spec, raw);
            match r {
                ResolvedParam::Item(p) => {
                    let (name, loc) = parameter_identity(p);
                    let key = (name.to_owned(), loc);
                    *seen.entry(key.clone()).or_insert(0) += 1;
                    if seen[&key] == 2 {
                        ctx.error(
                            op_path.to_owned(),
                            format_args!(
                                ".parameters: duplicate parameter `{name}` in `{loc}` ({origin}[{i}])"
                            ),
                        );
                    }
                }
                ResolvedParam::UnresolvedExternal if ignore_external => {
                    *has_unresolved_external = true;
                }
                _ => {}
            }
        }
    }
    if let Some(p) = path_item_params {
        dup_pass(
            ctx,
            op_path,
            p,
            "path-item",
            ignore_external,
            &mut has_unresolved_external,
        );
    }
    if let Some(p) = op_params {
        dup_pass(
            ctx,
            op_path,
            p,
            "operation",
            ignore_external,
            &mut has_unresolved_external,
        );
    }

    #[derive(Clone, Copy)]
    enum Kind {
        Path,
        Other,
    }
    fn kind_of(p: &Parameter) -> Kind {
        match p {
            Parameter::Path(_) => Kind::Path,
            _ => Kind::Other,
        }
    }
    let mut merged: BTreeMap<(String, &'static str), Kind> = BTreeMap::new();
    for params in [path_item_params, op_params].into_iter().flatten() {
        for raw in params {
            if let ResolvedParam::Item(p) = resolve_parameter(ctx.spec, raw) {
                let (name, loc) = parameter_identity(p);
                merged.insert((name.to_owned(), loc), kind_of(p));
            }
        }
    }

    let mut declared_path_params: BTreeSet<String> = BTreeSet::new();
    for ((name, _loc), kind) in &merged {
        if matches!(kind, Kind::Path) {
            declared_path_params.insert(name.clone());
        }
    }

    // Suppress only the "template var has no matching parameter" direction
    // when we encountered an external `$ref` we agreed not to follow. The
    // opposite direction (a declared path parameter that isn't in the
    // template) still fires regardless — those parameters are visible
    // locally, so the inconsistency is real.
    let skip_template_var_missing = has_unresolved_external;

    if !skip_template_var_missing {
        for var in &template_vars {
            if !declared_path_params.contains(var) {
                ctx.error(
                    op_path.to_owned(),
                    format_args!(
                        ".parameters: path template variable `{{{var}}}` has no matching `in: path` parameter"
                    ),
                );
            }
        }
    }
    for declared in &declared_path_params {
        if !template_vars.contains(declared) {
            ctx.error(
                op_path.to_owned(),
                format_args!(
                    ".parameters: path parameter `{declared}` does not match any `{{name}}` in the path template"
                ),
            );
        }
    }
}

/// Validate a list of security requirements. Marks each named scheme as
/// visited (for unused-scheme detection) and enforces:
/// * the named scheme exists in `components.securitySchemes`,
/// * `oauth2` scopes (if any) are listed in some flow,
/// * `apiKey` / `http` / `mutualTLS` / `openIdConnect` accept any scope
///   list — per OAS 3.1, the array MAY contain free-form role names that
///   are not otherwise defined in the document. (3.0 required these
///   arrays to be empty; 3.1 relaxed that.)
pub fn validate_security_requirements(
    ctx: &mut Context<Spec>,
    path: &str,
    requirements: &[BTreeMap<String, Vec<String>>],
) {
    let schemes_map = ctx
        .spec
        .components
        .as_ref()
        .and_then(|c| c.security_schemes.as_ref());

    for (i, req) in requirements.iter().enumerate() {
        for (name, scopes) in req {
            let scheme_ref = format!("#/components/securitySchemes/{name}");
            ctx.visit(scheme_ref.clone());

            let Some(map) = schemes_map else {
                ctx.error(
                    path.to_owned(),
                    format_args!(
                        "[{i}].`{name}`: no `components.securitySchemes` on the spec to resolve against"
                    ),
                );
                continue;
            };
            let Some(scheme_or) = map.get(name) else {
                ctx.error(
                    path.to_owned(),
                    format_args!("[{i}].`{name}`: not declared in `components.securitySchemes`"),
                );
                continue;
            };
            let Ok(scheme) = scheme_or.get_item(ctx.spec) else {
                continue;
            };

            match scheme {
                SecurityScheme::OAuth2(o) => {
                    for scope in scopes {
                        let scope_ref = format!("{scheme_ref}/{scope}");
                        ctx.visit(scope_ref);
                        let in_any = [
                            o.flows
                                .implicit
                                .as_ref()
                                .map(|f| f.scopes.contains_key(scope)),
                            o.flows
                                .password
                                .as_ref()
                                .map(|f| f.scopes.contains_key(scope)),
                            o.flows
                                .client_credentials
                                .as_ref()
                                .map(|f| f.scopes.contains_key(scope)),
                            o.flows
                                .authorization_code
                                .as_ref()
                                .map(|f| f.scopes.contains_key(scope)),
                        ]
                        .into_iter()
                        .flatten()
                        .any(|x| x);
                        if !in_any {
                            ctx.error(
                                path.to_owned(),
                                format_args!(
                                    "[{i}].`{name}`: scope `{scope}` not declared in any flow's scopes"
                                ),
                            );
                        }
                    }
                }
                SecurityScheme::OpenIdConnect(_) => {
                    // Scopes resolved at runtime via the OIDC discovery doc.
                }
                SecurityScheme::ApiKey(_)
                | SecurityScheme::HTTP(_)
                | SecurityScheme::MutualTLS(_) => {
                    // OAS 3.1: "For other security scheme types, the array
                    // MAY contain a list of role names which are required
                    // for the execution, but are not otherwise defined or
                    // exchanged in-band." So we do NOT require empty arrays
                    // here (that was 3.0's rule).
                }
            }
        }
    }
}

/// Validate Spec.tags: every tag name MUST be unique.
pub fn validate_tag_uniqueness(ctx: &mut Context<Spec>, tags: &[Tag]) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in tags {
        *counts.entry(t.name.as_str()).or_insert(0) += 1;
    }
    for (name, n) in counts {
        if n > 1 {
            ctx.error(
                "#.tags".to_owned(),
                format_args!("duplicate tag name `{name}` (found {n} times)"),
            );
        }
    }
}

/// Validate that no two paths in a path map collapse to the same canonical
/// (template-stripped) form. Extension keys (`x-...`) are skipped.
/// Applied to `Spec.paths` only; webhook keys are arbitrary identifiers
/// (not URL templates) per OAS 3.1.2 and `Spec.validate` skips this check
/// for them.
pub fn validate_path_template_uniqueness<V>(
    ctx: &mut Context<Spec>,
    section: &str,
    paths: &BTreeMap<String, V>,
) {
    let mut canonicals: HashMap<String, Vec<&str>> = HashMap::new();
    for key in paths.keys() {
        if key.starts_with("x-") {
            continue;
        }
        canonicals
            .entry(canonical_path(key))
            .or_default()
            .push(key.as_str());
    }
    for (canonical, keys) in canonicals {
        if keys.len() > 1 {
            ctx.error(
                section.to_owned(),
                format_args!(
                    "templated paths {keys:?} collapse to the same shape `{canonical}`; OAS forbids equivalent templates"
                ),
            );
        }
    }
}

/// Per-path entry point: emit cross-cutting checks that only make sense for
/// operations actually mounted on a path with a template (i.e. `Spec.paths`).
/// Operation-level `security` is intentionally validated by
/// `Operation::validate_with_context`, not here, so it also fires for
/// operations nested inside Callback / Webhook path items.
pub fn validate_path_item(ctx: &mut Context<Spec>, template: &str, path: &str, item: &PathItem) {
    // If the path entry is a `$ref` wrapper (no inline operations / params),
    // follow the chain to the effective PathItem so the path-template ↔
    // parameter correspondence check sees the operations actually mounted
    // at this template. Inline content (when present) takes precedence.
    let effective = if item.parameters.is_none() && item.operations.is_none() {
        resolve_path_item_chain(ctx.spec, item)
    } else {
        item
    };
    let pi_params = effective.parameters.as_deref();
    if let Some(ops) = &effective.operations {
        for (method, op) in ops {
            let op_path = format!("{path}.{method}");
            validate_operation_parameters(
                ctx,
                &op_path,
                template,
                pi_params,
                op.parameters.as_deref(),
            );
        }
    }
}

/// Walk `PathItem.reference` hops with cycle detection. Stops at the first
/// item without a `$ref`, on a dangling target, an empty/external ref, or
/// a cycle (returning the current item in those cases — error reporting
/// is `PathItem::validate_with_context`'s responsibility).
fn resolve_path_item_chain<'a>(spec: &'a Spec, item: &'a PathItem) -> &'a PathItem {
    let mut current = item;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(r) = current.reference.as_deref() {
        if r.is_empty() || !seen.insert(r.to_owned()) {
            return current;
        }
        let Some(next) = find_path_item_by_ref(spec, r) else {
            return current;
        };
        current = next;
    }
    current
}

fn find_path_item_by_ref<'a>(spec: &'a Spec, reference: &str) -> Option<&'a PathItem> {
    let unescape = |s: &str| s.replace("~1", "/").replace("~0", "~");
    if let Some(after) = reference.strip_prefix("#/paths/") {
        if after.contains('/') {
            return None;
        }
        spec.paths.as_ref()?.paths.get(&unescape(after))
    } else if let Some(after) = reference.strip_prefix("#/webhooks/") {
        if after.contains('/') {
            return None;
        }
        spec.webhooks.as_ref()?.paths.get(&unescape(after))
    } else if let Some(after) = reference.strip_prefix("#/components/pathItems/") {
        if after.contains('/') {
            return None;
        }
        spec.components
            .as_ref()?
            .path_items
            .as_ref()?
            .get(&unescape(after))
    } else if let Some(after) = reference.strip_prefix("#/components/callbacks/") {
        let mut split = after.splitn(2, '/');
        let (Some(cb_token), Some(expr_token)) = (split.next(), split.next()) else {
            return None;
        };
        if expr_token.contains('/') {
            return None;
        }
        let cb_name = unescape(cb_token);
        let expr = unescape(expr_token);
        let cb_ref = spec
            .components
            .as_ref()?
            .callbacks
            .as_ref()?
            .get(&cb_name)?;
        let cb = cb_ref.get_item(spec).ok()?;
        cb.paths.get(&expr)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_1::components::Components;
    use crate::v3_1::parameter::{InCookie, InHeader, InPath, InQuery};
    use crate::v3_1::security_scheme::{
        AuthorizationCodeOAuth2Flow, ImplicitOAuth2Flow, OAuth2Flows, OAuth2SecurityScheme,
        OpenIdConnectSecurityScheme, SecurityScheme,
    };
    use crate::v3_1::spec::Spec;
    use crate::validation::Context;
    use crate::validation::ValidationErrorsExt;

    fn path_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(InPath {
            name: name.into(),
            description: None,
            required: true,
            deprecated: None,
            style: None,
            explode: None,
            schema: None,
            example: None,
            examples: None,
            content: None,
            extensions: None,
        }))
    }

    fn query_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(InQuery {
            name: name.into(),
            description: None,
            required: None,
            deprecated: None,
            allow_empty_value: None,
            style: None,
            explode: None,
            allow_reserved: None,
            schema: None,
            example: None,
            examples: None,
            content: None,
            extensions: None,
        }))
    }

    fn header_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Header(InHeader {
            name: name.into(),
            description: None,
            required: None,
            deprecated: None,
            style: None,
            explode: None,
            schema: None,
            example: None,
            examples: None,
            content: None,
            extensions: None,
        }))
    }

    fn cookie_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Cookie(InCookie {
            name: name.into(),
            description: None,
            required: None,
            deprecated: None,
            style: None,
            explode: None,
            schema: None,
            example: None,
            examples: None,
            content: None,
            extensions: None,
        }))
    }

    fn spec_with_components(c: Components) -> Spec {
        Spec {
            components: Some(c),
            ..Default::default()
        }
    }

    #[test]
    fn duplicate_param_within_level_flagged() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![query_param("q"), query_param("q")];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("duplicate parameter `q`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn cookie_and_header_locations_distinct() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![header_param("session"), cookie_param("session")];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        assert!(
            ctx.errors.is_empty(),
            "different locations should not duplicate: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn template_var_missing_path_param() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, None);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("template variable `{id}`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn path_param_without_template_var() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![path_param("id")];
        validate_operation_parameters(&mut ctx, "op", "/no-vars", None, Some(&params));
        assert!(
            ctx.errors.iter().any(|e| e
                .contains("path parameter `id` does not match any `{name}` in the path template")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn equivalent_templates_flagged() {
        let mut paths: BTreeMap<String, PathItem> = BTreeMap::new();
        paths.insert("/pets/{id}".into(), PathItem::default());
        paths.insert("/pets/{name}".into(), PathItem::default());
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_path_template_uniqueness(&mut ctx, "#.paths", &paths);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("collapse to the same shape")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn duplicate_tag_names_flagged() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let tags = vec![
            Tag {
                name: "pets".into(),
                ..Default::default()
            },
            Tag {
                name: "pets".into(),
                ..Default::default()
            },
        ];
        validate_tag_uniqueness(&mut ctx, &tags);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("duplicate tag name `pets`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_oauth2_undefined_scope() {
        let flows = OAuth2Flows {
            implicit: Some(ImplicitOAuth2Flow {
                authorization_url: "https://x.example/auth".into(),
                refresh_url: None,
                scopes: BTreeMap::from([("read".to_owned(), "Read".to_owned())]),
                extensions: None,
            }),
            ..Default::default()
        };
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "o".to_owned(),
            RefOr::new_item(SecurityScheme::OAuth2(Box::new(OAuth2SecurityScheme {
                flows,
                description: None,
                extensions: None,
            }))),
        );
        let comp = Components {
            security_schemes: Some(schemes),
            ..Default::default()
        };
        let spec: &'static Spec = Box::leak(Box::new(spec_with_components(comp)));
        let mut ctx = Context::new(spec, Options::new());
        let mut req: BTreeMap<String, Vec<String>> = BTreeMap::new();
        req.insert("o".to_owned(), vec!["write".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("scope `write` not declared")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_apikey_with_role_array_accepted_in_3_1() {
        // Per OAS 3.1, non-oauth2 schemes MAY carry role-name arrays.
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "ak".to_owned(),
            RefOr::new_item(SecurityScheme::ApiKey(Box::default())),
        );
        let comp = Components {
            security_schemes: Some(schemes),
            ..Default::default()
        };
        let spec: &'static Spec = Box::leak(Box::new(spec_with_components(comp)));
        let mut ctx = Context::new(spec, Options::new());
        let mut req: BTreeMap<String, Vec<String>> = BTreeMap::new();
        req.insert("ak".to_owned(), vec!["admin".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors.is_empty(),
            "non-empty role names should be allowed in 3.1: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_mutual_tls_with_role_array_accepted() {
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "mtls".to_owned(),
            RefOr::new_item(SecurityScheme::MutualTLS(Box::default())),
        );
        let comp = Components {
            security_schemes: Some(schemes),
            ..Default::default()
        };
        let spec: &'static Spec = Box::leak(Box::new(spec_with_components(comp)));
        let mut ctx = Context::new(spec, Options::new());
        let mut req: BTreeMap<String, Vec<String>> = BTreeMap::new();
        req.insert("mtls".to_owned(), vec!["operator".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn security_openidconnect_accepts_any_scopes() {
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "oid".to_owned(),
            RefOr::new_item(SecurityScheme::OpenIdConnect(Box::new(
                OpenIdConnectSecurityScheme {
                    open_id_connect_url: "https://x.example/.well-known".into(),
                    description: None,
                    extensions: None,
                },
            ))),
        );
        let comp = Components {
            security_schemes: Some(schemes),
            ..Default::default()
        };
        let spec: &'static Spec = Box::leak(Box::new(spec_with_components(comp)));
        let mut ctx = Context::new(spec, Options::new());
        let mut req: BTreeMap<String, Vec<String>> = BTreeMap::new();
        req.insert("oid".to_owned(), vec!["openid".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn security_missing_scheme_reported() {
        let comp = Components {
            security_schemes: Some(BTreeMap::new()),
            ..Default::default()
        };
        let spec: &'static Spec = Box::leak(Box::new(spec_with_components(comp)));
        let mut ctx = Context::new(spec, Options::new());
        let mut req: BTreeMap<String, Vec<String>> = BTreeMap::new();
        req.insert("missing".to_owned(), vec![]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors.mentions("not declared"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn external_ref_suppresses_template_correspondence_under_ignore_external() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let params: Vec<RefOr<Parameter>> = vec![RefOr::new_ref(
            "https://other.example/spec.yaml#/components/parameters/PetId".to_owned(),
        )];

        let mut ctx = Context::new(spec, Options::new());
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, Some(&params));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("template variable `{id}`")),
            "errors: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(spec, Options::IgnoreExternalReferences.only());
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, Some(&params));
        assert!(
            ctx.errors.iter().all(|e| !e.contains("template variable")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_path_item_follows_internal_ref_chain() {
        // A `paths` entry with only `$ref` set must still drive the
        // path-template ↔ parameter check via the resolved target.
        use crate::v3_1::operation::Operation;
        use crate::v3_1::parameter::{InPath, Parameter};
        use crate::v3_1::response::{Response, Responses};

        let target = PathItem {
            operations: Some(BTreeMap::from([(
                "get".to_owned(),
                Operation {
                    parameters: Some(vec![RefOr::new_item(Parameter::Path(InPath {
                        name: "wrong".into(),
                        description: None,
                        required: true,
                        deprecated: None,
                        style: None,
                        explode: None,
                        schema: None,
                        example: None,
                        examples: None,
                        content: Some(BTreeMap::from([(
                            "application/json".to_owned(),
                            crate::v3_1::media_type::MediaType::default(),
                        )])),
                        extensions: None,
                    }))]),
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
                },
            )])),
            ..Default::default()
        };
        let mut cp = BTreeMap::new();
        cp.insert("Reusable".to_owned(), target);
        let comp = Components {
            path_items: Some(cp),
            ..Default::default()
        };
        let spec: &'static Spec = Box::leak(Box::new(Spec {
            components: Some(comp),
            ..Default::default()
        }));

        // Caller mounts the reusable item under template `/users/{id}`,
        // so the `wrong` parameter should be flagged.
        let item = PathItem {
            reference: Some("#/components/pathItems/Reusable".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(spec, Options::new());
        validate_path_item(&mut ctx, "/users/{id}", "#.paths[/users/{id}]", &item);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("template variable `{id}`") || e.contains("parameter `wrong`")),
            "expected param-mismatch report after chain follow: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn authorization_code_flow_is_smoke_compiled() {
        // Construction sanity for one of the four oauth2 flows so we know
        // the import surface is right.
        let _ = AuthorizationCodeOAuth2Flow {
            authorization_url: "x".into(),
            token_url: "y".into(),
            refresh_url: None,
            scopes: BTreeMap::new(),
            extensions: None,
        };
    }
}
