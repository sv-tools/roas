//! Forward conversion from OpenAPI v2 (Swagger 2.0) to OpenAPI v3.0.
//!
//! Converts a [`crate::v2::spec::Spec`] into a [`crate::v3_0::spec::Spec`] by
//! reshaping the document on-the-fly via `serde_json::Value` rather than
//! field-by-field copying. v2 and v3.0 share most of their JSON shape; the
//! transformations applied here cover the structural differences:
//!
//! * `swagger: "2.0"` → `openapi: "3.0.4"`.
//! * `host` / `basePath` / `schemes` → `servers` array (URLs assembled).
//! * Top-level `consumes` / `produces` are folded into per-operation
//!   `requestBody.content` / `response.content` media-type maps.
//! * Body parameters (`in: body`) become a sibling `requestBody` on the
//!   owning operation; non-body parameters get a nested `schema` object built
//!   from the v2 inline `type`/`format`/etc. fields, and `collectionFormat`
//!   becomes `style` + `explode`.
//! * Form parameters (`in: formData`) are gathered into a synthesised
//!   `requestBody` whose content is `application/x-www-form-urlencoded` (or
//!   `multipart/form-data` if any of them is `type: file`).
//! * Response `schema` / `examples` move into `response.content[<mime>]`
//!   driven by the operation's effective `produces` list.
//! * Response `headers` lose their typed v2 enum form and become v3.0
//!   header objects with a nested `schema`.
//! * `definitions` / top-level `parameters` / `responses` /
//!   `securityDefinitions` move into `components.{schemas|parameters|
//!   responses|securitySchemes}`. Top-level body / formData parameters
//!   migrate to `components.requestBodies` instead, and operation `$ref`s
//!   that point at them are re-routed there. All other `$ref`s are
//!   remapped accordingly.
//! * Path-item body / formData parameters are promoted to each operation
//!   under the path that has no body of its own. Path-item non-body
//!   parameters survive as v3.0 path-item parameters.
//! * Operation-level `schemes` becomes operation-level `servers`,
//!   inheriting `host`/`basePath` from the spec but overriding the
//!   scheme(s).
//! * Schema discriminators rewritten from `discriminator: "<name>"` to
//!   `discriminator: { propertyName: "<name>" }`. `x-nullable` becomes
//!   `nullable`.
//! * Security schemes: `Basic` becomes HTTP with `scheme: basic`; OAuth2
//!   `flow` becomes `flows: { <flow>: { … } }` with the v2→v3 flow rename
//!   (`application` → `clientCredentials`, `accessCode` → `authorizationCode`).
//!
//! Lossy edges deliberately accepted:
//!
//! * `collectionFormat: tsv` is dropped (no v3.0 equivalent).
//! * `allowEmptyValue` is preserved on `Query` parameters, dropped elsewhere.
//! * v2 `Items` array-of-arrays nesting becomes a recursively-built `Schema`.
//! * `discriminator` on a plain `ObjectSchema` is dropped — v3.0 carries
//!   it only on composition (`allOf` / `oneOf` / `anyOf`) shapes.
//!
//! The conversion serialises the v2 input with serde, runs the transforms,
//! and deserialises as a v3.0 spec. If the input is a valid v2 document the
//! output is a structurally valid v3.0 document; semantic regressions are
//! surfaced by `Spec::validate`.

use crate::v2::spec::Spec as V2Spec;
use crate::v3_0::spec::Spec as V3Spec;
use serde_json::{Map, Value};
use std::collections::HashSet;

impl From<V2Spec> for V3Spec {
    fn from(v2: V2Spec) -> Self {
        let mut value = serde_json::to_value(v2).expect("v2::Spec serializes");
        transform_spec(&mut value);
        serde_json::from_value(value).expect("transformed spec deserializes as v3_0::Spec")
    }
}

/// Top-level orchestration. The input is the JSON form of a v2 spec; on
/// return it is the JSON form of a v3.0 spec.
fn transform_spec(spec: &mut Value) {
    let Value::Object(obj) = spec else {
        return;
    };

    obj.remove("swagger");
    obj.insert("openapi".into(), Value::String("3.0.4".to_owned()));

    // host + basePath + schemes → servers, unless the doc already has a
    // x-servers list (Redoc extension that v2 users used as a v3 backport;
    // we promote it to native servers). Keep host / basePath available
    // for per-operation server overrides.
    let host = obj.remove("host").and_then(string);
    let base_path = obj.remove("basePath").and_then(string);
    let spec_schemes = obj
        .remove("schemes")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let spec_schemes_str: Vec<String> = spec_schemes
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    // A non-empty `x-servers` (Redoc extension) wins; empty or absent
    // falls back to assembling from host/basePath/schemes so users
    // don't lose their server info to an empty list.
    let x_servers = obj.remove("x-servers");
    let promoted_x_servers = match x_servers {
        Some(Value::Array(arr)) if !arr.is_empty() => {
            obj.insert("servers".into(), Value::Array(arr));
            true
        }
        _ => false,
    };
    if !promoted_x_servers
        && let Some(servers) =
            assemble_servers(host.as_deref(), base_path.as_deref(), &spec_schemes_str)
    {
        obj.insert("servers".into(), servers);
    }

    let spec_consumes = take_string_array(obj, "consumes");
    let spec_produces = take_string_array(obj, "produces");

    // Top-level parameters need to be split: body / formData entries
    // can't survive as v3.0 `Parameter` components (v3.0 has no body or
    // formData parameter location), so they migrate to
    // `components.requestBodies` instead. Operation `$ref`s pointing at
    // them get re-routed accordingly when we walk operations below.
    let mut body_param_names: HashSet<String> = HashSet::new();
    let mut form_param_names: HashSet<String> = HashSet::new();
    // Original (raw) v2 formData parameter definitions, keyed by name.
    // Operations with mixed inline + `$ref` form params resolve refs
    // against this map so the referenced field can be merged into the
    // operation's synthesised multipart schema instead of being lost.
    let mut form_param_defs: Map<String, Value> = Map::new();
    let mut converted_parameters: Map<String, Value> = Map::new();
    let mut request_bodies: Map<String, Value> = Map::new();
    if let Some(Value::Object(parameters)) = obj.remove("parameters") {
        for (name, value) in parameters {
            match parameter_location(&value) {
                Some("body") => {
                    body_param_names.insert(name.clone());
                    if let Some(rb) = build_body_request_body(Some(value), &spec_consumes) {
                        request_bodies.insert(name, rb);
                    }
                }
                Some("formData") => {
                    form_param_names.insert(name.clone());
                    form_param_defs.insert(name.clone(), value.clone());
                    if let Some(rb) = build_form_request_body(vec![value], &spec_consumes) {
                        request_bodies.insert(name, rb);
                    }
                }
                _ => {
                    if let Some(p) = transform_non_body_parameter(value) {
                        converted_parameters.insert(name, p);
                    }
                }
            }
        }
    }

    let definitions = obj.remove("definitions");
    let responses = obj.remove("responses");
    let security_definitions = obj.remove("securityDefinitions");

    let path_ctx = PathCtx {
        spec_consumes: &spec_consumes,
        spec_produces: &spec_produces,
        body_param_names: &body_param_names,
        form_param_names: &form_param_names,
        form_param_defs: &form_param_defs,
        host: host.as_deref(),
        base_path: base_path.as_deref(),
    };
    if let Some(paths) = obj.get_mut("paths") {
        transform_paths(paths, &path_ctx);
    }

    // Reshape the components container after walking paths so the
    // request-bodies map captures everything the spec exposes.
    let has_components = definitions.is_some()
        || !converted_parameters.is_empty()
        || responses.is_some()
        || security_definitions.is_some()
        || !request_bodies.is_empty();
    if has_components {
        let mut components = Map::new();
        if let Some(d) = definitions {
            components.insert("schemas".into(), d);
        }
        if !converted_parameters.is_empty() {
            components.insert("parameters".into(), Value::Object(converted_parameters));
        }
        if let Some(mut r) = responses {
            // Components-level responses also need their `schema` lifted
            // into a `content` map. v3.0 has no spec-level produces, so
            // default to application/json for inputs that don't pin one.
            if let Value::Object(map) = &mut r {
                for (_, resp) in map.iter_mut() {
                    transform_response(resp, &spec_produces);
                }
            }
            components.insert("responses".into(), r);
        }
        if !request_bodies.is_empty() {
            components.insert("requestBodies".into(), Value::Object(request_bodies));
        }
        if let Some(mut sd) = security_definitions {
            transform_security_definitions(&mut sd);
            components.insert("securitySchemes".into(), sd);
        }
        obj.insert("components".into(), Value::Object(components));
    }

    // Walk the entire document for cross-cutting transforms (ref
    // remapping + schema-shape rewrites). The walk is position-aware
    // so opaque payloads (`example` / `default` / `enum` / `const` /
    // ExampleObject `value`, `x-*` Specification Extensions,
    // `Link.parameters` / `Link.requestBody`) round-trip
    // byte-for-byte.
    walk(spec, &body_param_names, &form_param_names, Pos::Generic);
}

/// Context threaded into every path / operation transform. Borrowed by
/// reference so it stays out of the recursive-walk hot path.
struct PathCtx<'a> {
    spec_consumes: &'a [String],
    spec_produces: &'a [String],
    body_param_names: &'a HashSet<String>,
    form_param_names: &'a HashSet<String>,
    form_param_defs: &'a Map<String, Value>,
    host: Option<&'a str>,
    base_path: Option<&'a str>,
}

