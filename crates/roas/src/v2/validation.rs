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
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::common::helpers::validate_unique_by;
use crate::common::reference::RefOr;
use crate::common::reference::ResolveReference;
use crate::v2::operation::Operation;
use crate::v2::parameter::{InFormData, InHeader, InPath, InQuery, Parameter};
use crate::v2::path_item::PathItem;
use crate::v2::security_scheme::{SecurityScheme, SecuritySchemeOAuth2Flow};
use crate::v2::spec::Spec;
use crate::validation::Options;
use crate::validation::{Context, PushError};

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
        let mut seen: HashMap<(String, &'static str), usize> = HashMap::new();
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
    let mut merged: HashMap<(String, &'static str), Kind> = HashMap::new();
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
            // OAS 2.0 schema: each scope array is `uniqueItems: true`.
            validate_unique_by(scopes, ctx, format!("{path}: [{i}].`{name}`"), |s| {
                s.clone()
            });
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

/// Walk `security_definitions`: run each scheme's per-scheme validator
/// (URL-required-by-flow rules, etc.) and report unused schemes — those
/// not referenced by any `security` requirement at the top level or any
/// operation level — unless `Options::IgnoreUnusedSecuritySchemes` is set.
///
/// Must run AFTER `validate_security_requirements` has marked used
/// schemes via `ctx.visit("#/securityDefinitions/{name}")`.
///
/// We pre-collect the names so the `&mut Context` borrow used by each
/// `validate_with_context` call doesn't overlap with the immutable borrow
/// of `ctx.spec.security_definitions`. Each scheme is cloned once per
/// iteration (a small enum), which is cheaper than cloning the whole map.
pub fn validate_security_definitions(ctx: &mut Context<Spec>) {
    let names: Vec<String> = ctx
        .spec
        .security_definitions
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for name in names {
        let p = format!("#/securityDefinitions/{name}");
        let Some(scheme) = ctx
            .spec
            .security_definitions
            .as_ref()
            .and_then(|m| m.get(&name))
            .cloned()
        else {
            continue;
        };
        crate::validation::ValidateWithContext::validate_with_context(&scheme, ctx, p.clone());
        if !ctx.is_visited(&p) && !ctx.is_option(Options::IgnoreUnusedSecuritySchemes) {
            ctx.error(p, "unused");
        }
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
    use crate::v2::items::Items;
    use crate::v2::parameter::{
        ArrayParameter, BooleanParameter, InBody, InFormData, InHeader, InPath, InQuery,
        IntegerParameter, NumberParameter, Parameter, StringParameter,
    };
    use crate::v2::path_item::PathItem;
    use crate::v2::response::{Response, Responses};
    use crate::v2::schema::{Schema, StringSchema};
    use crate::v2::security_scheme::{
        ApiKeySecurityScheme, BasicSecurityScheme, OAuth2SecurityScheme, Scopes, SecurityScheme,
        SecuritySchemeApiKeyLocation, SecuritySchemeOAuth2Flow,
    };
    use crate::v2::spec::Spec;
    use crate::validation::Context;
    use crate::validation::Options;
    use crate::validation::ValidationErrorsExt;

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
            ctx.errors
                .mentions("path parameter `id` does not match any `{name}` in the path template"),
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
            ctx.errors.mentions("must not be empty"),
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
    fn unused_scheme_is_reported() {
        // A scheme that is not referenced from any security requirement
        // should be flagged as unused (matching v3 Components behavior).
        let mut defs = BTreeMap::new();
        defs.insert(
            "orphan".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));
        let mut ctx = Context::new(spec, Options::new());
        validate_security_definitions(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("#/securityDefinitions/orphan") && e.contains("unused")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn unused_scheme_silenced_by_option() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "orphan".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));
        let mut ctx = Context::new(spec, Options::IgnoreUnusedSecuritySchemes.only());
        validate_security_definitions(&mut ctx);
        assert!(!ctx.errors.mentions("unused"), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn used_scheme_is_not_flagged_as_unused() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "used".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        let spec = spec_with_security_definitions(defs);
        let spec: &'static Spec = Box::leak(Box::new(spec));
        let mut ctx = Context::new(spec, Options::new());
        // Mark as used, simulating what `validate_security_requirements`
        // would do when processing `Spec.security` or operation-level security.
        ctx.visit("#/securityDefinitions/used".to_owned());
        validate_security_definitions(&mut ctx);
        assert!(!ctx.errors.mentions("unused"), "errors: {:?}", ctx.errors);
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
    fn security_requirement_scope_array_is_unique() {
        // OAS 2.0: each scope array is `uniqueItems: true`.
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
        req.insert("o".to_owned(), vec!["read".to_owned(), "read".to_owned()]);
        validate_security_requirements(&mut ctx, "#.security", &[req]);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.security: [0].`o`[1]: duplicate value"),
            "errors: {:?}",
            ctx.errors
        );
    }

    // Helpers for non-String parameter variants used to exercise in_header_name,
    // in_query_name, in_path_name, and in_formdata_name for all branches.

    fn header_integer_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Header(Box::new(InHeader::Integer(
            IntegerParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn header_number_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Header(Box::new(InHeader::Number(
            NumberParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn header_boolean_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Header(Box::new(InHeader::Boolean(
            BooleanParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn header_array_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Header(Box::new(InHeader::Array(
            ArrayParameter {
                name: name.into(),
                items: Items::default(),
                ..Default::default()
            },
        ))))
    }

    fn query_integer_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(Box::new(InQuery::Integer(
            IntegerParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn query_number_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(Box::new(InQuery::Number(
            NumberParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn query_boolean_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(Box::new(InQuery::Boolean(
            BooleanParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn query_array_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(Box::new(InQuery::Array(ArrayParameter {
            name: name.into(),
            items: Items::default(),
            ..Default::default()
        }))))
    }

    fn path_integer_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath::Integer(
            IntegerParameter {
                name: name.into(),
                required: Some(true),
                ..Default::default()
            },
        ))))
    }

    fn path_number_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath::Number(NumberParameter {
            name: name.into(),
            required: Some(true),
            ..Default::default()
        }))))
    }

    fn path_boolean_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath::Boolean(
            BooleanParameter {
                name: name.into(),
                required: Some(true),
                ..Default::default()
            },
        ))))
    }

    fn path_array_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath::Array(ArrayParameter {
            name: name.into(),
            required: Some(true),
            items: Items::default(),
            ..Default::default()
        }))))
    }

    fn formdata_integer_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::FormData(Box::new(InFormData::Integer(
            IntegerParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn formdata_number_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::FormData(Box::new(InFormData::Number(
            NumberParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn formdata_boolean_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::FormData(Box::new(InFormData::Boolean(
            BooleanParameter {
                name: name.into(),
                ..Default::default()
            },
        ))))
    }

    fn formdata_array_param(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::FormData(Box::new(InFormData::Array(
            ArrayParameter {
                name: name.into(),
                items: Items::default(),
                ..Default::default()
            },
        ))))
    }

    #[test]
    fn duplicate_detection_covers_all_header_variants() {
        // Exercises in_header_name for Integer, Number, Boolean, and Array
        // variants by triggering the duplicate-detection code path.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));

        for (make, label) in [
            (
                header_integer_param as fn(&str) -> RefOr<Parameter>,
                "integer",
            ),
            (header_number_param, "number"),
            (header_boolean_param, "boolean"),
            (header_array_param, "array"),
        ] {
            let params = vec![make("h"), make("h")];
            let mut ctx = Context::new(spec, Options::new());
            validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
            assert!(
                ctx.errors.mentions("duplicate parameter `h`"),
                "{label} header duplicate not detected: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn duplicate_detection_covers_all_query_variants() {
        // Exercises in_query_name for Integer, Number, Boolean, and Array.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));

        for (make, label) in [
            (
                query_integer_param as fn(&str) -> RefOr<Parameter>,
                "integer",
            ),
            (query_number_param, "number"),
            (query_boolean_param, "boolean"),
            (query_array_param, "array"),
        ] {
            let params = vec![make("q"), make("q")];
            let mut ctx = Context::new(spec, Options::new());
            validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
            assert!(
                ctx.errors.mentions("duplicate parameter `q`"),
                "{label} query duplicate not detected: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn duplicate_detection_covers_all_path_variants() {
        // Exercises in_path_name for Integer, Number, Boolean, and Array.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));

        for (make, label) in [
            (
                path_integer_param as fn(&str) -> RefOr<Parameter>,
                "integer",
            ),
            (path_number_param, "number"),
            (path_boolean_param, "boolean"),
            (path_array_param, "array"),
        ] {
            let params = vec![make("id"), make("id")];
            let mut ctx = Context::new(spec, Options::new());
            validate_operation_parameters(&mut ctx, "op", "/{id}", None, Some(&params));
            assert!(
                ctx.errors.mentions("duplicate parameter `id`"),
                "{label} path duplicate not detected: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn duplicate_detection_covers_all_formdata_variants() {
        // Exercises in_formdata_name for Integer, Number, Boolean, Array, and File.
        use crate::v2::parameter::FileParameter;
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));

        for (make, label) in [
            (
                formdata_integer_param as fn(&str) -> RefOr<Parameter>,
                "integer",
            ),
            (formdata_number_param, "number"),
            (formdata_boolean_param, "boolean"),
            (formdata_array_param, "array"),
        ] {
            let params = vec![make("f"), make("f")];
            let mut ctx = Context::new(spec, Options::new());
            validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
            assert!(
                ctx.errors.mentions("duplicate parameter `f`"),
                "{label} formdata duplicate not detected: {:?}",
                ctx.errors
            );
        }

        // File variant
        let file_param = |name: &str| {
            RefOr::new_item(Parameter::FormData(Box::new(InFormData::File(
                FileParameter {
                    name: name.into(),
                    ..Default::default()
                },
            ))))
        };
        let params = vec![file_param("upload"), file_param("upload")];
        let mut ctx = Context::new(spec, Options::new());
        validate_operation_parameters(&mut ctx, "op", "/p", None, Some(&params));
        assert!(
            ctx.errors.mentions("duplicate parameter `upload`"),
            "file formdata duplicate not detected: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_path_item_with_no_operations_does_not_panic() {
        // Exercises the `if let Some(ops) = &item.operations` else branch
        // (path item that has no operations — the `}` at line 383 is only
        // hit when operations is None).
        let item = PathItem {
            reference: None,
            operations: None,
            parameters: None,
            extensions: None,
        };
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        let mut ctx = Context::new(spec, Options::new());
        validate_path_item(&mut ctx, "/p", "#.paths[/p]", &item);
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
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
