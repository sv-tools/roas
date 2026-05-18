//! `Spec::collapse` for OAS 2.0 — lift every inline `Schema`,
//! `Parameter`, and `Response` into the top-level `definitions`,
//! `parameters`, and `responses` bags.
//!
//! All of the heavy lifting (dedup, naming, the generic `lift_ref_or`
//! routine, the `LiftableBag` trait, the `Bag<T>` storage) lives in
//! [`crate::common::collapse`]. This module just provides the v2
//! pieces:
//!
//! * The concrete [`Collapser`] struct (one `Bag<T>` field per
//!   component type plus the loader handle).
//! * A [`LiftableBag`] impl per component type, with the per-type
//!   [`tree-walk`](LiftableBag::walk) that calls
//!   [`lift_ref_or`](crate::common::collapse::lift_ref_or) on every
//!   nested component slot.
//! * A small [`collapse_spec`] entrypoint that owns the Collapser,
//!   runs phase 1 (seed bags) + phase 2a (recurse into pre-existing
//!   bag entries) + phase 2b (walk paths), then writes each bag back.
//!
//! The v2 bags differ from v3 components in two ways:
//!
//! * They live at the *root* of the spec (`spec.definitions`,
//!   `spec.parameters`, `spec.responses`) rather than nested under
//!   `components`.
//! * They hold *bare* values (`BTreeMap<String, T>`) rather than
//!   `BTreeMap<String, RefOr<T>>` — a bag entry can't itself be a
//!   `$ref`. [`Bag<T>`] still stores entries as `RefOr<T>`
//!   internally, so this module wraps in/out of `RefOr::new_item`
//!   at the bag boundary.
//!
//! Inline `RefOr<Header>` / `RefOr<Items>` slots are walked in
//! place but never lifted — v2 has no top-level `headers` or
//! `items` bag.

use std::collections::BTreeMap;

use crate::common::bool_or::BoolOr;
use crate::common::collapse::{Bag, HasLoader, LiftableBag, NameContext, lift_ref_or};
use crate::common::reference::RefOr;
use crate::loader::Loader;
use crate::v2::parameter::Parameter;
use crate::v2::path_item::{PathItem, Paths};
use crate::v2::response::{Response, Responses};
use crate::v2::schema::{ObjectSchema, Schema};
use crate::v2::spec::Spec;

pub use crate::common::collapse::CollapseError;

// ── Collapser: per-bag state + loader handle ────────────────────────────

pub(crate) struct Collapser<'a> {
    schemas: Bag<Schema>,
    parameters: Bag<Parameter>,
    responses: Bag<Response>,
    loader: Option<&'a mut Loader>,
}

impl HasLoader for Collapser<'_> {
    fn loader_mut(&mut self) -> Option<&mut Loader> {
        self.loader.as_deref_mut()
    }
}

// ── LiftableBag impls per component type ────────────────────────────────

impl<'a> LiftableBag<Collapser<'a>> for Schema {
    const PREFIX: &'static str = "#/definitions/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.schemas
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        recurse_schema(item, ctx, c)
    }

    fn name_hint(item: &Self) -> Option<String> {
        schema_title(item).map(str::to_owned)
    }
}

impl<'a> LiftableBag<Collapser<'a>> for Parameter {
    const PREFIX: &'static str = "#/parameters/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.parameters
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        walk_parameter(item, ctx, c)
    }

    fn name_hint(item: &Self) -> Option<String> {
        let hint = parameter_name_hint(item);
        if hint.is_empty() { None } else { Some(hint) }
    }
}

impl<'a> LiftableBag<Collapser<'a>> for Response {
    const PREFIX: &'static str = "#/responses/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.responses
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        walk_response(item, ctx, c)
    }
}

// ── Walkers: per-type tree recursion ────────────────────────────────────

fn recurse_schema(
    schema: &mut Schema,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    match schema {
        Schema::AllOf(s) => {
            for (i, child) in s.all_of.iter_mut().enumerate() {
                lift_ref_or::<Schema, _>(child, ctx.push(&format!("allOf[{i}]")), c)?;
            }
        }
        Schema::Object(o) => recurse_object_schema(o.as_mut(), ctx, c)?,
        Schema::Array(a) => {
            if let Some(items) = a.items.as_mut() {
                lift_ref_or::<Schema, _>(items, ctx.push("items"), c)?;
            }
        }
        // Primitive variants (String, Integer, Number, Boolean, Null)
        // carry no nested schema slots.
        Schema::String(_)
        | Schema::Integer(_)
        | Schema::Number(_)
        | Schema::Boolean(_)
        | Schema::Null(_) => {}
    }
    Ok(())
}

