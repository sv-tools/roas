//! v2 merge: per-component `MergeWithContext<Spec>` impls plus the
//! public `impl Merge for Spec`.
//!
//! Mirrors the v3.0/v3.1/v3.2 ports. v2's data model is quite a bit
//! flatter than v3:
//!
//! * Top-level has `host` / `basePath` / `schemes` / `consumes` /
//!   `produces` instead of a `servers` array.
//! * There is no `components` wrapper; `definitions`, `parameters`,
//!   `responses`, and `securityDefinitions` sit at the spec root as
//!   bare maps.
//! * Parameter / Header / Items are multi-level typed enums; for
//!   merge we recurse one variant level (so `InQuery::String` x
//!   `InQuery::String` merges field-wise) and treat the leaf typed
//!   structs (`StringParameter`, `IntegerParameter`, …) as opaque —
//!   they replace on mismatch. The typed-leaf rule keeps the file
//!   tractable and matches how v2 callers usually compose specs
//!   (whole-parameter replacement is the dominant pattern when the
//!   inner shape differs).
//! * `Schema` is leaf-replace by default; `DeepMergeObjectSchemas`
//!   recurses into `ObjectSchema` (`properties` / `required` /
//!   `additional_properties`).

use enumset::EnumSet;

use crate::common::merge::{
    PathGuard, merge_extensions, merge_opt_map, merge_opt_scalar, merge_opt_struct,
    merge_opt_vec_by_key, merge_opt_vec_set_union, merge_replace_list_when_nonempty,
    merge_required_scalar,
};
use crate::common::reference::RefOr;
use crate::merge::{
    ConflictKind, Merge, MergeContext, MergeError, MergeOptions, MergeReport, MergeWithContext,
};

use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::header::Header;
use crate::v2::info::{Contact, Info, License, Logo};
use crate::v2::items::Items;
use crate::v2::operation::{CodeSample, Operation};
use crate::v2::parameter::{InFormData, InHeader, InPath, InQuery, Parameter};
use crate::v2::path_item::{PathItem, Paths};
use crate::v2::response::{Response, Responses};
use crate::v2::schema::{ObjectSchema, Schema};
use crate::v2::security_scheme::SecurityScheme;
use crate::v2::spec::{Server, Spec, TagGroup};
use crate::v2::tag::Tag;

#[allow(clippy::ptr_arg)]
fn key_str(s: &String, out: &mut String) {
    out.push_str(s);
}

