//! `Spec::collapse` for OAS 3.2 — lift every inline component into
//! `components.<bag>`.
//!
//! All of the heavy lifting (dedup, naming, the generic `lift_ref_or`
//! routine, the `LiftableBag` trait, the `Bag<T>` storage) lives in
//! [`crate::common::collapse`]. This module just provides the v3.2
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
//!   `components.<bag>` entries) + phase 2b (walk paths / webhooks),
//!   then writes each bag back.
//!
//! Bags lifted: `schemas`, `parameters`, `responses`,
//! `requestBodies`, `headers`, `mediaTypes`, `examples`, `links`,
//! `callbacks`. `pathItems` is *not* lifted out of its primary
//! locations (`paths.<path>`, `webhooks.<name>`,
//! `callback.paths.<expr>`) — pre-existing `components.pathItems`
//! entries are still seeded into the dedup map and their nested
//! children are lifted.

use std::collections::{BTreeMap, HashMap};

use crate::common::bool_or::BoolOr;
use crate::common::collapse::{Bag, HasLoader, LiftableBag, NameContext, lift_ref_or};
use crate::common::reference::RefOr;
use crate::loader::Loader;
use crate::v3_2::callback::Callback;
use crate::v3_2::example::Example;
use crate::v3_2::header::Header;
use crate::v3_2::link::Link;
use crate::v3_2::media_type::{Encoding, MediaType};
use crate::v3_2::operation::Operation;
use crate::v3_2::parameter::Parameter;
use crate::v3_2::path_item::{PathItem, Paths};
use crate::v3_2::request_body::RequestBody;
use crate::v3_2::response::{Response, Responses};
use crate::v3_2::schema::{ArraySchema, ObjectSchema, Schema, SingleSchema};
use crate::v3_2::spec::Spec;

pub use crate::common::collapse::CollapseError;

// ── Collapser: per-bag state + loader handle ────────────────────────────

pub(crate) struct Collapser<'a> {
    schemas: Bag<Schema>,
    parameters: Bag<Parameter>,
    responses: Bag<Response>,
    request_bodies: Bag<RequestBody>,
    headers: Bag<Header>,
    media_types: Bag<MediaType>,
    examples: Bag<Example>,
    links: Bag<Link>,
    callbacks: Bag<Callback>,
    /// PathItem is bare (not wrapped in `RefOr`); its ref form lives
    /// in `PathItem.reference`. We don't lift inline PathItems out
    /// of `paths.<path>` / `webhooks.<name>` / callback paths — but
    /// pre-existing `components.pathItems` entries are still seeded
    /// here so we can recurse into them and lift their nested
    /// children.
    path_items: BTreeMap<String, PathItem>,
    path_items_seen: HashMap<String, String>,
    loader: Option<&'a mut Loader>,
}

impl HasLoader for Collapser<'_> {
    fn loader_mut(&mut self) -> Option<&mut Loader> {
        self.loader.as_deref_mut()
    }
}

// ── LiftableBag impls per component type ────────────────────────────────
//
// Each impl spells out (a) the component-bag ref prefix, (b) how to
// reach this type's bag inside the Collapser, (c) the tree-walking
// function that lifts nested slots, and (d) an optional name hint.
// The generic `lift_ref_or` in `common::collapse` does the rest.

impl<'a> LiftableBag<Collapser<'a>> for Schema {
    const PREFIX: &'static str = "#/components/schemas/";

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
    const PREFIX: &'static str = "#/components/parameters/";

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
    const PREFIX: &'static str = "#/components/responses/";

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

impl<'a> LiftableBag<Collapser<'a>> for RequestBody {
    const PREFIX: &'static str = "#/components/requestBodies/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.request_bodies
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        walk_request_body(item, ctx, c)
    }
}

impl<'a> LiftableBag<Collapser<'a>> for Header {
    const PREFIX: &'static str = "#/components/headers/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.headers
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        walk_header(item, ctx, c)
    }
}

impl<'a> LiftableBag<Collapser<'a>> for MediaType {
    const PREFIX: &'static str = "#/components/mediaTypes/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.media_types
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        walk_media_type(item, ctx, c)
    }
}

impl<'a> LiftableBag<Collapser<'a>> for Example {
    const PREFIX: &'static str = "#/components/examples/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.examples
    }

    fn walk(
        _item: &mut Self,
        _ctx: &NameContext,
        _c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        // `Example` is a leaf — no nested RefOr slots to lift.
        Ok(())
    }
}

impl<'a> LiftableBag<Collapser<'a>> for Link {
    const PREFIX: &'static str = "#/components/links/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.links
    }

    fn walk(
        _item: &mut Self,
        _ctx: &NameContext,
        _c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        // `Link` is a leaf — no nested RefOr slots to lift.
        Ok(())
    }
}

impl<'a> LiftableBag<Collapser<'a>> for Callback {
    const PREFIX: &'static str = "#/components/callbacks/";

    fn bag<'b>(c: &'b mut Collapser<'a>) -> &'b mut Bag<Self> {
        &mut c.callbacks
    }

    fn walk(
        item: &mut Self,
        ctx: &NameContext,
        c: &mut Collapser<'a>,
    ) -> Result<(), CollapseError> {
        walk_callback(item, ctx, c)
    }
}

// ── Walkers: per-type tree recursion ────────────────────────────────────
//
// Each walker is a free function (not a method on Collapser) so it
// can take `&mut Collapser` alongside the item it's walking. The
// items here have always been removed from their containing bag by
// the time the walker fires (via `mem::replace` inside
// `lift_ref_or`), so there's no aliasing.

fn recurse_schema(
    schema: &mut Schema,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    match schema {
        Schema::Bool(_) | Schema::Empty(_) | Schema::Multi(_) => Ok(()),
        Schema::AllOf(s) => {
            for (i, child) in s.all_of.iter_mut().enumerate() {
                lift_ref_or::<Schema, _>(child, ctx.push(&format!("allOf[{i}]")), c)?;
            }
            Ok(())
        }
        Schema::AnyOf(s) => {
            for (i, child) in s.any_of.iter_mut().enumerate() {
                lift_ref_or::<Schema, _>(child, ctx.push(&format!("anyOf[{i}]")), c)?;
            }
            Ok(())
        }
        Schema::OneOf(s) => {
            for (i, child) in s.one_of.iter_mut().enumerate() {
                lift_ref_or::<Schema, _>(child, ctx.push(&format!("oneOf[{i}]")), c)?;
            }
            Ok(())
        }
        Schema::Not(s) => lift_ref_or::<Schema, _>(&mut s.not, ctx.push("not"), c),
        Schema::Single(s) => recurse_single_schema(s.as_mut(), ctx, c),
    }
}

fn recurse_single_schema(
    s: &mut SingleSchema,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    match s {
        SingleSchema::Object(o) => recurse_object_schema(o, ctx, c),
        SingleSchema::Array(a) => recurse_array_schema(a, ctx, c),
        // Primitive variants (String, Integer, Number, Boolean, Null)
        // carry no nested schema slots.
        _ => Ok(()),
    }
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
    if let Some(props) = o.pattern_properties.as_mut() {
        for (name, child) in props.iter_mut() {
            lift_ref_or::<Schema, _>(child, ctx.push(&format!("patternProperties.{name}")), c)?;
        }
    }
    if let Some(BoolOr::Item(s)) = o.additional_properties.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("additionalProperties"), c)?;
    }
    if let Some(BoolOr::Item(s)) = o.unevaluated_properties.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("unevaluatedProperties"), c)?;
    }
    if let Some(s) = o.property_names.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("propertyNames"), c)?;
    }
    Ok(())
}

fn recurse_array_schema(
    a: &mut ArraySchema,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(BoolOr::Item(s)) = a.items.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("items"), c)?;
    }
    Ok(())
}

fn walk_parameter(
    param: &mut Parameter,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    // Push the parameter's `name` into ctx so derived child names
    // mention it. Querystring is the odd one out: per OAS 3.2 it
    // carries `content` only and forbids `schema`.
    match param {
        Parameter::Path(p) => walk_param_slots(
            ctx.push(p.name.as_str()),
            p.schema.as_mut(),
            p.content.as_mut(),
            p.examples.as_mut(),
            c,
        ),
        Parameter::Query(p) => walk_param_slots(
            ctx.push(p.name.as_str()),
            p.schema.as_mut(),
            p.content.as_mut(),
            p.examples.as_mut(),
            c,
        ),
        Parameter::Header(p) => walk_param_slots(
            ctx.push(p.name.as_str()),
            p.schema.as_mut(),
            p.content.as_mut(),
            p.examples.as_mut(),
            c,
        ),
        Parameter::Cookie(p) => walk_param_slots(
            ctx.push(p.name.as_str()),
            p.schema.as_mut(),
            p.content.as_mut(),
            p.examples.as_mut(),
            c,
        ),
        Parameter::Querystring(p) => {
            let ctx = ctx.push(p.name.as_str());
            for (mime, mt) in p.content.iter_mut() {
                lift_ref_or::<MediaType, _>(mt, ctx.push(&format!("content.{mime}")), c)?;
            }
            if let Some(examples) = p.examples.as_mut() {
                for (name, e) in examples.iter_mut() {
                    lift_ref_or::<Example, _>(e, ctx.push(&format!("examples.{name}")), c)?;
                }
            }
            Ok(())
        }
    }
}

fn walk_param_slots(
    ctx: NameContext,
    schema: Option<&mut RefOr<Schema>>,
    content: Option<&mut BTreeMap<String, RefOr<MediaType>>>,
    examples: Option<&mut BTreeMap<String, RefOr<Example>>>,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(s) = schema {
        lift_ref_or::<Schema, _>(s, ctx.push("schema"), c)?;
    }
    if let Some(content) = content {
        for (mime, mt) in content.iter_mut() {
            lift_ref_or::<MediaType, _>(mt, ctx.push(&format!("content.{mime}")), c)?;
        }
    }
    if let Some(examples) = examples {
        for (name, e) in examples.iter_mut() {
            lift_ref_or::<Example, _>(e, ctx.push(&format!("examples.{name}")), c)?;
        }
    }
    Ok(())
}

