//! Cross-cutting v2 validation rules that span multiple objects.
//!
//! Each helper is invoked from `Spec::validate` to enforce rules from the
//! OpenAPI 2.0 spec that don't naturally fit on a single struct's
//! `ValidateWithContext` impl:
//!
//! * security_definitions / security cross-referencing
//! * Operation parameters: body/formData exclusivity, (name, in) uniqueness,
//!   path-parameter-name vs path-template correspondence
//! * Responses: at least one entry required
//! * allowEmptyValue: only meaningful on `query` / `formData` parameters

use lazy_regex::regex;
use std::collections::{BTreeMap, BTreeSet};

use crate::common::helpers::{Context, PushError};
use crate::common::reference::ResolveReference;
use crate::v2::operation::Operation;
use crate::v2::parameter::{InFormData, InHeader, InPath, InQuery, Parameter};
use crate::v2::path_item::PathItem;
use crate::v2::reference::RefOr;
use crate::v2::security_scheme::{SecurityScheme, SecuritySchemeOAuth2Flow};
use crate::v2::spec::Spec;

/// Resolve a `RefOr<Parameter>` against the spec's `#/parameters/...` pool.
fn resolve_parameter<'a>(spec: &'a Spec, p: &'a RefOr<Parameter>) -> Option<&'a Parameter> {
    match p {
        RefOr::Item(p) => Some(p),
        RefOr::Ref(r) => {
            <Spec as ResolveReference<Parameter>>::resolve_reference(spec, &r.reference)
        }
    }
}

/// (name, location-string) identity tuple for a Parameter, used for
/// duplicate detection per the v2 spec uniqueness rule.
fn parameter_identity(p: &Parameter) -> (&str, &'static str) {
    match p {
        Parameter::Body(b) => (b.name.as_str(), "body"),
        Parameter::Header(h) => (in_header_name(h), "header"),
        Parameter::Query(q) => (in_query_name(q), "query"),
        Parameter::Path(p) => (in_path_name(p), "path"),
        Parameter::FormData(f) => (in_formdata_name(f), "formData"),
    }
}

fn in_header_name(h: &InHeader) -> &str {
    match h {
        InHeader::String(p) => &p.name,
        InHeader::Integer(p) => &p.name,
        InHeader::Number(p) => &p.name,
        InHeader::Boolean(p) => &p.name,
        InHeader::Array(p) => &p.name,
    }
}
fn in_query_name(q: &InQuery) -> &str {
    match q {
        InQuery::String(p) => &p.name,
        InQuery::Integer(p) => &p.name,
        InQuery::Number(p) => &p.name,
        InQuery::Boolean(p) => &p.name,
        InQuery::Array(p) => &p.name,
    }
}
fn in_path_name(p: &InPath) -> &str {
    match p {
        InPath::String(p) => &p.name,
        InPath::Integer(p) => &p.name,
        InPath::Number(p) => &p.name,
        InPath::Boolean(p) => &p.name,
        InPath::Array(p) => &p.name,
    }
}
fn in_formdata_name(f: &InFormData) -> &str {
    match f {
        InFormData::String(p) => &p.name,
        InFormData::Integer(p) => &p.name,
        InFormData::Number(p) => &p.name,
        InFormData::Boolean(p) => &p.name,
        InFormData::Array(p) => &p.name,
        InFormData::File(p) => &p.name,
    }
}

/// Extract `{name}` placeholders from a path template.
fn path_template_variables(template: &str) -> BTreeSet<String> {
    let re = regex!(r"\{([^}]+)\}");
    re.captures_iter(template)
        .map(|c| c.get(1).unwrap().as_str().to_owned())
        .collect()
}