fn fmt_param_key(k: &(String, &'static str), out: &mut String) {
    out.push_str(&k.0);
    out.push(':');
    out.push_str(k.1);
}

// ----- Public entry point -----

impl Merge for Spec {
    fn merge(
        &mut self,
        other: Self,
        options: EnumSet<MergeOptions>,
    ) -> Result<MergeReport, MergeError> {
        let mut ctx: MergeContext<()> = MergeContext::new(&(), options);
        let mut path = String::from("#");

        if options.contains(MergeOptions::ErrorOnConflict) {
            let mut working = self.clone();
            <Spec as MergeWithContext<()>>::merge_with_context(
                &mut working,
                other,
                &mut ctx,
                &mut path,
            );
            if ctx.errored {
                return Err(MergeError {
                    conflicts: ctx.conflicts,
                });
            }
            *self = working;
            return Ok(MergeReport {
                conflicts: ctx.conflicts,
            });
        }

        <Spec as MergeWithContext<()>>::merge_with_context(self, other, &mut ctx, &mut path);
        ctx.into()
    }
}

// ----- Spec -----

impl MergeWithContext<()> for Spec {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        // swagger version + info: same documented contract as v3.x —
        // base wins by default; MergeInfo opts into replacement.
        if ctx.is_option(MergeOptions::MergeInfo) {
            merge_required_scalar(
                &mut self.swagger,
                other.swagger,
                ctx,
                path,
                ".swagger",
                ConflictKind::RequiredScalarOverridden,
            );
            {
                let mut guard = PathGuard::new(path, ".info");
                self.info
                    .merge_with_context(other.info, ctx, guard.path_mut());
            }
        } else {
            if self.swagger != other.swagger {
                record_kept_base_or_error(
                    ctx,
                    path,
                    ".swagger",
                    ConflictKind::RequiredScalarOverridden,
                );
            }
            if !ctx.errored && self.info != other.info {
                record_kept_base_or_error(
                    ctx,
                    path,
                    ".info",
                    ConflictKind::RequiredScalarOverridden,
                );
            }
        }
        merge_opt_scalar(
            &mut self.host,
            other.host,
            ctx,
            path,
            ".host",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.base_path,
            other.base_path,
            ctx,
            path,
            ".basePath",
            ConflictKind::ScalarOverridden,
        );
        merge_replace_list_when_nonempty(&mut self.schemes, other.schemes, ctx, path, ".schemes");
        merge_replace_list_when_nonempty(
            &mut self.consumes,
            other.consumes,
            ctx,
            path,
            ".consumes",
        );
        merge_replace_list_when_nonempty(
            &mut self.produces,
            other.produces,
            ctx,
            path,
            ".produces",
        );
        // v2 `paths` is required (not Option) — merge in place.
        {
            let mut guard = PathGuard::new(path, ".paths");
            self.paths
                .merge_with_context(other.paths, ctx, guard.path_mut());
        }
        merge_opt_map(
            &mut self.definitions,
            other.definitions,
            ctx,
            path,
            ".definitions",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.parameters,
            other.parameters,
            ctx,
            path,
            ".parameters",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.responses,
            other.responses,
            ctx,
            path,
            ".responses",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.security_definitions,
            other.security_definitions,
            ctx,
            path,
            ".securityDefinitions",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_replace_list_when_nonempty(
            &mut self.security,
            other.security,
            ctx,
            path,
            ".security",
        );
        merge_opt_vec_by_key(
            &mut self.tags,
            other.tags,
            ctx,
            path,
            ".tags",
            |t: &Tag| t.name.clone(),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            path,
            ".externalDocs",
        );
        merge_replace_list_when_nonempty(
            &mut self.x_servers,
            other.x_servers,
            ctx,
            path,
            ".x-servers",
        );
        merge_opt_vec_by_key(
            &mut self.x_tag_groups,
            other.x_tag_groups,
            ctx,
            path,
            ".x-tagGroups",
            |g: &TagGroup| g.name.clone(),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for TagGroup {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.name,
            other.name,
            ctx,
            path,
            ".name",
            ConflictKind::RequiredScalarOverridden,
        );
        let mut opt_base = Some(std::mem::take(&mut self.tags));
        merge_opt_vec_set_union(&mut opt_base, Some(other.tags), ctx, path, ".tags");
        self.tags = opt_base.unwrap_or_default();
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- Paths / PathItem / Operation -----

impl MergeWithContext<()> for Paths {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        for (k, incoming) in other.paths {
            if let Some(base_v) = self.paths.get_mut(&k) {
                let original_len = path.len();
                path.push('[');
                path.push_str(&k);
                path.push(']');
                base_v.merge_with_context(incoming, ctx, path);
                path.truncate(original_len);
                if ctx.errored {
                    return;
                }
            } else {
                self.paths.insert(k, incoming);
            }
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for PathItem {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.reference,
            other.reference,
            ctx,
            path,
            ".$ref",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.operations,
            other.operations,
            ctx,
            path,
            "",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_vec_by_key(
            &mut self.parameters,
            other.parameters,
            ctx,
            path,
            ".parameters",
            parameter_ref_key,
            |b, i, c, p| b.merge_with_context(i, c, p),
            fmt_param_key,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for Operation {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_vec_set_union(&mut self.tags, other.tags, ctx, path, ".tags");
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            path,
            ".summary",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            path,
            ".externalDocs",
        );
        merge_opt_scalar(
            &mut self.operation_id,
            other.operation_id,
            ctx,
            path,
            ".operationId",
            ConflictKind::ScalarOverridden,
        );
        merge_replace_list_when_nonempty(
            &mut self.consumes,
            other.consumes,
            ctx,
            path,
            ".consumes",
        );
        merge_replace_list_when_nonempty(
            &mut self.produces,
            other.produces,
            ctx,
            path,
            ".produces",
        );
        merge_opt_vec_by_key(
            &mut self.parameters,
            other.parameters,
            ctx,
            path,
            ".parameters",
            parameter_ref_key,
            |b, i, c, p| b.merge_with_context(i, c, p),
            fmt_param_key,
        );
        {
            let mut guard = PathGuard::new(path, ".responses");
            self.responses
                .merge_with_context(other.responses, ctx, guard.path_mut());
        }
        merge_replace_list_when_nonempty(&mut self.schemes, other.schemes, ctx, path, ".schemes");
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_replace_list_when_nonempty(
            &mut self.security,
            other.security,
            ctx,
            path,
            ".security",
        );
        merge_opt_vec_by_key(
            &mut self.x_code_samples,
            other.x_code_samples,
            ctx,
            path,
            ".x-codeSamples",
            |c: &CodeSample| (c.lang.clone(), c.label.clone().unwrap_or_default()),
            |b, i, c, p| b.merge_with_context(i, c, p),
            |k, out| {
                out.push_str(&k.0);
                if !k.1.is_empty() {
                    out.push(':');
                    out.push_str(&k.1);
                }
            },
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for CodeSample {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.lang,
            other.lang,
            ctx,
            path,
            ".lang",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.label,
            other.label,
            ctx,
            path,
            ".label",
            ConflictKind::ScalarOverridden,
        );
        merge_required_scalar(
            &mut self.source,
            other.source,
            ctx,
            path,
            ".source",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- Parameter (typed enum — leaf-replace inner structs) -----

fn parameter_ref_key(p: &RefOr<Parameter>) -> (String, &'static str) {
    match p {
        RefOr::Ref(r) => (r.reference.clone(), "ref"),
        RefOr::Item(p) => match p {
            Parameter::Body(b) => (b.name.clone(), "body"),
            Parameter::Path(_) => (parameter_name(p), "path"),
            Parameter::Query(_) => (parameter_name(p), "query"),
            Parameter::Header(_) => (parameter_name(p), "header"),
            Parameter::FormData(_) => (parameter_name(p), "formData"),
        },
    }
}

fn parameter_name(p: &Parameter) -> String {
    use crate::v2::parameter::{InFormData, InHeader, InPath, InQuery};
    match p {
        Parameter::Body(b) => b.name.clone(),
        Parameter::Path(p) => match &**p {
            InPath::String(p) => p.name.clone(),
            InPath::Integer(p) => p.name.clone(),
            InPath::Number(p) => p.name.clone(),
            InPath::Boolean(p) => p.name.clone(),
            InPath::Array(p) => p.name.clone(),
        },
        Parameter::Query(p) => match &**p {
            InQuery::String(p) => p.name.clone(),
            InQuery::Integer(p) => p.name.clone(),
            InQuery::Number(p) => p.name.clone(),
            InQuery::Boolean(p) => p.name.clone(),
            InQuery::Array(p) => p.name.clone(),
        },
        Parameter::Header(p) => match &**p {
            InHeader::String(p) => p.name.clone(),
            InHeader::Integer(p) => p.name.clone(),
            InHeader::Number(p) => p.name.clone(),
            InHeader::Boolean(p) => p.name.clone(),
            InHeader::Array(p) => p.name.clone(),
        },
        Parameter::FormData(p) => match &**p {
            InFormData::String(p) => p.name.clone(),
            InFormData::Integer(p) => p.name.clone(),
            InFormData::Number(p) => p.name.clone(),
            InFormData::Boolean(p) => p.name.clone(),
            InFormData::Array(p) => p.name.clone(),
            InFormData::File(p) => p.name.clone(),
        },
    }
}

/// `Parameter` dispatch: when both sides are the same outer variant
/// (Body/Path/Query/Header/FormData), recurse one level into the
/// inner enum. Mismatched outer variants replace with conflict
/// recording.
impl MergeWithContext<()> for Parameter {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        match (self, other) {
            (Parameter::Body(a), Parameter::Body(b)) => {
                if ctx.should_take_incoming(path, ConflictKind::ScalarOverridden) {
                    *a = b;
                }
            }
            (Parameter::Path(a), Parameter::Path(b)) => leaf_replace_in_path(a, *b, ctx, path),
            (Parameter::Query(a), Parameter::Query(b)) => leaf_replace_in_query(a, *b, ctx, path),
            (Parameter::Header(a), Parameter::Header(b)) => {
                leaf_replace_in_header(a, *b, ctx, path)
            }
            (Parameter::FormData(a), Parameter::FormData(b)) => {
                leaf_replace_in_form_data(a, *b, ctx, path)
            }
            (slot, incoming) => {
                if ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
                    *slot = incoming;
                }
            }
        }
    }
}

fn leaf_replace_in_path(a: &mut InPath, b: InPath, ctx: &mut MergeContext<()>, path: &mut str) {
    if *a != b && ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
        *a = b;
    }
}

fn leaf_replace_in_query(a: &mut InQuery, b: InQuery, ctx: &mut MergeContext<()>, path: &mut str) {
    if *a != b && ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
        *a = b;
    }
}

fn leaf_replace_in_header(
    a: &mut InHeader,
    b: InHeader,
    ctx: &mut MergeContext<()>,
    path: &mut str,
) {
    if *a != b && ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
        *a = b;
    }
}

fn leaf_replace_in_form_data(
    a: &mut InFormData,
    b: InFormData,
    ctx: &mut MergeContext<()>,
    path: &mut str,
) {
    if *a != b && ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
        *a = b;
    }
}

// ----- Header (typed enum — leaf replace) -----

impl MergeWithContext<()> for Header {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        if *self != other && ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch)
        {
            *self = other;
        }
    }
}

// ----- Items (typed enum — leaf replace) -----

impl MergeWithContext<()> for Items {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        if *self != other && ctx.should_take_incoming(path, ConflictKind::ScalarOverridden) {
            *self = other;
        }
    }
}

// ----- Response / Responses -----

impl MergeWithContext<()> for Responses {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        match (&mut self.default, other.default) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".default");
                b.merge_with_context(i, ctx, guard.path_mut());
            }
        }
        merge_opt_map(
            &mut self.responses,
            other.responses,
            ctx,
            path,
            "",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for Response {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::RequiredScalarOverridden,
        );
        match (&mut self.schema, other.schema) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".schema");
                b.merge_with_context(i, ctx, guard.path_mut());
            }
        }
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            path,
            ".headers",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        // examples here is Option<BTreeMap<String, serde_json::Value>>
        // — treat like extensions (per-key incoming wins).
        merge_extensions(&mut self.examples, other.examples, ctx, path, ".examples");
        merge_opt_scalar(
            &mut self.x_summary,
            other.x_summary,
            ctx,
            path,
            ".x-summary",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- Schema (leaf by default; ObjectSchema deep merge) -----

impl MergeWithContext<()> for Schema {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        if ctx.is_option(MergeOptions::DeepMergeObjectSchemas)
            && let Schema::Object(self_obj) = self
            && let Schema::Object(other_obj) = &other
        {
            // Take ownership of `other` by re-matching now that we
            // know the variant.
            let _ = other_obj;
            let Schema::Object(other_obj) = other else {
                unreachable!()
            };
            self_obj.merge_with_context(*other_obj, ctx, path);
            return;
        }
        leaf_replace_schema(self, other, ctx, path);
    }
}

