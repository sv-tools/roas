//! Cross-cutting v3.0 validation rules that span multiple objects.
//!
//! Each helper is invoked from `Spec::validate` to enforce rules from the
//! OpenAPI 3.0.4 spec that don't naturally fit on a single struct's
//! `ValidateWithContext` impl:
//!
//! * Parameter (name, in) uniqueness with operation-overrides-pathItem semantics
//! * Path template `{var}` ↔ `in: path` parameter correspondence
//! * Equivalent templated paths (e.g. `/pets/{id}` vs `/pets/{name}`) collisions
//! * Tag-name uniqueness in `Spec.tags`
//! * Security requirement validation (top-level + operation-level), including
//!   the OAS 3.0 rule that scope arrays must be empty for non-oauth2 /
//!   non-openIdConnect schemes.

use lazy_regex::regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::common::helpers::{Context, PushError};
use crate::common::reference::ResolveReference;
use crate::v3_0::parameter::{InCookie, InHeader, InPath, InQuery, Parameter};
use crate::v3_0::path_item::PathItem;
use crate::v3_0::reference::RefOr;
use crate::v3_0::security_scheme::SecurityScheme;
use crate::v3_0::spec::Spec;
use crate::v3_0::tag::Tag;
use crate::validation::Options;

/// Result of attempting to resolve a `RefOr<Parameter>` for cross-cutting
/// validation purposes. The distinction between an unresolved *internal* ref
/// (already a hard error elsewhere via `RefOr::validate_with_context`) and an
/// unresolved *external* ref matters: when the user has set
/// `IgnoreExternalReferences`, we must not report a missing-`in: path`
/// parameter error for a template variable that may well be defined in the
/// external document we agreed to skip.
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

/// Extract `{name}` placeholders from a path template.
fn path_template_variables(template: &str) -> BTreeSet<String> {
    let re = regex!(r"\{([^}]+)\}");
    re.captures_iter(template)
        .map(|c| c.get(1).unwrap().as_str().to_owned())
        .collect()
}

/// Reduce `/pets/{id}` and `/pets/{name}` to the same canonical form so
/// "equivalent templated paths" can be detected as a spec violation.
/// Per OAS: "Templated paths with the same hierarchy but different templated
/// names MUST NOT exist as they are identical."
fn canonical_path(template: &str) -> String {
    regex!(r"\{[^}]+\}")
        .replace_all(template, "{}")
        .into_owned()
}

/// Validate operation-level parameter rules:
/// * (name, in) duplicates *within the same level* are flagged as spec violations.
/// * Operation-level entries override path-item-level entries with the same key.
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

    // Whether we encountered any external `$ref` we couldn't follow under the
    // current Options. If so, the path-template correspondence check is
    // suppressed below — the missing path parameter may legitimately be
    // declared in the external document we chose to skip.
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

    // Merge: keyed by (name, in). Operation-level overrides path-item-level.
    // We only need the kind of the *winning* parameter for path counting,
    // so we store an enum tag — sidestepping lifetime issues that would arise
    // from holding `&Parameter` references across the collection boundary.
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

    // If we deliberately skipped any external `$ref`, a template variable
    // that looks unmatched here may legitimately be defined in the external
    // document we agreed not to follow. Suppress only the
    // `template-var-missing-parameter` direction in that case. The opposite
    // direction (a *declared* path parameter whose name is not in the
    // template) still fires unconditionally — those parameters are visible
    // locally, so the inconsistency is real regardless of external refs.
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

/// Validate a list of security requirements (used at both spec and
/// operation level). Marks each named scheme as visited (for unused-scheme
/// detection) and enforces:
/// * the named scheme exists in `components.securitySchemes`,
/// * apiKey / http schemes carry only an empty scope array,
/// * oauth2 / openIdConnect schemes' scopes (if any) are listed in some flow.
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
            // Always mark the scheme as visited even if it's missing —
            // unused-detection should not double-fault on a name we already
            // reported as missing.
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
                    // OpenID Connect resolves scopes at runtime; any list is acceptable.
                }
                SecurityScheme::ApiKey(_) | SecurityScheme::HTTP(_) => {
                    if !scopes.is_empty() {
                        ctx.error(
                            path.to_owned(),
                            format_args!(
                                "[{i}].`{name}`: scheme of type `{scheme}` requires an empty scope list, found {n}",
                                n = scopes.len()
                            ),
                        );
                    }
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

/// Validate that no two paths in `Spec.paths` collapse to the same
/// canonical (template-stripped) form. Extension keys (`x-...`) are skipped.
pub fn validate_path_template_uniqueness(
    ctx: &mut Context<Spec>,
    paths: &BTreeMap<String, PathItem>,
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
                "#.paths".to_owned(),
                format_args!(
                    "templated paths {keys:?} collapse to the same shape `{canonical}`; OAS forbids equivalent templates"
                ),
            );
        }
    }
}