/// Build a `servers` array from v2's host/basePath/schemes, returning
/// `None` if there is no useful data (in which case v3.0 uses the default
/// "/").
fn assemble_servers(
    host: Option<&str>,
    base_path: Option<&str>,
    schemes: &[String],
) -> Option<Value> {
    let host = host.unwrap_or("").trim();
    let base = base_path.unwrap_or("").trim();
    if host.is_empty() && base.is_empty() {
        // No host or basePath to anchor the URL. Bare `https://`-style
        // entries aren't useful to downstream tooling — fall back to
        // omitting `servers` so v3.0's implicit `/` default kicks in.
        return None;
    }
    let default_schemes;
    let schemes: &[String] = if schemes.is_empty() {
        default_schemes = vec!["https".to_owned()];
        &default_schemes
    } else {
        schemes
    };
    let mut out = Vec::with_capacity(schemes.len());
    for scheme in schemes {
        let url = if host.is_empty() {
            // Relative basePath only — schemes don't apply.
            base.to_owned()
        } else {
            format!("{scheme}://{host}{base}")
        };
        let mut entry = Map::new();
        entry.insert("url".into(), Value::String(url));
        out.push(Value::Object(entry));
    }
    // If host was empty we will have produced N copies of the same
    // base-path URL, one per scheme. Dedupe to keep the output minimal.
    out.dedup();
    Some(Value::Array(out))
}

fn transform_paths(paths: &mut Value, ctx: &PathCtx<'_>) {
    let Value::Object(obj) = paths else { return };
    for (name, item) in obj.iter_mut() {
        if name.starts_with("x-") {
            continue;
        }
        let Value::Object(item_obj) = item else {
            continue;
        };
        // Path-level body / formData parameters apply to every operation
        // under the path in v2. v3.0 has no path-level requestBody, so we
        // pull them out here and let `transform_operation` use them as a
        // fallback when an operation has no body of its own.
        let mut path_body: Option<Value> = None;
        let mut path_forms: Vec<Value> = Vec::new();
        if let Some(Value::Array(parameters)) = item_obj.remove("parameters") {
            let mut new_path_params: Vec<Value> = Vec::with_capacity(parameters.len());
            for p in parameters {
                match classify_parameter(&p, ctx.body_param_names, ctx.form_param_names) {
                    ParamKind::Body => {
                        path_body = Some(p);
                    }
                    ParamKind::Form => {
                        path_forms.push(p);
                    }
                    ParamKind::Other => {
                        if let Some(rewritten) = transform_non_body_parameter(p) {
                            new_path_params.push(rewritten);
                        }
                    }
                }
            }
            if !new_path_params.is_empty() {
                item_obj.insert("parameters".into(), Value::Array(new_path_params));
            }
        }
        for (method, op) in item_obj.iter_mut() {
            if !is_http_method(method) {
                continue;
            }
            transform_operation(op, ctx, path_body.as_ref(), &path_forms);
        }
    }
}

/// What v3.0 slot a v2 parameter (inline or `$ref`) maps to.
enum ParamKind {
    Body,
    Form,
    Other,
}

fn classify_parameter(
    p: &Value,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) -> ParamKind {
    match parameter_location(p) {
        Some("body") => return ParamKind::Body,
        Some("formData") => return ParamKind::Form,
        Some(_) => return ParamKind::Other,
        None => {}
    }
    // No `in:` — must be a `$ref`. Resolve against the top-level
    // parameter name we extracted from the v2 doc.
    if let Some(name) = ref_local_name(p, "#/parameters/") {
        if body_param_names.contains(name) {
            return ParamKind::Body;
        }
        if form_param_names.contains(name) {
            return ParamKind::Form;
        }
    }
    ParamKind::Other
}

fn ref_local_name<'a>(p: &'a Value, prefix: &str) -> Option<&'a str> {
    p.get("$ref")?.as_str()?.strip_prefix(prefix)
}