fn leaf_replace_schema(base: &mut Schema, other: Schema, ctx: &mut MergeContext<()>, path: &str) {
    if *base != other && ctx.should_take_incoming(path, ConflictKind::SchemaLeafReplaced) {
        *base = other;
    }
}

impl MergeWithContext<()> for ObjectSchema {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.title,
            other.title,
            ctx,
            path,
            ".title",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.properties,
            other.properties,
            ctx,
            path,
            ".properties",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_vec_set_union(&mut self.required, other.required, ctx, path, ".required");
        merge_opt_scalar(
            &mut self.max_properties,
            other.max_properties,
            ctx,
            path,
            ".maxProperties",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.min_properties,
            other.min_properties,
            ctx,
            path,
            ".minProperties",
            ConflictKind::ScalarOverridden,
        );
        match (&mut self.additional_properties, other.additional_properties) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".additionalProperties");
                b.merge_with_context(i, ctx, guard.path_mut());
            }
        }
        merge_opt_scalar(
            &mut self.read_only,
            other.read_only,
            ctx,
            path,
            ".readOnly",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            path,
            ".externalDocs",
        );
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- SecurityScheme (leaf replace) -----

impl MergeWithContext<()> for SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        if *self != other && ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch)
        {
            *self = other;
        }
    }
}