/// Per-path entry point: emit cross-cutting checks that only make sense for
/// the operations actually mounted on `Spec.paths` — currently parameter
/// dedup and path-template ↔ `in: path` correspondence. Operation-level
/// `security` is intentionally NOT validated here: it runs from
/// `Operation::validate_with_context` so it also fires for operations
/// nested inside Callback path items (which this function never sees).
pub fn validate_path_item(ctx: &mut Context<Spec>, template: &str, path: &str, item: &PathItem) {
    let pi_params = item.parameters.as_deref();
    if let Some(ops) = &item.operations {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::v3_0::components::Components;
    use crate::v3_0::parameter::{InCookie, InHeader, InPath, InQuery};
    use crate::v3_0::reference::RefOr;
    use crate::v3_0::security_scheme::{
        ApiKeyLocation, ApiKeySecurityScheme, HttpScheme, HttpSecurityScheme, ImplicitOAuth2Flow,
        OAuth2Flows, OAuth2SecurityScheme, OpenIdConnectSecurityScheme, SecurityScheme,
    };
    use crate::v3_0::spec::Spec;
    use crate::validation::Options;

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
    fn op_overrides_path_item_no_dup_error() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let pi = vec![query_param("limit")];
        let op = vec![query_param("limit")];
        validate_operation_parameters(&mut ctx, "op", "/p", Some(&pi), Some(&op));
        assert!(
            ctx.errors.is_empty(),
            "override should not be a duplicate: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn cookie_and_header_locations_distinct() {
        // Same `name` is fine if `in` differs.
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
                .any(|e| e.contains("path template variable `{id}`")),
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
    fn external_ref_suppresses_template_correspondence_under_ignore_external() {
        // A parameter whose `$ref` points to an external document we have
        // chosen to skip should NOT trigger the "template variable has no
        // matching `in: path` parameter" error — the external doc may well
        // declare it.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let params: Vec<RefOr<Parameter>> = vec![RefOr::new_ref(
            "https://other.example/spec.yaml#/components/parameters/PetId",
        )];

        // Without IgnoreExternalReferences: the error fires (we have no info).
        let mut ctx = Context::new(spec, Options::new());
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, Some(&params));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("template variable `{id}`")),
            "errors: {:?}",
            ctx.errors
        );

        // With IgnoreExternalReferences: suppressed (the external doc may
        // define `id`).
        let mut ctx = Context::new(spec, Options::IgnoreExternalReferences.only());
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, Some(&params));
        assert!(
            ctx.errors.iter().all(|e| !e.contains("template variable")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn template_correspondence_ok() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![path_param("id")];
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, Some(&params));
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn equivalent_templates_are_flagged() {
        let mut paths: BTreeMap<String, PathItem> = BTreeMap::new();
        paths.insert("/pets/{id}".into(), PathItem::default());
        paths.insert("/pets/{name}".into(), PathItem::default());

        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_path_template_uniqueness(&mut ctx, &paths);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("collapse to the same shape")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn distinct_templates_are_not_flagged() {
        let mut paths: BTreeMap<String, PathItem> = BTreeMap::new();
        paths.insert("/pets/{id}".into(), PathItem::default());
        paths.insert("/pets/{id}/owner".into(), PathItem::default());
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_path_template_uniqueness(&mut ctx, &paths);
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
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
    fn security_apikey_with_scopes_invalid() {
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "ak".to_owned(),
            RefOr::new_item(SecurityScheme::ApiKey(Box::new(ApiKeySecurityScheme {
                name: "X".into(),
                location: ApiKeyLocation::Header,
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
        req.insert("ak".to_owned(), vec!["read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("requires an empty scope list")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_http_with_scopes_invalid() {
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "h".to_owned(),
            RefOr::new_item(SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
                scheme: HttpScheme::Bearer,
                bearer_format: None,
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
        req.insert("h".to_owned(), vec!["read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("requires an empty scope list")),
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
        req.insert(
            "oid".to_owned(),
            vec!["openid".to_owned(), "email".to_owned()],
        );
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors.is_empty(),
            "openIdConnect should accept any scopes: {:?}",
            ctx.errors
        );
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
            ctx.errors.iter().any(|e| e.contains("not declared")),
            "errors: {:?}",
            ctx.errors
        );
    }
}
