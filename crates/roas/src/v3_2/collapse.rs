//! `Spec::collapse` — lift every inline component out of the spec tree
//! into `components.<bag>`.
//!
//! Walks the entire spec, replacing each inline `RefOr::Item<T>` with a
//! `RefOr::Ref` pointing at a freshly-interned entry under
//! `components.<bag>.<name>`. Schemas get a name from `schema.title`
//! when present; every other component type derives its name from a
//! sanitised dot-joined path through the spec tree. Structurally
//! identical components (serde-canonical JSON equality) collapse to a
//! single entry; every call site that previously held the same inline
//! shape now points at the same `$ref`.
//!
//! When the caller passes a [`Loader`], every external `$ref` (anything
//! not starting with `#`) is fetched, parsed as the bag's concrete
//! type, run through the same recursion + dedup pipeline, and rewritten
//! as a local `#/components/<bag>/<name>` ref. The dedup map is shared
//! between lifted inline values and fetched external ones, so two
//! structurally identical sources collapse together.
//!
//! Bags lifted in this module today: `schemas`, `parameters`,
//! `responses`, `requestBodies`, `headers`. Recursion goes leaf-up
//! — schemas are lifted before their containing parameters /
//! headers / responses / requestBodies, so each parent's canonical
//! JSON has its children already substituted for refs before it goes
//! through dedup. `examples`, `links`, `callbacks`, `pathItems`,
//! `mediaTypes` follow in subsequent commits.
//!
//! This is the OAS 3.2 implementation; v3.0 / v3.1 / v2 follow in
//! subsequent PRs.

use std::collections::{BTreeMap, HashMap};
use std::mem;

use crate::common::bool_or::BoolOr;
use crate::common::reference::{Ref, RefOr};
use crate::loader::{Loader, LoaderError};
use crate::v3_2::callback::Callback;
use crate::v3_2::components::Components;
use crate::v3_2::header::Header;
use crate::v3_2::media_type::MediaType;
use crate::v3_2::operation::Operation;
use crate::v3_2::parameter::Parameter;
use crate::v3_2::path_item::{PathItem, Paths};
use crate::v3_2::request_body::RequestBody;
use crate::v3_2::response::{Response, Responses};
use crate::v3_2::schema::{ArraySchema, ObjectSchema, Schema, SingleSchema};
use crate::v3_2::spec::Spec;

const SCHEMA_PREFIX: &str = "#/components/schemas/";

/// Error returned by [`Spec::collapse`](crate::v3_2::spec::Spec::collapse).
///
/// Only fallible legs are loader-driven external-ref resolution and
/// JSON serialisation of a schema for dedup; inline tree-rewriting
/// itself never fails.
#[derive(Debug, thiserror::Error)]
pub enum CollapseError {
    /// The loader was invoked to resolve an external `$ref` and failed —
    /// no fetcher registered, fetch error, parse error, or missing JSON
    /// Pointer target. The underlying `LoaderError` is exposed as the
    /// error source.
    #[error("failed to resolve external reference `{reference}`")]
    External {
        reference: String,
        #[source]
        source: LoaderError,
    },

