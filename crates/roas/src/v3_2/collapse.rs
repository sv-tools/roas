//! `Spec::collapse` — lift every inline schema into `components.schemas`.
//!
//! Walks the entire spec, replacing each inline `RefOr::Item<Schema>` with
//! a `RefOr::Ref` pointing at a freshly-interned entry under
//! `components.schemas.<name>`. Names come from `schema.title` when
//! present, otherwise from a sanitised dot-joined path through the spec
//! tree. Structurally identical schemas (serde-canonical JSON equality)
//! collapse to a single component; every call site that previously had
//! the inline schema gets the same `$ref`.
//!
//! When the caller passes a [`Loader`], every external `$ref` (anything
//! not starting with `#`) is fetched, parsed as a `Schema`, run through
//! the same recursion + dedup pipeline, and rewritten as a local
//! `#/components/schemas/<name>` ref. The dedup map is shared between
//! lifted inline schemas and fetched external schemas, so two
//! structurally identical sources collapse together.
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
    // Take the existing `components.schemas` out of the spec so the
    // Collapser owns it mutably while we walk the rest of the tree.
    // We write it back at the very end.
    let initial = spec
        .components
        .as_mut()
        .and_then(|c| c.schemas.take())
        .unwrap_or_default();

    let mut collapser = Collapser {
        schemas: BTreeMap::new(),
        seen: HashMap::new(),
        loader,
    };

    // ── Phase 1: seed pre-existing components.schemas ────────────────
    // Each pre-existing entry keeps its name; we seed the dedup map so
    // newly-lifted equivalents collapse onto the existing names.
    for (name, value) in initial {
        if let RefOr::Item(schema) = &value {
            let canonical = serde_json::to_string(schema)?;
            collapser
                .seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
        }
        collapser.schemas.insert(name, value);
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
                .seen
                .entry(canonical)
                .or_insert_with(|| name.clone());
            collapser.schemas.insert(name, RefOr::new_item(schema));
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

    // Write the lifted schemas back to components.schemas.
    if !collapser.schemas.is_empty() {
        spec.components.get_or_insert_with(Default::default).schemas = Some(collapser.schemas);
    }

    Ok(())
}