fn recurse_object_schema(
    o: &mut ObjectSchema,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(props) = o.properties.as_mut() {
        for (name, child) in props.iter_mut() {
            lift_ref_or::<Schema, _>(child, ctx.push(&format!("properties.{name}")), c)?;
        }
    }
    if let Some(BoolOr::Item(s)) = o.additional_properties.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("additionalProperties"), c)?;
    }
    // `all_of` holds `RefOr<ObjectSchema>` rather than `RefOr<Schema>` —
    // there's no `definitions` slot that matches `ObjectSchema` directly,
    // so we walk inline children in place rather than lifting them.
    if let Some(all_of) = o.all_of.as_mut() {
        for (i, child) in all_of.iter_mut().enumerate() {
            if let RefOr::Item(inner) = child {
                recurse_object_schema(inner, &ctx.push(&format!("allOf[{i}]")), c)?;
            }
        }
    }
    Ok(())
}

fn walk_parameter(
    param: &mut Parameter,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    // Only the Body variant carries a schema slot. The typed variants
    // (Header / Path / Query / FormData) encode their type inline and
    // have no nested ref slots.
    if let Parameter::Body(b) = param {
        let ctx = ctx.push(b.name.as_str());
        lift_ref_or::<Schema, _>(&mut b.schema, ctx.push("schema"), c)?;
    }
    Ok(())
}

fn walk_response(
    r: &mut Response,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(s) = r.schema.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("schema"), c)?;
    }
    Ok(())
}

fn walk_responses(
    responses: &mut Responses,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(default) = responses.default.as_mut() {
        lift_ref_or::<Response, _>(default, ctx.push("default"), c)?;
    }
    if let Some(map) = responses.responses.as_mut() {
        for (status, resp) in map.iter_mut() {
            lift_ref_or::<Response, _>(resp, ctx.push(status), c)?;
        }
    }
    Ok(())
}

fn walk_path_item(
    pi: &mut PathItem,
    ctx: NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(params) = pi.parameters.as_mut() {
        for (i, p) in params.iter_mut().enumerate() {
            lift_ref_or::<Parameter, _>(p, ctx.push(&format!("parameters[{i}]")), c)?;
        }
    }
    if let Some(ops) = pi.operations.as_mut() {
        for (method, op) in ops.iter_mut() {
            walk_operation(op, ctx.push(method), c)?;
        }
    }
    Ok(())
}

fn walk_operation(
    op: &mut crate::v2::operation::Operation,
    ctx: NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    // Prefer `operationId` for naming the operation's children — it's
    // the canonical, author-chosen identifier and keeps derived names
    // stable across spec edits.
    let ctx = match op.operation_id.as_deref() {
        Some(id) if !id.is_empty() => NameContext::new([id]),
        _ => ctx,
    };
    if let Some(params) = op.parameters.as_mut() {
        for (i, p) in params.iter_mut().enumerate() {
            lift_ref_or::<Parameter, _>(p, ctx.push(&format!("parameters[{i}]")), c)?;
        }
    }
    walk_responses(&mut op.responses, &ctx.push("responses"), c)?;
    Ok(())
}

fn walk_paths(
    paths: &mut Paths,
    ctx: NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    for (path_key, pi) in paths.paths.iter_mut() {
        walk_path_item(pi, ctx.push(path_key), c)?;
    }
    Ok(())
}

// ── Orchestration ──────────────────────────────────────────────────────