fn is_http_method(name: &str) -> bool {
    matches!(
        name,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

fn transform_operation(
    op: &mut Value,
    ctx: &PathCtx<'_>,
    path_body: Option<&Value>,
    path_forms: &[Value],
) {
    let Value::Object(obj) = op else { return };

    let consumes = take_string_array(obj, "consumes");
    let consumes: Vec<String> = if consumes.is_empty() {
        ctx.spec_consumes.to_vec()
    } else {
        consumes
    };
    let produces = take_string_array(obj, "produces");
    let produces: Vec<String> = if produces.is_empty() {
        ctx.spec_produces.to_vec()
    } else {
        produces
    };

    // v2 `schemes` overrides the spec-level scheme list for this
    // operation; v3.0 has operation-level `servers` that achieve the
    // same effect.
    let op_schemes = take_string_array(obj, "schemes");
    if !op_schemes.is_empty()
        && let Some(servers) = assemble_servers(ctx.host, ctx.base_path, &op_schemes)
    {
        obj.insert("servers".into(), servers);
    }

    // Pull body and formData out before rewriting the rest.
    let mut body_param: Option<Value> = None;
    let mut form_params: Vec<Value> = Vec::new();
    let mut other_params: Vec<Value> = Vec::new();
    if let Some(Value::Array(parameters)) = obj.remove("parameters") {
        for p in parameters {
            match classify_parameter(&p, ctx.body_param_names, ctx.form_param_names) {
                ParamKind::Body => body_param = Some(p),
                ParamKind::Form => form_params.push(p),
                ParamKind::Other => other_params.push(p),
            }
        }
    }
    // Path-level body / formData fall through when the operation has
    // none of its own.
    if body_param.is_none()
        && let Some(pb) = path_body
    {
        body_param = Some(pb.clone());
    }
    // Path-level form params apply to every operation under the path
    // in v2; operation-level entries override only the same name (per
    // OAS2 §parameter resolution). Merge by name with operation-level
    // winning so unrelated path-level fields aren't lost.
    if !path_forms.is_empty() {
        form_params = merge_form_params(path_forms, form_params);
    }

    let mut new_params = Vec::with_capacity(other_params.len());
    for p in other_params {
        if let Some(rewritten) = transform_non_body_parameter(p) {
            new_params.push(rewritten);
        }
    }
    if !new_params.is_empty() {
        obj.insert("parameters".into(), Value::Array(new_params));
    }

    // A `$ref` body is moved straight to `requestBody` — `remap_refs`
    // will rewrite the path against `body_param_names` so the final
    // pointer lands in `#/components/requestBodies/`.
    if let Some(body) = body_param {
        if body.as_object().is_some_and(|o| o.contains_key("$ref")) {
            obj.insert("requestBody".into(), body);
        } else if let Some(rb) = build_body_request_body(Some(body), &consumes) {
            obj.insert("requestBody".into(), rb);
        }
    } else if !form_params.is_empty()
        && let Some(rb) =
            build_form_request_body_or_ref(form_params, &consumes, ctx.form_param_defs)
    {
        obj.insert("requestBody".into(), rb);
    }

    if let Some(responses) = obj.get_mut("responses")
        && let Value::Object(resp_obj) = responses
    {
        for (_, resp) in resp_obj.iter_mut() {
            transform_response(resp, &produces);
        }
    }
}

fn parameter_location(p: &Value) -> Option<&str> {
    p.get("in")?.as_str()
}

/// Rewrite a non-body, non-formData v2 parameter into a v3.0 parameter.
///
/// Always returns `Some`. `$ref` entries pass through untouched so the
/// global [`remap_refs`] pass can retarget the pointer; inline entries
/// have their flat type-shape fields collected into a nested `schema`
/// and any `collectionFormat` translated into `style` + `explode`.
fn transform_non_body_parameter(mut p: Value) -> Option<Value> {
    if p.is_object() && p.as_object().is_some_and(|o| o.contains_key("$ref")) {
        return Some(p);
    }
    let Value::Object(obj) = &mut p else {
        return Some(p);
    };
    let location = obj.get("in").and_then(|v| v.as_str()).map(str::to_owned);
    let collection_format = obj.remove("collectionFormat").and_then(string);
    if let Some((style, explode)) = collection_format
        .as_deref()
        .zip(location.as_deref())
        .and_then(|(cf, loc)| collection_format_to_style(cf, loc))
    {
        obj.insert("style".into(), Value::String(style.into()));
        obj.insert("explode".into(), Value::Bool(explode));
    }
    // allowEmptyValue is only valid on `query` in v3.0.
    if location.as_deref() != Some("query") {
        obj.remove("allowEmptyValue");
    }
    extract_parameter_schema(obj);
    Some(p)
}

/// Pull every type-shape field (`type`, `format`, `enum`, `items`, `min*`,
/// `max*`, `pattern`, `default`, `multipleOf`, `uniqueItems`) out of a
/// v2-style flat parameter / header / form-field map and into a nested
/// `schema` sibling.
fn extract_parameter_schema(obj: &mut Map<String, Value>) {
    const SCHEMA_KEYS: &[&str] = &[
        "type",
        "format",
        "enum",
        "items",
        "default",
        "multipleOf",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
        "uniqueItems",
    ];
    let mut schema = Map::new();
    for k in SCHEMA_KEYS {
        if let Some(v) = obj.remove(*k) {
            schema.insert((*k).into(), v);
        }
    }
    if let Some(items) = schema.get_mut("items") {
        transform_items(items);
    }
    if !schema.is_empty() {
        obj.insert("schema".into(), Value::Object(schema));
    }
}

/// Recursively turn a v2 `Items` (a flat `{type,format,items,...}`) into a
/// schema-shaped `{type,format,items: <Schema>, ...}`. The shape is already
/// close enough that we just need to recurse into nested `items`.
fn transform_items(items: &mut Value) {
    let Value::Object(obj) = items else { return };
    obj.remove("collectionFormat");
    if let Some(inner) = obj.get_mut("items") {
        transform_items(inner);
    }
}

fn collection_format_to_style(cf: &str, location: &str) -> Option<(&'static str, bool)> {
    // v3.0 restricts `style` per location:
    //   path:   matrix | label | simple
    //   header: simple
    //   query / cookie: form | spaceDelimited | pipeDelimited | deepObject
    // Anything outside the location's allowed set would make the
    // converted JSON fail to deserialize as a v3_0::Parameter, so we
    // fall back to `simple` for non-query / non-cookie sites.
    match (cf, location) {
        ("csv", "query" | "cookie") => Some(("form", false)),
        ("csv", _) => Some(("simple", false)),
        ("ssv", "query" | "cookie") => Some(("spaceDelimited", false)),
        ("pipes", "query" | "cookie") => Some(("pipeDelimited", false)),
        ("multi", "query" | "cookie") => Some(("form", true)),
        // ssv / pipes / multi outside query|cookie → use the only
        // location-valid style and keep the array semantics on
        // `explode`.
        ("ssv" | "pipes", _) => Some(("simple", false)),
        ("multi", _) => Some(("simple", true)),
        // tsv has no v3.0 equivalent.
        _ => None,
    }
}

/// Wrap each `(name, raw value)` pair from a v2 `x-examples` map (or
/// any similarly-shaped name → value table) as a v3.0 Example Object
/// (`{value: <raw>}`). v3.0's `MediaType.examples` is typed
/// `BTreeMap<String, RefOr<Example>>` so a bare JSON value would fail
/// to deserialize.
fn wrap_named_values_as_examples(map: Map<String, Value>) -> Value {
    let wrapped: Map<String, Value> = map
        .into_iter()
        .map(|(name, raw)| {
            let mut example = Map::new();
            example.insert("value".into(), raw);
            (name, Value::Object(example))
        })
        .collect();
    Value::Object(wrapped)
}

/// Build a `requestBody` from a single v2 body parameter. Returns `None`
/// when there is no body parameter.
fn build_body_request_body(body: Option<Value>, consumes: &[String]) -> Option<Value> {
    let body = body?;
    // Inline-only path: $ref bodies are routed by the caller (operation
    // transform stores the ref directly under `requestBody`; the global
    // remap step retargets the pointer).
    if body.as_object().is_some_and(|o| o.contains_key("$ref")) {
        return Some(body);
    }

    let Value::Object(mut p) = body else {
        return None;
    };
    let description = p.remove("description");
    let required = p.remove("required");
    let schema = p.remove("schema");
    // v2's `x-examples` is `BTreeMap<String, serde_json::Value>` —
    // bare values, not Example Objects. Wrap each entry as
    // `{value: <original>}` so the result deserializes against
    // `MediaType.examples: BTreeMap<String, RefOr<Example>>`.
    let examples = p.remove("x-examples").and_then(|v| match v {
        Value::Object(map) => Some(wrap_named_values_as_examples(map)),
        _ => None,
    });
    let mut content = Map::new();
    let mut mime_types = if consumes.is_empty() {
        vec!["application/json".to_owned()]
    } else {
        consumes.to_vec()
    };
    // The last media-type entry takes ownership of `schema` / `examples`
    // instead of cloning; the common single-`consumes` case clones nothing.
    let last_mime = mime_types.pop();
    for mime in mime_types {
        let mut media = Map::new();
        if let Some(s) = &schema {
            media.insert("schema".into(), s.clone());
        }
        if let Some(ex) = &examples {
            media.insert("examples".into(), ex.clone());
        }
        content.insert(mime, Value::Object(media));
    }
    if let Some(mime) = last_mime {
        let mut media = Map::new();
        if let Some(s) = schema {
            media.insert("schema".into(), s);
        }
        if let Some(ex) = examples {
            media.insert("examples".into(), ex);
        }
        content.insert(mime, Value::Object(media));
    }
    let mut out = Map::new();
    if let Some(d) = description {
        out.insert("description".into(), d);
    }
    if let Some(r) = required {
        out.insert("required".into(), r);
    }
    out.insert("content".into(), Value::Object(content));
    Some(Value::Object(out))
}

/// Synthesise a `requestBody` from a list of v2 formData parameters.
/// Pick the right shape for an operation's `requestBody` when its v2
/// formData parameters might be a mix of inline entries and `$ref`s
/// pointing at top-level formData components.
///
/// v3.0 has a single `requestBody` slot per operation, so the rules
/// are:
///
/// * If there is exactly one parameter and it is a `$ref`, route it
///   straight through. The global remap step retargets
///   `#/parameters/<n>` to `#/components/requestBodies/<n>` so the
///   operation can reuse the component. This preserves v2's
///   "reference a single file upload" idiom.
/// * Otherwise, resolve every `$ref` against the original v2 formData
///   parameter definitions and merge the resulting inline entries
///   into a single synthesised form-encoded request body. v2 permits
///   multiple formData params (inline or via refs) and v3.0 can
///   represent them all as properties on one object schema, so this
///   keeps every named field instead of dropping any of them.
fn build_form_request_body_or_ref(
    form_params: Vec<Value>,
    consumes: &[String],
    form_param_defs: &Map<String, Value>,
) -> Option<Value> {
    if form_params.len() == 1
        && form_params[0]
            .as_object()
            .is_some_and(|o| o.contains_key("$ref"))
    {
        return form_params.into_iter().next();
    }
    let mut resolved = Vec::with_capacity(form_params.len());
    for p in form_params {
        match p.as_object() {
            Some(o) if o.contains_key("$ref") => {
                if let Some(name) = ref_local_name(&p, "#/parameters/")
                    && let Some(def) = form_param_defs.get(name)
                {
                    resolved.push(def.clone());
                }
                // A ref that doesn't resolve in `form_param_defs` is
                // pointing at a non-formData parameter (or an
                // unresolvable name) and can't merge into a multipart
                // schema — drop it rather than panic on the ambiguity.
            }
            _ => resolved.push(p),
        }
    }
    build_form_request_body(resolved, consumes)
}

/// Merge two v2 formData parameter lists by `name`, with `overrides`
/// winning over `base`. Used to apply v2's "operation parameters
/// override path-level by (name, in)" rule on the formData slice while
/// still keeping path-level fields the operation hasn't redefined.
fn merge_form_params(base: &[Value], overrides: Vec<Value>) -> Vec<Value> {
    let override_names: HashSet<String> = overrides
        .iter()
        .filter_map(|p| {
            // For inline params the key is `name`. For `$ref` params we
            // use the local component name as the dedupe key — refs
            // from the override list shadow path-level entries pointing
            // at the same component too.
            p.get("name")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .or_else(|| ref_local_name(p, "#/parameters/").map(str::to_owned))
        })
        .collect();
    let mut out = Vec::with_capacity(base.len() + overrides.len());
    for p in base {
        let key = p
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| ref_local_name(p, "#/parameters/").map(str::to_owned));
        if let Some(k) = key
            && override_names.contains(&k)
        {
            continue;
        }
        out.push(p.clone());
    }
    out.extend(overrides);
    out
}

fn build_form_request_body(form_params: Vec<Value>, consumes: &[String]) -> Option<Value> {
    if form_params.is_empty() {
        return None;
    }
    let any_file = form_params
        .iter()
        .any(|p| p.get("type").and_then(|v| v.as_str()) == Some("file"));
    let mime_types: Vec<String> = if !consumes.is_empty() {
        consumes.to_vec()
    } else if any_file {
        vec!["multipart/form-data".to_owned()]
    } else {
        vec!["application/x-www-form-urlencoded".to_owned()]
    };

    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in form_params {
        let Value::Object(mut p_obj) = p else {
            continue;
        };
        let name = match p_obj.remove("name").and_then(string) {
            Some(n) => n,
            None => continue,
        };
        if p_obj.remove("required").and_then(|v| v.as_bool()) == Some(true) {
            required.push(Value::String(name.clone()));
        }
        // Strip parameter-only fields and treat what remains as a Schema.
        // `description` survives — it's a valid Schema keyword and the
        // single most useful piece of v2 metadata to keep on each
        // multipart property.
        for k in &["in", "allowEmptyValue", "collectionFormat"] {
            p_obj.remove(*k);
        }
        if p_obj.get("type").and_then(|v| v.as_str()) == Some("file") {
            // multipart form file → schema { type: string, format: binary }.
            p_obj.insert("type".into(), Value::String("string".into()));
            p_obj.insert("format".into(), Value::String("binary".into()));
        }
        if let Some(items) = p_obj.get_mut("items") {
            transform_items(items);
        }
        properties.insert(name, Value::Object(p_obj));
    }

    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }

    let mut content = Map::new();
    let mut mime_types = mime_types;
    // Last entry moves `schema` in; the single-`consumes` case clones nothing.
    let last_mime = mime_types.pop();
    for mime in mime_types {
        let mut media = Map::new();
        media.insert("schema".into(), Value::Object(schema.clone()));
        content.insert(mime, Value::Object(media));
    }
    if let Some(mime) = last_mime {
        let mut media = Map::new();
        media.insert("schema".into(), Value::Object(schema));
        content.insert(mime, Value::Object(media));
    }

    let mut out = Map::new();
    out.insert("content".into(), Value::Object(content));
    Some(Value::Object(out))
}