/// Validate operation-level parameter rules:
/// * body / formData exclusivity (at most one body; body and formData cannot coexist),
/// * (name, in) uniqueness — duplicates *within the same level* are flagged.
/// * path parameter names match `{name}` in the path template.
///
/// Per OAS v2: "If a parameter is already defined at the Path Item, the new
/// definition will override it, but can never remove it." So we first detect
/// within-level duplicates (a real spec violation), then **merge** the lists by
/// (name, in) with operation-level entries replacing path-item-level entries
/// — and only then run body/formData/path-template checks on the merged set.
///
/// Per-parameter `allowEmptyValue` location rules are enforced inside each
/// parameter's own `validate_with_context` (`must_not_allow_empty_value` for
/// `header` / `path`), not here, to avoid duplicate errors.
pub fn validate_operation_parameters(
    ctx: &mut Context<Spec>,
    op_path: &str,
    template: &str,
    path_item_params: Option<&[RefOr<Parameter>]>,
    op_params: Option<&[RefOr<Parameter>]>,
) {
    let template_vars = path_template_variables(template);

    // Within-level duplicate detection: report once per (name, in) per layer.
    let mut emit_within_level_dups = |params: &[RefOr<Parameter>], origin: &str| {
        let mut seen: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
        for (i, raw) in params.iter().enumerate() {
            let Some(p) = resolve_parameter(ctx.spec, raw) else {
                continue;
            };
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
    };
    if let Some(p) = path_item_params {
        emit_within_level_dups(p, "path-item");
    }
    if let Some(p) = op_params {
        emit_within_level_dups(p, "operation");
    }

    // Merge: keyed by (name, in). Operation-level overrides path-item-level
    // (same key replaces). We only need the kind of the *winning* parameter
    // for body/formData/path counting, so we store it as an enum tag — this
    // sidesteps lifetime issues that would arise from holding `&Parameter`
    // references across the closure / collection boundary.
    #[derive(Clone, Copy)]
    enum Kind {
        Body,
        FormData,
        Path,
        Other,
    }
    fn kind_of(p: &Parameter) -> Kind {
        match p {
            Parameter::Body(_) => Kind::Body,
            Parameter::FormData(_) => Kind::FormData,
            Parameter::Path(_) => Kind::Path,
            _ => Kind::Other,
        }
    }
    let mut merged: BTreeMap<(String, &'static str), Kind> = BTreeMap::new();
    if let Some(params) = path_item_params {
        for raw in params {
            if let Some(p) = resolve_parameter(ctx.spec, raw) {
                let (name, loc) = parameter_identity(p);
                merged.insert((name.to_owned(), loc), kind_of(p));
            }
        }
    }
    if let Some(params) = op_params {
        for raw in params {
            if let Some(p) = resolve_parameter(ctx.spec, raw) {
                let (name, loc) = parameter_identity(p);
                merged.insert((name.to_owned(), loc), kind_of(p));
            }
        }
    }

    let mut body_count = 0usize;
    let mut form_count = 0usize;
    let mut declared_path_params: BTreeSet<String> = BTreeSet::new();
    for ((name, _loc), kind) in &merged {
        match kind {
            Kind::Body => body_count += 1,
            Kind::FormData => form_count += 1,
            Kind::Path => {
                declared_path_params.insert(name.clone());
            }
            Kind::Other => {}
        }
    }

    if body_count > 1 {
        ctx.error(
            op_path.to_owned(),
            format_args!(".parameters: only one body parameter allowed, found {body_count}"),
        );
    }
    if body_count > 0 && form_count > 0 {
        ctx.error(
            op_path.to_owned(),
            "`body` and `formData` parameters cannot coexist on the same operation",
        );
    }

    // Each `{name}` in the template must have a matching `in: path` parameter.
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
    // And each path parameter must appear in the template.
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

/// Validate Spec-level security rules:
/// * Each security_definitions entry is itself walked (already done elsewhere).
/// * Top-level `security` requirements: the scheme name must exist; non-OAuth2
///   schemes must have an empty scope list; OAuth2 scopes must be defined.
/// * Operation-level `security`: same checks.
pub fn validate_security_requirements(
    ctx: &mut Context<Spec>,
    path: &str,
    requirements: &[BTreeMap<String, Vec<String>>],
) {
    let defs = ctx.spec.security_definitions.as_ref();
    for (i, req) in requirements.iter().enumerate() {
        for (name, scopes) in req {
            let Some(defs) = defs else {
                ctx.error(
                    path.to_owned(),
                    format_args!(
                        "[{i}].`{name}`: no securityDefinitions on the spec to resolve against"
                    ),
                );
                continue;
            };
            let Some(scheme) = defs.get(name) else {
                ctx.error(
                    path.to_owned(),
                    format_args!("[{i}].`{name}`: not declared in `securityDefinitions`"),
                );
                continue;
            };
            // record the scheme as visited so that "unused security scheme" detection works.
            ctx.visit(format!("#/securityDefinitions/{name}"));

            match scheme {
                SecurityScheme::OAuth2(o) => {
                    for scope in scopes {
                        if !o.scopes.scopes.contains_key(scope) {
                            ctx.error(
                                path.to_owned(),
                                format_args!(
                                    "[{i}].`{name}`: scope `{scope}` not declared in scheme's scopes"
                                ),
                            );
                        }
                    }
                    // Also flag schemes whose token/auth URL prerequisites were
                    // never even set (defense in depth — schemes' own
                    // validators should catch this, but we double-check here
                    // because security_definitions wasn't always walked).
                    let needs_auth = matches!(
                        o.flow,
                        SecuritySchemeOAuth2Flow::Implicit | SecuritySchemeOAuth2Flow::AccessCode
                    );
                    let needs_token = matches!(
                        o.flow,
                        SecuritySchemeOAuth2Flow::Password
                            | SecuritySchemeOAuth2Flow::Application
                            | SecuritySchemeOAuth2Flow::AccessCode
                    );
                    if needs_auth && o.authorization_url.is_none() {
                        ctx.error(
                            path.to_owned(),
                            format_args!(
                                "[{i}].`{name}`: scheme requires `authorizationUrl` for flow `{}`",
                                o.flow,
                            ),
                        );
                    }
                    if needs_token && o.token_url.is_none() {
                        ctx.error(
                            path.to_owned(),
                            format_args!(
                                "[{i}].`{name}`: scheme requires `tokenUrl` for flow `{}`",
                                o.flow,
                            ),
                        );
                    }
                }
                SecurityScheme::Basic(_) | SecurityScheme::ApiKey(_) => {
                    if !scopes.is_empty() {
                        ctx.error(
                            path.to_owned(),
                            format_args!(
                                "[{i}].`{name}`: non-OAuth2 scheme requirement must list no scopes"
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// Walk `security_definitions` and run each scheme's per-scheme validator
/// (URL-required-by-flow rules, etc.). Unused-scheme detection is wired up
/// from `Spec::validate` via `validate_not_visited`.
///
/// We pre-collect the names so the `&mut Context` borrow used by each
/// `validate_with_context` call doesn't overlap with the immutable borrow
/// of `ctx.spec.security_definitions`. The schemes themselves are *not*
/// cloned — they're looked up by reference per iteration.
pub fn validate_security_definitions(ctx: &mut Context<Spec>) {
    let names: Vec<String> = ctx
        .spec
        .security_definitions
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for name in names {
        let p = format!("#/securityDefinitions/{name}");
        // Snapshot the scheme into an owned value: cloning a single
        // `SecurityScheme` (a small enum) is cheaper than refactoring
        // `ValidateWithContext` to split the &mut Context borrow.
        let Some(scheme) = ctx
            .spec
            .security_definitions
            .as_ref()
            .and_then(|m| m.get(&name))
            .cloned()
        else {
            continue;
        };
        crate::common::helpers::ValidateWithContext::validate_with_context(&scheme, ctx, p);
    }
}

/// Validate operation-level + path-item parameters together with knowledge of
/// the path template. Called from `Spec::validate` for each path entry.
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
            validate_operation_security(ctx, &op_path, op);
        }
    }
}

fn validate_operation_security(ctx: &mut Context<Spec>, op_path: &str, op: &Operation) {
    if let Some(sec) = &op.security {
        validate_security_requirements(ctx, &format!("{op_path}.security"), sec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::v2::parameter::{InBody, InFormData, InPath, InQuery, Parameter, StringParameter};
    use crate::v2::path_item::PathItem;
    use crate::v2::reference::RefOr;
    use crate::v2::response::{Response, Responses};
    use crate::v2::schema::{Schema, StringSchema};
    use crate::v2::security_scheme::{
        ApiKeySecurityScheme, BasicSecurityScheme, OAuth2SecurityScheme, Scopes, SecurityScheme,
        SecuritySchemeApiKeyLocation, SecuritySchemeOAuth2Flow,
    };
    use crate::v2::spec::Spec;
    use crate::validation::Options;

    fn body_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Body(Box::new(InBody {
            name: name.into(),
            description: None,
            required: None,
            schema: RefOr::new_item(Schema::from(StringSchema::default())),
            x_examples: None,
            extensions: None,
        })))
    }

    fn formdata_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::FormData(Box::new(InFormData::String(
            StringParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn query_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(Box::new(InQuery::String(
            StringParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn path_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath::String(StringParameter {
            name: name.into(),
            required: Some(true),
            ..Default::default()
        }))))
    }

    fn path_param_aev(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath::String(StringParameter {
            name: name.into(),
            required: Some(true),
            allow_empty_value: Some(true),
            ..Default::default()
        }))))
    }

    fn spec_with_security_definitions(defs: BTreeMap<String, SecurityScheme>) -> Spec {
        Spec {
            security_definitions: Some(defs),
            ..Default::default()
        }
    }

    #[test]
    fn body_formdata_exclusivity() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![body_param("b"), formdata_param("f")];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("`body` and `formData` parameters cannot coexist")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn multiple_body_params_error() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![body_param("a"), body_param("b")];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("only one body parameter allowed")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn op_level_param_overrides_path_item_does_not_double_count() {
        // Per spec, an operation-level parameter with the same (name, in)
        // as a path-item-level parameter *overrides* it — not a duplicate
        // and not double-counted toward body / formData totals.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let path_item = vec![body_param("payload")];
        let op = vec![body_param("payload")];
        validate_operation_parameters(&mut ctx, "op", "/p", Some(&path_item), Some(&op));
        assert!(
            ctx.errors.is_empty(),
            "override should not duplicate or inflate counts: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn within_level_duplicate_still_flagged_after_merge() {
        // True within-level duplicates (two body params at the operation
        // level) remain a spec violation even with the merge step.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let op = vec![body_param("x"), body_param("x")];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&op));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("duplicate parameter `x` in `body`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn duplicate_name_in_location() {
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
    fn path_template_variable_missing_param() {
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
    fn path_param_without_template_variable() {
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
    fn path_template_correspondence_ok() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![path_param("id")];
        validate_operation_parameters(&mut ctx, "op", "/users/{id}", None, Some(&params));
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn allow_empty_value_only_for_query_or_formdata() {
        // The rule lives on each parameter's own validator (via
        // `must_not_allow_empty_value` in `parameter.rs`), so we exercise it
        // by running per-parameter validation rather than the cross-cutting
        // helper. This avoids the duplicate-error pattern flagged in PR #100
        // review.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let p = path_param_aev("id");
        p.validate_with_context(&mut ctx, "op.parameters[0]".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must not allow empty value")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn parameters_resolve_via_ref() {
        let mut spec = Spec::default();
        let p = Parameter::Query(Box::new(InQuery::String(StringParameter {
            name: "shared".into(),
            ..Default::default()
        })));
        spec.define_parameter("shared", p).unwrap();
        let spec: &'static Spec = Box::leak(Box::new(spec));

        let mut ctx = Context::new(spec, Options::new());
        let params = vec![
            RefOr::<Parameter>::new_ref("#/parameters/shared"),
            RefOr::<Parameter>::new_ref("#/parameters/shared"),
        ];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("duplicate parameter `shared`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn parameters_unresolvable_ref_skipped() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let params = vec![RefOr::<Parameter>::new_ref("#/parameters/missing")];
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        // No path-template vars, no params resolve, so no errors.
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn security_undeclared_scheme_when_no_definitions() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("foo".to_owned(), vec![]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("no securityDefinitions on the spec")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_requires_existing_scheme() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "basic".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));

        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("missing".to_owned(), vec![]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not declared in `securityDefinitions`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_basic_with_scopes_is_invalid() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "b".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));

        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("b".to_owned(), vec!["read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("non-OAuth2 scheme requirement must list no scopes")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_apikey_with_scopes_is_invalid() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "ak".to_owned(),
            SecurityScheme::ApiKey(ApiKeySecurityScheme {
                name: "X".into(),
                location: SecuritySchemeApiKeyLocation::Header,
                ..Default::default()
            }),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));

        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("ak".to_owned(), vec!["read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("non-OAuth2 scheme requirement must list no scopes")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_oauth2_undefined_scope() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "o".to_owned(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: Some("https://x.example.com/a".into()),
                token_url: None,
                scopes: Scopes::from([("read".to_owned(), "Read".to_owned())]),
                description: None,
                extensions: None,
            }),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));

        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("o".to_owned(), vec!["write".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("scope `write` not declared in scheme's scopes")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn security_oauth2_missing_token_or_auth() {
        // Implicit without authorizationUrl
        let mut defs = BTreeMap::new();
        defs.insert(
            "o".to_owned(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: None,
                token_url: None,
                scopes: Scopes::from([("read".to_owned(), "Read".to_owned())]),
                description: None,
                extensions: None,
            }),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));
        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("o".to_owned(), vec!["read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("scheme requires `authorizationUrl`")),
            "errors: {:?}",
            ctx.errors
        );

        // Password without tokenUrl
        let mut defs = BTreeMap::new();
        defs.insert(
            "o".to_owned(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Password,
                authorization_url: None,
                token_url: None,
                scopes: Scopes::from([("read".to_owned(), "Read".to_owned())]),
                description: None,
                extensions: None,
            }),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));
        let mut ctx = Context::new(spec, Options::new());
        let mut req = BTreeMap::new();
        req.insert("o".to_owned(), vec!["read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("scheme requires `tokenUrl`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_security_definitions_walks_each() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "o".to_owned(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: None,
                token_url: None,
                scopes: Scopes::default(),
                description: None,
                extensions: None,
            }),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));
        let mut ctx = Context::new(spec, Options::new());
        validate_security_definitions(&mut ctx);
        // Empty scopes + missing authorizationUrl produce errors from per-scheme validate.
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_security_definitions_none() {
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_security_definitions(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn validate_path_item_invokes_op_validators() {
        // Build a spec, path with templated path /users/{id}, an operation
        // missing the corresponding `in: path` parameter — should produce an error.
        let op = crate::v2::operation::Operation {
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
            reference: None,
            operations: Some(ops),
            parameters: None,
            extensions: None,
        };

        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_path_item(&mut ctx, "/users/{id}", "#.paths[/users/{id}]", &item);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("path template variable `{id}`")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_path_item_with_op_security() {
        // Operation has security referencing a scheme not defined.
        let mut sec_req = BTreeMap::new();
        sec_req.insert("missing".to_owned(), vec![]);
        let op = crate::v2::operation::Operation {
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            security: Some(vec![sec_req]),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("get".to_owned(), op);
        let item = PathItem {
            reference: None,
            operations: Some(ops),
            parameters: None,
            extensions: None,
        };

        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_path_item(&mut ctx, "/p", "#.paths[/p]", &item);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("no securityDefinitions on the spec")),
            "errors: {:?}",
            ctx.errors
        );
    }
}