pub(crate) fn collapse_spec(
    spec: &mut Spec,
    loader: Option<&mut Loader>,
) -> Result<(), CollapseError> {
    // Phase 0: take each existing root bag out of the spec. v2's
    // bags hold bare values; wrap them as `RefOr::Item` for the
    // shared `Bag<T>` storage.
    let initial_schemas = take_bare_bag(&mut spec.definitions);
    let initial_parameters = take_bare_bag(&mut spec.parameters);
    let initial_responses = take_bare_bag(&mut spec.responses);

    let mut c = Collapser {
        schemas: Bag::default(),
        parameters: Bag::default(),
        responses: Bag::default(),
        loader,
    };

    // Phase 1: seed every bag from its existing entries so dedup
    // collapses newly-lifted equivalents onto the existing names.
    c.schemas.seed(initial_schemas)?;
    c.parameters.seed(initial_parameters)?;
    c.responses.seed(initial_responses)?;

    // Phase 2a: recurse into each pre-existing inline component,
    // lifting its nested children.
    recurse_existing::<Schema>(&mut c, &["definitions"])?;
    recurse_existing::<Parameter>(&mut c, &["parameters"])?;
    recurse_existing::<Response>(&mut c, &["responses"])?;

    // Phase 2b: walk paths. `spec.paths` is required (no `Option`
    // wrapper) in v2.
    walk_paths(&mut spec.paths, NameContext::new(["paths"]), &mut c)?;

    // Phase 3: write each lifted bag back into the spec, unwrapping
    // the `RefOr::Item` shell at the boundary.
    if !c.schemas.is_empty() {
        spec.definitions = Some(unwrap_bag(c.schemas.into_map()));
    }
    if !c.parameters.is_empty() {
        spec.parameters = Some(unwrap_bag(c.parameters.into_map()));
    }
    if !c.responses.is_empty() {
        spec.responses = Some(unwrap_bag(c.responses.into_map()));
    }

    Ok(())
}