fn transform_response(resp: &mut Value, produces: &[String]) {
    if resp.as_object().is_some_and(|o| o.contains_key("$ref")) {
        return;
    }
    let Value::Object(obj) = resp else { return };

    // v2 response.headers is a map from name to a typed header enum; v3.0
    // expects a Header object with a nested schema.
    if let Some(Value::Object(headers)) = obj.get_mut("headers") {
        for (_, h) in headers.iter_mut() {
            transform_header(h);
        }
    }

    let schema = obj.remove("schema");
    let examples = obj.remove("examples");
    if schema.is_some() || examples.is_some() {
        let mime_types = if !produces.is_empty() {
            produces.to_vec()
        } else {
            vec!["application/json".to_owned()]
        };
        let mut content = Map::new();
        // v2 examples is a map { mime: value }. We attach each example to
        // its matching media-type entry; for media types that have no
        // example, we still create the entry from the schema if any.
        let example_map = match examples {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        for mime in &mime_types {
            let mut media = Map::new();
            if let Some(s) = &schema {
                media.insert("schema".into(), s.clone());
            }
            if let Some(ex) = example_map.get(mime) {
                media.insert("example".into(), ex.clone());
            }
            content.insert(mime.clone(), Value::Object(media));
        }
        // Surface any examples whose MIME type wasn't in `produces` by
        // adding them as additional content entries.
        for (mime, ex) in &example_map {
            if !content.contains_key(mime) {
                let mut media = Map::new();
                if let Some(s) = &schema {
                    media.insert("schema".into(), s.clone());
                }
                media.insert("example".into(), ex.clone());
                content.insert(mime.clone(), Value::Object(media));
            }
        }
        if !content.is_empty() {
            obj.insert("content".into(), Value::Object(content));
        }
    }
}

fn transform_header(header: &mut Value) {
    if header.as_object().is_some_and(|o| o.contains_key("$ref")) {
        return;
    }
    let Value::Object(obj) = header else { return };
    obj.remove("collectionFormat");
    extract_parameter_schema(obj);
}

fn transform_security_definitions(value: &mut Value) {
    let Value::Object(map) = value else { return };
    for (_, scheme) in map.iter_mut() {
        let Value::Object(s) = scheme else { continue };
        match s.get("type").and_then(|v| v.as_str()) {
            Some("basic") => {
                s.insert("type".into(), Value::String("http".into()));
                s.insert("scheme".into(), Value::String("basic".into()));
            }
            Some("oauth2") => {
                let flow = s.remove("flow").and_then(string);
                let auth_url = s.remove("authorizationUrl");
                let token_url = s.remove("tokenUrl");
                let scopes = s
                    .remove("scopes")
                    .unwrap_or_else(|| Value::Object(Map::new()));
                let flow_key = match flow.as_deref() {
                    Some("application") => "clientCredentials",
                    Some("accessCode") => "authorizationCode",
                    Some("implicit") => "implicit",
                    Some("password") => "password",
                    _ => "implicit",
                };
                let mut flow_obj = Map::new();
                if let Some(u) = auth_url {
                    flow_obj.insert("authorizationUrl".into(), u);
                }
                if let Some(u) = token_url {
                    flow_obj.insert("tokenUrl".into(), u);
                }
                flow_obj.insert("scopes".into(), scopes);
                let mut flows = Map::new();
                flows.insert(flow_key.into(), Value::Object(flow_obj));
                s.insert("flows".into(), Value::Object(flows));
            }
            _ => {}
        }
    }
}

/// Position of the current node relative to OAS structural
/// boundaries. Threading this through the walker keeps the
/// schema-only rewrites (`x-nullable` → `nullable`, string
/// `discriminator` → `{propertyName}` object) and the `$ref`
/// remapping from touching opaque user payloads while still
/// reaching every real sub-schema.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pos {
    /// Not yet inside a schema or Link. Refs are still remapped; the
    /// walker watches for `schema` / `schemas` / `links` to transition.
    Generic,
    /// The current object IS a Schema. Apply schema rewrites and
    /// recurse into sub-schemas while skipping instance-valued
    /// JSON-Schema keywords (`example`/`examples`/`default`/`enum`/
    /// `const`).
    Schema,
    /// The current object is a `BTreeMap<String, Schema>` (e.g.
    /// `components.schemas`, `properties`). Each entry's value is a
    /// schema.
    SchemaMap,
    /// The current object IS a Link Object. `parameters` and
    /// `requestBody` hold arbitrary JSON and are not walked.
    Link,
    /// The current object is a `BTreeMap<String, Link>` (e.g.
    /// `components.links`, `Response.links`). Each entry's value is
    /// a Link.
    LinkMap,
}

fn walk(
    value: &mut Value,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
    pos: Pos,
) {
    match value {
        Value::Object(obj) => match pos {
            Pos::Schema => walk_schema_object(obj, body_param_names, form_param_names),
            Pos::SchemaMap => {
                for (_, v) in obj.iter_mut() {
                    walk(v, body_param_names, form_param_names, Pos::Schema);
                }
            }
            Pos::Link => walk_link_object(obj, body_param_names, form_param_names),
            Pos::LinkMap => {
                for (_, v) in obj.iter_mut() {
                    walk(v, body_param_names, form_param_names, Pos::Link);
                }
            }
            Pos::Generic => walk_generic_object(obj, body_param_names, form_param_names),
        },
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                walk(v, body_param_names, form_param_names, pos);
            }
        }
        _ => {}
    }
}

fn walk_schema_object(
    obj: &mut Map<String, Value>,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) {
    remap_ref_in_place(obj, body_param_names, form_param_names);
    // `x-nullable` → `nullable`.
    if let Some(v) = obj.remove("x-nullable") {
        obj.insert("nullable".into(), v);
    }
    // v2 discriminator is a string; v3.0 expects an object.
    if let Some(Value::String(name)) = obj.get("discriminator").cloned() {
        let mut d = Map::new();
        d.insert("propertyName".into(), Value::String(name));
        obj.insert("discriminator".into(), Value::Object(d));
    }
    for (k, v) in obj.iter_mut() {
        if is_extension_key(k) {
            continue;
        }
        match k.as_str() {
            // Schema instance-valued keywords carry arbitrary user
            // JSON, never sub-schemas.
            "example" | "examples" | "default" | "enum" | "const" => continue,
            "items"
            | "not"
            | "additionalProperties"
            // `additionalItems` is the draft-04 tuple-tail keyword
            // — a single sub-schema (or boolean schema) describing
            // items past the tuple prefix. JSON Schema 2020-12
            // dropped it in favour of `prefixItems` + `items`; v2
            // uses the draft-04 form. Walk as Schema so the
            // rewrites reach its body.
            | "additionalItems"
            | "contains"
            | "propertyNames"
            | "if"
            | "then"
            | "else"
            | "unevaluatedItems"
            | "unevaluatedProperties" => walk(v, body_param_names, form_param_names, Pos::Schema),
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                walk(v, body_param_names, form_param_names, Pos::Schema)
            }
            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
                walk(v, body_param_names, form_param_names, Pos::SchemaMap)
            }
            // Draft-04 `dependencies` is a hybrid map: each entry's
            // value is *either* an array of property names (instance
            // data) *or* a sub-schema. Inspect at runtime and walk
            // object-shaped entries as Schema; array-shaped entries
            // are skipped.
            "dependencies" => walk_dependencies(v, body_param_names, form_param_names),
            _ => walk(v, body_param_names, form_param_names, Pos::Generic),
        }
    }
}

/// Walker for draft-04 `dependencies`. Each entry's value is either
/// an array of property names (skip) or a sub-schema (walk).
fn walk_dependencies(
    value: &mut Value,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) {
    let Value::Object(map) = value else { return };
    for (_, entry) in map.iter_mut() {
        if let Value::Object(_) = entry {
            walk(entry, body_param_names, form_param_names, Pos::Schema);
        }
        // Array form (`["a", "b"]`) is a property-name list — skip.
    }
}

fn walk_generic_object(
    obj: &mut Map<String, Value>,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) {
    remap_ref_in_place(obj, body_param_names, form_param_names);
    for (k, v) in obj.iter_mut() {
        if is_extension_key(k) {
            continue;
        }
        match k.as_str() {
            // `schema` lives on Parameter / Header / MediaType; its
            // value is a Schema Object.
            "schema" => walk(v, body_param_names, form_param_names, Pos::Schema),
            // `schemas` is the components-level map of named schemas.
            "schemas" => walk(v, body_param_names, form_param_names, Pos::SchemaMap),
            // `links` is a map of named Link Objects.
            "links" => walk(v, body_param_names, form_param_names, Pos::LinkMap),
            // ExampleObject's instance value, and the example /
            // examples carriers on MediaType / Parameter / Header.
            // Skip recursion entirely — none of these hold schemas.
            "example" | "examples" | "value" => continue,
            _ => walk(v, body_param_names, form_param_names, Pos::Generic),
        }
    }
}

/// Walk a Link Object's keys. `parameters` (a `Map<String, runtime-
/// expression>`) and `requestBody` (free-form runtime expression)
/// hold opaque user payloads and must round-trip byte-for-byte.
fn walk_link_object(
    obj: &mut Map<String, Value>,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) {
    remap_ref_in_place(obj, body_param_names, form_param_names);
    for (k, v) in obj.iter_mut() {
        if is_extension_key(k) {
            continue;
        }
        match k.as_str() {
            "parameters" | "requestBody" => continue,
            _ => walk(v, body_param_names, form_param_names, Pos::Generic),
        }
    }
}

fn remap_ref_in_place(
    obj: &mut Map<String, Value>,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) {
    if let Some(Value::String(s)) = obj.get_mut("$ref") {
        *s = remap_ref_path(s, body_param_names, form_param_names);
    }
}