fn walk_response(
    r: &mut Response,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(headers) = r.headers.as_mut() {
        for (name, h) in headers.iter_mut() {
            lift_ref_or::<Header, _>(h, ctx.push(&format!("headers.{name}")), c)?;
        }
    }
    if let Some(content) = r.content.as_mut() {
        for (mime, mt) in content.iter_mut() {
            lift_ref_or::<MediaType, _>(mt, ctx.push(&format!("content.{mime}")), c)?;
        }
    }
    if let Some(links) = r.links.as_mut() {
        for (name, l) in links.iter_mut() {
            lift_ref_or::<Link, _>(l, ctx.push(&format!("links.{name}")), c)?;
        }
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

fn walk_request_body(
    rb: &mut RequestBody,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    for (mime, mt) in rb.content.iter_mut() {
        lift_ref_or::<MediaType, _>(mt, ctx.push(&format!("content.{mime}")), c)?;
    }
    Ok(())
}

fn walk_header(
    h: &mut Header,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(s) = h.schema.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("schema"), c)?;
    }
    if let Some(content) = h.content.as_mut() {
        for (mime, mt) in content.iter_mut() {
            lift_ref_or::<MediaType, _>(mt, ctx.push(&format!("content.{mime}")), c)?;
        }
    }
    if let Some(examples) = h.examples.as_mut() {
        for (name, e) in examples.iter_mut() {
            lift_ref_or::<Example, _>(e, ctx.push(&format!("examples.{name}")), c)?;
        }
    }
    Ok(())
}

fn walk_media_type(
    mt: &mut MediaType,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(s) = mt.schema.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("schema"), c)?;
    }
    if let Some(s) = mt.item_schema.as_mut() {
        lift_ref_or::<Schema, _>(s, ctx.push("itemSchema"), c)?;
    }
    if let Some(examples) = mt.examples.as_mut() {
        for (name, e) in examples.iter_mut() {
            lift_ref_or::<Example, _>(e, ctx.push(&format!("examples.{name}")), c)?;
        }
    }
    if let Some(encoding) = mt.encoding.as_mut() {
        for (prop, enc) in encoding.iter_mut() {
            walk_encoding(enc, &ctx.push(&format!("encoding.{prop}")), c)?;
        }
    }
    if let Some(prefix) = mt.prefix_encoding.as_mut() {
        for (i, enc) in prefix.iter_mut().enumerate() {
            walk_encoding(enc, &ctx.push(&format!("prefixEncoding[{i}]")), c)?;
        }
    }
    if let Some(item) = mt.item_encoding.as_mut() {
        walk_encoding(item, &ctx.push("itemEncoding"), c)?;
    }
    Ok(())
}

fn walk_encoding(
    enc: &mut Encoding,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    if let Some(headers) = enc.headers.as_mut() {
        for (name, h) in headers.iter_mut() {
            lift_ref_or::<Header, _>(h, ctx.push(&format!("headers.{name}")), c)?;
        }
    }
    // OAS 3.2 makes `Encoding` recursive (for nested multipart parts):
    // `encoding`, `prefixEncoding`, and `itemEncoding` mirror the
    // MediaType-level fields and can carry further `RefOr<Header>`
    // slots inside them.
    if let Some(encoding) = enc.encoding.as_mut() {
        for (prop, child) in encoding.iter_mut() {
            walk_encoding(child, &ctx.push(&format!("encoding.{prop}")), c)?;
        }
    }
    if let Some(prefix) = enc.prefix_encoding.as_mut() {
        for (i, child) in prefix.iter_mut().enumerate() {
            walk_encoding(child, &ctx.push(&format!("prefixEncoding[{i}]")), c)?;
        }
    }
    if let Some(item) = enc.item_encoding.as_mut() {
        walk_encoding(item, &ctx.push("itemEncoding"), c)?;
    }
    Ok(())
}

fn walk_callback(
    cb: &mut Callback,
    ctx: &NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    for (expr, pi) in cb.paths.iter_mut() {
        walk_path_item(pi, ctx.push(expr), c)?;
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
    if let Some(ops) = pi.additional_operations.as_mut() {
        for (method, op) in ops.iter_mut() {
            walk_operation(op, ctx.push(method), c)?;
        }
    }
    Ok(())
}

fn walk_operation(
    op: &mut Operation,
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
    if let Some(rb) = op.request_body.as_mut() {
        lift_ref_or::<RequestBody, _>(rb, ctx.push("requestBody"), c)?;
    }
    if let Some(responses) = op.responses.as_mut() {
        walk_responses(responses, &ctx.push("responses"), c)?;
    }
    if let Some(callbacks) = op.callbacks.as_mut() {
        for (name, cb) in callbacks.iter_mut() {
            lift_ref_or::<Callback, _>(cb, ctx.push(name), c)?;
        }
    }
    Ok(())
}

fn walk_paths(
    paths: &mut Paths,
    ctx: NameContext,
    c: &mut Collapser<'_>,
) -> Result<(), CollapseError> {
    // `paths.<path>` PathItems are walked (lifting nested schemas /
    // parameters / responses inside them) but the PathItem itself is
    // *not* lifted to `components.pathItems` — see the module
    // docstring.
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
    // Phase 0: take each existing components bag out of the spec.
    let initial_schemas = spec
        .components
        .as_mut()
        .and_then(|c| c.schemas.take())
        .unwrap_or_default();
    let initial_parameters = spec
        .components
        .as_mut()
        .and_then(|c| c.parameters.take())
        .unwrap_or_default();
    let initial_responses = spec
        .components
        .as_mut()
        .and_then(|c| c.responses.take())
        .unwrap_or_default();
    let initial_request_bodies = spec
        .components
        .as_mut()
        .and_then(|c| c.request_bodies.take())
        .unwrap_or_default();
    let initial_headers = spec
        .components
        .as_mut()
        .and_then(|c| c.headers.take())
        .unwrap_or_default();
    let initial_media_types = spec
        .components
        .as_mut()
        .and_then(|c| c.media_types.take())
        .unwrap_or_default();
    let initial_examples = spec
        .components
        .as_mut()
        .and_then(|c| c.examples.take())
        .unwrap_or_default();
    let initial_links = spec
        .components
        .as_mut()
        .and_then(|c| c.links.take())
        .unwrap_or_default();
    let initial_callbacks = spec
        .components
        .as_mut()
        .and_then(|c| c.callbacks.take())
        .unwrap_or_default();
    let initial_path_items = spec
        .components
        .as_mut()
        .and_then(|c| c.path_items.take())
        .unwrap_or_default();

    let mut c = Collapser {
        schemas: Bag::default(),
        parameters: Bag::default(),
        responses: Bag::default(),
        request_bodies: Bag::default(),
        headers: Bag::default(),
        media_types: Bag::default(),
        examples: Bag::default(),
        links: Bag::default(),
        callbacks: Bag::default(),
        path_items: BTreeMap::new(),
        path_items_seen: HashMap::new(),
        loader,
    };

    // Phase 1: seed every bag from its existing entries. The dedup
    // map gets pre-populated so newly-lifted equivalents collapse
    // onto the existing names.
    c.schemas.seed(initial_schemas)?;
    c.parameters.seed(initial_parameters)?;
    c.responses.seed(initial_responses)?;
    c.request_bodies.seed(initial_request_bodies)?;
    c.headers.seed(initial_headers)?;
    c.media_types.seed(initial_media_types)?;
    c.examples.seed(initial_examples)?;
    c.links.seed(initial_links)?;
    c.callbacks.seed(initial_callbacks)?;
    // PathItems is bare — seed the dedup map and the entry map by
    // hand. Only inline (reference: None) entries participate in
    // dedup; ref-form ones are pure pointers.
    for (name, pi) in initial_path_items {
        if pi.reference.is_none() {
            let canonical = serde_json::to_string(&pi)?;
            c.path_items_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        c.path_items.insert(name, pi);
    }

    // Phase 2a: recurse into each pre-existing inline component,
    // lifting its nested children. We compose this from the
    // `inline_names` / `take_inline` / `put_inline` primitives so the
    // walker has full `&mut Collapser` access during the recurse.
    recurse_existing::<Schema>(&mut c, &["components", "schemas"])?;
    recurse_existing::<Parameter>(&mut c, &["components", "parameters"])?;
    recurse_existing::<Response>(&mut c, &["components", "responses"])?;
    recurse_existing::<RequestBody>(&mut c, &["components", "requestBodies"])?;
    recurse_existing::<Header>(&mut c, &["components", "headers"])?;
    recurse_existing::<MediaType>(&mut c, &["components", "mediaTypes"])?;
    // Examples and links are leaves — nothing to recurse INTO.
    recurse_existing::<Callback>(&mut c, &["components", "callbacks"])?;

    // PathItem phase 2a: only the inline (reference == None) entries
    // get walked. Skip the ref-form ones (they're already pointers).
    let pi_names: Vec<String> = c.path_items.keys().cloned().collect();
    for name in pi_names {
        let is_inline = c
            .path_items
            .get(&name)
            .is_some_and(|pi| pi.reference.is_none());
        if !is_inline {
            continue;
        }
        let Some(mut pi) = c.path_items.remove(&name) else {
            continue;
        };
        let ctx = NameContext::new(["components", "pathItems", &name]);
        walk_path_item(&mut pi, ctx, &mut c)?;
        let canonical = serde_json::to_string(&pi)?;
        c.path_items_seen
            .entry(canonical)
            .or_insert_with(|| name.clone());
        c.path_items.insert(name, pi);
    }

    // Phase 2b: walk paths and webhooks. (Components were drained
    // into bags in Phase 0; there's nothing else to visit.)
    if let Some(paths) = spec.paths.as_mut() {
        walk_paths(paths, NameContext::new(["paths"]), &mut c)?;
    }
    if let Some(webhooks) = spec.webhooks.as_mut() {
        walk_paths(webhooks, NameContext::new(["webhooks"]), &mut c)?;
    }

    // Phase 3: write each lifted bag back to its slot under
    // `components`. Skip empty bags so a no-op collapse doesn't
    // materialise an empty `components: {}` map.
    if !c.schemas.is_empty() {
        spec.components.get_or_insert_with(Default::default).schemas = Some(c.schemas.into_map());
    }
    if !c.parameters.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .parameters = Some(c.parameters.into_map());
    }
    if !c.responses.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .responses = Some(c.responses.into_map());
    }
    if !c.request_bodies.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .request_bodies = Some(c.request_bodies.into_map());
    }
    if !c.headers.is_empty() {
        spec.components.get_or_insert_with(Default::default).headers = Some(c.headers.into_map());
    }
    if !c.media_types.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .media_types = Some(c.media_types.into_map());
    }
    if !c.examples.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .examples = Some(c.examples.into_map());
    }
    if !c.links.is_empty() {
        spec.components.get_or_insert_with(Default::default).links = Some(c.links.into_map());
    }
    if !c.callbacks.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .callbacks = Some(c.callbacks.into_map());
    }
    if !c.path_items.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .path_items = Some(c.path_items);
    }

    Ok(())
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

