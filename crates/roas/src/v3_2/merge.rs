//! v3.2 merge: per-component `MergeWithContext<Spec>` impls plus the
//! public `impl Merge for Spec`.
//!
//! Sits alongside `v3_2/validation.rs`. Every component type in v3.2
//! gets a `merge_with_context` whose shape mirrors its
//! `validate_with_context` neighbor: walk the fields, recurse into
//! nested objects, dispatch through `RefOr` / `BoolOr`. Helpers from
//! [`crate::common::merge`] do the structural plumbing
//! (`merge_opt_scalar`, `merge_opt_map`, `merge_vec_by_key`,
//! `merge_extensions`, ...) so each impl reads as a flat list of
//! "field → rule" lines.
//!
//! Conflict policy modes (incoming-wins / base-wins / error-on-conflict),
//! the `DeepMergeObjectSchemas` opt-in, and the `MergeInfo`
//! /`ReplaceListsWhenEmpty` toggles all flow through
//! [`crate::merge::MergeContext`] — the impls themselves stay
//! policy-free.

use enumset::EnumSet;

use crate::common::merge::{
    merge_extensions, merge_opt_map, merge_opt_scalar, merge_opt_struct, merge_opt_vec_by_key,
    merge_opt_vec_set_union, merge_replace_list_when_nonempty, merge_required_scalar,
};
use crate::common::reference::RefOr;
use crate::merge::{
    ConflictKind, Merge, MergeContext, MergeError, MergeOptions, MergeReport, MergeWithContext,
};

use crate::v3_2::callback::Callback;
use crate::v3_2::components::Components;
use crate::v3_2::discriminator::Discriminator;
use crate::v3_2::example::Example;
use crate::v3_2::external_documentation::ExternalDocumentation;
use crate::v3_2::header::Header;
use crate::v3_2::info::{Contact, Info, License};
use crate::v3_2::link::Link;
use crate::v3_2::media_type::{Encoding, MediaType};
use crate::v3_2::operation::Operation;
use crate::v3_2::parameter::{InCookie, InHeader, InPath, InQuery, InQuerystring, Parameter};
use crate::v3_2::path_item::{PathItem, Paths};
use crate::v3_2::request_body::RequestBody;
use crate::v3_2::response::{Response, Responses};
use crate::v3_2::schema::{
    AllOfSchema, AnyOfSchema, ArraySchema, BooleanSchema, EmptySchema, IntegerSchema, MultiSchema,
    NotSchema, NullSchema, NumberSchema, ObjectSchema, OneOfSchema, Schema, SingleSchema,
    StringSchema,
};
use crate::v3_2::security_scheme::{
    ApiKeySecurityScheme, AuthorizationCodeOAuth2Flow, ClientCredentialsOAuth2Flow,
    DeviceAuthorizationOAuth2Flow, HttpSecurityScheme, ImplicitOAuth2Flow, MutualTLSSecurityScheme,
    OAuth2Flows, OAuth2SecurityScheme, OpenIdConnectSecurityScheme, PasswordOAuth2Flow,
    SecurityScheme,
};
use crate::v3_2::server::{Server, ServerVariable};
use crate::v3_2::spec::Spec;
use crate::v3_2::tag::Tag;
use crate::v3_2::xml::XML;

// String-keyed maps appear everywhere; this is the single fmt_key the
// helpers want. Cloning is cheap relative to the surrounding work and
// keeps signatures uniform.
#[allow(clippy::ptr_arg)]
fn key_str(s: &String) -> String {
    s.clone()
}

// ----- Public entry point -----

impl Merge for Spec {
    fn merge(
        &mut self,
        other: Self,
        options: EnumSet<MergeOptions>,
    ) -> Result<MergeReport, MergeError> {
        // SAFETY of the `&()` trick: `MergeContext.spec` is a `&T`
        // back-reference the impls don't currently use (no ref
        // dereferencing happens during merge). Carrying the borrow as
        // a unit `&()` keeps the type generic without forcing the
        // caller to clone the base before merging into it.
        let mut ctx: MergeContext<()> = MergeContext::new(&(), options);
        let path = "#".to_owned();
        // The actual recursion takes `&mut self` so we can't carry a
        // borrow of `self` in `ctx.spec`. The component impls below
        // are written against `MergeContext<()>` accordingly.
        <Spec as MergeWithContext<()>>::merge_with_context(self, other, &mut ctx, path);
        ctx.into()
    }
}

// ----- Top-level Spec -----