    /// A schema couldn't be serialised to JSON for the dedup map. In
    /// practice every concrete `Schema` is `Serialize` so this only
    /// surfaces under custom serde error paths; it's exposed rather
    /// than panicked on so callers can decide their own fallback.
    #[error("failed to serialise schema for dedup")]
    Serialize(#[from] serde_json::Error),
}

/// Crate-internal entrypoint. The public surface is
/// [`Spec::collapse`](crate::v3_2::spec::Spec::collapse), which
/// thin-wraps this.
pub(crate) fn collapse_spec(
    spec: &mut Spec,
    loader: Option<&mut Loader>,
) -> Result<(), CollapseError> {
    // Take each existing components bag out of the spec so the
    // Collapser owns it mutably while we walk the rest of the tree.
    // We write each one back at the very end.
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

    let mut collapser = Collapser {
        schemas: BTreeMap::new(),
        schemas_seen: HashMap::new(),
        parameters: BTreeMap::new(),
        parameters_seen: HashMap::new(),
        responses: BTreeMap::new(),
        responses_seen: HashMap::new(),
        request_bodies: BTreeMap::new(),
        request_bodies_seen: HashMap::new(),
        headers: BTreeMap::new(),
        headers_seen: HashMap::new(),
        loader,
    };

    // ── Phase 1: seed pre-existing components.* bags ────────────────
    // Each pre-existing entry keeps its name; we seed the dedup map so
    // newly-lifted equivalents collapse onto the existing names.
    for (name, value) in initial_schemas {
        if let RefOr::Item(schema) = &value {
            let canonical = serde_json::to_string(schema)?;
            collapser
                .schemas_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        collapser.schemas.insert(name, value);
    }
    for (name, value) in initial_parameters {
        if let RefOr::Item(p) = &value {
            let canonical = serde_json::to_string(p)?;
            collapser
                .parameters_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        collapser.parameters.insert(name, value);
    }
    for (name, value) in initial_responses {
        if let RefOr::Item(r) = &value {
            let canonical = serde_json::to_string(r)?;
            collapser
                .responses_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        collapser.responses.insert(name, value);
    }
    for (name, value) in initial_request_bodies {
        if let RefOr::Item(rb) = &value {
            let canonical = serde_json::to_string(rb)?;
            collapser
                .request_bodies_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        collapser.request_bodies.insert(name, value);
    }
    for (name, value) in initial_headers {
        if let RefOr::Item(h) = &value {
            let canonical = serde_json::to_string(h)?;
            collapser
                .headers_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        collapser.headers.insert(name, value);
    }

    // ── Phase 2a: recurse into pre-existing components.schemas ───────
    // Pull each entry out by name, recurse into it (which lifts nested
    // schemas back into `self.schemas`), and put it back. Working on
    // owned data sidesteps the aliasing problem of mutating the bag
    // while we're iterating it.
    let existing_names: Vec<String> = collapser.schemas.keys().cloned().collect();
    for name in existing_names {
        if let Some(RefOr::Item(_)) = collapser.schemas.get(&name) {
            let Some(RefOr::Item(mut schema)) = collapser.schemas.remove(&name) else {
                continue;
            };
            let ctx = NameContext::new(["components", "schemas", &name]);
            collapser.recurse_schema(&mut schema, &ctx)?;
            // Refresh the dedup entry — children may have changed the
            // canonical form of the parent.
            let canonical = serde_json::to_string(&schema)?;
            collapser
                .schemas_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
            collapser.schemas.insert(name, RefOr::new_item(schema));
        }
    }

    // Same Phase 2a treatment for components.parameters: pre-existing
    // entries keep their names, but their inline nested schemas / content
    // schemas still get lifted into `self.schemas`.
    let existing_param_names: Vec<String> = collapser.parameters.keys().cloned().collect();
    for name in existing_param_names {
        if let Some(RefOr::Item(_)) = collapser.parameters.get(&name) {
            let Some(RefOr::Item(mut param)) = collapser.parameters.remove(&name) else {
                continue;
            };
            let ctx = NameContext::new(["components", "parameters", &name]);
            collapser.walk_parameter(&mut param, ctx)?;
            let canonical = serde_json::to_string(&param)?;
            collapser
                .parameters_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
            collapser.parameters.insert(name, RefOr::new_item(param));
        }
    }

    // Same for components.responses.
    let existing_resp_names: Vec<String> = collapser.responses.keys().cloned().collect();
    for name in existing_resp_names {
        if let Some(RefOr::Item(_)) = collapser.responses.get(&name) {
            let Some(RefOr::Item(mut resp)) = collapser.responses.remove(&name) else {
                continue;
            };
            let ctx = NameContext::new(["components", "responses", &name]);
            collapser.walk_response(&mut resp, ctx)?;
            let canonical = serde_json::to_string(&resp)?;
            collapser
                .responses_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
            collapser.responses.insert(name, RefOr::new_item(resp));
        }
    }

    // Same for components.requestBodies.
    let existing_rb_names: Vec<String> = collapser.request_bodies.keys().cloned().collect();
    for name in existing_rb_names {
        if let Some(RefOr::Item(_)) = collapser.request_bodies.get(&name) {
            let Some(RefOr::Item(mut rb)) = collapser.request_bodies.remove(&name) else {
                continue;
            };
            let ctx = NameContext::new(["components", "requestBodies", &name]);
            collapser.walk_request_body(&mut rb, ctx)?;
            let canonical = serde_json::to_string(&rb)?;
            collapser
                .request_bodies_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
            collapser.request_bodies.insert(name, RefOr::new_item(rb));
        }
    }

    // Same for components.headers.
    let existing_header_names: Vec<String> = collapser.headers.keys().cloned().collect();
    for name in existing_header_names {
        if let Some(RefOr::Item(_)) = collapser.headers.get(&name) {
            let Some(RefOr::Item(mut h)) = collapser.headers.remove(&name) else {
                continue;
            };
            let ctx = NameContext::new(["components", "headers", &name]);
            collapser.walk_header(&mut h, ctx)?;
            let canonical = serde_json::to_string(&h)?;
            collapser
                .headers_seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
            collapser.headers.insert(name, RefOr::new_item(h));
        }
    }

    // ── Phase 2b: walk the rest of the spec ──────────────────────────
    if let Some(paths) = spec.paths.as_mut() {
        collapser.walk_paths(paths, NameContext::new(["paths"]))?;
    }
    if let Some(webhooks) = spec.webhooks.as_mut() {
        collapser.walk_paths(webhooks, NameContext::new(["webhooks"]))?;
    }
    if let Some(components) = spec.components.as_mut() {
        collapser.walk_components_non_schemas(components, NameContext::new(["components"]))?;
    }

    // Write each lifted bag back to its slot under `components`. The
    // `get_or_insert_with` only creates a `Components` when at least
    // one bag is non-empty; an unconditional touch would leave a
    // stray empty `components: {}` on collapse-of-empty-input round
    // trips.
    if !collapser.schemas.is_empty() {
        spec.components.get_or_insert_with(Default::default).schemas = Some(collapser.schemas);
    }
    if !collapser.parameters.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .parameters = Some(collapser.parameters);
    }
    if !collapser.responses.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .responses = Some(collapser.responses);
    }
    if !collapser.request_bodies.is_empty() {
        spec.components
            .get_or_insert_with(Default::default)
            .request_bodies = Some(collapser.request_bodies);
    }
    if !collapser.headers.is_empty() {
        spec.components.get_or_insert_with(Default::default).headers = Some(collapser.headers);
    }

    Ok(())
}

struct Collapser<'a> {
    /// In-progress `components.schemas` bag. Grows as schemas are lifted.
    schemas: BTreeMap<String, RefOr<Schema>>,
    /// Per-bag dedup map: canonical-JSON-serialised `Schema` →
    /// component name. Kept separate from other bags' dedup maps
    /// because each bag holds a different type.
    schemas_seen: HashMap<String, String>,
    /// In-progress `components.parameters` bag.
    parameters: BTreeMap<String, RefOr<Parameter>>,
    parameters_seen: HashMap<String, String>,
    /// In-progress `components.responses` bag.
    responses: BTreeMap<String, RefOr<Response>>,
    responses_seen: HashMap<String, String>,
    /// In-progress `components.requestBodies` bag.
    request_bodies: BTreeMap<String, RefOr<RequestBody>>,
    request_bodies_seen: HashMap<String, String>,
    /// In-progress `components.headers` bag.
    headers: BTreeMap<String, RefOr<Header>>,
    headers_seen: HashMap<String, String>,
    /// Optional loader for resolving external `$ref`s.
    loader: Option<&'a mut Loader>,
}

impl<'a> Collapser<'a> {
    // ── Spec-tree walk ──────────────────────────────────────────────

    fn walk_paths(&mut self, paths: &mut Paths, ctx: NameContext) -> Result<(), CollapseError> {
        for (path_key, path_item) in paths.paths.iter_mut() {
            self.walk_path_item(path_item, ctx.push(path_key))?;
        }
        Ok(())
    }

    fn walk_path_item(
        &mut self,
        path_item: &mut PathItem,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(params) = path_item.parameters.as_mut() {
            for (i, p) in params.iter_mut().enumerate() {
                self.lift_ref_or_parameter(p, ctx.push(&format!("parameters[{i}]")))?;
            }
        }
        if let Some(ops) = path_item.operations.as_mut() {
            for (method, op) in ops.iter_mut() {
                self.walk_operation(op, ctx.push(method))?;
            }
        }
        if let Some(ops) = path_item.additional_operations.as_mut() {
            for (method, op) in ops.iter_mut() {
                self.walk_operation(op, ctx.push(method))?;
            }
        }
        Ok(())
    }

    fn walk_operation(
        &mut self,
        op: &mut Operation,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        // Prefer `operationId` for naming the operation's children —
        // it's the canonical, author-chosen identifier and keeps
        // derived names like `<operationId>Request` / `<operationId>200`
        // stable across spec edits.
        let ctx = match op.operation_id.as_deref() {
            Some(id) if !id.is_empty() => NameContext::new([id]),
            _ => ctx,
        };
        if let Some(params) = op.parameters.as_mut() {
            for (i, p) in params.iter_mut().enumerate() {
                self.lift_ref_or_parameter(p, ctx.push(&format!("parameters[{i}]")))?;
            }
        }
        if let Some(rb) = op.request_body.as_mut() {
            self.lift_ref_or_request_body(rb, ctx.push("requestBody"))?;
        }
        if let Some(responses) = op.responses.as_mut() {
            self.walk_responses(responses, ctx.push("responses"))?;
        }
        if let Some(callbacks) = op.callbacks.as_mut() {
            for (name, cb) in callbacks.iter_mut() {
                self.walk_ref_or_callback(cb, ctx.push(name))?;
            }
        }
        Ok(())
    }

    fn walk_responses(
        &mut self,
        responses: &mut Responses,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(default) = responses.default.as_mut() {
            self.lift_ref_or_response(default, ctx.push("default"))?;
        }
        if let Some(map) = responses.responses.as_mut() {
            for (status, resp) in map.iter_mut() {
                self.lift_ref_or_response(resp, ctx.push(status))?;
            }
        }
        Ok(())
    }

    /// Lift an inline `RefOr<Response>`: recurse into the response
    /// (lifting nested headers / content schemas first), intern it,
    /// rewrite the slot to a `#/components/responses/<name>` ref.
    /// External `$ref`s are resolved via the loader when present;
    /// internal refs are left alone.
    fn lift_ref_or_response(
        &mut self,
        slot: &mut RefOr<Response>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        match slot {
            RefOr::Ref(r) => {
                if is_internal_ref(&r.reference) {
                    return Ok(());
                }
                let Some(loader) = self.loader.as_deref_mut() else {
                    return Ok(());
                };
                let reference = r.reference.clone();
                let mut fetched: Response =
                    loader.resolve_reference_as(&reference).map_err(|source| {
                        CollapseError::External {
                            reference: reference.clone(),
                            source,
                        }
                    })?;
                let derived_ctx = NameContext::from_external_ref(&reference, &ctx);
                self.walk_response(&mut fetched, derived_ctx.clone())?;
                let name = self.intern_response(fetched, derived_ctx)?;
                *slot = RefOr::new_ref(format!("#/components/responses/{name}"));
                Ok(())
            }
            RefOr::Item(_) => {
                let placeholder = RefOr::Ref(Ref::new(String::new()));
                let owned = mem::replace(slot, placeholder);
                let RefOr::Item(mut response) = owned else {
                    unreachable!("matched RefOr::Item above");
                };
                self.walk_response(&mut response, ctx.clone())?;
                let name = self.intern_response(response, ctx)?;
                *slot = RefOr::new_ref(format!("#/components/responses/{name}"));
                Ok(())
            }
        }
    }

    fn walk_response(
        &mut self,
        response: &mut Response,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(headers) = response.headers.as_mut() {
            for (name, h) in headers.iter_mut() {
                self.lift_ref_or_header(h, ctx.push(&format!("headers.{name}")))?;
            }
        }
        if let Some(content) = response.content.as_mut() {
            for (mime, mt) in content.iter_mut() {
                self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
            }
        }
        Ok(())
    }

    /// Lift an inline `RefOr<Header>`: recurse first (lift nested
    /// schema + content media-type schemas), intern the header,
    /// rewrite the slot to a `#/components/headers/<name>` ref.
    /// External `$ref`s resolve via the loader when present.
    fn lift_ref_or_header(
        &mut self,
        slot: &mut RefOr<Header>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        match slot {
            RefOr::Ref(r) => {
                if is_internal_ref(&r.reference) {
                    return Ok(());
                }
                let Some(loader) = self.loader.as_deref_mut() else {
                    return Ok(());
                };
                let reference = r.reference.clone();
                let mut fetched: Header =
                    loader.resolve_reference_as(&reference).map_err(|source| {
                        CollapseError::External {
                            reference: reference.clone(),
                            source,
                        }
                    })?;
                let derived_ctx = NameContext::from_external_ref(&reference, &ctx);
                self.walk_header(&mut fetched, derived_ctx.clone())?;
                let name = self.intern_header(fetched, derived_ctx)?;
                *slot = RefOr::new_ref(format!("#/components/headers/{name}"));
                Ok(())
            }
            RefOr::Item(_) => {
                let placeholder = RefOr::Ref(Ref::new(String::new()));
                let owned = mem::replace(slot, placeholder);
                let RefOr::Item(mut header) = owned else {
                    unreachable!("matched RefOr::Item above");
                };
                self.walk_header(&mut header, ctx.clone())?;
                let name = self.intern_header(header, ctx)?;
                *slot = RefOr::new_ref(format!("#/components/headers/{name}"));
                Ok(())
            }
        }
    }

    fn walk_header(&mut self, header: &mut Header, ctx: NameContext) -> Result<(), CollapseError> {
        if let Some(s) = header.schema.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("schema"))?;
        }
        if let Some(content) = header.content.as_mut() {
            for (mime, mt) in content.iter_mut() {
                self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
            }
        }
        Ok(())
    }

    fn walk_ref_or_media_type(
        &mut self,
        item: &mut RefOr<MediaType>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let RefOr::Item(m) = item {
            self.walk_media_type(m, ctx)?;
        }
        Ok(())
    }

    fn walk_media_type(
        &mut self,
        mt: &mut MediaType,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(s) = mt.schema.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("schema"))?;
        }
        if let Some(s) = mt.item_schema.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("itemSchema"))?;
        }
        Ok(())
    }

    /// Lift an inline `RefOr<Parameter>`: recurse into the parameter
    /// (lifting its nested `schema` / `content[…].schema` first),
    /// intern the resulting parameter, rewrite the slot to a
    /// `#/components/parameters/<name>` ref. External `$ref`s are
    /// resolved via the loader when one is present; internal refs are
    /// left alone.
    fn lift_ref_or_parameter(
        &mut self,
        slot: &mut RefOr<Parameter>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        match slot {
            RefOr::Ref(r) => {
                if is_internal_ref(&r.reference) {
                    return Ok(());
                }
                let Some(loader) = self.loader.as_deref_mut() else {
                    return Ok(());
                };
                let reference = r.reference.clone();
                let mut fetched: Parameter =
                    loader.resolve_reference_as(&reference).map_err(|source| {
                        CollapseError::External {
                            reference: reference.clone(),
                            source,
                        }
                    })?;
                let derived_ctx = NameContext::from_external_ref(&reference, &ctx);
                self.walk_parameter(&mut fetched, derived_ctx.clone())?;
                let name = self.intern_parameter(fetched, derived_ctx)?;
                *slot = RefOr::new_ref(format!("#/components/parameters/{name}"));
                Ok(())
            }
            RefOr::Item(_) => {
                let placeholder = RefOr::Ref(Ref::new(String::new()));
                let owned = mem::replace(slot, placeholder);
                let RefOr::Item(mut parameter) = owned else {
                    unreachable!("matched RefOr::Item above");
                };
                self.walk_parameter(&mut parameter, ctx.clone())?;
                let name = self.intern_parameter(parameter, ctx)?;
                *slot = RefOr::new_ref(format!("#/components/parameters/{name}"));
                Ok(())
            }
        }
    }

    fn walk_parameter(
        &mut self,
        param: &mut Parameter,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        // Push the parameter's `name` field so derived schema names
        // mention it: `getPets_parameters[0]_limit_schema` is more
        // readable than the bare index. Querystring is the odd one
        // out: per OAS 3.2, it carries `content` only and forbids
        // `schema`, so we walk its `content` map directly.
        match param {
            Parameter::Path(p) => self.walk_param_schema_and_content(
                ctx.push(p.name.as_str()),
                p.schema.as_mut(),
                p.content.as_mut(),
            ),
            Parameter::Query(p) => self.walk_param_schema_and_content(
                ctx.push(p.name.as_str()),
                p.schema.as_mut(),
                p.content.as_mut(),
            ),
            Parameter::Header(p) => self.walk_param_schema_and_content(
                ctx.push(p.name.as_str()),
                p.schema.as_mut(),
                p.content.as_mut(),
            ),
            Parameter::Cookie(p) => self.walk_param_schema_and_content(
                ctx.push(p.name.as_str()),
                p.schema.as_mut(),
                p.content.as_mut(),
            ),
            Parameter::Querystring(p) => {
                let ctx = ctx.push(p.name.as_str());
                for (mime, mt) in p.content.iter_mut() {
                    self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
                }
                Ok(())
            }
        }
    }

    fn walk_param_schema_and_content(
        &mut self,
        ctx: NameContext,
        schema: Option<&mut RefOr<Schema>>,
        content: Option<&mut BTreeMap<String, RefOr<MediaType>>>,
    ) -> Result<(), CollapseError> {
        if let Some(s) = schema {
            self.lift_ref_or_schema(s, ctx.push("schema"))?;
        }
        if let Some(content) = content {
            for (mime, mt) in content.iter_mut() {
                self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
            }
        }
        Ok(())
    }

    /// Walk into a `RequestBody`, lifting its content schemas. Used
    /// by both phase 2a (pre-existing components.requestBodies) and
    /// the lift path on inline bodies.
    fn walk_request_body(
        &mut self,
        rb: &mut RequestBody,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        for (mime, mt) in rb.content.iter_mut() {
            self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
        }
        Ok(())
    }

    /// Lift an inline `RefOr<RequestBody>`: recurse into the body
    /// first (lift nested content schemas), intern, rewrite the slot
    /// to a `#/components/requestBodies/<name>` ref. External `$ref`s
    /// are resolved via the loader when present; internal refs are
    /// left alone.
    fn lift_ref_or_request_body(
        &mut self,
        slot: &mut RefOr<RequestBody>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        match slot {
            RefOr::Ref(r) => {
                if is_internal_ref(&r.reference) {
                    return Ok(());
                }
                let Some(loader) = self.loader.as_deref_mut() else {
                    return Ok(());
                };
                let reference = r.reference.clone();
                let mut fetched: RequestBody =
                    loader.resolve_reference_as(&reference).map_err(|source| {
                        CollapseError::External {
                            reference: reference.clone(),
                            source,
                        }
                    })?;
                let derived_ctx = NameContext::from_external_ref(&reference, &ctx);
                self.walk_request_body(&mut fetched, derived_ctx.clone())?;
                let name = self.intern_request_body(fetched, derived_ctx)?;
                *slot = RefOr::new_ref(format!("#/components/requestBodies/{name}"));
                Ok(())
            }
            RefOr::Item(_) => {
                let placeholder = RefOr::Ref(Ref::new(String::new()));
                let owned = mem::replace(slot, placeholder);
                let RefOr::Item(mut rb) = owned else {
                    unreachable!("matched RefOr::Item above");
                };
                self.walk_request_body(&mut rb, ctx.clone())?;
                let name = self.intern_request_body(rb, ctx)?;
                *slot = RefOr::new_ref(format!("#/components/requestBodies/{name}"));
                Ok(())
            }
        }
    }

    fn walk_ref_or_callback(
        &mut self,
        item: &mut RefOr<Callback>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let RefOr::Item(cb) = item {
            for (expr, pi) in cb.paths.iter_mut() {
                self.walk_path_item(pi, ctx.push(expr))?;
            }
        }
        Ok(())
    }

    fn walk_components_non_schemas(
        &mut self,
        components: &mut Components,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        // `components.schemas`, `components.parameters`,
        // `components.responses`, `components.requestBodies`, and
        // `components.headers` are handled separately by phases 1 + 2a
        // above; this walks every other bag for nested slots.
        if let Some(map) = components.path_items.as_mut() {
            for (name, pi) in map.iter_mut() {
                self.walk_path_item(pi, ctx.push(&format!("pathItems.{name}")))?;
            }
        }
        if let Some(map) = components.callbacks.as_mut() {
            for (name, cb) in map.iter_mut() {
                self.walk_ref_or_callback(cb, ctx.push(&format!("callbacks.{name}")))?;
            }
        }
        if let Some(map) = components.media_types.as_mut() {
            for (name, mt) in map.iter_mut() {
                self.walk_ref_or_media_type(mt, ctx.push(&format!("mediaTypes.{name}")))?;
            }
        }
        Ok(())
    }

    // ── Lift + dedup core ──────────────────────────────────────────

    /// Lift an inline `RefOr<Schema>`: recurse into nested schemas
    /// first (so children are already refs before we serialize the
    /// parent for dedup), intern the result, rewrite the slot to a
    /// `#/components/schemas/<name>` ref. External `$ref`s are
    /// resolved via the loader when one is present; internal refs
    /// (`#/...`) are left alone.
    fn lift_ref_or_schema(
        &mut self,
        slot: &mut RefOr<Schema>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        match slot {
            RefOr::Ref(r) => {
                if is_internal_ref(&r.reference) {
                    return Ok(());
                }
                // External ref. With a loader, fetch + lift; without
                // one, leave the ref alone.
                let Some(loader) = self.loader.as_deref_mut() else {
                    return Ok(());
                };
                let reference = r.reference.clone();
                let mut fetched: Schema =
                    loader.resolve_reference_as(&reference).map_err(|source| {
                        CollapseError::External {
                            reference: reference.clone(),
                            source,
                        }
                    })?;
                let derived_ctx = NameContext::from_external_ref(&reference, &ctx);
                self.recurse_schema(&mut fetched, &derived_ctx)?;
                let name = self.intern_schema(fetched, derived_ctx)?;
                *slot = RefOr::new_ref(format!("{SCHEMA_PREFIX}{name}"));
                Ok(())
            }
            RefOr::Item(_) => {
                // Take ownership out of the slot so we can recurse +
                // intern without aliasing.
                let placeholder = RefOr::Ref(Ref::new(String::new()));
                let owned = mem::replace(slot, placeholder);
                let RefOr::Item(mut schema) = owned else {
                    unreachable!("we matched RefOr::Item above");
                };
                self.recurse_schema(&mut schema, &ctx)?;
                let name = self.intern_schema(schema, ctx)?;
                *slot = RefOr::new_ref(format!("{SCHEMA_PREFIX}{name}"));
                Ok(())
            }
        }
    }

    /// Recurse into a `Schema`, lifting nested `RefOr<Schema>` slots
    /// in place. After this returns, every nested inline schema has
    /// been moved to `self.schemas` and replaced with a `$ref` — so
    /// serialising `schema` produces canonical JSON we can key the
    /// dedup map on.
    fn recurse_schema(
        &mut self,
        schema: &mut Schema,
        ctx: &NameContext,
    ) -> Result<(), CollapseError> {
        match schema {
            Schema::Bool(_) | Schema::Empty(_) | Schema::Multi(_) => Ok(()),
            Schema::AllOf(s) => {
                for (i, child) in s.all_of.iter_mut().enumerate() {
                    self.lift_ref_or_schema(child, ctx.push(&format!("allOf[{i}]")))?;
                }
                Ok(())
            }
            Schema::AnyOf(s) => {
                for (i, child) in s.any_of.iter_mut().enumerate() {
                    self.lift_ref_or_schema(child, ctx.push(&format!("anyOf[{i}]")))?;
                }
                Ok(())
            }
            Schema::OneOf(s) => {
                for (i, child) in s.one_of.iter_mut().enumerate() {
                    self.lift_ref_or_schema(child, ctx.push(&format!("oneOf[{i}]")))?;
                }
                Ok(())
            }
            Schema::Not(s) => self.lift_ref_or_schema(&mut s.not, ctx.push("not")),
            Schema::Single(s) => self.recurse_single_schema(s.as_mut(), ctx),
        }
    }

    fn recurse_single_schema(
        &mut self,
        s: &mut SingleSchema,
        ctx: &NameContext,
    ) -> Result<(), CollapseError> {
        match s {
            SingleSchema::Object(o) => self.recurse_object_schema(o, ctx),
            SingleSchema::Array(a) => self.recurse_array_schema(a, ctx),
            // Primitive variants (String, Integer, Number, Boolean, Null)
            // carry no nested schema slots.
            _ => Ok(()),
        }
    }

    fn recurse_object_schema(
        &mut self,
        o: &mut ObjectSchema,
        ctx: &NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(props) = o.properties.as_mut() {
            for (name, child) in props.iter_mut() {
                self.lift_ref_or_schema(child, ctx.push(&format!("properties.{name}")))?;
            }
        }
        if let Some(props) = o.pattern_properties.as_mut() {
            for (name, child) in props.iter_mut() {
                self.lift_ref_or_schema(child, ctx.push(&format!("patternProperties.{name}")))?;
            }
        }
        if let Some(BoolOr::Item(s)) = o.additional_properties.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("additionalProperties"))?;
        }
        if let Some(BoolOr::Item(s)) = o.unevaluated_properties.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("unevaluatedProperties"))?;
        }
        if let Some(s) = o.property_names.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("propertyNames"))?;
        }
        Ok(())
    }

    fn recurse_array_schema(
        &mut self,
        a: &mut ArraySchema,
        ctx: &NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(BoolOr::Item(s)) = a.items.as_mut() {
            self.lift_ref_or_schema(s, ctx.push("items"))?;
        }
        Ok(())
    }

    /// Insert `schema` into the components.schemas bag. If a
    /// structurally identical schema is already there (canonical-JSON
    /// equality), return the existing name and drop `schema`.
    /// Otherwise generate a name (via `title`, falling back to context)
    /// and insert.
    ///
    /// Dedup is *strict*: `title` is part of the canonical form, so
    /// two schemas that differ only in `title` presence don't collapse.
    /// First-seen wins for the component name on a dedup hit.
    fn intern_schema(&mut self, schema: Schema, ctx: NameContext) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&schema)?;
        if let Some(existing) = self.schemas_seen.get(&canonical) {
            return Ok(existing.clone());
        }
        let name = self.generate_name(&schema, &ctx);
        self.schemas_seen.insert(canonical, name.clone());
        self.schemas.insert(name.clone(), RefOr::new_item(schema));
        Ok(name)
    }

    /// Same shape as [`Self::intern_schema`] but for `Parameter`. No
    /// `title` field on parameters — we use a `<name><In>` hint
    /// (e.g., `limitQuery`, `petIdPath`) when the parameter carries a
    /// non-empty `name`, otherwise the surrounding context.
    fn intern_parameter(
        &mut self,
        parameter: Parameter,
        ctx: NameContext,
    ) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&parameter)?;
        if let Some(existing) = self.parameters_seen.get(&canonical) {
            return Ok(existing.clone());
        }
        let hint = parameter_name_hint(&parameter);
        let base = if hint.is_empty() {
            ctx.derive_name()
        } else {
            sanitize_component_name(hint)
        };
        let name = unique_name(&self.parameters, &base);
        self.parameters_seen.insert(canonical, name.clone());
        self.parameters
            .insert(name.clone(), RefOr::new_item(parameter));
        Ok(name)
    }

    /// Same shape as [`Self::intern_schema`] but for `Response`.
    /// Responses have no canonical name field — naming falls back to
    /// the surrounding context (e.g., the status code pushed by the
    /// walker).
    fn intern_response(
        &mut self,
        response: Response,
        ctx: NameContext,
    ) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&response)?;
        if let Some(existing) = self.responses_seen.get(&canonical) {
            return Ok(existing.clone());
        }
        let base = ctx.derive_name();
        let name = unique_name(&self.responses, &base);
        self.responses_seen.insert(canonical, name.clone());
        self.responses
            .insert(name.clone(), RefOr::new_item(response));
        Ok(name)
    }

    /// Same shape as [`Self::intern_schema`] but for `RequestBody`.
    /// No canonical name field — naming is context-derived.
    fn intern_request_body(
        &mut self,
        rb: RequestBody,
        ctx: NameContext,
    ) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&rb)?;
        if let Some(existing) = self.request_bodies_seen.get(&canonical) {
            return Ok(existing.clone());
        }
        let base = ctx.derive_name();
        let name = unique_name(&self.request_bodies, &base);
        self.request_bodies_seen.insert(canonical, name.clone());
        self.request_bodies
            .insert(name.clone(), RefOr::new_item(rb));
        Ok(name)
    }

    /// Same shape as [`Self::intern_schema`] but for `Header`.
    /// Header has no canonical name field — naming is
    /// context-derived (typically the header key pushed by the walker).
    fn intern_header(&mut self, header: Header, ctx: NameContext) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&header)?;
        if let Some(existing) = self.headers_seen.get(&canonical) {
            return Ok(existing.clone());
        }
        let base = ctx.derive_name();
        let name = unique_name(&self.headers, &base);
        self.headers_seen.insert(canonical, name.clone());
        self.headers.insert(name.clone(), RefOr::new_item(header));
        Ok(name)
    }

    fn generate_name(&self, schema: &Schema, ctx: &NameContext) -> String {
        let base = match schema_title(schema) {
            Some(t) => sanitize_component_name(t),
            None => ctx.derive_name(),
        };
        if !self.schemas.contains_key(&base) {
            return base;
        }
        for i in 2..u32::MAX {
            let candidate = format!("{base}_{i}");
            if !self.schemas.contains_key(&candidate) {
                return candidate;
            }
        }
        // 2 ^ 32 - 2 distinct names is unreachable in practice; if we
        // ever hit it, the suffix loop has bigger problems than we can
        // recover from at runtime.
        unreachable!("exhausted u32 suffixes for `{base}`");
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn is_internal_ref(reference: &str) -> bool {
    reference.starts_with('#')
}

/// `<name><In>` hint for a `Parameter` — e.g. `limitQuery`, `petIdPath`.
/// Returns the empty string when the parameter has no usable `name`,
/// signalling that the caller should fall back to context-derived
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

/// Pick the first non-colliding name in `bag` starting from `base`. On
/// collision, appends `_2`, `_3`, …. Shared by every bag's intern
/// method.
fn unique_name<V>(bag: &BTreeMap<String, V>, base: &str) -> String {
    if !bag.contains_key(base) {
        return base.to_owned();
    }
    for i in 2..u32::MAX {
        let candidate = format!("{base}_{i}");
        if !bag.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("exhausted u32 suffixes for `{base}`");
}

fn schema_title(schema: &Schema) -> Option<&str> {
    match schema {
        // Composition forms (AllOf / AnyOf / OneOf / Not) and the
        // bare-`true`/`false` / empty-object forms don't carry a
        // `title` field on this crate's types — return None and let
        // the context-path naming kick in.
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

/// Normalise a candidate name to OAS component-name format
/// (`^[a-zA-Z0-9.\-_]+$`). Replaces invalid chars with `_`, collapses
/// runs of `_`, and trims leading/trailing `_`. An empty result falls
/// back to `Schema`.
fn sanitize_component_name(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let trimmed = out.trim_matches('_').to_owned();
    if trimmed.is_empty() {
        "Schema".to_owned()
    } else {
        trimmed
    }
}

/// Context-derived name accumulator. Carries the path through the
/// spec tree (e.g., `["getPets", "responses", "200", "content",
/// "application/json", "schema"]`) so `derive_name` can flatten it
/// into a valid component name.
#[derive(Clone)]
struct NameContext {
    parts: Vec<String>,
}

impl NameContext {
    fn new<I, S>(parts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            parts: parts.into_iter().map(Into::into).collect(),
        }
    }

    fn push(&self, part: &str) -> Self {
        let mut next = self.clone();
        next.parts.push(part.to_owned());
        next
    }

    fn derive_name(&self) -> String {
        sanitize_component_name(self.parts.join("_"))
    }

    /// Derive a name for a schema fetched via an external `$ref`. If
    /// the reference has a JSON Pointer fragment, use the last segment
    /// (e.g., `external.json#/components/schemas/Pet` → `Pet`); else
    /// fall back to the surrounding context.
    fn from_external_ref(reference: &str, fallback: &NameContext) -> Self {
        if let Some((_, fragment)) = reference.split_once('#')
            && let Some(last) = fragment.rsplit('/').next()
            && !last.is_empty()
        {
            return NameContext::new([last.to_owned()]);
        }
        fallback.clone()
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
        // The response is now lifted; navigate into the lifted response.
        let v = serde_json::to_value(&spec).unwrap();
        let resp_ref = v["paths"]["/pets"]["get"]["responses"]["200"]["$ref"]
            .as_str()
            .expect("response slot must be lifted to a ref");
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        let schema_slot =
            &v["components"]["responses"][resp_name]["content"]["application/json"]["schema"];
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
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        assert_eq!(
            v["components"]["responses"][resp_name]["content"]["application/json"]["schema"]["$ref"],
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
        let resp_name = a_resp.trim_start_matches("#/components/responses/");
        // The shared response's body points at the single Pet schema.
        assert_eq!(
            v["components"]["responses"][resp_name]["content"]["application/json"]["schema"]["$ref"],
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
            let resp_name = resp_ref.trim_start_matches("#/components/responses/");
            assert_eq!(
                v["components"]["responses"][resp_name]["content"]["application/json"]["schema"]["$ref"],
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
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        assert_eq!(
            v["components"]["responses"][resp_name]["content"]["application/json"]["schema"]["$ref"],
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
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        assert_eq!(
            v["components"]["responses"][resp_name]["content"]["application/json"]["schema"]["$ref"],
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
        let resp_name = resp_ref.trim_start_matches("#/components/responses/");
        assert_eq!(
            v["components"]["responses"][resp_name]["content"]["application/json"]["schema"]["$ref"],
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
            let resp_name = resp_ref.trim_start_matches("#/components/responses/");
            v["components"]["responses"][resp_name]["content"]["application/json"]["schema"].clone()
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

    // ── Helper coverage: sanitize_component_name + NameContext ───────────

    #[test]
    fn sanitize_component_name_handles_edge_cases() {
        assert_eq!(sanitize_component_name("Pet"), "Pet");
        // Slashes, spaces, and brackets all collapse to underscores.
        assert_eq!(
            sanitize_component_name("paths./pets[0].schema"),
            "paths._pets_0_.schema"
        );
        // Trim leading/trailing underscores produced by sanitisation.
        assert_eq!(sanitize_component_name("/foo/"), "foo");
        // Spaces collapse via the run-of-underscore reduction.
        assert_eq!(sanitize_component_name("Hello World"), "Hello_World");
        // Empty / all-invalid input falls back to the literal "Schema".
        assert_eq!(sanitize_component_name("///"), "Schema");
        assert_eq!(sanitize_component_name(""), "Schema");
    }

    #[test]
    fn name_context_from_external_ref_falls_back_on_empty_fragment() {
        let fallback = NameContext::new(["fallback"]);
        // No `#`: use fallback.
        let ctx = NameContext::from_external_ref("external.json", &fallback);
        assert_eq!(ctx.derive_name(), "fallback");
        // Empty fragment (`#`): also falls back.
        let ctx = NameContext::from_external_ref("external.json#", &fallback);
        assert_eq!(ctx.derive_name(), "fallback");
        // Fragment with only a slash (trailing) — last segment is empty,
        // so we fall back to the surrounding context.
        let ctx = NameContext::from_external_ref("external.json#/", &fallback);
        assert_eq!(ctx.derive_name(), "fallback");
    }

    #[test]
    fn name_context_from_external_ref_uses_last_pointer_segment() {
        let fallback = NameContext::new(["fallback"]);
        let ctx =
            NameContext::from_external_ref("external.json#/components/schemas/Pet", &fallback);
        assert_eq!(ctx.derive_name(), "Pet");
    }

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

    #[test]
    fn unique_name_appends_suffix_against_existing_keys() {
        let mut bag: BTreeMap<String, ()> = BTreeMap::new();
        assert_eq!(unique_name(&bag, "foo"), "foo");
        bag.insert("foo".to_owned(), ());
        assert_eq!(unique_name(&bag, "foo"), "foo_2");
        bag.insert("foo_2".to_owned(), ());
        assert_eq!(unique_name(&bag, "foo"), "foo_3");
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
            v["components"]["responses"]["NotFound"]["content"]["application/json"]["schema"]["$ref"],
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
        let rb_name = rb_ref.trim_start_matches("#/components/requestBodies/");
        // Nested schema is also lifted.
        assert_eq!(
            v["components"]["requestBodies"][rb_name]["content"]["application/json"]["schema"]["$ref"],
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