/// OAS / JSON Schema Specification Extension prefix. The walker
/// skips recursion through `x-*` keys so user-supplied extension
/// payloads round-trip byte-for-byte.
fn is_extension_key(k: &str) -> bool {
    k.starts_with("x-")
}

fn remap_ref_path(
    s: &str,
    body_param_names: &HashSet<String>,
    form_param_names: &HashSet<String>,
) -> String {
    if let Some(rest) = s.strip_prefix("#/parameters/") {
        if body_param_names.contains(rest) || form_param_names.contains(rest) {
            return format!("#/components/requestBodies/{rest}");
        }
        return format!("#/components/parameters/{rest}");
    }
    // Order matters: longer prefixes first so we don't shadow shorter ones.
    const MAPPINGS: &[(&str, &str)] = &[
        ("#/definitions/", "#/components/schemas/"),
        ("#/responses/", "#/components/responses/"),
        ("#/securityDefinitions/", "#/components/securitySchemes/"),
    ];
    for (old, new) in MAPPINGS {
        if let Some(rest) = s.strip_prefix(old) {
            return format!("{new}{rest}");
        }
    }
    s.to_owned()
}

fn string(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        _ => None,
    }
}

fn take_string_array(obj: &mut Map<String, Value>, key: &str) -> Vec<String> {
    match obj.remove(key) {
        Some(Value::Array(arr)) => arr.into_iter().filter_map(string).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::spec::Spec as V2Spec;
    use crate::v3_0::spec::Spec as V3Spec;
    use crate::validation::Validate;

    fn v2_from_json(s: &str) -> V2Spec {
        serde_json::from_str(s).expect("v2 spec parses")
    }

    /// Convert every checked-in v2 test fixture and assert the result
    /// validates as a structurally valid v3.0 spec. This is the closest
    /// thing to an integration test for the conversion: it covers a
    /// petstore variant tree (with examples, refs, body params, headers)
    /// plus the Uber example (large path catalogue, OAuth2).
    #[test]
    fn all_v2_fixtures_convert_to_valid_v3_0() {
        let fixtures: &[(&str, &str)] = &[
            (
                "petstore_minimal",
                include_str!("../../tests/v2_data/petstore_minimal.json"),
            ),
            (
                "petstore-simple",
                include_str!("../../tests/v2_data/petstore-simple.json"),
            ),
            (
                "petstore-with-external-docs",
                include_str!("../../tests/v2_data/petstore-with-external-docs.json"),
            ),
            (
                "petstore_expanded",
                include_str!("../../tests/v2_data/petstore_expanded.json"),
            ),
            (
                "petstore",
                include_str!("../../tests/v2_data/petstore.json"),
            ),
            (
                "petstore_full",
                include_str!("../../tests/v2_data/petstore_full.json"),
            ),
            (
                "api_with_examples",
                include_str!("../../tests/v2_data/api_with_examples.json"),
            ),
            ("uber", include_str!("../../tests/v2_data/uber.json")),
        ];
        for (name, raw) in fixtures {
            let v2: V2Spec =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{name}: parse: {e}"));
            let v3: V3Spec = v2.into();
            assert_eq!(v3.openapi.as_str(), "3.0.4", "{name} openapi version");
            // Some v2 fixtures use tags on operations without declaring
            // them at the spec level, and the unused-* validators are
            // strict. Conversion preserves shape, not semantic gaps in
            // the source spec — allow them.
            let opts = crate::validation::Options::new()
                | crate::validation::Options::IgnoreMissingTags
                | crate::validation::IGNORE_UNUSED;
            if let Err(e) = v3.validate(opts, None) {
                panic!("{name}: converted spec did not validate cleanly:\n{e}");
            }
        }
    }

    #[test]
    fn petstore_minimal_round_trips_to_valid_v3_0() {
        let v2: V2Spec = v2_from_json(include_str!("../../tests/v2_data/petstore_minimal.json"));
        let v3: V3Spec = v2.into();
        // openapi version landed.
        assert_eq!(v3.openapi.as_str(), "3.0.4");
        // host + basePath + schemes assembled into servers.
        let servers = v3.servers.as_ref().expect("servers populated");
        assert!(
            servers
                .iter()
                .any(|s| s.url == "http://petstore.swagger.io/api")
        );
        // definitions moved into components.schemas.
        let components = v3.components.as_ref().expect("components populated");
        let schemas = components.schemas.as_ref().expect("schemas populated");
        assert!(schemas.contains_key("Pet"));
        // The inline `$ref: "#/definitions/Pet"` in the response body
        // has been rewritten and the response now has a content map with
        // application/json driven by the operation's `produces`.
        let _ = v3.paths.iter().next().expect("at least one path");
        // The result is structurally valid v3.0.
        assert!(
            v3.validate(Default::default(), None).is_ok(),
            "converted spec must validate clean"
        );
    }

    #[test]
    fn body_parameter_becomes_request_body() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/pets": {
                    "post": {
                        "consumes": ["application/json"],
                        "parameters": [{
                            "in": "body",
                            "name": "pet",
                            "required": true,
                            "schema": { "$ref": "#/definitions/Pet" }
                        }],
                        "responses": { "201": { "description": "ok" } }
                    }
                }
            },
            "definitions": {
                "Pet": { "type": "object" }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let post = &value["paths"]["/pets"]["post"];
        assert!(post.get("parameters").is_none(), "body param removed");
        let request_body = &post["requestBody"];
        assert_eq!(request_body["required"], Value::Bool(true));
        let schema_ref = &request_body["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(schema_ref, "#/components/schemas/Pet");
    }

    #[test]
    fn form_data_becomes_url_encoded_request_body() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/login": {
                    "post": {
                        "parameters": [
                            {"in":"formData","name":"username","type":"string","required":true},
                            {"in":"formData","name":"password","type":"string","required":true}
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let post = &value["paths"]["/login"]["post"];
        assert!(
            post["parameters"].is_null(),
            "formData removed from parameters"
        );
        let content = &post["requestBody"]["content"]["application/x-www-form-urlencoded"];
        assert_eq!(content["schema"]["type"], "object");
        assert_eq!(
            content["schema"]["properties"]["username"]["type"],
            "string"
        );
        let required = content["schema"]["required"].as_array().unwrap();
        assert!(required.contains(&Value::String("username".into())));
    }

    #[test]
    fn form_data_with_file_uses_multipart() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "parameters": [
                            {"in":"formData","name":"file","type":"file"}
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let content = &value["paths"]["/upload"]["post"]["requestBody"]["content"];
        let media = &content["multipart/form-data"]["schema"]["properties"]["file"];
        assert_eq!(media["type"], "string");
        assert_eq!(media["format"], "binary");
    }

    #[test]
    fn query_parameter_gets_nested_schema_and_style() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "tags",
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "multi"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items"]["get"]["parameters"][0];
        assert_eq!(p["in"], "query");
        assert_eq!(p["schema"]["type"], "array");
        assert_eq!(p["schema"]["items"]["type"], "string");
        assert_eq!(p["style"], "form");
        assert_eq!(p["explode"], true);
        assert!(p.get("type").is_none(), "type folded into schema");
        assert!(
            p.get("collectionFormat").is_none(),
            "collectionFormat removed"
        );
    }

    #[test]
    fn response_schema_and_examples_become_content_map() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "produces": ["application/json", "application/xml"],
            "paths": {
                "/x": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "schema": {"type": "string"},
                                "examples": {
                                    "application/json": "hi",
                                    "text/plain": "plain"
                                }
                            }
                        }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let content = &value["paths"]["/x"]["get"]["responses"]["200"]["content"];
        assert_eq!(content["application/json"]["schema"]["type"], "string");
        assert_eq!(content["application/xml"]["schema"]["type"], "string");
        assert_eq!(content["application/json"]["example"], "hi");
        // Off-list example surfaces too.
        assert_eq!(content["text/plain"]["example"], "plain");
    }

    #[test]
    fn schema_example_default_payloads_are_preserved_byte_for_byte() {
        // Schema-instance-valued keywords carry arbitrary user JSON
        // (the keys below mirror v2 schema shape on purpose:
        // `x-nullable`, string `discriminator`, `{$ref: …}`). The
        // position-aware walker skips recursion into them when inside
        // a Schema so they round-trip byte-for-byte.
        //
        // Exercise the walker directly on hand-built JSON since v2's
        // typed `Schema` drops fields the variant doesn't declare
        // (e.g. `enum` on `ObjectSchema`) at the deserialization step.
        let mut v: Value = serde_json::json!({
            "type": "object",
            "example": {
                "x-nullable": true,
                "discriminator": "kind",
                "$ref": "#/definitions/Inner"
            },
            "default": {
                "x-nullable": false,
                "$ref": "#/definitions/Inner"
            },
            "enum": [
                {"x-nullable": true, "$ref": "#/definitions/Other"}
            ]
        });
        let body: HashSet<String> = HashSet::new();
        let form: HashSet<String> = HashSet::new();
        super::walk(&mut v, &body, &form, super::Pos::Schema);
        // Example payload: every field preserved verbatim — no
        // x-nullable rename, no discriminator object form, no $ref
        // remap.
        assert_eq!(v["example"]["x-nullable"], true);
        assert_eq!(v["example"]["discriminator"], "kind");
        assert_eq!(v["example"]["$ref"], "#/definitions/Inner");
        // Default and enum payloads same story.
        assert_eq!(v["default"]["x-nullable"], false);
        assert_eq!(v["default"]["$ref"], "#/definitions/Inner");
        assert_eq!(v["enum"][0]["x-nullable"], true);
        assert_eq!(v["enum"][0]["$ref"], "#/definitions/Other");
    }

    #[test]
    fn additional_items_and_dependencies_sub_schemas_get_rewrites() {
        // Draft-04 `additionalItems` is a single sub-schema; draft-04
        // `dependencies` is a per-entry hybrid map (array of names OR
        // sub-schema). Both must receive the schema-shape rewrites
        // (`x-nullable` → `nullable`, string `discriminator` →
        // object) when their values are schemas.
        let mut v: Value = serde_json::json!({
            "type": "object",
            "additionalItems": {
                "type": "string",
                "x-nullable": true
            },
            "dependencies": {
                // Array form: an instance-data property-name list.
                // Stays verbatim.
                "kind": ["name", "tag"],
                // Schema form: receives the schema rewrites.
                "extras": {
                    "type": "object",
                    "x-nullable": true,
                    "discriminator": "category"
                }
            }
        });
        let body: HashSet<String> = HashSet::new();
        let form: HashSet<String> = HashSet::new();
        super::walk(&mut v, &body, &form, super::Pos::Schema);
        // additionalItems sub-schema: x-nullable → nullable.
        assert_eq!(v["additionalItems"]["nullable"], true);
        assert!(v["additionalItems"].get("x-nullable").is_none());
        // dependencies.extras: schema rewrites applied.
        let extras = &v["dependencies"]["extras"];
        assert_eq!(extras["nullable"], true);
        assert_eq!(extras["discriminator"]["propertyName"], "category");
        assert!(extras.get("x-nullable").is_none());
        // dependencies.kind: array of property names — untouched.
        assert_eq!(
            v["dependencies"]["kind"],
            serde_json::json!(["name", "tag"])
        );
    }

    #[test]
    fn x_extension_payload_is_opaque_to_walker() {
        // `x-*` Specification Extensions are user JSON; the walker
        // must not apply schema rewrites or `$ref` remapping to
        // anything inside them.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "definitions": {
                "Doc": {
                    "type": "object",
                    "x-trap": {
                        "x-nullable": true,
                        "discriminator": "kind",
                        "$ref": "#/definitions/Inner"
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let trap = &value["components"]["schemas"]["Doc"]["x-trap"];
        assert_eq!(trap["x-nullable"], true);
        assert_eq!(trap["discriminator"], "kind");
        assert_eq!(trap["$ref"], "#/definitions/Inner");
    }

    #[test]
    fn responses_default_status_code_still_walks_normally() {
        // `default` is a JSON Schema instance-valued keyword inside a
        // schema, but it's also a status-code key in the Responses
        // object. The Generic-position walker must not treat
        // `default` as opaque outside of a schema — its $ref needs
        // remapping. Pin this regression down: the converted spec
        // must validate (which requires the ref to resolve).
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "produces": ["application/json"],
                        "responses": {
                            "default": {
                                "description": "err",
                                "schema": {"$ref": "#/definitions/Err"}
                            }
                        }
                    }
                }
            },
            "definitions": {"Err": {"type": "object"}}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let schema_ref = &value["paths"]["/x"]["get"]["responses"]["default"]["content"]["application/json"]
            ["schema"]["$ref"];
        assert_eq!(schema_ref, "#/components/schemas/Err");
    }

    #[test]
    fn ref_paths_are_remapped_to_components() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "parameters": [{"$ref": "#/parameters/limit"}],
                        "responses": {
                            "default": {"$ref": "#/responses/Err"}
                        }
                    }
                }
            },
            "parameters": {
                "limit": {"in": "query", "name": "limit", "type": "integer"}
            },
            "responses": {
                "Err": {"description": "err", "schema": {"$ref": "#/definitions/Err"}}
            },
            "definitions": {
                "Err": {"type": "object"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        assert_eq!(
            value["paths"]["/x"]["get"]["parameters"][0]["$ref"],
            "#/components/parameters/limit"
        );
        assert_eq!(
            value["paths"]["/x"]["get"]["responses"]["default"]["$ref"],
            "#/components/responses/Err"
        );
        // Nested ref inside the lifted `content[<mime>].schema`: the
        // global `remap_refs` walk runs after `transform_response` has
        // moved the schema under `content`, so a `$ref` to a v2
        // definition still gets retargeted to `#/components/schemas/`.
        let err_resp = &value["components"]["responses"]["Err"];
        let schema_ref = &err_resp["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(schema_ref, "#/components/schemas/Err");
    }

    #[test]
    fn top_level_non_body_parameter_gets_nested_schema() {
        // Reusable v2 query parameter must keep its `type/items/...`
        // collected into a `schema` so v3.0 can deserialize it.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{"$ref": "#/parameters/Limit"}],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            },
            "parameters": {
                "Limit": {
                    "in": "query",
                    "name": "limit",
                    "type": "integer",
                    "format": "int32"
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let limit = &value["components"]["parameters"]["Limit"];
        assert_eq!(limit["in"], "query");
        assert_eq!(limit["schema"]["type"], "integer");
        assert_eq!(limit["schema"]["format"], "int32");
        assert!(limit.get("type").is_none(), "type folded into schema");
        // The operation's $ref keeps targeting the parameter (not body).
        let p_ref = &value["paths"]["/items"]["get"]["parameters"][0]["$ref"];
        assert_eq!(p_ref, "#/components/parameters/Limit");
    }

    #[test]
    fn top_level_body_parameter_becomes_request_body_component() {
        // Reusable v2 body parameter must end up in
        // `components.requestBodies`, not `components.parameters`, and
        // operation `$ref`s pointing at it must move out of `parameters`
        // into `requestBody`.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/pets": {
                    "post": {
                        "parameters": [{"$ref": "#/parameters/PetBody"}],
                        "responses": { "201": { "description": "ok" } }
                    }
                }
            },
            "parameters": {
                "PetBody": {
                    "in": "body",
                    "name": "pet",
                    "required": true,
                    "schema": {"$ref": "#/definitions/Pet"}
                }
            },
            "definitions": {"Pet": {"type": "object"}}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // Pet body lives in components.requestBodies, not parameters.
        assert!(
            value["components"]["parameters"]["PetBody"].is_null(),
            "body must NOT land in components.parameters"
        );
        let pet_body = &value["components"]["requestBodies"]["PetBody"];
        assert_eq!(pet_body["required"], true);
        let schema_ref = &pet_body["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(schema_ref, "#/components/schemas/Pet");
        // Operation: parameters dropped, requestBody is a $ref to the
        // requestBodies component.
        let post = &value["paths"]["/pets"]["post"];
        assert!(
            post["parameters"].is_null(),
            "body $ref removed from parameters"
        );
        assert_eq!(
            post["requestBody"]["$ref"],
            "#/components/requestBodies/PetBody"
        );
    }

    #[test]
    fn form_data_ref_becomes_request_body_ref() {
        // A v2 operation that references a top-level formData parameter
        // by `$ref` must end up with `requestBody: {$ref: …}` pointing
        // at the synthesised `components.requestBodies/<name>` entry —
        // not an empty form-encoded body.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "parameters": [{"$ref": "#/parameters/File"}],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            },
            "parameters": {
                "File": {"in": "formData", "name": "file", "type": "file"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // The component is built once.
        let file_rb = &value["components"]["requestBodies"]["File"];
        let props = &file_rb["content"]["multipart/form-data"]["schema"]["properties"];
        assert_eq!(props["file"]["format"], "binary");
        // The operation references it.
        let post = &value["paths"]["/upload"]["post"];
        assert!(post["parameters"].is_null());
        assert_eq!(
            post["requestBody"]["$ref"],
            "#/components/requestBodies/File"
        );
    }

    #[test]
    fn mixed_inline_and_ref_form_params_keep_every_field() {
        // v2 permits an operation to use both an inline formData param
        // and a `$ref` to a top-level formData component. v3.0 can
        // represent them together as a single multipart schema, so the
        // referenced field MUST be inlined into the operation body
        // rather than dropped.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "post": {
                        "parameters": [
                            {"$ref": "#/parameters/File"},
                            {"in": "formData", "name": "meta", "type": "string"}
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            },
            "parameters": {
                "File": {"in": "formData", "name": "file", "type": "file"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let post = &value["paths"]["/upload"]["post"];
        // An inline form is present, so the operation body is the
        // synthesised one — not a $ref to the component.
        assert!(post["requestBody"].get("$ref").is_none());
        let props = &post["requestBody"]["content"]["multipart/form-data"]["schema"]["properties"];
        assert_eq!(props["meta"]["type"], "string");
        // The referenced file field is inlined too.
        assert_eq!(props["file"]["type"], "string");
        assert_eq!(props["file"]["format"], "binary");
    }

    #[test]
    fn path_level_form_field_merges_with_operation_form_field() {
        // Path-level `file` plus operation-level `meta` — both must
        // survive on the operation's body. Operation-level overrides
        // are by `name` only; unrelated path-level fields stay.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "parameters": [
                        {"in": "formData", "name": "file", "type": "file"}
                    ],
                    "post": {
                        "parameters": [
                            {"in": "formData", "name": "meta", "type": "string"}
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let body = &value["paths"]["/upload"]["post"]["requestBody"];
        // Multipart because at least one field is `type: file`.
        let props = &body["content"]["multipart/form-data"]["schema"]["properties"];
        assert_eq!(props["meta"]["type"], "string");
        assert_eq!(props["file"]["format"], "binary");
    }

    #[test]
    fn operation_form_overrides_path_level_form_by_name() {
        // Path-level `tag: string` and operation-level `tag: integer`
        // share the same name; only the operation's version survives.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "parameters": [
                        {"in": "formData", "name": "tag", "type": "string"}
                    ],
                    "post": {
                        "parameters": [
                            {"in": "formData", "name": "tag", "type": "integer"}
                        ],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let props = &value["paths"]["/x"]["post"]["requestBody"]["content"]["application/x-www-form-urlencoded"]
            ["schema"]["properties"];
        assert_eq!(props["tag"]["type"], "integer");
    }

    #[test]
    fn path_level_form_ref_promotes_to_each_operation() {
        // Path-level $ref forms must propagate to each operation under
        // the path that has no body of its own, same as path-level body
        // and inline form parameters do.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/upload": {
                    "parameters": [{"$ref": "#/parameters/File"}],
                    "post": {
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            },
            "parameters": {
                "File": {"in": "formData", "name": "file", "type": "file"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        assert_eq!(
            value["paths"]["/upload"]["post"]["requestBody"]["$ref"],
            "#/components/requestBodies/File"
        );
    }

    #[test]
    fn path_level_body_promotes_to_each_operation() {
        // Path-item parameters apply to every operation under the path
        // in v2. v3.0 has no path-level requestBody, so the body must
        // promote to each operation that doesn't have one of its own.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items/{id}": {
                    "parameters": [
                        {"in": "path", "name": "id", "type": "string", "required": true},
                        {"in": "body", "name": "patch", "schema": {"type": "object"}}
                    ],
                    "put": {
                        "responses": { "200": { "description": "ok" } }
                    },
                    "post": {
                        "parameters": [
                            {"in": "body", "name": "create", "schema": {"type": "string"}}
                        ],
                        "responses": { "201": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // PUT inherits the path-level body.
        let put = &value["paths"]["/items/{id}"]["put"];
        assert_eq!(
            put["requestBody"]["content"]["application/json"]["schema"]["type"],
            "object"
        );
        // POST keeps its own body and ignores the path-level fallback.
        let post = &value["paths"]["/items/{id}"]["post"];
        assert_eq!(
            post["requestBody"]["content"]["application/json"]["schema"]["type"],
            "string"
        );
        // Path-level path parameter survives as path-level v3.0 parameter.
        let path_params = &value["paths"]["/items/{id}"]["parameters"];
        assert_eq!(path_params[0]["name"], "id");
        assert_eq!(path_params[0]["schema"]["type"], "string");
    }

    #[test]
    fn schemes_without_host_or_base_path_omits_servers() {
        // Without a host or basePath there's nothing for the scheme to
        // anchor. Returning a bare `https://` URL would mislead tooling,
        // so we drop `servers` entirely and let v3.0's implicit `/`
        // default apply.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "schemes": ["https"],
            "paths": {}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        assert!(
            v3.servers.is_none(),
            "no servers expected, got {:?}",
            v3.servers
        );
    }

    #[test]
    fn base_path_only_assembles_one_relative_server() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "basePath": "/v1",
            "schemes": ["https", "http"],
            "paths": {}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let servers = v3.servers.as_ref().expect("servers populated");
        assert_eq!(servers.len(), 1, "deduped to a single relative entry");
        assert_eq!(servers[0].url, "/v1");
    }

    #[test]
    fn operation_level_schemes_become_operation_servers() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v1",
            "schemes": ["https"],
            "paths": {
                "/secure": {
                    "get": {
                        "schemes": ["wss"],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // Spec-level servers reflect the spec-level https scheme.
        let spec_url = &value["servers"][0]["url"];
        assert_eq!(spec_url, "https://api.example.com/v1");
        // Operation-level servers override with wss.
        let op_url = &value["paths"]["/secure"]["get"]["servers"][0]["url"];
        assert_eq!(op_url, "wss://api.example.com/v1");
    }

    #[test]
    fn security_basic_becomes_http_basic() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "auth": {"type": "basic"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let scheme = &value["components"]["securitySchemes"]["auth"];
        assert_eq!(scheme["type"], "http");
        // `HttpScheme::Basic` deserialises from the v2 "basic" alias and
        // re-serialises in its canonical "Basic" form.
        assert_eq!(scheme["scheme"], "Basic");
    }

    #[test]
    fn security_oauth2_flow_becomes_flows_object() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "oauth": {
                    "type": "oauth2",
                    "flow": "accessCode",
                    "authorizationUrl": "https://example.com/auth",
                    "tokenUrl": "https://example.com/token",
                    "scopes": {"read": "read access"}
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let scheme = &value["components"]["securitySchemes"]["oauth"];
        assert_eq!(scheme["type"], "oauth2");
        let flow = &scheme["flows"]["authorizationCode"];
        assert_eq!(flow["authorizationUrl"], "https://example.com/auth");
        assert_eq!(flow["tokenUrl"], "https://example.com/token");
        assert_eq!(flow["scopes"]["read"], "read access");
    }

    #[test]
    fn schema_x_nullable_becomes_nullable() {
        // v2 keeps `discriminator: <name>` on plain ObjectSchema; v3.0
        // only carries it on composition types, so a discriminator on an
        // Object is intentionally lossy on conversion. The rename of
        // `x-nullable` to `nullable` is the structural rewrite that this
        // test pins down.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "definitions": {
                "Pet": {
                    "type": "object",
                    "x-nullable": true,
                    "properties": {"id": {"type": "integer"}}
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let pet = &value["components"]["schemas"]["Pet"];
        assert_eq!(pet["nullable"], true);
        assert!(pet.get("x-nullable").is_none());
    }

    #[test]
    fn allof_discriminator_string_becomes_object() {
        // v2 `discriminator: <name>` on the composition form turns into
        // v3.0 `discriminator: { propertyName: <name> }`.
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "definitions": {
                "Cat": {
                    "allOf": [
                        {"$ref": "#/definitions/Pet"},
                        {"type": "object", "properties": {"meow": {"type": "string"}}}
                    ],
                    "discriminator": "kind"
                },
                "Pet": {"type": "object"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let cat = &value["components"]["schemas"]["Cat"];
        assert_eq!(cat["discriminator"]["propertyName"], "kind");
    }

    /// x-servers (Redoc extension) wins over host/basePath/schemes when
    /// non-empty; an empty x-servers array falls back to the assembled
    /// host+basePath+schemes list.
    #[test]
    fn x_servers_non_empty_wins_over_assembled_servers() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v1",
            "schemes": ["https"],
            "x-servers": [{"url": "https://override.example.com"}],
            "paths": {}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let servers = v3.servers.as_ref().expect("servers populated");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].url, "https://override.example.com");
    }

    /// assemble_servers: when no schemes are provided, defaults to https.
    #[test]
    fn assemble_servers_defaults_to_https_when_no_schemes() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "host": "api.example.com",
            "basePath": "/v2",
            "paths": {}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let servers = v3.servers.as_ref().expect("servers populated");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].url, "https://api.example.com/v2");
    }

    /// Paths object: x- extension keys are skipped during path iteration.
    #[test]
    fn x_extension_key_in_paths_is_skipped() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "x-internal-note": "some extension",
                "/real": {
                    "get": {
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        // Must not panic; the x- key is not a path item and should be skipped.
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // The real path is present.
        assert!(value["paths"]["/real"]["get"].is_object());
    }

    /// Top-level `responses` component entries also get their schema
    /// lifted into a `content` map (line 192 in transform_spec).
    #[test]
    fn top_level_responses_component_gets_content_map() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "produces": ["application/json"],
            "responses": {
                "ErrorResp": {
                    "description": "an error",
                    "schema": {"$ref": "#/definitions/Error"}
                }
            },
            "definitions": {"Error": {"type": "object"}}
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let comp_resp = &value["components"]["responses"]["ErrorResp"];
        assert_eq!(comp_resp["description"], "an error");
        let schema_ref = &comp_resp["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(schema_ref, "#/components/schemas/Error");
    }

    /// Body parameter with description, required, and x-examples.
    #[test]
    fn body_param_with_description_and_x_examples_produces_complete_request_body() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/pets": {
                    "post": {
                        "parameters": [{
                            "in": "body",
                            "name": "pet",
                            "description": "A pet",
                            "required": true,
                            "schema": {"type": "object"},
                            "x-examples": {
                                "cat": {"name": "Whiskers"}
                            }
                        }],
                        "responses": { "201": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let rb = &value["paths"]["/pets"]["post"]["requestBody"];
        assert_eq!(rb["description"], "A pet");
        assert_eq!(rb["required"], true);
        // x-examples wrapped as Example Objects.
        let examples = &rb["content"]["application/json"]["examples"];
        assert_eq!(examples["cat"]["value"]["name"], "Whiskers");
    }

    /// transform_non_body_parameter: when collectionFormat is `ssv` outside
    /// query/cookie it falls back to `simple`.
    #[test]
    fn collection_format_ssv_on_path_becomes_simple() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items/{tags}": {
                    "get": {
                        "parameters": [{
                            "in": "path",
                            "name": "tags",
                            "required": true,
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "ssv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items/{tags}"]["get"]["parameters"][0];
        assert_eq!(p["style"], "simple");
        assert_eq!(p["explode"], false);
    }

    /// collectionFormat `pipes` on query becomes `pipeDelimited`.
    #[test]
    fn collection_format_pipes_on_query_becomes_pipe_delimited() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "tags",
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "pipes"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items"]["get"]["parameters"][0];
        assert_eq!(p["style"], "pipeDelimited");
    }

    /// collectionFormat `multi` on a non-query/cookie location falls back to `simple`.
    #[test]
    fn collection_format_multi_on_header_becomes_simple() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "parameters": [{
                            "in": "header",
                            "name": "X-Tags",
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "multi"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/x"]["get"]["parameters"][0];
        assert_eq!(p["style"], "simple");
        assert_eq!(p["explode"], true);
    }

    /// collectionFormat `csv` on query becomes `form`.
    #[test]
    fn collection_format_csv_on_query_becomes_form() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "tags",
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "csv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items"]["get"]["parameters"][0];
        assert_eq!(p["style"], "form");
        assert_eq!(p["explode"], false);
    }

    /// collectionFormat `csv` on a path/header becomes `simple`.
    #[test]
    fn collection_format_csv_on_path_becomes_simple() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items/{id}": {
                    "get": {
                        "parameters": [{
                            "in": "path",
                            "name": "id",
                            "required": true,
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "csv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items/{id}"]["get"]["parameters"][0];
        assert_eq!(p["style"], "simple");
    }

    /// collectionFormat `ssv` on query becomes `spaceDelimited`.
    #[test]
    fn collection_format_ssv_on_query_becomes_space_delimited() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "tags",
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "ssv"
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items"]["get"]["parameters"][0];
        assert_eq!(p["style"], "spaceDelimited");
    }

    /// transform_items: deeply nested `items` has collectionFormat stripped
    /// recursively.
    #[test]
    fn nested_items_collectionformat_stripped() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/items": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "tags",
                            "type": "array",
                            "collectionFormat": "csv",
                            "items": {
                                "type": "array",
                                "collectionFormat": "csv",
                                "items": {"type": "string"}
                            }
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/items"]["get"]["parameters"][0];
        // No collectionFormat at any nesting level.
        assert!(p["schema"]["items"].get("collectionFormat").is_none());
    }

    /// Response header with $ref is passed through unchanged.
    #[test]
    fn response_header_ref_passes_through() {
        // V2 Header is a `type`-tagged enum, so a $ref-only header cannot go
        // through the typed model. Use the raw JSON transform instead.
        let mut v: serde_json::Value = serde_json::json!({
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "produces": ["application/json"],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "headers": {
                                    "X-Rate-Limit": {
                                        "$ref": "#/x-headerDefs/rateLimit"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        super::transform_spec(&mut v);
        let header = &v["paths"]["/x"]["get"]["responses"]["200"]["headers"]["X-Rate-Limit"];
        // $ref is preserved verbatim through transform_header.
        assert!(header.get("$ref").is_some());
    }

    /// oauth2 with `implicit` flow maps to the `implicit` key.
    #[test]
    fn oauth2_implicit_flow_preserved() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "oauth": {
                    "type": "oauth2",
                    "flow": "implicit",
                    "authorizationUrl": "https://example.com/auth",
                    "scopes": {"read": "read access"}
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let scheme = &value["components"]["securitySchemes"]["oauth"];
        assert!(scheme["flows"]["implicit"].is_object());
        assert_eq!(
            scheme["flows"]["implicit"]["authorizationUrl"],
            "https://example.com/auth"
        );
    }

    /// oauth2 with `password` flow maps to the `password` key.
    #[test]
    fn oauth2_password_flow_preserved() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "oauth": {
                    "type": "oauth2",
                    "flow": "password",
                    "tokenUrl": "https://example.com/token",
                    "scopes": {"write": "write access"}
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let scheme = &value["components"]["securitySchemes"]["oauth"];
        assert!(scheme["flows"]["password"].is_object());
        assert_eq!(
            scheme["flows"]["password"]["tokenUrl"],
            "https://example.com/token"
        );
    }

    /// oauth2 with `application` flow maps to `clientCredentials`.
    #[test]
    fn oauth2_application_flow_becomes_client_credentials() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "oauth": {
                    "type": "oauth2",
                    "flow": "application",
                    "tokenUrl": "https://example.com/token",
                    "scopes": {"admin": "admin"}
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let scheme = &value["components"]["securitySchemes"]["oauth"];
        assert!(scheme["flows"]["clientCredentials"].is_object());
    }

    /// oauth2 with no flow field falls back to `implicit` (the default arm
    /// of the `match flow.as_deref()` in `transform_security_definitions`).
    #[test]
    fn oauth2_missing_flow_falls_back_to_implicit() {
        // Build the JSON directly (bypassing the v2 typed model which requires
        // a valid `flow` enum value) so we exercise the `_ => "implicit"` arm.
        use crate::v2::spec::Spec as V2Spec;
        let mut v: serde_json::Value = serde_json::json!({
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "securityDefinitions": {
                "oauth": {
                    "type": "oauth2",
                    "scopes": {}
                }
            }
        });
        // Transform on the raw JSON so the `flow` absence hits `_ => "implicit"`.
        super::transform_spec(&mut v);
        let scheme = &v["components"]["securitySchemes"]["oauth"];
        assert!(scheme["flows"]["implicit"].is_object());
    }

    /// Links in responses go through Pos::Link so `parameters` and
    /// `requestBody` payloads are opaque (not walked for $ref remapping).
    #[test]
    fn link_object_parameters_and_request_body_are_opaque() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/pets": {
                    "get": {
                        "produces": ["application/json"],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "schema": {"type": "object"},
                                "x-links": {
                                    "GetPetById": {
                                        "operationId": "getPet",
                                        "parameters": {
                                            "petId": "#/definitions/ShouldNotBeRemapped"
                                        },
                                        "requestBody": "#/definitions/ShouldNotBeRemapped"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // x-links is an extension — round-trips verbatim; walk doesn't touch its values.
        let link = &value["paths"]["/pets"]["get"]["responses"]["200"]["x-links"]["GetPetById"];
        assert_eq!(
            link["parameters"]["petId"],
            "#/definitions/ShouldNotBeRemapped"
        );
        assert_eq!(link["requestBody"], "#/definitions/ShouldNotBeRemapped");
    }

    /// The `links` (v3.0 native) map uses Pos::LinkMap — each entry is a Link.
    #[test]
    fn v3_links_in_response_use_link_map_position() {
        // A v3.0-style links map on a response. "links" is not a v2 field so
        // this must go through the raw JSON transform to avoid deserialization
        // failures. The walker uses Pos::LinkMap so `parameters` entries are
        // treated as opaque runtime expressions and must not be rewritten.
        let mut v: serde_json::Value = serde_json::json!({
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/pets": {
                    "get": {
                        "produces": ["application/json"],
                        "responses": {
                            "200": {
                                "description": "ok",
                                "schema": {"type": "object"},
                                "links": {
                                    "GetPetById": {
                                        "operationId": "getPet",
                                        "parameters": {
                                            "petId": "$response.body#/id"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        super::transform_spec(&mut v);
        let param = &v["paths"]["/pets"]["get"]["responses"]["200"]["links"]["GetPetById"]["parameters"]
            ["petId"];
        // The runtime expression string must not be rewritten.
        assert_eq!(param, "$response.body#/id");
    }

    /// A $ref to a non-special prefix is returned as-is by remap_ref_path.
    #[test]
    fn unmatched_ref_prefix_passes_through() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "parameters": [{"$ref": "external.yaml#/components/parameters/Limit"}],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // External ref must be preserved verbatim.
        let p = &value["paths"]["/x"]["get"]["parameters"][0]["$ref"];
        assert_eq!(p, "external.yaml#/components/parameters/Limit");
    }

    /// A `$ref` to a `#/securityDefinitions/` path gets remapped to
    /// `#/components/securitySchemes/`.
    #[test]
    fn security_definitions_ref_remapped() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "security": [{"auth": []}],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            },
            "securityDefinitions": {
                "auth": {"type": "apiKey", "name": "api_key", "in": "header"}
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        // The scheme must live under securitySchemes.
        assert!(
            value["components"]["securitySchemes"]["auth"].is_object(),
            "auth scheme must be in securitySchemes"
        );
    }

    /// form_param with `allowEmptyValue` and `collectionFormat` fields are
    /// stripped during form body synthesis.
    #[test]
    fn form_param_allowemptyvalue_and_collectionformat_stripped() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/login": {
                    "post": {
                        "parameters": [{
                            "in": "formData",
                            "name": "tags",
                            "type": "array",
                            "items": {"type": "string"},
                            "collectionFormat": "csv",
                            "allowEmptyValue": true
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let props = &value["paths"]["/login"]["post"]["requestBody"]["content"]["application/x-www-form-urlencoded"]
            ["schema"]["properties"];
        let tags = &props["tags"];
        // collectionFormat and allowEmptyValue must not appear on the schema property.
        assert!(tags.get("collectionFormat").is_none());
        assert!(tags.get("allowEmptyValue").is_none());
        assert_eq!(tags["type"], "array");
    }

    /// `allowEmptyValue` is only valid on query parameters in v3.0;
    /// it must be stripped from non-query parameters.
    #[test]
    fn allow_empty_value_stripped_from_path_param() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x/{id}": {
                    "get": {
                        "parameters": [{
                            "in": "path",
                            "name": "id",
                            "required": true,
                            "type": "string",
                            "allowEmptyValue": true
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/x/{id}"]["get"]["parameters"][0];
        assert!(
            p.get("allowEmptyValue").is_none(),
            "must be stripped from path param"
        );
    }

    /// `allowEmptyValue` is preserved on query parameters.
    #[test]
    fn allow_empty_value_kept_on_query_param() {
        let raw = r##"{
            "swagger": "2.0",
            "info": { "title": "t", "version": "1" },
            "paths": {
                "/x": {
                    "get": {
                        "parameters": [{
                            "in": "query",
                            "name": "q",
                            "type": "string",
                            "allowEmptyValue": true
                        }],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }"##;
        let v3: V3Spec = v2_from_json(raw).into();
        let value = serde_json::to_value(&v3).unwrap();
        let p = &value["paths"]["/x"]["get"]["parameters"][0];
        assert_eq!(p["allowEmptyValue"], true);
    }
}