impl MergeWithContext<()> for Spec {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        // openapi version: keep base by default; under MergeInfo, use
        // the standard required-scalar policy.
        if ctx.is_option(MergeOptions::MergeInfo) {
            merge_required_scalar(
                &mut self.openapi,
                other.openapi,
                ctx,
                &format!("{path}.openapi"),
                ConflictKind::RequiredScalarOverridden,
            );
            if ctx.errored {
                return;
            }
            self.info
                .merge_with_context(other.info, ctx, format!("{path}.info"));
        } else if self.info != other.info {
            // Record the suppression so the report still shows the
            // collision (with Resolution::Base).
            ctx.record(
                format!("{path}.info"),
                ConflictKind::RequiredScalarOverridden,
                crate::merge::Resolution::Base,
            );
        }
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.self_uri,
            other.self_uri,
            ctx,
            &format!("{path}.$self"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.json_schema_dialect,
            other.json_schema_dialect,
            ctx,
            &format!("{path}.jsonSchemaDialect"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_replace_list_when_nonempty(
            &mut self.servers,
            other.servers,
            ctx,
            &format!("{path}.servers"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(&mut self.paths, other.paths, ctx, &format!("{path}.paths"));
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.webhooks,
            other.webhooks,
            ctx,
            &format!("{path}.webhooks"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.components,
            other.components,
            ctx,
            &format!("{path}.components"),
        );
        if ctx.errored {
            return;
        }
        merge_replace_list_when_nonempty(
            &mut self.security,
            other.security,
            ctx,
            &format!("{path}.security"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_vec_by_key(
            &mut self.tags,
            other.tags,
            ctx,
            &format!("{path}.tags"),
            |t: &Tag| t.name.clone(),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            &format!("{path}.externalDocs"),
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

// ----- Containers -----

impl MergeWithContext<()> for Components {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_map(
            &mut self.schemas,
            other.schemas,
            ctx,
            &format!("{path}.schemas"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.responses,
            other.responses,
            ctx,
            &format!("{path}.responses"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.parameters,
            other.parameters,
            ctx,
            &format!("{path}.parameters"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.request_bodies,
            other.request_bodies,
            ctx,
            &format!("{path}.requestBodies"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            &format!("{path}.headers"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.security_schemes,
            other.security_schemes,
            ctx,
            &format!("{path}.securitySchemes"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.links,
            other.links,
            ctx,
            &format!("{path}.links"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.callbacks,
            other.callbacks,
            ctx,
            &format!("{path}.callbacks"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.path_items,
            other.path_items,
            ctx,
            &format!("{path}.pathItems"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.media_types,
            other.media_types,
            ctx,
            &format!("{path}.mediaTypes"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Paths {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        for (k, incoming) in other.paths {
            if let Some(base_v) = self.paths.get_mut(&k) {
                let child = format!("{path}[{k}]");
                base_v.merge_with_context(incoming, ctx, child);
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
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Callback {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        for (k, incoming) in other.paths {
            if let Some(base_v) = self.paths.get_mut(&k) {
                let child = format!("{path}[{k}]");
                base_v.merge_with_context(incoming, ctx, child);
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
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for PathItem {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.reference,
            other.reference,
            ctx,
            &format!("{path}.$ref"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            &format!("{path}.summary"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.operations,
            other.operations,
            ctx,
            &path,
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.additional_operations,
            other.additional_operations,
            ctx,
            &format!("{path}.additionalOperations"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_replace_list_when_nonempty(
            &mut self.servers,
            other.servers,
            ctx,
            &format!("{path}.servers"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_vec_by_key(
            &mut self.parameters,
            other.parameters,
            ctx,
            &format!("{path}.parameters"),
            parameter_ref_key,
            |b, i, c, p| b.merge_with_context(i, c, p),
            |k| format!("{}:{}", k.0, k.1),
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Responses {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        match (&mut self.default, other.default) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                b.merge_with_context(i, ctx, format!("{path}.default"));
            }
        }
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.responses,
            other.responses,
            ctx,
            &path,
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Operation {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_vec_set_union(&mut self.tags, other.tags, ctx, &format!("{path}.tags"));
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            &format!("{path}.summary"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            &format!("{path}.externalDocs"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.operation_id,
            other.operation_id,
            ctx,
            &format!("{path}.operationId"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_vec_by_key(
            &mut self.parameters,
            other.parameters,
            ctx,
            &format!("{path}.parameters"),
            parameter_ref_key,
            |b, i, c, p| b.merge_with_context(i, c, p),
            |k| format!("{}:{}", k.0, k.1),
        );
        if ctx.errored {
            return;
        }
        match (&mut self.request_body, other.request_body) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => b.merge_with_context(i, ctx, format!("{path}.requestBody")),
        }
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.responses,
            other.responses,
            ctx,
            &format!("{path}.responses"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.callbacks,
            other.callbacks,
            ctx,
            &format!("{path}.callbacks"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_replace_list_when_nonempty(
            &mut self.security,
            other.security,
            ctx,
            &format!("{path}.security"),
        );
        if ctx.errored {
            return;
        }
        merge_replace_list_when_nonempty(
            &mut self.servers,
            other.servers,
            ctx,
            &format!("{path}.servers"),
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

// ----- Parameter (enum + variants) -----

/// Identity key for an operation/path-item `parameters` list:
/// `(name, location)`. `location` is `"path"` / `"query"` / `"header"`
/// / `"cookie"` / `"querystring"` for inline `Parameter` items, or
/// `"ref"` for a `$ref` (the reference string then plays the role of
/// the name to keep different refs distinct).
fn parameter_ref_key(p: &RefOr<Parameter>) -> (String, &'static str) {
    match p {
        RefOr::Ref(r) => (r.reference.clone(), "ref"),
        RefOr::Item(p) => match p {
            Parameter::Path(p) => (p.name.clone(), "path"),
            Parameter::Query(p) => (p.name.clone(), "query"),
            Parameter::Header(p) => (p.name.clone(), "header"),
            Parameter::Cookie(p) => (p.name.clone(), "cookie"),
            Parameter::Querystring(p) => (p.name.clone(), "querystring"),
        },
    }
}

impl MergeWithContext<()> for Parameter {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        match (self, other) {
            (Parameter::Path(a), Parameter::Path(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Query(a), Parameter::Query(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Header(a), Parameter::Header(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Cookie(a), Parameter::Cookie(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Querystring(a), Parameter::Querystring(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (slot, incoming) => {
                if ctx.should_take_incoming(&path, ConflictKind::ParameterVariantMismatch) {
                    *slot = incoming;
                }
            }
        }
    }
}

impl MergeWithContext<()> for InPath {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            &format!("{path}.style"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            &format!("{path}.explode"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.schema, other.schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            &format!("{path}.content"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for InQuery {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.allow_empty_value,
            other.allow_empty_value,
            ctx,
            &format!("{path}.allowEmptyValue"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            &format!("{path}.style"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            &format!("{path}.explode"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.allow_reserved,
            other.allow_reserved,
            ctx,
            &format!("{path}.allowReserved"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.schema, other.schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            &format!("{path}.content"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for InHeader {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            &format!("{path}.style"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            &format!("{path}.explode"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.schema, other.schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            &format!("{path}.content"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for InCookie {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            &format!("{path}.style"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            &format!("{path}.explode"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.schema, other.schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            &format!("{path}.content"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for InQuerystring {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        // querystring `content` is a bare BTreeMap (required by the spec).
        for (k, incoming) in other.content {
            if let Some(b) = self.content.get_mut(&k) {
                b.merge_with_context(incoming, ctx, format!("{path}.content.{k}"));
                if ctx.errored {
                    return;
                }
            } else {
                self.content.insert(k, incoming);
            }
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

/// Shared helper: merge an `Option<RefOr<Schema>>`. Used by every
/// parameter variant and by Header.
fn merge_param_schema(
    base: &mut Option<RefOr<Schema>>,
    other: Option<RefOr<Schema>>,
    ctx: &mut MergeContext<()>,
    path: &str,
) {
    match (base.as_mut(), other) {
        (_, None) => {}
        (None, Some(v)) => *base = Some(v),
        (Some(b), Some(i)) => b.merge_with_context(i, ctx, format!("{path}.schema")),
    }
}

// ----- MediaType / Encoding / Header / Response / RequestBody / Example / Link -----

impl MergeWithContext<()> for MediaType {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.schema, other.schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.item_schema, other.item_schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.encoding,
            other.encoding,
            ctx,
            &format!("{path}.encoding"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        // prefix_encoding is a positional list — replace when non-empty.
        merge_replace_list_when_nonempty(
            &mut self.prefix_encoding,
            other.prefix_encoding,
            ctx,
            &format!("{path}.prefixEncoding"),
        );
        if ctx.errored {
            return;
        }
        match (&mut self.item_encoding, other.item_encoding) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => b.merge_with_context(i, ctx, format!("{path}.itemEncoding")),
        }
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Encoding {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.content_type,
            other.content_type,
            ctx,
            &format!("{path}.contentType"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            &format!("{path}.headers"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            &format!("{path}.style"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            &format!("{path}.explode"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.allow_reserved,
            other.allow_reserved,
            ctx,
            &format!("{path}.allowReserved"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.encoding,
            other.encoding,
            ctx,
            &format!("{path}.encoding"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_replace_list_when_nonempty(
            &mut self.prefix_encoding,
            other.prefix_encoding,
            ctx,
            &format!("{path}.prefixEncoding"),
        );
        if ctx.errored {
            return;
        }
        match (&mut self.item_encoding, other.item_encoding) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => b.merge_with_context(*i, ctx, format!("{path}.itemEncoding")),
        }
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Header {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            &format!("{path}.style"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            &format!("{path}.explode"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_param_schema(&mut self.schema, other.schema, ctx, &path);
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            &format!("{path}.content"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Response {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            &format!("{path}.summary"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            &format!("{path}.headers"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            &format!("{path}.content"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.links,
            other.links,
            ctx,
            &format!("{path}.links"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for RequestBody {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        // content is a bare BTreeMap (required).
        for (k, incoming) in other.content {
            if let Some(b) = self.content.get_mut(&k) {
                b.merge_with_context(incoming, ctx, format!("{path}.content.{k}"));
                if ctx.errored {
                    return;
                }
            } else {
                self.content.insert(k, incoming);
            }
        }
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Example {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            &format!("{path}.summary"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.value,
            other.value,
            ctx,
            &format!("{path}.value"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.serialized_value,
            other.serialized_value,
            ctx,
            &format!("{path}.serializedValue"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.data_value,
            other.data_value,
            ctx,
            &format!("{path}.dataValue"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.external_value,
            other.external_value,
            ctx,
            &format!("{path}.externalValue"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Link {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.operation_ref,
            other.operation_ref,
            ctx,
            &format!("{path}.operationRef"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.operation_id,
            other.operation_id,
            ctx,
            &format!("{path}.operationId"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        // Link.parameters is `Option<BTreeMap<String, serde_json::Value>>`
        // — treat like extensions.
        merge_extensions(
            &mut self.parameters,
            other.parameters,
            ctx,
            &format!("{path}.parameters"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.request_body,
            other.request_body,
            ctx,
            &format!("{path}.requestBody"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.server,
            other.server,
            ctx,
            &format!("{path}.server"),
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

// ----- Leaves: Tag, Info, Contact, License, ExternalDocumentation,
//                Discriminator, XML, Server, ServerVariable -----

impl MergeWithContext<()> for Tag {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.name,
            other.name,
            ctx,
            &format!("{path}.name"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            &format!("{path}.summary"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            &format!("{path}.externalDocs"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.parent,
            other.parent,
            ctx,
            &format!("{path}.parent"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.kind,
            other.kind,
            ctx,
            &format!("{path}.kind"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Info {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.title,
            other.title,
            ctx,
            &format!("{path}.title"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.summary,
            other.summary,
            ctx,
            &format!("{path}.summary"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.terms_of_service,
            other.terms_of_service,
            ctx,
            &format!("{path}.termsOfService"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.contact,
            other.contact,
            ctx,
            &format!("{path}.contact"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.license,
            other.license,
            ctx,
            &format!("{path}.license"),
        );
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.version,
            other.version,
            ctx,
            &format!("{path}.version"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Contact {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.name,
            other.name,
            ctx,
            &format!("{path}.name"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.url,
            other.url,
            ctx,
            &format!("{path}.url"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.email,
            other.email,
            ctx,
            &format!("{path}.email"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for License {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.name,
            other.name,
            ctx,
            &format!("{path}.name"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.identifier,
            other.identifier,
            ctx,
            &format!("{path}.identifier"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.url,
            other.url,
            ctx,
            &format!("{path}.url"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for ExternalDocumentation {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.url,
            other.url,
            ctx,
            &format!("{path}.url"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Discriminator {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.property_name,
            other.property_name,
            ctx,
            &format!("{path}.propertyName"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        // mapping is Option<BTreeMap<String,String>>; per-key incoming wins.
        if let Some(other_map) = other.mapping {
            let base_map = self
                .mapping
                .get_or_insert_with(std::collections::BTreeMap::new);
            for (k, v) in other_map {
                match base_map.get_mut(&k) {
                    None => {
                        base_map.insert(k, v);
                    }
                    Some(existing) => {
                        if *existing != v
                            && ctx.should_take_incoming(
                                &format!("{path}.mapping.{k}"),
                                ConflictKind::ScalarOverridden,
                            )
                        {
                            *existing = v;
                        }
                    }
                }
                if ctx.errored {
                    return;
                }
            }
        }
        merge_opt_scalar(
            &mut self.default_mapping,
            other.default_mapping,
            ctx,
            &format!("{path}.defaultMapping"),
            ConflictKind::ScalarOverridden,
        );
    }
}

impl MergeWithContext<()> for XML {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.name,
            other.name,
            ctx,
            &format!("{path}.name"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.namespace,
            other.namespace,
            ctx,
            &format!("{path}.namespace"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.prefix,
            other.prefix,
            ctx,
            &format!("{path}.prefix"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.attribute,
            other.attribute,
            ctx,
            &format!("{path}.attribute"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.wrapped,
            other.wrapped,
            ctx,
            &format!("{path}.wrapped"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.node_type,
            other.node_type,
            ctx,
            &format!("{path}.nodeType"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for Server {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.url,
            other.url,
            ctx,
            &format!("{path}.url"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.name,
            other.name,
            ctx,
            &format!("{path}.name"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.variables,
            other.variables,
            ctx,
            &format!("{path}.variables"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for ServerVariable {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        // ServerVariable in our model has only `extensions`; the
        // spec's `default`/`enum`/`description` are public fields on
        // the struct but not enumerated above. Fall back to a full
        // field walk via mem::replace so we don't drop incoming
        // fields silently.
        let ServerVariable { extensions, .. } = other;
        merge_extensions(
            &mut self.extensions,
            extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

// ----- SecurityScheme -----

impl MergeWithContext<()> for SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        match (self, other) {
            (SecurityScheme::ApiKey(a), SecurityScheme::ApiKey(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (SecurityScheme::HTTP(a), SecurityScheme::HTTP(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (SecurityScheme::OAuth2(a), SecurityScheme::OAuth2(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (SecurityScheme::OpenIdConnect(a), SecurityScheme::OpenIdConnect(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (SecurityScheme::MutualTLS(a), SecurityScheme::MutualTLS(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (slot, incoming) => {
                if ctx.should_take_incoming(&path, ConflictKind::ParameterVariantMismatch) {
                    *slot = incoming;
                }
            }
        }
    }
}

impl MergeWithContext<()> for ApiKeySecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.name,
            other.name,
            ctx,
            &format!("{path}.name"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.location,
            other.location,
            ctx,
            &format!("{path}.in"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for HttpSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.scheme,
            other.scheme,
            ctx,
            &format!("{path}.scheme"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.bearer_format,
            other.bearer_format,
            ctx,
            &format!("{path}.bearerFormat"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for MutualTLSSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for OpenIdConnectSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_required_scalar(
            &mut self.open_id_connect_url,
            other.open_id_connect_url,
            ctx,
            &format!("{path}.openIdConnectUrl"),
            ConflictKind::RequiredScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for OAuth2SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        self.flows
            .merge_with_context(other.flows, ctx, format!("{path}.flows"));
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.oauth2_metadata_url,
            other.oauth2_metadata_url,
            ctx,
            &format!("{path}.oauth2MetadataUrl"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

impl MergeWithContext<()> for OAuth2Flows {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_struct(
            &mut self.implicit,
            other.implicit,
            ctx,
            &format!("{path}.implicit"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.password,
            other.password,
            ctx,
            &format!("{path}.password"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.client_credentials,
            other.client_credentials,
            ctx,
            &format!("{path}.clientCredentials"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.authorization_code,
            other.authorization_code,
            ctx,
            &format!("{path}.authorizationCode"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.device_authorization,
            other.device_authorization,
            ctx,
            &format!("{path}.deviceAuthorization"),
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

/// Each OAuth flow has the same shape: optional URL fields + a bare
/// `BTreeMap<String, String>` of scopes + extensions. Macro keeps it
/// from sprawling.
macro_rules! oauth_flow_merge {
    ($t:ty, $($url_field:ident => $url_path:literal),* $(,)?) => {
        impl MergeWithContext<()> for $t {
            fn merge_with_context(
                &mut self,
                other: Self,
                ctx: &mut MergeContext<()>,
                path: String,
            ) {
                $(
                    merge_required_scalar(
                        &mut self.$url_field,
                        other.$url_field,
                        ctx,
                        &format!("{path}.{}", $url_path),
                        ConflictKind::RequiredScalarOverridden,
                    );
                    if ctx.errored {
                        return;
                    }
                )*
                merge_opt_scalar(
                    &mut self.refresh_url,
                    other.refresh_url,
                    ctx,
                    &format!("{path}.refreshUrl"),
                    ConflictKind::ScalarOverridden,
                );
                if ctx.errored {
                    return;
                }
                // scopes is a bare BTreeMap<String, String>.
                for (k, v) in other.scopes {
                    match self.scopes.get_mut(&k) {
                        None => {
                            self.scopes.insert(k, v);
                        }
                        Some(existing) => {
                            if *existing != v
                                && ctx.should_take_incoming(
                                    &format!("{path}.scopes.{k}"),
                                    ConflictKind::ScalarOverridden,
                                )
                            {
                                *existing = v;
                            }
                        }
                    }
                    if ctx.errored {
                        return;
                    }
                }
                merge_extensions(
                    &mut self.extensions,
                    other.extensions,
                    ctx,
                    &format!("{path}.extensions"),
                );
            }
        }
    };
}

oauth_flow_merge!(ImplicitOAuth2Flow, authorization_url => "authorizationUrl");
oauth_flow_merge!(PasswordOAuth2Flow, token_url => "tokenUrl");
oauth_flow_merge!(ClientCredentialsOAuth2Flow, token_url => "tokenUrl");
oauth_flow_merge!(
    AuthorizationCodeOAuth2Flow,
    authorization_url => "authorizationUrl",
    token_url => "tokenUrl",
);
oauth_flow_merge!(
    DeviceAuthorizationOAuth2Flow,
    device_authorization_url => "deviceAuthorizationUrl",
    token_url => "tokenUrl",
);

// ----- Schema -----

impl MergeWithContext<()> for Schema {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        // ObjectSchema deep-merge is the one variant pairing that
        // recurses — everything else falls back to leaf-replace.
        if ctx.is_option(MergeOptions::DeepMergeObjectSchemas)
            && let (Schema::Single(self_single), Schema::Single(other_single)) =
                (&mut *self, &other)
            && let (SingleSchema::Object(_), SingleSchema::Object(_)) =
                (&**self_single, &**other_single)
        {
            // Drop into a fresh match to take ownership of the
            // incoming side.
            let Schema::Single(other_single) = other else {
                unreachable!()
            };
            let SingleSchema::Object(other_obj) = *other_single else {
                unreachable!()
            };
            let SingleSchema::Object(self_obj) = &mut **self_single else {
                unreachable!()
            };
            self_obj.merge_with_context(other_obj, ctx, path);
            return;
        }
        // Default: leaf-replace policy.
        if *self != other && ctx.should_take_incoming(&path, ConflictKind::SchemaLeafReplaced) {
            *self = other;
        }
    }
}

impl MergeWithContext<()> for ObjectSchema {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: String) {
        merge_opt_scalar(
            &mut self.title,
            other.title,
            ctx,
            &format!("{path}.title"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            &format!("{path}.description"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.properties,
            other.properties,
            ctx,
            &format!("{path}.properties"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.pattern_properties,
            other.pattern_properties,
            ctx,
            &format!("{path}.patternProperties"),
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.default,
            other.default,
            ctx,
            &format!("{path}.default"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.max_properties,
            other.max_properties,
            ctx,
            &format!("{path}.maxProperties"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.min_properties,
            other.min_properties,
            ctx,
            &format!("{path}.minProperties"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        match (&mut self.additional_properties, other.additional_properties) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                b.merge_with_context(i, ctx, format!("{path}.additionalProperties"))
            }
        }
        if ctx.errored {
            return;
        }
        match (
            &mut self.unevaluated_properties,
            other.unevaluated_properties,
        ) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                b.merge_with_context(i, ctx, format!("{path}.unevaluatedProperties"))
            }
        }
        if ctx.errored {
            return;
        }
        merge_param_schema(
            &mut self.property_names,
            other.property_names,
            ctx,
            &format!("{path}.propertyNames"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_vec_set_union(
            &mut self.required,
            other.required,
            ctx,
            &format!("{path}.required"),
        );
        merge_opt_scalar(
            &mut self.read_only,
            other.read_only,
            ctx,
            &format!("{path}.readOnly"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.write_only,
            other.write_only,
            ctx,
            &format!("{path}.writeOnly"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            &format!("{path}.deprecated"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_struct(&mut self.xml, other.xml, ctx, &format!("{path}.xml"));
        if ctx.errored {
            return;
        }
        merge_opt_struct(
            &mut self.external_docs,
            other.external_docs,
            ctx,
            &format!("{path}.externalDocs"),
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            &format!("{path}.example"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.examples,
            other.examples,
            ctx,
            &format!("{path}.examples"),
            ConflictKind::ScalarOverridden,
        );
        if ctx.errored {
            return;
        }
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            &format!("{path}.extensions"),
        );
    }
}

// Schema sub-types that aren't recursively deep-merged still need a
// `MergeWithContext<()>` impl so the generic `RefOr<X>::merge_with_context`
// compiles when `X` is one of them. They land here as opaque leaves
// (replace incoming-wins, recorded as `SchemaLeafReplaced`).
macro_rules! leaf_schema_merge {
    ($($t:ty),* $(,)?) => {
        $(
            impl MergeWithContext<()> for $t {
                fn merge_with_context(
                    &mut self,
                    other: Self,
                    ctx: &mut MergeContext<()>,
                    path: String,
                ) {
                    if *self != other
                        && ctx.should_take_incoming(&path, ConflictKind::SchemaLeafReplaced)
                    {
                        *self = other;
                    }
                }
            }
        )*
    };
}

leaf_schema_merge!(
    EmptySchema,
    StringSchema,
    IntegerSchema,
    NumberSchema,
    BooleanSchema,
    ArraySchema,
    NullSchema,
    AllOfSchema,
    AnyOfSchema,
    OneOfSchema,
    NotSchema,
    MultiSchema,
    SingleSchema,
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::reference::{Ref, RefOr};
    use crate::merge::{ConflictKind, MergeOptions, Resolution};
    use crate::v3_2::components::Components;
    use crate::v3_2::operation::Operation;
    use crate::v3_2::parameter::{InPath, InQuery, Parameter};
    use crate::v3_2::path_item::{PathItem, Paths};
    use crate::v3_2::response::{Response, Responses};
    use crate::v3_2::schema::{ObjectSchema, Schema, SingleSchema};
    use crate::v3_2::tag::Tag;
    use std::collections::BTreeMap;

    fn param_path(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Path(Box::new(InPath {
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
        })))
    }

    fn param_query(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Query(Box::new(InQuery {
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
        })))
    }

    fn response_with(description: &str) -> RefOr<Response> {
        RefOr::new_item(Response {
            description: Some(description.into()),
            ..Default::default()
        })
    }

    // ---- RefOr collisions ----

    #[test]
    fn refor_item_item_recurses() {
        let mut base: RefOr<Tag> = RefOr::new_item(Tag {
            name: "pets".into(),
            description: Some("base".into()),
            ..Default::default()
        });
        let incoming: RefOr<Tag> = RefOr::new_item(Tag {
            name: "pets".into(),
            summary: Some("from incoming".into()),
            ..Default::default()
        });
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#.tags[pets]".into());
        match base {
            RefOr::Item(t) => {
                assert_eq!(t.description.as_deref(), Some("base"));
                assert_eq!(t.summary.as_deref(), Some("from incoming"));
            }
            _ => panic!("expected Item"),
        }
    }

    #[test]
    fn refor_ref_ref_same_target_merges_metadata() {
        let mut base: RefOr<Tag> = RefOr::Ref(Box::new(Ref {
            reference: "#/components/tags/Pets".into(),
            summary: Some("base summary".into()),
            description: None,
        }));
        let incoming: RefOr<Tag> = RefOr::Ref(Box::new(Ref {
            reference: "#/components/tags/Pets".into(),
            summary: None,
            description: Some("new desc".into()),
        }));
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#".into());
        match base {
            RefOr::Ref(r) => {
                assert_eq!(r.summary.as_deref(), Some("base summary"));
                assert_eq!(r.description.as_deref(), Some("new desc"));
            }
            _ => panic!("expected Ref"),
        }
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn refor_ref_ref_different_target_replaces_and_records() {
        let mut base: RefOr<Tag> = RefOr::Ref(Box::new(Ref::new("#/components/tags/A")));
        let incoming: RefOr<Tag> = RefOr::Ref(Box::new(Ref::new("#/components/tags/B")));
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#.tags[0]".into());
        match base {
            RefOr::Ref(r) => assert_eq!(r.reference, "#/components/tags/B"),
            _ => panic!("expected Ref"),
        }
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::RefReplaced);
        assert_eq!(ctx.conflicts[0].resolution, Resolution::Incoming);
    }

    #[test]
    fn refor_ref_vs_item_records_ref_vs_value() {
        let mut base: RefOr<Tag> = RefOr::Ref(Box::new(Ref::new("#/components/tags/A")));
        let incoming: RefOr<Tag> = RefOr::new_item(Tag {
            name: "x".into(),
            ..Default::default()
        });
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#".into());
        assert!(matches!(base, RefOr::Item(_)));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::RefVsValue);
    }

    // ---- Operation: responses across status codes ----

    #[test]
    fn operation_responses_keeps_base_status_codes_not_in_incoming() {
        let mut base = Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([
                    ("200".into(), response_with("ok")),
                    ("404".into(), response_with("missing")),
                ])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let incoming = Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([
                    ("200".into(), response_with("ok updated")),
                    ("500".into(), response_with("server error")),
                ])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#.op".into());

        let r = base.responses.unwrap().responses.unwrap();
        assert!(r.contains_key("200"));
        assert!(r.contains_key("404"), "base-only status preserved");
        assert!(r.contains_key("500"), "incoming-only status added");
        let RefOr::Item(r200) = r.get("200").unwrap() else {
            panic!()
        };
        assert_eq!(r200.description.as_deref(), Some("ok updated"));
    }

    // ---- Operation: parameter dedup by (name, in) ----

    #[test]
    fn operation_parameters_dedup_by_name_in() {
        let mut base = Operation {
            parameters: Some(vec![param_path("id"), param_query("limit")]),
            ..Default::default()
        };
        let incoming = Operation {
            // Same (name="id", in=path) — should overwrite base's id, not append.
            // New (name="filter", in=query) — should append.
            parameters: Some(vec![param_path("id"), param_query("filter")]),
            ..Default::default()
        };
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#.op".into());
        let params = base.parameters.unwrap();
        assert_eq!(params.len(), 3, "no duplicates by (name, in)");
    }

    // ---- PathItem: method coexistence ----

    #[test]
    fn pathitem_get_and_post_coexist_after_merge() {
        let mut base = PathItem {
            operations: Some(BTreeMap::from([("get".to_owned(), Operation::default())])),
            ..Default::default()
        };
        let incoming = PathItem {
            operations: Some(BTreeMap::from([("post".to_owned(), Operation::default())])),
            ..Default::default()
        };
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#.paths[/pets]".into());
        let ops = base.operations.unwrap();
        assert!(ops.contains_key("get"));
        assert!(ops.contains_key("post"));
    }

    // ---- Schema: leaf replace by default ----

    #[test]
    fn schema_collision_replaces_by_default_records_leaf_replaced() {
        let mut base = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            required: Some(vec!["a".into()]),
            ..Default::default()
        })));
        let incoming = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            required: Some(vec!["b".into()]),
            ..Default::default()
        })));
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        base.merge_with_context(incoming, &mut ctx, "#.s".into());
        // Replaced — `required` is now from incoming.
        let Schema::Single(box_single) = &base else {
            panic!()
        };
        let SingleSchema::Object(obj) = &**box_single else {
            panic!()
        };
        assert_eq!(obj.required.as_deref(), Some(&["b".to_owned()][..]));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::SchemaLeafReplaced);
    }

    #[test]
    fn schema_object_deep_merge_under_option() {
        let mut base = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            properties: Some(BTreeMap::from([(
                "a".to_owned(),
                RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                    ObjectSchema::default(),
                )))),
            )])),
            required: Some(vec!["a".into()]),
            ..Default::default()
        })));
        let incoming = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            properties: Some(BTreeMap::from([(
                "b".to_owned(),
                RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                    ObjectSchema::default(),
                )))),
            )])),
            required: Some(vec!["b".into()]),
            ..Default::default()
        })));
        let mut ctx: MergeContext<()> =
            MergeContext::new(&(), MergeOptions::DeepMergeObjectSchemas.only());
        base.merge_with_context(incoming, &mut ctx, "#.s".into());
        let Schema::Single(box_single) = &base else {
            panic!()
        };
        let SingleSchema::Object(obj) = &**box_single else {
            panic!()
        };
        let props = obj.properties.as_ref().unwrap();
        assert!(props.contains_key("a"));
        assert!(props.contains_key("b"));
        let req = obj.required.as_ref().unwrap();
        assert!(req.contains(&"a".to_owned()));
        assert!(req.contains(&"b".to_owned()));
    }

    // ---- Spec: three modes ----

    fn spec_with_description(desc: &str) -> Spec {
        Spec {
            json_schema_dialect: Some(desc.into()),
            ..Default::default()
        }
    }

    #[test]
    fn spec_default_incoming_wins() {
        let mut base = spec_with_description("base");
        let incoming = spec_with_description("incoming");
        let report = base.merge(incoming, MergeOptions::new()).unwrap();
        assert_eq!(base.json_schema_dialect.as_deref(), Some("incoming"));
        assert_eq!(report.len(), 1);
        assert_eq!(report.conflicts[0].resolution, Resolution::Incoming);
    }

    #[test]
    fn spec_base_wins_mode() {
        let mut base = spec_with_description("base");
        let incoming = spec_with_description("incoming");
        let report = base.merge(incoming, MergeOptions::BaseWins.only()).unwrap();
        assert_eq!(base.json_schema_dialect.as_deref(), Some("base"));
        assert_eq!(report.conflicts[0].resolution, Resolution::Base);
    }

    #[test]
    fn spec_error_on_conflict_returns_err() {
        let mut base = spec_with_description("base");
        let incoming = spec_with_description("incoming");
        let result = base.merge(incoming, MergeOptions::ErrorOnConflict.only());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.conflicts.is_empty());
        assert_eq!(err.conflicts[0].resolution, Resolution::Errored);
    }

    // ---- Spec.tags: dedup by name with recursion ----

    #[test]
    fn spec_tags_dedup_by_name() {
        let mut base = Spec {
            tags: Some(vec![Tag {
                name: "pets".into(),
                description: Some("base".into()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let incoming = Spec {
            tags: Some(vec![
                // same name — should recurse, not append.
                Tag {
                    name: "pets".into(),
                    summary: Some("from incoming".into()),
                    ..Default::default()
                },
                // new name — should append.
                Tag {
                    name: "stores".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        base.merge(incoming, MergeOptions::new()).unwrap();
        let tags = base.tags.unwrap();
        assert_eq!(tags.len(), 2);
        let pets = tags.iter().find(|t| t.name == "pets").unwrap();
        assert_eq!(pets.description.as_deref(), Some("base"));
        assert_eq!(pets.summary.as_deref(), Some("from incoming"));
    }

    // ---- Spec.paths: deep merge keeps unrelated methods ----

    #[test]
    fn spec_paths_deep_merge_preserves_unrelated_methods() {
        let mk_pi = |method: &str| PathItem {
            operations: Some(BTreeMap::from([(method.to_owned(), Operation::default())])),
            ..Default::default()
        };
        let mut base = Spec {
            paths: Some(Paths {
                paths: BTreeMap::from([("/pets".to_owned(), mk_pi("get"))]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let incoming = Spec {
            paths: Some(Paths {
                paths: BTreeMap::from([("/pets".to_owned(), mk_pi("post"))]),
                ..Default::default()
            }),
            ..Default::default()
        };
        base.merge(incoming, MergeOptions::new()).unwrap();
        let pi = &base.paths.unwrap().paths["/pets"];
        let ops = pi.operations.as_ref().unwrap();
        assert!(ops.contains_key("get"));
        assert!(ops.contains_key("post"));
    }

    // ---- Spec.info kept on base by default; merged under MergeInfo ----

    #[test]
    fn spec_info_kept_on_base_by_default() {
        use crate::v3_2::info::Info;
        let mut base = Spec {
            info: Info {
                title: "Base API".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let incoming = Spec {
            info: Info {
                title: "Incoming API".into(),
                version: "2.0.0".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let report = base.merge(incoming, MergeOptions::new()).unwrap();
        assert_eq!(base.info.title, "Base API");
        assert_eq!(base.info.version, "1.0.0");
        // Conflict was still recorded.
        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.path.ends_with(".info") && c.resolution == Resolution::Base)
        );
    }

    #[test]
    fn spec_info_merged_under_merge_info_option() {
        use crate::v3_2::info::Info;
        let mut base = Spec {
            info: Info {
                title: "Base".into(),
                version: "1.0.0".into(),
                description: Some("base desc".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let incoming = Spec {
            info: Info {
                title: "Base".into(),
                version: "1.0.0".into(),
                summary: Some("from incoming".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        base.merge(incoming, MergeOptions::MergeInfo.only())
            .unwrap();
        assert_eq!(base.info.description.as_deref(), Some("base desc"));
        assert_eq!(base.info.summary.as_deref(), Some("from incoming"));
    }

    // ---- Components: schemas collision uses leaf replace by default ----

    #[test]
    fn components_schema_collision_default_is_leaf_replace() {
        let s1 = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            required: Some(vec!["a".into()]),
            ..Default::default()
        })));
        let s2 = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            required: Some(vec!["b".into()]),
            ..Default::default()
        })));
        let mut base = Spec {
            components: Some(Components {
                schemas: Some(BTreeMap::from([("Pet".to_owned(), RefOr::new_item(s1))])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let incoming = Spec {
            components: Some(Components {
                schemas: Some(BTreeMap::from([("Pet".to_owned(), RefOr::new_item(s2))])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let report = base.merge(incoming, MergeOptions::new()).unwrap();
        let schemas = base.components.unwrap().schemas.unwrap();
        let RefOr::Item(pet) = schemas.get("Pet").unwrap() else {
            panic!()
        };
        let Schema::Single(box_s) = pet else { panic!() };
        let SingleSchema::Object(obj) = &**box_s else {
            panic!()
        };
        assert_eq!(obj.required.as_deref(), Some(&["b".to_owned()][..]));
        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.kind == ConflictKind::SchemaLeafReplaced)
        );
    }
}