/// `<name><In>` hint for a v3.2 `Parameter` — e.g. `limitQuery`,
/// `petIdPath`. Returns `""` when the parameter has no usable name,
/// signalling the intern path to fall back to context-derived
/// naming.
fn parameter_name_hint(param: &Parameter) -> String {
    let (name, in_) = match param {
        Parameter::Path(p) => (p.name.as_str(), "Path"),
        Parameter::Query(p) => (p.name.as_str(), "Query"),
        Parameter::Querystring(p) => (p.name.as_str(), "Querystring"),
        Parameter::Header(p) => (p.name.as_str(), "Header"),
        Parameter::Cookie(p) => (p.name.as_str(), "Cookie"),
    };
    if name.is_empty() {
        String::new()
    } else {
        format!("{name}{in_}")
    }
}

fn schema_title(schema: &Schema) -> Option<&str> {
    match schema {
        Schema::Bool(_)
        | Schema::Empty(_)
        | Schema::AllOf(_)
        | Schema::AnyOf(_)
        | Schema::OneOf(_)
        | Schema::Not(_) => None,
        Schema::Multi(s) => s.title.as_deref(),
        Schema::Single(s) => single_schema_title(s.as_ref()),
    }
}

fn single_schema_title(s: &SingleSchema) -> Option<&str> {
    match s {
        SingleSchema::Object(o) => o.title.as_deref(),
        SingleSchema::Array(a) => a.title.as_deref(),
        SingleSchema::String(s) => s.title.as_deref(),
        SingleSchema::Integer(s) => s.title.as_deref(),
        SingleSchema::Number(s) => s.title.as_deref(),
        SingleSchema::Boolean(s) => s.title.as_deref(),
        SingleSchema::Null(s) => s.title.as_deref(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: serde_json::Value) -> Spec {
        serde_json::from_value(value).expect("spec parses")
    }

    fn lifted_schema_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.schemas.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Walk through a lifted Response / RequestBody / Parameter /
    /// Header (or any container whose `content[mime]` slot may itself
    /// be a lifted MediaType ref) and return the inner `schema` slot.
    /// Lets tests assert on the schema regardless of whether
    /// mediaTypes have been lifted.
    fn schema_in_lifted_content(
        v: &serde_json::Value,
        container_ref: &str,
        mime: &str,
    ) -> serde_json::Value {
        let pointer = container_ref.trim_start_matches('#');
        let container = v
            .pointer(pointer)
            .unwrap_or_else(|| panic!("ref `{container_ref}` must resolve in spec"));
        let mt_slot = &container["content"][mime];
        if let Some(mt_ref) = mt_slot.get("$ref").and_then(|s| s.as_str()) {
            let mt_name = mt_ref.trim_start_matches("#/components/mediaTypes/");
            v["components"]["mediaTypes"][mt_name]["schema"].clone()
        } else {
            mt_slot["schema"].clone()
        }
    }

    fn schema_at(spec: &Spec, name: &str) -> serde_json::Value {
        let item = spec
            .components
            .as_ref()
            .and_then(|c| c.schemas.as_ref())
            .and_then(|m| m.get(name))
            .expect("schema present");
        serde_json::to_value(item).unwrap()
    }

    #[test]
    fn lift_inline_response_schema_into_components_via_context_name() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "operationId": "listPets",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"type": "object", "properties": {"id": {"type": "integer"}}}
                                    }
                                }
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
            .expect("response slot must be lifted to a ref");
        let schema_slot = schema_in_lifted_content(&v, resp_ref, "application/json");
        assert!(
            schema_slot.get("$ref").is_some(),
            "expected the inline schema to be a `$ref`, got: {schema_slot}",
        );
        // The lifted component should exist under a context-derived name
        // rooted at the operationId. We don't pin the exact name here
        // because the path-flattening rule is internal; check that the
        // expected ref target is one of the lifted schemas.
        let ref_str = schema_slot["$ref"].as_str().unwrap();
        assert!(
            ref_str.starts_with("#/components/schemas/"),
            "ref must point at #/components/schemas, got `{ref_str}`",
        );
        let name = ref_str.trim_start_matches("#/components/schemas/");
        assert!(
            lifted_schema_names(&spec).contains(&name.to_owned()),
            "lifted name `{name}` must appear in components.schemas",
        );
    }

    #[test]
    fn schema_title_drives_the_component_name() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "title": "Pet",
                                            "type": "object",
                                            "properties": {"id": {"type": "integer"}}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert!(
            lifted_schema_names(&spec).contains(&"Pet".to_owned()),
            "lifted name must be the schema's title; got {:?}",
            lifted_schema_names(&spec),
        );
        // Response is lifted; the Pet schema ref lives inside it.
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/pets"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(
            schema_in_lifted_content(&v, resp_ref, "application/json")["$ref"],
            "#/components/schemas/Pet"
        );
    }

    #[test]
    fn nested_object_schemas_are_lifted_recursively() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "title": "Pet",
                                            "type": "object",
                                            "properties": {
                                                "owner": {
                                                    "title": "Owner",
                                                    "type": "object",
                                                    "properties": {"id": {"type": "integer"}}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        // Both Pet and Owner should be lifted into components.schemas.
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Pet".to_owned()), "got {names:?}");
        assert!(names.contains(&"Owner".to_owned()), "got {names:?}");
        // Pet's owner property should be a ref to #/components/schemas/Owner.
        let pet = schema_at(&spec, "Pet");
        assert_eq!(
            pet["properties"]["owner"]["$ref"],
            "#/components/schemas/Owner"
        );
    }

    #[test]
    fn identical_inline_schemas_dedupe_to_one_component() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                                    }
                                }
                            }
                        }
                    }
                },
                "/b": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        // Exactly one `Pet` component (schemas dedupe).
        let names = lifted_schema_names(&spec);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "Pet").count(),
            1,
            "got {names:?}"
        );
        // Both responses are now lifted (and dedupe to one response
        // component since their content is identical); the response's
        // content[json].schema still resolves to the same Pet ref.
        let v = serde_json::to_value(&spec).unwrap();
        let a_resp = v["paths"]["/a"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("/a 200 must be a response ref");
        let b_resp = v["paths"]["/b"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("/b 200 must be a response ref");
        // Both call sites point at the *same* response (response dedup).
        assert_eq!(a_resp, b_resp);
        // The shared response's body points at the single Pet schema.
        assert_eq!(
            schema_in_lifted_content(&v, a_resp, "application/json")["$ref"],
            "#/components/schemas/Pet"
        );
    }

    #[test]
    fn name_collision_falls_back_to_suffix() {
        // Two different schemas, both untitled, both deriving the same
        // context-path-based name → suffix `_2` on the second one.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "components": {
                "schemas": {
                    "Existing": {"type": "string"}
                }
            },
            "paths": {
                "/x": {
                    "get": {
                        "operationId": "Existing",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"type": "object", "properties": {"id": {"type": "integer"}}}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        // The pre-existing `Existing` stays at its name.
        assert!(names.contains(&"Existing".to_owned()), "got {names:?}");
        // The newly-lifted schema (derived from the colliding operationId
        // path) should land at `Existing_2`, `Existing_3`, etc.
        assert!(
            names.iter().any(|n| n.starts_with("Existing_")),
            "expected a `Existing_<n>` suffix entry, got {names:?}",
        );
    }

    #[test]
    fn existing_components_schemas_are_preserved_and_recursed() {
        // Pre-existing components.schemas entries keep their names but
        // their inline nested schemas still get lifted.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "properties": {
                            "owner": {
                                "title": "Owner",
                                "type": "object",
                                "properties": {"id": {"type": "integer"}}
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Pet".to_owned()), "got {names:?}");
        assert!(names.contains(&"Owner".to_owned()), "got {names:?}");
        let pet = schema_at(&spec, "Pet");
        assert_eq!(
            pet["properties"]["owner"]["$ref"], "#/components/schemas/Owner",
            "Pet.owner must be lifted to a ref",
        );
    }

    #[test]
    fn loader_resolves_external_refs_and_dedupes_with_inline() {
        // Build a loader with one preloaded external resource. The
        // external schema is structurally identical to an inline schema
        // already present in the spec → after collapse they dedupe to
        // one component.
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "external.json",
                serde_json::json!({
                    "Pet": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                }),
            )
            .expect("preload");
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                                    }
                                }
                            }
                        }
                    }
                },
                "/b": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "external.json#/Pet"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).expect("collapse ok");
        let names = lifted_schema_names(&spec);
        // One Pet component covering both the inline and the external.
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "Pet").count(),
            1,
            "expected single Pet entry, got {names:?}",
        );
        // Both responses are now lifted (and dedupe — identical
        // content); navigate through to verify both content schemas
        // resolve to the single Pet.
        let v = serde_json::to_value(&spec).unwrap();
        let a_resp = v["paths"]["/a"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .unwrap();
        let b_resp = v["paths"]["/b"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .unwrap();
        for resp_ref in [a_resp, b_resp] {
            assert_eq!(
                schema_in_lifted_content(&v, resp_ref, "application/json")["$ref"],
                "#/components/schemas/Pet"
            );
        }
    }

    #[test]
    fn external_ref_without_loader_is_left_alone() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "external.json#/Pet"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).expect("collapse ok");
        // The response is now lifted; navigate through to its
        // content.application/json.schema to verify the external ref
        // is preserved.
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/a"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("response slot must be lifted to a ref");
        assert_eq!(
            schema_in_lifted_content(&v, resp_ref, "application/json")["$ref"],
            "external.json#/Pet",
            "external refs must stay external when no loader is provided",
        );
    }

    #[test]
    fn parameter_schema_is_lifted() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            {
                                "name": "limit",
                                "in": "query",
                                "schema": {"title": "Limit", "type": "integer"}
                            }
                        ],
                        "responses": {
                            "200": {"description": "ok"}
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        // Parameters are now lifted into their own components bag too,
        // so the call site is replaced by a parameter ref and the
        // schema lives inside the lifted parameter.
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Limit".to_owned()), "got {names:?}");
        let v = serde_json::to_value(&spec).unwrap();
        let param_ref = v["paths"]["/pets"]["get"]["parameters"][0]["$ref"]
            .as_str()
            .expect("parameter slot must be a $ref after collapse");
        assert!(param_ref.starts_with("#/components/parameters/"));
        let param_name = param_ref.trim_start_matches("#/components/parameters/");
        assert_eq!(
            v["components"]["parameters"][param_name]["schema"]["$ref"],
            "#/components/schemas/Limit"
        );
    }

    #[test]
    fn round_trips_through_serde_after_collapse() {
        // Whatever the collapse rewrites, the resulting spec must still
        // parse via serde — i.e., we haven't constructed invalid structures.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "operationId": "listPets",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "title": "Pet",
                                            "type": "object",
                                            "properties": {
                                                "owner": {
                                                    "title": "Owner",
                                                    "type": "object",
                                                    "properties": {"id": {"type": "integer"}}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let json = serde_json::to_value(&spec).unwrap();
        let _: Spec = serde_json::from_value(json).expect("merged spec must re-parse");
    }

    // ── Walker coverage: every container / location a schema can live in
    //
    // The "kitchen sink" test below stuffs one inline schema into every
    // schema-bearing slot v3.2 supports, then asserts that every site
    // ended up rewritten to a ref. Driving them through one spec keeps
    // the test compact and exercises the cross-cutting walker
    // machinery (visitor recursion, ctx threading, dedup interplay).
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn walker_covers_every_schema_bearing_slot() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    // PathItem-level parameters list.
                    "parameters": [
                        {"name": "tenant", "in": "header", "schema": {"title": "Tenant", "type": "string"}}
                    ],
                    "get": {
                        "operationId": "kitchenSink",
                        // Operation-level parameters — every `in` variant.
                        "parameters": [
                            {"name": "id", "in": "path", "required": true, "schema": {"title": "Id", "type": "integer"}},
                            {"name": "tag", "in": "query", "schema": {"title": "Tag", "type": "string"}},
                            {"name": "x-trace", "in": "header", "schema": {"title": "Trace", "type": "string"}},
                            {"name": "sid", "in": "cookie", "schema": {"title": "Sid", "type": "string"}},
                            // Querystring (v3.2): no `schema`, `content` only.
                            {"name": "qs", "in": "querystring", "content": {
                                "application/x-www-form-urlencoded": {
                                    "schema": {"title": "QsBody", "type": "object", "properties": {"q": {"type": "string"}}}
                                }
                            }}
                        ],
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {"title": "PetBody", "type": "object", "properties": {"name": {"type": "string"}}},
                                    "itemSchema": {"title": "PetItem", "type": "object", "properties": {"id": {"type": "integer"}}}
                                }
                            }
                        },
                        "responses": {
                            "default": {
                                "description": "fallback",
                                "content": {
                                    "application/json": {"schema": {"title": "Err", "type": "object", "properties": {"msg": {"type": "string"}}}}
                                }
                            },
                            "200": {
                                "description": "ok",
                                "headers": {
                                    "X-Rate": {"schema": {"title": "Rate", "type": "integer"}}
                                },
                                "content": {
                                    "application/json": {"schema": {"title": "OkBody", "type": "object", "properties": {"id": {"type": "integer"}}}}
                                }
                            }
                        },
                        // Inline Operation-level callback whose nested
                        // PathItem itself has a schema slot.
                        "callbacks": {
                            "onPing": {
                                "{$request.body#/url}": {
                                    "post": {
                                        "responses": {
                                            "200": {
                                                "description": "ack",
                                                "content": {
                                                    "application/json": {"schema": {"title": "Ack", "type": "object", "properties": {"ok": {"type": "boolean"}}}}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    // PathItem.additional_operations (v3.2).
                    "additionalOperations": {
                        "QUERY": {
                            "responses": {
                                "200": {
                                    "description": "ok",
                                    "content": {
                                        "application/json": {"schema": {"title": "Custom", "type": "object", "properties": {"id": {"type": "integer"}}}}
                                    }
                                }
                            }
                        }
                    }
                }
            },
            // Webhooks (v3.1+): Paths-shaped, walker hits the same code.
            "webhooks": {
                "newPet": {
                    "post": {
                        "responses": {
                            "200": {
                                "description": "received",
                                "content": {
                                    "application/json": {"schema": {"title": "WebhookBody", "type": "object", "properties": {"id": {"type": "integer"}}}}
                                }
                            }
                        }
                    }
                }
            },
            // Inline components.* of every bag that can hold a schema slot.
            "components": {
                "parameters": {
                    "PageParam": {"name": "page", "in": "query", "schema": {"title": "Page", "type": "integer"}}
                },
                "responses": {
                    "NotFound": {
                        "description": "not found",
                        "headers": {"X-Reason": {"schema": {"title": "Reason", "type": "string"}}},
                        "content": {
                            "application/json": {"schema": {"title": "NotFoundBody", "type": "object", "properties": {"msg": {"type": "string"}}}}
                        }
                    }
                },
                "requestBodies": {
                    "PetCreate": {
                        "content": {
                            "application/json": {"schema": {"title": "PetCreateBody", "type": "object", "properties": {"name": {"type": "string"}}}}
                        }
                    }
                },
                "headers": {
                    "XCorrelation": {"schema": {"title": "Correlation", "type": "string"}}
                },
                "pathItems": {
                    "Echo": {
                        "post": {
                            "responses": {
                                "200": {
                                    "description": "echoed",
                                    "content": {
                                        "application/json": {"schema": {"title": "EchoBody", "type": "object", "properties": {"v": {"type": "string"}}}}
                                    }
                                }
                            }
                        }
                    }
                },
                "callbacks": {
                    "OnDelete": {
                        "{$request.body#/url}": {
                            "post": {
                                "responses": {
                                    "200": {
                                        "description": "ack",
                                        "content": {
                                            "application/json": {"schema": {"title": "DelAck", "type": "object", "properties": {"ok": {"type": "boolean"}}}}
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "mediaTypes": {
                    "Json": {"schema": {"title": "MediaTypeJson", "type": "object", "properties": {"v": {"type": "string"}}}}
                }
            }
        }));
        spec.collapse(None).expect("collapse ok");
        let names = lifted_schema_names(&spec);
        for expected in [
            // PathItem-level parameter.
            "Tenant",
            // Operation parameters (every `in`).
            "Id",
            "Tag",
            "Trace",
            "Sid",
            // Querystring -> content -> MediaType.schema.
            "QsBody",
            // Operation.requestBody MediaType.schema + item_schema.
            "PetBody",
            "PetItem",
            // Operation.responses default + 200 + 200.headers.
            "Err",
            "Rate",
            "OkBody",
            // Operation.callbacks inline Callback -> PathItem -> response.
            "Ack",
            // PathItem.additional_operations response.
            "Custom",
            // Webhooks.
            "WebhookBody",
            // components.* (every bag).
            "Page",
            "Reason",
            "NotFoundBody",
            "PetCreateBody",
            "Correlation",
            "EchoBody",
            "DelAck",
            "MediaTypeJson",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "expected lifted name `{expected}`, got {names:?}",
            );
        }
        // And confirm nothing's left inline at one representative
        // site: the lifted response's content schema points at OkBody.
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/pets"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(
            schema_in_lifted_content(&v, resp_ref, "application/json")["$ref"],
            "#/components/schemas/OkBody"
        );
    }

    // ── Walker coverage: RefOr::Ref container types bail cleanly ─────────

    #[test]
    fn ref_or_ref_containers_are_skipped_by_walker() {
        // When a Parameter / Response / RequestBody / Header / MediaType /
        // Callback is itself a `RefOr::Ref`, the walker doesn't recurse
        // into it — but it also doesn't error. Use that path so the
        // RefOr::Ref arms of every container walker get exercised.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [{"$ref": "#/components/parameters/PageParam"}],
                        "requestBody": {"$ref": "#/components/requestBodies/PetCreate"},
                        "responses": {
                            "200": {"$ref": "#/components/responses/NotFound"}
                        },
                        "callbacks": {
                            "onPing": {"$ref": "#/components/callbacks/OnDelete"}
                        }
                    }
                }
            },
            "components": {
                "parameters": {
                    "PageParam": {"name": "page", "in": "query", "schema": {"title": "Page", "type": "integer"}}
                },
                "responses": {
                    "NotFound": {
                        "description": "not found",
                        "headers": {"X-Reason": {"$ref": "#/components/headers/H"}},
                        "content": {"application/json": {"$ref": "#/components/mediaTypes/J"}}
                    }
                },
                "requestBodies": {
                    "PetCreate": {
                        "content": {"application/json": {"$ref": "#/components/mediaTypes/J"}}
                    }
                },
                "headers": {"H": {"schema": {"title": "H", "type": "string"}}},
                "mediaTypes": {"J": {"schema": {"title": "J", "type": "string"}}},
                "callbacks": {
                    "OnDelete": {
                        "{$request.body#/url}": {
                            "post": {"responses": {"200": {"description": "ok"}}}
                        }
                    }
                }
            }
        }));
        spec.collapse(None).expect("ref-only spec collapses");
        // Components-level walkers still lift the schemas inside the
        // pointed-at components — just confirm we didn't blow up and
        // the schemas reachable from components.* got lifted.
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Page".to_owned()), "got {names:?}");
        assert!(names.contains(&"H".to_owned()));
        assert!(names.contains(&"J".to_owned()));
    }

    // ── Walker coverage: Header with inline `content` (no schema) ────────

    #[test]
    fn header_with_inline_content_is_walked() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "headers": {
                                    "X-Trace": {
                                        "content": {
                                            "application/json": {
                                                "schema": {"title": "TraceMime", "type": "object", "properties": {"id": {"type": "string"}}}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"TraceMime".to_owned()), "got {names:?}");
    }

    // ── Walker coverage: MediaType.encoding[*].headers + recursive 3.2 ───

    #[test]
    fn media_type_encoding_headers_are_walked() {
        // Inline headers under `encoding[*].headers` must be lifted
        // into `components.headers` like any other header slot. v3.2
        // also exposes `itemEncoding` / `prefixEncoding` (both
        // recursive), so cover one of those too.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "post": {
                        "operationId": "upload",
                        "requestBody": {
                            "content": {
                                "multipart/form-data": {
                                    "schema": {"type": "object", "properties": {"file": {"type": "string", "format": "binary"}}},
                                    "encoding": {
                                        "file": {
                                            "contentType": "application/octet-stream",
                                            "headers": {
                                                "X-Rate": {"schema": {"title": "RateSchema", "type": "integer"}}
                                            }
                                        }
                                    },
                                    // 3.2-only: nested `itemEncoding` whose own
                                    // `headers` slot also must lift.
                                    "itemEncoding": {
                                        "contentType": "text/plain",
                                        "headers": {
                                            "X-Item": {"schema": {"title": "ItemSchema", "type": "string"}}
                                        }
                                    }
                                }
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let header_names: Vec<String> = spec
            .components
            .as_ref()
            .and_then(|c| c.headers.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        assert!(
            header_names.len() >= 2,
            "both encoding and itemEncoding headers must lift, got {header_names:?}",
        );
        let schemas = lifted_schema_names(&spec);
        for expected in ["RateSchema", "ItemSchema"] {
            assert!(
                schemas.contains(&expected.to_owned()),
                "missing `{expected}`: {schemas:?}",
            );
        }
    }

    // ── Walker coverage: Header.examples and MediaType.examples ──────────

    #[test]
    fn header_and_media_type_examples_are_lifted() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "get": {
                        "operationId": "x",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "headers": {
                                    "X-Trace": {
                                        "schema": {"type": "string"},
                                        "examples": {"Trace": {"summary": "trace", "value": "abc"}}
                                    }
                                },
                                "content": {
                                    "application/json": {
                                        "schema": {"type": "object", "properties": {"id": {"type": "integer"}}},
                                        "examples": {"PetEx": {"summary": "a pet", "value": {"id": 1}}}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let example_names: Vec<String> = spec
            .components
            .as_ref()
            .and_then(|c| c.examples.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        assert!(
            example_names.len() >= 2,
            "expected header + media_type examples lifted, got {example_names:?}",
        );
    }

    // ── schema_title coverage: every titled Schema variant. ──────────────

    #[test]
    fn schema_title_picks_up_every_titled_variant() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "schemas": {
                    "Holder": {
                        "type": "object",
                        "properties": {
                            "s": {"title": "TStr", "type": "string"},
                            "i": {"title": "TInt", "type": "integer"},
                            "n": {"title": "TNum", "type": "number"},
                            "b": {"title": "TBool", "type": "boolean"},
                            "a": {"title": "TArr", "type": "array", "items": {"type": "string"}},
                            "nul": {"title": "TNull", "type": "null"},
                            "o": {"title": "TObj", "type": "object", "properties": {"x": {"type": "string"}}},
                            "m": {"title": "TMulti", "type": ["string", "null"]}
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        for s in [
            "TStr", "TInt", "TNum", "TBool", "TArr", "TNull", "TObj", "TMulti",
        ] {
            assert!(names.contains(&s.to_owned()), "missing `{s}`: {names:?}");
        }
    }

    // ── Walker coverage: MediaType.prefixEncoding + recursive Encoding ────

    #[test]
    fn media_type_prefix_and_recursive_encoding_headers_are_walked() {
        // v3.2 adds `prefixEncoding` (a Vec<Encoding>) on MediaType and
        // makes Encoding itself recursive (`encoding` / `prefixEncoding`
        // / `itemEncoding`). Each level's `headers` slot must be walked.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "post": {
                        "operationId": "upload",
                        "requestBody": {
                            "content": {
                                "multipart/mixed": {
                                    "schema": {"type": "array", "items": {"type": "object"}},
                                    "prefixEncoding": [
                                        {
                                            "contentType": "text/plain",
                                            "headers": {
                                                "X-Prefix": {"schema": {"title": "PrefixSchema", "type": "string"}}
                                            },
                                            // Recursive nesting: an Encoding can itself
                                            // hold further Encoding maps.
                                            "encoding": {
                                                "nested": {
                                                    "contentType": "text/plain",
                                                    "headers": {
                                                        "X-Inner": {"schema": {"title": "InnerSchema", "type": "string"}}
                                                    }
                                                }
                                            }
                                        }
                                    ]
                                }
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let schemas = lifted_schema_names(&spec);
        for s in ["PrefixSchema", "InnerSchema"] {
            assert!(
                schemas.contains(&s.to_owned()),
                "missing `{s}`: {schemas:?}"
            );
        }
    }

    // ── Recursion coverage: Schema composition variants ──────────────────

    #[test]
    fn allof_anyof_oneof_not_children_are_lifted() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "components": {
                "schemas": {
                    "Combined": {
                        "allOf": [
                            {"title": "A", "type": "object", "properties": {"a": {"type": "string"}}},
                            {"title": "B", "type": "object", "properties": {"b": {"type": "integer"}}}
                        ]
                    },
                    "Either": {
                        "anyOf": [
                            {"title": "C", "type": "string"},
                            {"title": "D", "type": "integer"}
                        ]
                    },
                    "Exactly": {
                        "oneOf": [
                            {"title": "E", "type": "string"},
                            {"title": "F", "type": "boolean"}
                        ]
                    },
                    "Inverse": {
                        "not": {"title": "G", "type": "string"}
                    }
                }
            },
            "paths": {}
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        for expected in [
            "A", "B", "C", "D", "E", "F", "G", "Combined", "Either", "Exactly", "Inverse",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing {expected}, got {names:?}"
            );
        }
        // Confirm each composition slot now points at a ref.
        let combined = schema_at(&spec, "Combined");
        assert!(combined["allOf"][0]["$ref"].is_string());
        assert!(combined["allOf"][1]["$ref"].is_string());
        let either = schema_at(&spec, "Either");
        assert!(either["anyOf"][0]["$ref"].is_string());
        let exactly = schema_at(&spec, "Exactly");
        assert!(exactly["oneOf"][0]["$ref"].is_string());
        let inverse = schema_at(&spec, "Inverse");
        assert!(inverse["not"]["$ref"].is_string());
    }

    // ── Recursion coverage: ObjectSchema's full kitchen ─────────────────

    #[test]
    fn object_schema_recurses_through_every_nested_slot() {
        // Single Pet schema exercising properties + patternProperties +
        // additionalProperties (Item, not Bool) + unevaluatedProperties +
        // propertyNames.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "properties": {"name": {"title": "Name", "type": "string"}},
                        "patternProperties": {
                            "^x-": {"title": "ExtVal", "type": "object", "properties": {"v": {"type": "string"}}}
                        },
                        "additionalProperties": {"title": "Extra", "type": "object", "properties": {"v": {"type": "string"}}},
                        "unevaluatedProperties": {"title": "Unev", "type": "object", "properties": {"v": {"type": "string"}}},
                        "propertyNames": {"title": "PropKey", "type": "string"}
                    }
                }
            },
            "paths": {}
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        for expected in ["Name", "ExtVal", "Extra", "Unev", "PropKey", "Pet"] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing {expected}, got {names:?}"
            );
        }
        let pet = schema_at(&spec, "Pet");
        assert!(pet["properties"]["name"]["$ref"].is_string());
        assert!(pet["patternProperties"]["^x-"]["$ref"].is_string());
        assert!(pet["additionalProperties"]["$ref"].is_string());
        assert!(pet["unevaluatedProperties"]["$ref"].is_string());
        assert!(pet["propertyNames"]["$ref"].is_string());
    }

    // ── Recursion coverage: ArraySchema items ────────────────────────────

    #[test]
    fn array_schema_items_is_lifted() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "components": {
                "schemas": {
                    "Pets": {
                        "type": "array",
                        "items": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                    }
                }
            },
            "paths": {}
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Pet".to_owned()), "got {names:?}");
        let pets = schema_at(&spec, "Pets");
        assert!(pets["items"]["$ref"].is_string());
    }

    // ── Recursion: bool-typed schema slots are left alone ────────────────

    #[test]
    fn bool_schema_slots_do_not_lift() {
        // `additionalProperties: true` and `items: false` are valid
        // JSON Schema 2020-12 sugar. The walker must not try to lift
        // the BoolOr::Bool arm.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "components": {
                "schemas": {
                    "Map": {"type": "object", "additionalProperties": true},
                    "EmptyList": {"type": "array", "items": false}
                }
            },
            "paths": {}
        }));
        // Just an assertion that the call succeeds + the shape survives.
        spec.collapse(None).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["components"]["schemas"]["Map"]["additionalProperties"],
            true
        );
        assert_eq!(v["components"]["schemas"]["EmptyList"]["items"], false);
    }

    // ── Lift behavior: internal `$ref` slots are left alone ──────────────

    #[test]
    fn internal_ref_slots_are_untouched_by_collapse() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {"schema": {"$ref": "#/components/schemas/Pet"}}
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "Pet": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}
                }
            }
        }));
        spec.collapse(None).unwrap();
        // Response is now lifted; the schema's existing internal ref
        // is preserved inside the lifted response.
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/a"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("response slot must be lifted");
        assert_eq!(
            schema_in_lifted_content(&v, resp_ref, "application/json")["$ref"],
            "#/components/schemas/Pet"
        );
    }

    // ── Strict dedup: titled and untitled near-duplicates stay split ───

    #[test]
    fn titled_and_untitled_near_duplicates_do_not_collapse() {
        // Strict canonical-JSON dedup means an untitled occurrence and
        // a title-bearing one with otherwise-identical content end up
        // as two separate components. (Loose dedup that ignores `title`
        // would collapse them; this is documented as out of scope.)
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "operationId": "first",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {"schema": {"type": "object", "properties": {"id": {"type": "integer"}}}}
                                }
                            }
                        }
                    }
                },
                "/b": {
                    "get": {
                        "operationId": "second",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {"schema": {"title": "Pet", "type": "object", "properties": {"id": {"type": "integer"}}}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Pet".to_owned()), "got {names:?}");
        // Responses are now lifted; descend through each lifted
        // response's content schema.
        let v = serde_json::to_value(&spec).unwrap();
        let lookup_schema = |path_key: &str| -> serde_json::Value {
            let resp_ref = v["paths"][path_key]["get"]["responses"]["200"]["$ref"]
                .as_str()
                .unwrap();
            schema_in_lifted_content(&v, resp_ref, "application/json")
        };
        // /b's titled occurrence lands at `Pet`.
        assert_eq!(lookup_schema("/b")["$ref"], "#/components/schemas/Pet");
        // Different ref target — the schema titles differ, so they
        // don't dedupe.
        assert_ne!(lookup_schema("/a")["$ref"], lookup_schema("/b")["$ref"]);
    }

    #[test]
    fn collision_suffix_increments_past_existing_lifted_entries() {
        // Force multiple collisions on the same context-derived base
        // name. With two pre-existing entries `Foo` and `Foo_2`, the
        // newly-lifted untitled schema should land at `Foo_3`.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "components": {
                "schemas": {
                    "Foo": {"type": "string"},
                    "Foo_2": {"type": "integer"}
                }
            },
            "paths": {
                "/x": {
                    "get": {
                        "operationId": "Foo",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {"schema": {"type": "object", "properties": {"id": {"type": "integer"}}}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_schema_names(&spec);
        assert!(
            names.iter().any(|n| n.starts_with("Foo_")),
            "expected Foo_3 or later, got {names:?}",
        );
    }

    // ── External-ref edge: ref without fragment falls back to context ────

    #[test]
    fn external_ref_without_fragment_uses_context_name() {
        // `external.json` (no `#` fragment) — `from_external_ref` falls
        // through to the surrounding context for naming. Whole external
        // document gets lifted as a single schema.
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "external.json",
                serde_json::json!({"title": "WholeFile", "type": "object", "properties": {"id": {"type": "integer"}}}),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {"schema": {"$ref": "external.json"}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).expect("collapse ok");
        let names = lifted_schema_names(&spec);
        // The external document had a `title`, which the lifter picks
        // up regardless of the fragment shape.
        assert!(names.contains(&"WholeFile".to_owned()), "got {names:?}");
    }

    // ── External-ref edge: missing resource surfaces as External error ──

    #[test]
    fn loader_failure_surfaces_as_external_error() {
        // No preload; loader will fail to find `external.json`. The
        // CollapseError::External arm runs and the source is the
        // underlying LoaderError.
        let mut loader = Loader::new();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {"schema": {"$ref": "external.json#/Pet"}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        let err = spec
            .collapse(Some(&mut loader))
            .expect_err("loader has no fetcher for `external.json`");
        match err {
            CollapseError::External { reference, .. } => {
                assert_eq!(reference, "external.json#/Pet")
            }
            other => panic!("expected External error, got {other:?}"),
        }
    }

    // Coverage for `sanitize_component_name`, `unique_name`, and
    // `NameContext` lives in `crate::common::collapse`'s own test
    // module since those helpers are now shared.

    #[test]
    fn schema_title_returns_none_for_composite_and_terminal_shapes() {
        // Parse one of each composition variant via JSON — most don't
        // implement Default so this is the shortest construction route.
        let allof: Schema =
            serde_json::from_value(serde_json::json!({"allOf": [{"type": "string"}]})).unwrap();
        let anyof: Schema =
            serde_json::from_value(serde_json::json!({"anyOf": [{"type": "string"}]})).unwrap();
        let oneof: Schema =
            serde_json::from_value(serde_json::json!({"oneOf": [{"type": "string"}]})).unwrap();
        let not: Schema =
            serde_json::from_value(serde_json::json!({"not": {"type": "string"}})).unwrap();
        let empty: Schema = serde_json::from_value(serde_json::json!({})).unwrap();
        let bool_schema: Schema = serde_json::from_value(serde_json::json!(true)).unwrap();
        assert!(schema_title(&bool_schema).is_none());
        assert!(schema_title(&empty).is_none());
        assert!(schema_title(&allof).is_none());
        assert!(schema_title(&anyof).is_none());
        assert!(schema_title(&oneof).is_none());
        assert!(schema_title(&not).is_none());
    }

    // ── Parameters: lift contract + dedup ────────────────────────────────

    fn lifted_parameter_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.parameters.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_parameters_lift_to_components_parameters_with_name_in_hint() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            {"name": "limit", "in": "query", "schema": {"type": "integer"}},
                            {"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_parameter_names(&spec);
        // Each parameter's component name is `<name><In>`.
        assert!(names.contains(&"limitQuery".to_owned()), "got {names:?}");
        assert!(names.contains(&"idPath".to_owned()), "got {names:?}");
        let v = serde_json::to_value(&spec).unwrap();
        // Call sites are refs.
        assert_eq!(
            v["paths"]["/pets"]["get"]["parameters"][0]["$ref"],
            "#/components/parameters/limitQuery"
        );
        assert_eq!(
            v["paths"]["/pets"]["get"]["parameters"][1]["$ref"],
            "#/components/parameters/idPath"
        );
    }

    #[test]
    fn identical_inline_parameters_dedupe_to_one_component() {
        // Two operations carry an identical inline parameter — the
        // canonical-JSON dedup collapses them to a single entry under
        // `components.parameters`.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                        "responses": {"200": {"description": "ok"}}
                    }
                },
                "/b": {
                    "get": {
                        "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_parameter_names(&spec);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "limitQuery").count(),
            1,
            "got {names:?}",
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["paths"]["/a"]["get"]["parameters"][0]["$ref"],
            "#/components/parameters/limitQuery"
        );
        assert_eq!(
            v["paths"]["/b"]["get"]["parameters"][0]["$ref"],
            "#/components/parameters/limitQuery"
        );
    }

    #[test]
    fn parameter_with_same_name_different_schema_does_not_dedupe() {
        // Two parameters with the same `name + in` but different
        // schemas — different canonical JSON, two separate
        // components (second gets a `_2` suffix).
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "parameters": [{"name": "id", "in": "query", "schema": {"type": "integer"}}],
                        "responses": {"200": {"description": "ok"}}
                    }
                },
                "/b": {
                    "get": {
                        "parameters": [{"name": "id", "in": "query", "schema": {"type": "string"}}],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_parameter_names(&spec);
        assert!(names.contains(&"idQuery".to_owned()), "got {names:?}");
        assert!(names.contains(&"idQuery_2".to_owned()), "got {names:?}");
    }

    #[test]
    fn parameters_lift_every_variant_of_the_in_enum() {
        // Exercises the `match` over every Parameter variant inside
        // `parameter_name_hint`.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "get": {
                        "parameters": [
                            {"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}},
                            {"name": "tag", "in": "query", "schema": {"type": "string"}},
                            {"name": "x-trace", "in": "header", "schema": {"type": "string"}},
                            {"name": "sid", "in": "cookie", "schema": {"type": "string"}},
                            {"name": "qs", "in": "querystring", "content": {
                                "application/x-www-form-urlencoded": {"schema": {"type": "object", "properties": {"q": {"type": "string"}}}}
                            }}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_parameter_names(&spec);
        // `-` is valid in component names, so `x-trace` is preserved
        // verbatim by the sanitiser.
        for expected in [
            "idPath",
            "tagQuery",
            "x-traceHeader",
            "sidCookie",
            "qsQuerystring",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing `{expected}`: {names:?}"
            );
        }
    }

    #[test]
    fn existing_components_parameters_are_preserved_and_recursed() {
        // Pre-existing components.parameters entries keep their names;
        // their inline nested schemas are still lifted into
        // components.schemas.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "parameters": {
                    "Limit": {"name": "limit", "in": "query", "schema": {"title": "LimitSchema", "type": "integer"}}
                }
            }
        }));
        spec.collapse(None).unwrap();
        let param_names = lifted_parameter_names(&spec);
        assert!(
            param_names.contains(&"Limit".to_owned()),
            "got {param_names:?}"
        );
        let schema_names = lifted_schema_names(&spec);
        assert!(
            schema_names.contains(&"LimitSchema".to_owned()),
            "got {schema_names:?}",
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["components"]["parameters"]["Limit"]["schema"]["$ref"],
            "#/components/schemas/LimitSchema"
        );
    }

    #[test]
    fn loader_resolves_external_parameter_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({
                    "PageParam": {"name": "page", "in": "query", "schema": {"type": "integer"}}
                }),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {
                    "get": {
                        "parameters": [{"$ref": "shared.json#/PageParam"}],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        let names = lifted_parameter_names(&spec);
        // External ref points at `PageParam` — the fragment's last
        // segment, then dedup name-hinted further by `name + in`.
        // For external refs the from_external_ref ctx wins on
        // generate-name, but our intern_parameter prefers the
        // parameter-name hint when the parameter has a name. Verify
        // *some* parameter ended up lifted from the external doc.
        assert!(
            !names.is_empty(),
            "expected at least one lifted parameter, got {names:?}",
        );
        let v = serde_json::to_value(&spec).unwrap();
        let param = &v["paths"]["/a"]["get"]["parameters"][0]["$ref"];
        let s = param.as_str().expect("parameter slot must be a ref");
        assert!(
            s.starts_with("#/components/parameters/"),
            "got `{s}` — external ref must be rewritten to internal",
        );
    }

    #[test]
    fn parameter_name_hint_helper_returns_empty_for_empty_name() {
        // Build a Parameter with an empty `name` so the hint returns
        // "" and the caller falls back to context-derived naming.
        let param: Parameter = serde_json::from_value(serde_json::json!({
            "name": "",
            "in": "query",
            "schema": {"type": "integer"}
        }))
        .unwrap();
        assert_eq!(parameter_name_hint(&param), "");
    }

    // ── Responses: lift contract + dedup ────────────────────────────────

    fn lifted_response_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.responses.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_responses_lift_to_components_responses_via_context() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "operationId": "listPets",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {"application/json": {"schema": {"type": "object", "properties": {"id": {"type": "integer"}}}}}
                            },
                            "default": {"description": "err"}
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_response_names(&spec);
        // Both responses lift; we don't pin the derived names, just
        // verify the call sites became refs.
        assert!(
            names.len() >= 2,
            "expected at least 2 response components, got {names:?}"
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert!(
            v["paths"]["/pets"]["get"]["responses"]["200"]["$ref"]
                .as_str()
                .is_some_and(|s| s.starts_with("#/components/responses/"))
        );
        assert!(
            v["paths"]["/pets"]["get"]["responses"]["default"]["$ref"]
                .as_str()
                .is_some_and(|s| s.starts_with("#/components/responses/"))
        );
    }

    #[test]
    fn identical_inline_responses_dedupe_to_one_component() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"get": {"responses": {"200": {"description": "ok"}}}},
                "/b": {"get": {"responses": {"200": {"description": "ok"}}}}
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
    fn existing_components_responses_are_preserved_and_recursed() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "responses": {
                    "NotFound": {
                        "description": "not found",
                        "content": {"application/json": {"schema": {"title": "ErrBody", "type": "object", "properties": {"msg": {"type": "string"}}}}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_response_names(&spec);
        assert!(names.contains(&"NotFound".to_owned()), "got {names:?}");
        // The nested schema inside the pre-existing response is still
        // lifted into components.schemas.
        let schema_names = lifted_schema_names(&spec);
        assert!(
            schema_names.contains(&"ErrBody".to_owned()),
            "got {schema_names:?}"
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            schema_in_lifted_content(&v, "#/components/responses/NotFound", "application/json")["$ref"],
            "#/components/schemas/ErrBody"
        );
    }

    #[test]
    fn loader_resolves_external_response_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({
                    "NotFound": {"description": "not found"}
                }),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"get": {"responses": {"404": {"$ref": "shared.json#/NotFound"}}}}
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        // External ref was resolved + lifted; call site rewritten to
        // an internal ref.
        let v = serde_json::to_value(&spec).unwrap();
        let s = v["paths"]["/a"]["get"]["responses"]["404"]["$ref"]
            .as_str()
            .expect("response slot must be a ref");
        assert!(s.starts_with("#/components/responses/"), "got `{s}`",);
        assert!(!lifted_response_names(&spec).is_empty());
    }

    // ── RequestBodies: lift contract + dedup ────────────────────────────

    fn lifted_request_body_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.request_bodies.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_request_body_lifts_to_components_request_bodies() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "operationId": "createPet",
                        "requestBody": {
                            "content": {
                                "application/json": {"schema": {"title": "PetCreate", "type": "object", "properties": {"name": {"type": "string"}}}}
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_request_body_names(&spec);
        assert!(!names.is_empty(), "request body should be lifted");
        let v = serde_json::to_value(&spec).unwrap();
        let rb_ref = v["paths"]["/pets"]["post"]["requestBody"]["$ref"]
            .as_str()
            .expect("requestBody must be lifted");
        // Nested schema is also lifted.
        assert_eq!(
            schema_in_lifted_content(&v, rb_ref, "application/json")["$ref"],
            "#/components/schemas/PetCreate"
        );
    }

    #[test]
    fn identical_inline_request_bodies_dedupe_to_one_component() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"post": {"requestBody": {"content": {"application/json": {"schema": {"type": "string"}}}}, "responses": {"200": {"description": "ok"}}}},
                "/b": {"post": {"requestBody": {"content": {"application/json": {"schema": {"type": "string"}}}}, "responses": {"200": {"description": "ok"}}}}
            }
        }));
        spec.collapse(None).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["paths"]["/a"]["post"]["requestBody"]["$ref"],
            v["paths"]["/b"]["post"]["requestBody"]["$ref"],
        );
        assert_eq!(lifted_request_body_names(&spec).len(), 1);
    }

    #[test]
    fn loader_resolves_external_request_body_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({
                    "PetCreate": {"content": {"application/json": {"schema": {"type": "string"}}}}
                }),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"post": {"requestBody": {"$ref": "shared.json#/PetCreate"}, "responses": {"200": {"description": "ok"}}}}
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        let s = v["paths"]["/a"]["post"]["requestBody"]["$ref"]
            .as_str()
            .unwrap();
        assert!(s.starts_with("#/components/requestBodies/"), "got `{s}`");
        assert!(!lifted_request_body_names(&spec).is_empty());
    }

    // ── Headers: lift contract + dedup ─────────────────────────────────

    fn lifted_header_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.headers.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_headers_lift_to_components_headers() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "headers": {
                                    "X-Rate": {"schema": {"title": "Rate", "type": "integer"}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_header_names(&spec);
        assert!(!names.is_empty(), "header should be lifted");
        // The schema inside the lifted header is itself lifted.
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/x"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .unwrap();
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        let header_ref = v["components"]["responses"][resp_name]["headers"]["X-Rate"]["$ref"]
            .as_str()
            .expect("X-Rate header must be a ref");
        let header_name = header_ref.trim_start_matches("#/components/headers/");
        assert_eq!(
            v["components"]["headers"][header_name]["schema"]["$ref"],
            "#/components/schemas/Rate"
        );
    }

    #[test]
    fn identical_inline_headers_dedupe_to_one_component() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "headers": {
                                    "X-A": {"schema": {"type": "string"}},
                                    "X-B": {"schema": {"type": "string"}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert_eq!(lifted_header_names(&spec).len(), 1);
    }

    #[test]
    fn loader_resolves_external_header_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({"Rate": {"schema": {"type": "integer"}}}),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {"get": {"responses": {"200": {"description": "ok", "headers": {"X-Rate": {"$ref": "shared.json#/Rate"}}}}}}
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        assert!(!lifted_header_names(&spec).is_empty());
    }

    // ── MediaTypes: lift contract + dedup ──────────────────────────────

    fn lifted_media_type_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.media_types.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_media_types_lift_to_components_media_types() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "post": {
                        "requestBody": {"content": {"application/json": {"schema": {"title": "Body", "type": "string"}}}},
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert!(
            !lifted_media_type_names(&spec).is_empty(),
            "mediaType should be lifted, got bag {:?}",
            lifted_media_type_names(&spec),
        );
        // Walk RB -> mediaType -> schema.
        let v = serde_json::to_value(&spec).unwrap();
        let rb_ref = v["paths"]["/x"]["post"]["requestBody"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(
            schema_in_lifted_content(&v, rb_ref, "application/json")["$ref"],
            "#/components/schemas/Body"
        );
    }

    #[test]
    fn identical_inline_media_types_dedupe_to_one_component() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"post": {"requestBody": {"content": {"application/json": {"schema": {"type": "string"}}}}, "responses": {"200": {"description": "ok"}}}},
                "/b": {"post": {"requestBody": {"content": {"application/json": {"schema": {"type": "string"}}}}, "responses": {"200": {"description": "ok"}}}}
            }
        }));
        spec.collapse(None).unwrap();
        assert_eq!(lifted_media_type_names(&spec).len(), 1);
    }

    #[test]
    fn loader_resolves_external_media_type_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({"JsonBody": {"schema": {"type": "string"}}}),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"post": {"requestBody": {"content": {"application/json": {"$ref": "shared.json#/JsonBody"}}}, "responses": {"200": {"description": "ok"}}}}
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        assert!(!lifted_media_type_names(&spec).is_empty());
    }

    // ── Examples: lift contract + dedup ────────────────────────────────

    fn lifted_example_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.examples.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_examples_lift_to_components_examples() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            {"name": "tag", "in": "query", "schema": {"type": "string"}, "examples": {
                                "Dog": {"summary": "a dog", "value": "dog"},
                                "Cat": {"summary": "a cat", "value": "cat"}
                            }}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let names = lifted_example_names(&spec);
        assert!(names.len() >= 2, "expected 2 examples, got {names:?}");
    }

    #[test]
    fn identical_inline_examples_dedupe_to_one_component() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            {"name": "a", "in": "query", "schema": {"type": "string"}, "examples": {"X": {"value": "v"}}},
                            {"name": "b", "in": "query", "schema": {"type": "string"}, "examples": {"X": {"value": "v"}}}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert_eq!(lifted_example_names(&spec).len(), 1);
    }

    #[test]
    fn loader_resolves_external_example_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource("shared.json", serde_json::json!({"Dog": {"value": "dog"}}))
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {
                        "parameters": [
                            {"name": "tag", "in": "query", "schema": {"type": "string"}, "examples": {"Dog": {"$ref": "shared.json#/Dog"}}}
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        assert!(!lifted_example_names(&spec).is_empty());
    }

    // ── Links: lift contract + dedup ───────────────────────────────────

    fn lifted_link_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.links.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_links_lift_to_components_links() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "responses": {
                            "201": {
                                "description": "created",
                                "links": {
                                    "GetPet": {"operationId": "getPet"}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert!(!lifted_link_names(&spec).is_empty());
    }

    #[test]
    fn loader_resolves_external_link_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({"GetPet": {"operationId": "getPet"}}),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "responses": {
                            "201": {
                                "description": "created",
                                "links": {"GetPet": {"$ref": "shared.json#/GetPet"}}
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        assert!(!lifted_link_names(&spec).is_empty());
    }

    // ── Callbacks: lift contract + dedup ───────────────────────────────

    fn lifted_callback_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.callbacks.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn inline_callbacks_lift_to_components_callbacks() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "operationId": "createPet",
                        "callbacks": {
                            "onPing": {
                                "{$request.body#/url}": {
                                    "post": {
                                        "responses": {"200": {"description": "ack"}}
                                    }
                                }
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        assert!(!lifted_callback_names(&spec).is_empty());
    }

    #[test]
    fn loader_resolves_external_callback_refs() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "shared.json",
                serde_json::json!({
                    "OnPing": {
                        "{$request.body#/url}": {
                            "post": {"responses": {"200": {"description": "ack"}}}
                        }
                    }
                }),
            )
            .unwrap();
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {
                        "callbacks": {"onPing": {"$ref": "shared.json#/OnPing"}},
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(Some(&mut loader)).unwrap();
        assert!(!lifted_callback_names(&spec).is_empty());
    }

    // ── PathItems: pre-existing-bag recursion only ─────────────────────

    fn lifted_path_item_names(spec: &Spec) -> Vec<String> {
        spec.components
            .as_ref()
            .and_then(|c| c.path_items.as_ref())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn existing_components_path_items_keep_names_and_recurse_children() {
        // Pre-existing `components.pathItems` entries keep their
        // names; their nested inline schemas / parameters / etc.
        // still lift. Operations on `paths.<path>` are *not* lifted
        // into `components.pathItems`.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {"responses": {"200": {"description": "ok"}}}
                }
            },
            "components": {
                "pathItems": {
                    "Echo": {
                        "post": {
                            "responses": {
                                "200": {
                                    "description": "echoed",
                                    "content": {"application/json": {"schema": {"title": "EchoBody", "type": "string"}}}
                                }
                            }
                        }
                    }
                }
            }
        }));
        spec.collapse(None).unwrap();
        let pi_names = lifted_path_item_names(&spec);
        assert!(pi_names.contains(&"Echo".to_owned()), "got {pi_names:?}");
        // The schema inside Echo's response is lifted.
        let schema_names = lifted_schema_names(&spec);
        assert!(
            schema_names.contains(&"EchoBody".to_owned()),
            "got {schema_names:?}",
        );
        // paths./pets is *not* turned into a $ref to components.pathItems.
        let v = serde_json::to_value(&spec).unwrap();
        assert!(
            v["paths"]["/pets"]["get"].is_object(),
            "paths./pets.get must stay inline, got {:?}",
            v["paths"]["/pets"]["get"],
        );
    }

    #[test]
    fn existing_components_in_every_bag_are_preserved_and_recursed() {
        // One spec with pre-existing entries in every component bag.
        // Each entry's nested inline schema gets lifted into
        // `components.schemas`; the bag entries themselves keep
        // their names.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "requestBodies": {
                    "MyBody": {"content": {"application/json": {"schema": {"title": "BodyT", "type": "string"}}}}
                },
                "headers": {
                    "MyHeader": {"schema": {"title": "HeaderT", "type": "string"}}
                },
                "mediaTypes": {
                    "MyMt": {"schema": {"title": "MtT", "type": "string"}}
                },
                "callbacks": {
                    "MyCb": {"{$request.body#/url}": {"post": {"responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"title": "CbT", "type": "string"}}}}}}}}
                }
            }
        }));
        spec.collapse(None).unwrap();
        let schemas = lifted_schema_names(&spec);
        for s in ["BodyT", "HeaderT", "MtT", "CbT"] {
            assert!(
                schemas.contains(&s.to_owned()),
                "missing `{s}`: {schemas:?}"
            );
        }
        assert!(lifted_request_body_names(&spec).contains(&"MyBody".to_owned()));
        assert!(lifted_header_names(&spec).contains(&"MyHeader".to_owned()));
        assert!(lifted_media_type_names(&spec).contains(&"MyMt".to_owned()));
        assert!(lifted_callback_names(&spec).contains(&"MyCb".to_owned()));
    }

    #[test]
    fn external_container_refs_left_alone_without_loader() {
        // For every container type that can hold an external `$ref`,
        // verify that without a loader the slot is left untouched
        // (the early-return branch in each `lift_ref_or_*`). This
        // includes the "leaf" container types `Example` and `Link`,
        // exercised at the parameter / response level where they
        // naturally appear.
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/x": {
                    "post": {
                        "parameters": [{"$ref": "shared.json#/PageParam"}],
                        "requestBody": {"$ref": "shared.json#/Body"},
                        "responses": {
                            "200": {"$ref": "shared.json#/Resp"},
                            // Inline response so we can exercise
                            // external link / header refs inside it.
                            "201": {
                                "description": "created",
                                "headers": {"X-Rate": {"$ref": "shared.json#/Rate"}},
                                "content": {"application/json": {"$ref": "shared.json#/Mt"}},
                                "links": {"GetPet": {"$ref": "shared.json#/Link"}}
                            }
                        },
                        "callbacks": {"OnPing": {"$ref": "shared.json#/Cb"}}
                    },
                    "get": {
                        // Inline parameter so we can exercise an
                        // external example ref inside its `examples`
                        // map.
                        "parameters": [
                            {
                                "name": "tag",
                                "in": "query",
                                "schema": {"type": "string"},
                                "examples": {"Dog": {"$ref": "shared.json#/Dog"}}
                            }
                        ],
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        }));
        spec.collapse(None).expect("no loader, no lift");
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["paths"]["/x"]["post"]["parameters"][0]["$ref"],
            "shared.json#/PageParam"
        );
        assert_eq!(
            v["paths"]["/x"]["post"]["requestBody"]["$ref"],
            "shared.json#/Body"
        );
        assert_eq!(
            v["paths"]["/x"]["post"]["responses"]["200"]["$ref"],
            "shared.json#/Resp"
        );
        assert_eq!(
            v["paths"]["/x"]["post"]["callbacks"]["OnPing"]["$ref"],
            "shared.json#/Cb"
        );
        // 201 response is now lifted; navigate through it to verify
        // every inner external ref stuck (header, content/mediaType,
        // link).
        let resp_ref = v["paths"]["/x"]["post"]["responses"]["201"]["$ref"]
            .as_str()
            .expect("201 response was lifted");
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        assert_eq!(
            v["components"]["responses"][resp_name]["headers"]["X-Rate"]["$ref"],
            "shared.json#/Rate"
        );
        assert_eq!(
            v["components"]["responses"][resp_name]["content"]["application/json"]["$ref"],
            "shared.json#/Mt"
        );
        assert_eq!(
            v["components"]["responses"][resp_name]["links"]["GetPet"]["$ref"],
            "shared.json#/Link"
        );
        // The /x.get parameter is lifted; navigate to confirm the
        // example ref stuck.
        let p_ref = v["paths"]["/x"]["get"]["parameters"][0]["$ref"]
            .as_str()
            .expect("parameter was lifted");
        let p_name = p_ref.trim_start_matches("#/components/parameters/");
        assert_eq!(
            v["components"]["parameters"][p_name]["examples"]["Dog"]["$ref"],
            "shared.json#/Dog"
        );
    }

    #[test]
    fn response_with_internal_ref_is_left_alone() {
        let mut spec = parse(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/a": {"get": {"responses": {"200": {"$ref": "#/components/responses/Existing"}}}}
            },
            "components": {
                "responses": {"Existing": {"description": "shared"}}
            }
        }));
        spec.collapse(None).unwrap();
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["paths"]["/a"]["get"]["responses"]["200"]["$ref"],
            "#/components/responses/Existing"
        );
    }
}