struct Collapser<'a> {
    /// In-progress component bag. Grows as schemas are lifted.
    schemas: BTreeMap<String, RefOr<Schema>>,
    /// Dedup map: canonical-JSON-serialised Schema → component name.
    seen: HashMap<String, String>,
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
                self.walk_ref_or_parameter(p, ctx.push(&format!("parameters[{i}]")))?;
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
                self.walk_ref_or_parameter(p, ctx.push(&format!("parameters[{i}]")))?;
            }
        }
        if let Some(rb) = op.request_body.as_mut() {
            self.walk_ref_or_request_body(rb, ctx.push("requestBody"))?;
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
            self.walk_ref_or_response(default, ctx.push("default"))?;
        }
        if let Some(map) = responses.responses.as_mut() {
            for (status, resp) in map.iter_mut() {
                self.walk_ref_or_response(resp, ctx.push(status))?;
            }
        }
        Ok(())
    }

    fn walk_ref_or_response(
        &mut self,
        item: &mut RefOr<Response>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        // External `$ref`s on container types (Response, Parameter,
        // RequestBody, etc.) are left alone: this PR scopes lifting to
        // schemas only. Future PRs will recurse the same way.
        if let RefOr::Item(r) = item {
            self.walk_response(r, ctx)?;
        }
        Ok(())
    }

    fn walk_response(
        &mut self,
        response: &mut Response,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let Some(headers) = response.headers.as_mut() {
            for (name, h) in headers.iter_mut() {
                self.walk_ref_or_header(h, ctx.push(&format!("headers.{name}")))?;
            }
        }
        if let Some(content) = response.content.as_mut() {
            for (mime, mt) in content.iter_mut() {
                self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
            }
        }
        Ok(())
    }

    fn walk_ref_or_header(
        &mut self,
        item: &mut RefOr<Header>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let RefOr::Item(h) = item {
            self.walk_header(h, ctx)?;
        }
        Ok(())
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

    fn walk_ref_or_parameter(
        &mut self,
        item: &mut RefOr<Parameter>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let RefOr::Item(p) = item {
            self.walk_parameter(p, ctx)?;
        }
        Ok(())
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

    fn walk_ref_or_request_body(
        &mut self,
        item: &mut RefOr<RequestBody>,
        ctx: NameContext,
    ) -> Result<(), CollapseError> {
        if let RefOr::Item(rb) = item {
            for (mime, mt) in rb.content.iter_mut() {
                self.walk_ref_or_media_type(mt, ctx.push(&format!("content.{mime}")))?;
            }
        }
        Ok(())
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
        // `components.schemas` is handled separately by phases 1 + 2a
        // above; this walks every other bag for nested schema slots.
        if let Some(params) = components.parameters.as_mut() {
            for (name, p) in params.iter_mut() {
                self.walk_ref_or_parameter(p, ctx.push(&format!("parameters.{name}")))?;
            }
        }
        if let Some(map) = components.responses.as_mut() {
            for (name, r) in map.iter_mut() {
                self.walk_ref_or_response(r, ctx.push(&format!("responses.{name}")))?;
            }
        }
        if let Some(map) = components.request_bodies.as_mut() {
            for (name, rb) in map.iter_mut() {
                self.walk_ref_or_request_body(rb, ctx.push(&format!("requestBodies.{name}")))?;
            }
        }
        if let Some(map) = components.headers.as_mut() {
            for (name, h) in map.iter_mut() {
                self.walk_ref_or_header(h, ctx.push(&format!("headers.{name}")))?;
            }
        }
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
    /// structurally identical schema is already there, return the
    /// existing name and drop `schema`. Otherwise generate a name (via
    /// `title`, falling back to context) and insert.
    fn intern_schema(&mut self, schema: Schema, ctx: NameContext) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&schema)?;
        if let Some(existing) = self.seen.get(&canonical) {
            // Equivalent schema already lifted — return its name.
            // Optionally upgrade the name if the new schema has a
            // title and the existing entry was named from context.
            let existing = existing.clone();
            return Ok(self.maybe_upgrade_name(existing, &schema, canonical));
        }
        let name = self.generate_name(&schema, &ctx);
        self.seen.insert(canonical, name.clone());
        self.schemas.insert(name.clone(), RefOr::new_item(schema));
        Ok(name)
    }

    /// First-seen wins for the component name — unless the new
    /// occurrence carries an explicit `title` that the existing entry
    /// was missing. In that case, rename the existing entry so the
    /// human-readable title takes precedence over a context-derived
    /// name. All previously-emitted refs still resolve because
    /// `self.seen[canonical]` is updated to the new name.
    fn maybe_upgrade_name(
        &mut self,
        existing_name: String,
        new_schema: &Schema,
        canonical: String,
    ) -> String {
        let Some(title) = schema_title(new_schema) else {
            return existing_name;
        };
        let suggested = sanitize_component_name(title);
        if suggested == existing_name {
            return existing_name;
        }
        // Only upgrade when the existing schema *also* has no title —
        // otherwise we'd be overriding an author's deliberate choice.
        let existing_has_title = self
            .schemas
            .get(&existing_name)
            .and_then(|v| match v {
                RefOr::Item(s) => schema_title(s),
                RefOr::Ref(_) => None,
            })
            .is_some();
        if existing_has_title {
            return existing_name;
        }
        // Rename: same content, but stored under the title-derived name.
        let final_name = if self.schemas.contains_key(&suggested) {
            // Collision against an unrelated entry; keep the
            // context-derived name to avoid clobbering anything.
            return existing_name;
        } else {
            suggested
        };
        if let Some(value) = self.schemas.remove(&existing_name) {
            self.schemas.insert(final_name.clone(), value);
        }
        self.seen.insert(canonical, final_name.clone());
        final_name
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

    fn inline_to_ref(spec: &Spec, pointer: &str) -> serde_json::Value {
        let v = serde_json::to_value(spec).unwrap();
        v.pointer(pointer)
            .cloned()
            .unwrap_or_else(|| panic!("no value at `{pointer}` in serialised spec"))
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
        // Inline schema at the response should have become a `$ref`.
        let inline = inline_to_ref(
            &spec,
            "/paths/~1pets/get/responses/200/content/application~1json/schema",
        );
        assert!(
            inline.get("$ref").is_some(),
            "expected the inline schema to be replaced by a `$ref`, got: {inline}",
        );
        // The lifted component should exist under a context-derived name
        // rooted at the operationId. We don't pin the exact name here
        // because the path-flattening rule is internal; check that the
        // expected ref target is one of the lifted schemas.
        let ref_str = inline["$ref"].as_str().unwrap();
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
        let inline = inline_to_ref(
            &spec,
            "/paths/~1pets/get/responses/200/content/application~1json/schema",
        );
        assert_eq!(inline["$ref"], "#/components/schemas/Pet");
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
        // Exactly one `Pet` component, with both call sites pointing at it.
        let names = lifted_schema_names(&spec);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "Pet").count(),
            1,
            "got {names:?}"
        );
        let a = inline_to_ref(
            &spec,
            "/paths/~1a/get/responses/200/content/application~1json/schema",
        );
        let b = inline_to_ref(
            &spec,
            "/paths/~1b/get/responses/200/content/application~1json/schema",
        );
        assert_eq!(a["$ref"], "#/components/schemas/Pet");
        assert_eq!(b["$ref"], "#/components/schemas/Pet");
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
        let inline = inline_to_ref(
            &spec,
            "/paths/~1a/get/responses/200/content/application~1json/schema",
        );
        let external = inline_to_ref(
            &spec,
            "/paths/~1b/get/responses/200/content/application~1json/schema",
        );
        assert_eq!(inline["$ref"], "#/components/schemas/Pet");
        assert_eq!(external["$ref"], "#/components/schemas/Pet");
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
        let inline = inline_to_ref(
            &spec,
            "/paths/~1a/get/responses/200/content/application~1json/schema",
        );
        assert_eq!(
            inline["$ref"], "external.json#/Pet",
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
        let names = lifted_schema_names(&spec);
        assert!(names.contains(&"Limit".to_owned()), "got {names:?}");
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v["paths"]["/pets"]["get"]["parameters"][0]["schema"]["$ref"],
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
}