// ----- Tag / Info / Contact / License / Logo / ExternalDocumentation / Server -----

impl MergeWithContext<()> for Tag {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.name,
            other.name,
            ctx,
            path,
            ".name",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            path,
            ".externalDocs",
        );
        merge_opt_scalar(
            &mut self.x_display_name,
            other.x_display_name,
            ctx,
            path,
            ".x-displayName",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for Info {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.title,
            other.title,
            ctx,
            path,
            ".title",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.terms_of_service,
            other.terms_of_service,
            ctx,
            path,
            ".termsOfService",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_struct(&mut self.contact, other.contact, ctx, path, ".contact");
        merge_opt_struct(&mut self.license, other.license, ctx, path, ".license");
        merge_required_scalar(
            &mut self.version,
            other.version,
            ctx,
            path,
            ".version",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_struct(&mut self.x_logo, other.x_logo, ctx, path, ".x-logo");
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for Contact {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.name,
            other.name,
            ctx,
            path,
            ".name",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.url,
            other.url,
            ctx,
            path,
            ".url",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.email,
            other.email,
            ctx,
            path,
            ".email",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for License {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.name,
            other.name,
            ctx,
            path,
            ".name",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.url,
            other.url,
            ctx,
            path,
            ".url",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for Logo {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.url,
            other.url,
            ctx,
            path,
            ".url",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.background_color,
            other.background_color,
            ctx,
            path,
            ".backgroundColor",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.alt_text,
            other.alt_text,
            ctx,
            path,
            ".altText",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.href,
            other.href,
            ctx,
            path,
            ".href",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for ExternalDocumentation {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.url,
            other.url,
            ctx,
            path,
            ".url",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

impl MergeWithContext<()> for Server {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.url,
            other.url,
            ctx,
            path,
            ".url",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- Spec.info/swagger kept-base-or-error helper -----

fn record_kept_base_or_error(
    ctx: &mut MergeContext<()>,
    path: &mut String,
    segment: &str,
    kind: ConflictKind,
) {
    let original_len = path.len();
    path.push_str(segment);
    if ctx.is_option(MergeOptions::ErrorOnConflict) {
        ctx.record(path.clone(), kind, crate::merge::Resolution::Errored);
        ctx.errored = true;
    } else {
        ctx.record(path.clone(), kind, crate::merge::Resolution::Base);
    }
    path.truncate(original_len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::{MergeOptions, Resolution};
    use std::collections::BTreeMap;

    fn root_path() -> String {
        "#".to_owned()
    }

    fn report<S: MergeWithContext<()>>(
        base: &mut S,
        incoming: S,
        opts: EnumSet<MergeOptions>,
    ) -> Vec<crate::merge::MergeConflict> {
        let mut ctx: MergeContext<()> = MergeContext::new(&(), opts);
        let mut path = root_path();
        base.merge_with_context(incoming, &mut ctx, &mut path);
        ctx.conflicts
    }

    fn spec_with_info_desc(desc: &str) -> Spec {
        Spec {
            info: Info {
                title: "T".into(),
                version: "1".into(),
                description: Some(desc.into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // ---- Three modes via Info.description ----

    #[test]
    fn spec_default_incoming_wins() {
        let mut base = spec_with_info_desc("base");
        base.merge(
            spec_with_info_desc("incoming"),
            MergeOptions::MergeInfo.only(),
        )
        .unwrap();
        assert_eq!(base.info.description.as_deref(), Some("incoming"));
    }

    #[test]
    fn spec_error_on_conflict_rolls_back() {
        let mut base = spec_with_info_desc("base");
        let result = base.merge(
            spec_with_info_desc("incoming"),
            MergeOptions::ErrorOnConflict | MergeOptions::MergeInfo,
        );
        assert!(result.is_err());
        assert_eq!(base.info.description.as_deref(), Some("base"));
    }

    #[test]
    fn spec_base_wins() {
        let mut base = spec_with_info_desc("base");
        let report = base
            .merge(
                spec_with_info_desc("incoming"),
                MergeOptions::BaseWins | MergeOptions::MergeInfo,
            )
            .unwrap();
        assert_eq!(base.info.description.as_deref(), Some("base"));
        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.resolution == Resolution::Base)
        );
    }

    // ---- Path methods coexist + per-key response merge ----

    #[test]
    fn paths_get_post_coexist() {
        let mk_pi = |method: &str| PathItem {
            operations: Some(BTreeMap::from([(method.to_owned(), Operation::default())])),
            ..Default::default()
        };
        let mut base = Spec {
            paths: Paths {
                paths: BTreeMap::from([("/pets".to_owned(), mk_pi("get"))]),
                ..Default::default()
            },
            ..Default::default()
        };
        base.merge(
            Spec {
                paths: Paths {
                    paths: BTreeMap::from([("/pets".to_owned(), mk_pi("post"))]),
                    ..Default::default()
                },
                ..Default::default()
            },
            MergeOptions::new(),
        )
        .unwrap();
        let pi = &base.paths.paths["/pets"];
        let ops = pi.operations.as_ref().unwrap();
        assert!(ops.contains_key("get") && ops.contains_key("post"));
    }

    // ---- v2-specific: host / basePath / schemes / consumes / produces ----

    #[test]
    fn spec_host_base_path_and_schemes_merge() {
        use crate::v2::spec::Scheme;
        let mut base = Spec {
            host: Some("base.example".into()),
            base_path: Some("/api".into()),
            schemes: Some(vec![Scheme::HTTPS]),
            consumes: Some(vec!["application/json".into()]),
            produces: Some(vec!["application/json".into()]),
            ..Default::default()
        };
        let _ = base.merge(
            Spec {
                host: Some("inc.example".into()),
                base_path: Some("/v2".into()),
                schemes: Some(vec![Scheme::HTTP, Scheme::HTTPS]),
                consumes: Some(vec!["text/plain".into()]),
                produces: Some(vec!["application/xml".into()]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.host.as_deref(), Some("inc.example"));
        assert_eq!(base.base_path.as_deref(), Some("/v2"));
        assert_eq!(base.schemes.unwrap().len(), 2);
        assert_eq!(base.consumes.unwrap(), vec!["text/plain".to_owned()]);
    }

    // ---- definitions / parameters / responses / securityDefinitions per-key ----

    #[test]
    fn spec_top_level_bags_merge_per_key() {
        use crate::v2::schema::Schema as V2Schema;
        let mut base = Spec {
            definitions: Some(BTreeMap::from([("Pet".into(), V2Schema::default())])),
            ..Default::default()
        };
        let _ = base.merge(
            Spec {
                definitions: Some(BTreeMap::from([
                    ("Pet".into(), V2Schema::default()),
                    ("User".into(), V2Schema::default()),
                ])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let defs = base.definitions.unwrap();
        assert!(defs.contains_key("Pet"));
        assert!(defs.contains_key("User"));
    }

    // ---- Tag dedup + x_display_name ----

    #[test]
    fn tag_x_display_name_merges() {
        let mut base = Tag {
            name: "pets".into(),
            x_display_name: Some("Pet API".into()),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Tag {
                name: "pets".into(),
                x_display_name: Some("Pets!".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.x_display_name.as_deref(), Some("Pets!"));
    }

    // ---- x_tag_groups dedup ----

    #[test]
    fn spec_x_tag_groups_dedup_and_union() {
        let mut base = Spec {
            x_tag_groups: Some(vec![TagGroup {
                name: "auth".into(),
                tags: vec!["login".into()],
                ..Default::default()
            }]),
            ..Default::default()
        };
        let _ = base.merge(
            Spec {
                x_tag_groups: Some(vec![TagGroup {
                    name: "auth".into(),
                    tags: vec!["logout".into()],
                    ..Default::default()
                }]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let groups = base.x_tag_groups.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tags.len(), 2);
    }
}