fn take_bare_bag<T>(slot: &mut Option<BTreeMap<String, T>>) -> BTreeMap<String, RefOr<T>> {
    slot.take()
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| (k, RefOr::new_item(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn unwrap_bag<T>(m: BTreeMap<String, RefOr<T>>) -> BTreeMap<String, T> {
    m.into_iter()
        .filter_map(|(k, v)| match v {
            RefOr::Item(item) => Some((k, item)),
            // v2 bag entries can't themselves be `$ref` slots — the
            // shared `Bag<T>` only ever stores `Item` variants via
            // `seed`/`intern`. A `Ref` here means a future change to
            // the shared collapse machinery started writing them and
            // would silently drop entries from the v2 output. Trip
            // the debug assertion so the regression shows up loudly
            // in tests rather than as missing definitions.
            RefOr::Ref(r) => {
                debug_assert!(
                    false,
                    "v2 bag entry `{k}` is a $ref (`{}`) but bare bags can't hold refs",
                    r.reference,
                );
                None
            }
        })
        .collect()
}

/// Generic phase-2a driver: snapshot inline names of `T`'s bag,
/// pull each out, walk via the trait's `walk`, put back with
/// refreshed canonical form.
fn recurse_existing<T>(c: &mut Collapser<'_>, ctx_root: &[&str]) -> Result<(), CollapseError>
where
    T: for<'b> LiftableBag<Collapser<'b>>,
{
    let names = T::bag(c).inline_names();
    for name in names {
        let Some(mut item) = T::bag(c).take_inline(&name) else {
            continue;
        };
        let mut parts: Vec<String> = ctx_root.iter().map(|s| (*s).to_owned()).collect();
        parts.push(name.clone());
        let ctx = NameContext::new(parts);
        T::walk(&mut item, &ctx, c)?;
        T::bag(c).put_inline(name, item)?;
    }
    Ok(())
}

// ── Version-specific helpers ────────────────────────────────────────────

/// `<name><In>` hint for a v2 `Parameter` — e.g. `limitQuery`,
/// `petBody`. Returns `""` when the parameter has no usable name,
/// signalling the intern path to fall back to context-derived
/// naming.
fn parameter_name_hint(param: &Parameter) -> String {
    use crate::v2::parameter::{InFormData, InHeader, InPath, InQuery};
    let (name, in_) = match param {
        Parameter::Body(p) => (p.name.as_str(), "Body"),
        Parameter::Header(p) => {
            let name = match p.as_ref() {
                InHeader::String(p) => p.name.as_str(),
                InHeader::Integer(p) => p.name.as_str(),
                InHeader::Number(p) => p.name.as_str(),
                InHeader::Boolean(p) => p.name.as_str(),
                InHeader::Array(p) => p.name.as_str(),
            };
            (name, "Header")
        }
        Parameter::Path(p) => {
            let name = match p.as_ref() {
                InPath::String(p) => p.name.as_str(),
                InPath::Integer(p) => p.name.as_str(),
                InPath::Number(p) => p.name.as_str(),
                InPath::Boolean(p) => p.name.as_str(),
                InPath::Array(p) => p.name.as_str(),
            };
            (name, "Path")
        }
        Parameter::Query(p) => {
            let name = match p.as_ref() {
                InQuery::String(p) => p.name.as_str(),
                InQuery::Integer(p) => p.name.as_str(),
                InQuery::Number(p) => p.name.as_str(),
                InQuery::Boolean(p) => p.name.as_str(),
                InQuery::Array(p) => p.name.as_str(),
            };
            (name, "Query")
        }
        Parameter::FormData(p) => {
            let name = match p.as_ref() {
                InFormData::String(p) => p.name.as_str(),
                InFormData::Integer(p) => p.name.as_str(),
                InFormData::Number(p) => p.name.as_str(),
                InFormData::Boolean(p) => p.name.as_str(),
                InFormData::Array(p) => p.name.as_str(),
                InFormData::File(p) => p.name.as_str(),
            };
            (name, "FormData")
        }
    };
    if name.is_empty() {
        String::new()
    } else {
        format!("{name}{in_}")
    }
}

fn schema_title(schema: &Schema) -> Option<&str> {
    match schema {
        Schema::AllOf(s) => s.title.as_deref(),
        Schema::Object(s) => s.title.as_deref(),
        Schema::Array(s) => s.title.as_deref(),
        Schema::String(s) => s.title.as_deref(),
        Schema::Integer(s) => s.title.as_deref(),
        Schema::Number(s) => s.title.as_deref(),
        Schema::Boolean(s) => s.title.as_deref(),
        Schema::Null(s) => s.title.as_deref(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: serde_json::Value) -> Spec {
        serde_json::from_value(value).expect("spec parses")
    }

    fn lifted_schema_names(spec: &Spec) -> Vec<String> {
        spec.definitions
            .as_ref()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn lifted_parameter_names(spec: &Spec) -> Vec<String> {
        spec.parameters
            .as_ref()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn lifted_response_names(spec: &Spec) -> Vec<String> {
        spec.responses
            .as_ref()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn schema_at(spec: &Spec, name: &str) -> serde_json::Value {
        let item = spec
            .definitions
            .as_ref()
            .and_then(|m| m.get(name))
            .expect("schema present");
        serde_json::to_value(item).unwrap()
    }

    #[test]
    fn lift_inline_body_parameter_schema_into_definitions() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "operationId": "createPet",
                        "parameters": [
                            {
                                "in": "body",
                                "name": "pet",
                                "schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                            }
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).expect("collapse ok");
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Pet".to_owned()), "got {names:?}");
        let v = serde_json::to_value(&spec).unwrap();
        // The parameter is itself lifted (it's the inline first hit),
        // so dig through `parameters.<name>` to reach the schema ref.
        let p_ref = v["paths"]["/pets"]["post"]["parameters"][0]["$ref"]
            .as_str()
            .expect("parameter lifted");
        let p_name = p_ref.trim_start_matches("#/parameters/");
        assert_eq!(
            v["parameters"][p_name]["schema"]["$ref"],
            "#/definitions/Pet"
        );
    }

    #[test]
    fn lift_inline_response_schema_into_definitions() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "operationId": "listPets",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).expect("collapse ok");
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/pets"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("response lifted");
        let resp_name = resp_ref.trim_start_matches("#/responses/");
        assert_eq!(
            v["responses"][resp_name]["schema"]["$ref"],
            "#/definitions/Pet"
        );
        assert!(lifted_schema_names(&spec).contains(&"Pet".to_owned()));
    }

    #[test]
    fn parameter_name_hint_picks_up_typed_variants() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets/{id}": {
                    "get": {
                        "operationId": "getPet",
                        "parameters": [
                            {"name": "id", "in": "path", "required": true, "type": "integer"},
                            {"name": "tag", "in": "query", "type": "string"},
                            {"name": "x-trace", "in": "header", "type": "string"}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_parameter_names(&spec);
        for expected in ["idPath", "tagQuery", "x-traceHeader"] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing `{expected}`: {names:?}"
            );
        }
    }

    #[test]
    fn identical_inline_responses_dedupe_to_one_definition() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"get": {"operationId": "a", "responses": {"200": {"description": "ok"}}}},
                "/b": {"get": {"operationId": "b", "responses": {"200": {"description": "ok"}}}}
            }
        }));
        spec.collapse(None).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        // Both call sites point at the same lifted response.
        assert_eq!(
            v["paths"]["/a"]["get"]["responses"]["200"]["$ref"],
            v["paths"]["/b"]["get"]["responses"]["200"]["$ref"],
        );
        assert_eq!(lifted_response_names(&spec).len(), 1);
    }

    #[test]
    fn identical_named_inline_schemas_dedupe_to_one_definition() {
        // Two distinct paths whose bodies share the same titled
        // schema collapse onto one `definitions.<title>` entry.
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "post": {
                        "operationId": "a",
                        "parameters": [
                            {"in": "body", "name": "body", "schema": {"title": "Body", "type": "string"}}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                },
                "/b": {
                    "post": {
                        "operationId": "b",
                        "parameters": [
                            {"in": "body", "name": "body", "schema": {"title": "Body", "type": "string"}}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert_eq!(lifted_schema_names(&spec), vec!["Body".to_owned()]);
    }

    #[test]
    fn existing_definitions_keep_names_and_recurse_children() {
        // A pre-existing `definitions.Pet` keeps its name; the nested
        // inline schema inside it lifts as a new definition.
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "definitions": {
                "Pet": {
                    "type": "object",
                    "properties": {
                        "extras": {"title": "Extras", "type": "object", "properties": {"v": {"type": "string"}}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Pet".to_owned()), "got {names:?}");
        assert!(names.contains(&"Extras".to_owned()), "got {names:?}");
        let pet = schema_at(&spec, "Pet");
        assert_eq!(pet["properties"]["extras"]["$ref"], "#/definitions/Extras");
    }

    #[test]
    fn object_allof_children_stay_inline_but_inner_schemas_lift() {
        // OAS 2.0's `ObjectSchema.all_of` is `Vec<RefOr<ObjectSchema>>`
        // — there's no `definitions` slot that matches `ObjectSchema`
        // directly, so inline allOf children are walked in place. Their
        // nested `properties.<x>` slots (which hold `RefOr<Schema>`)
        // still lift normally.
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "definitions": {
                "Pet": {
                    "allOf": [
                        {"title": "PetBase", "type": "object", "properties": {"id": {"title": "Id", "type": "integer"}}}
                    ]
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Id".to_owned()), "got {names:?}");
        // The PetBase ObjectSchema itself isn't lifted — it stays
        // inline under definitions.Pet.allOf[0].
        let v = serde_json::to_value(&spec).unwrap();
        assert!(
            v["definitions"]["Pet"]["allOf"][0]["title"] == "PetBase",
            "allOf child must stay inline: {:?}",
            v["definitions"]["Pet"]["allOf"][0]
        );
        assert_eq!(
            v["definitions"]["Pet"]["allOf"][0]["properties"]["id"]["$ref"],
            "#/definitions/Id"
        );
    }

    #[test]
    fn array_items_lift_into_definitions() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "definitions": {
                "Pets": {
                    "type": "array",
                    "items": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert!(lifted_schema_names(&spec).contains(&"Pet".to_owned()));
        let pets = schema_at(&spec, "Pets");
        assert_eq!(pets["items"]["$ref"], "#/definitions/Pet");
    }

    #[test]
    fn internal_ref_slots_are_untouched() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "operationId": "a",
                        "responses": {
                            "200": {"description": "ok", "schema": {"$ref": "#/definitions/Pet"}}
                        }
                    }
                }
            },
            "definitions": {
                "Pet": {"type": "object", "properties": {"id": {"type": "integer"}}}
            }
        }));
        spec.collapse(None).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/a"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("response lifted");
        let resp_name = resp_ref.trim_start_matches("#/responses/");
        assert_eq!(
            v["responses"][resp_name]["schema"]["$ref"],
            "#/definitions/Pet"
        );
    }

    #[test]
    fn external_ref_without_loader_is_left_alone() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "operationId": "a",
                        "parameters": [{"$ref": "shared.json#/PageParam"}],
                        "responses": {
                            "200": {"$ref": "shared.json#/Ok"}
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["paths"]["/a"]["get"]["parameters"][0]["$ref"],
            "shared.json#/PageParam"
        );
        assert_eq!(
            v["paths"]["/a"]["get"]["responses"]["200"]["$ref"],
            "shared.json#/Ok"
        );
    }

    #[test]
    fn loader_resolves_external_schema_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({
                    "Pet": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                }),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "operationId": "createPet",
                        "parameters": [
                            {
                                "in": "body",
                                "name": "pet",
                                "schema": {"$ref": "shared.json#/Pet"}
                            }
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        assert!(lifted_schema_names(&spec).contains(&"Pet".to_owned()));
    }

    #[test]
    fn round_trips_through_serde_after_collapse() {
        let mut spec = parse(serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "operationId": "listPets",
                        "responses": {
                            "200": {"description": "ok", "schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}}
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let s = serde_json::to_string(&spec).unwrap();
        let _: Spec = serde_json::from_str(&s).expect("re-parses");
    }
}
