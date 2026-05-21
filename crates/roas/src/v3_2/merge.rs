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
    PathGuard, merge_extensions, merge_opt_map, merge_opt_scalar, merge_opt_struct,
    merge_opt_vec_by_key, merge_opt_vec_set_union, merge_replace_list_when_nonempty,
    merge_required_scalar,
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
use crate::v3_2::server::Server;
use crate::v3_2::spec::Spec;
use crate::v3_2::tag::Tag;
use crate::v3_2::xml::XML;

// String-keyed maps appear everywhere; this is the single fmt_key
// the helpers want. Writes the key directly into the path buffer
// instead of allocating a fresh String — the helpers' `fmt_key`
// signature is `Fn(&K, &mut String)` for exactly this reason.
#[allow(clippy::ptr_arg)]
fn key_str(s: &String, out: &mut String) {
    out.push_str(s);
}

/// `fmt_key` for the `(name, location)` keys used by
/// `Operation.parameters` / `PathItem.parameters`. Writes
/// `<name>:<location>` into the path buffer.
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
        // SAFETY of the `&()` trick: `MergeContext.spec` is a `&T`
        // back-reference the impls don't currently use (no ref
        // dereferencing happens during merge). Carrying the borrow as
        // a unit `&()` keeps the type generic without forcing the
        // caller to clone the base before merging into it.
        let mut ctx: MergeContext<()> = MergeContext::new(&(), options);
        // Single owned `String` for the whole merge — every child
        // pushes/truncates segments onto it via the helper guards.
        let mut path = String::from("#");

        if options.contains(MergeOptions::ErrorOnConflict) {
            // Strict-mode rollback: clone `self` into a working copy
            // and only commit it back on success. Without this, an
            // `Err` would leave `self` partially merged (additive
            // steps run before the first collision flips `ctx.errored`),
            // which contradicts the contract callers reach for
            // `ErrorOnConflict` to get. The clone cost is paid only
            // when the option is opted in.
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

        // Non-strict modes mutate `self` in place — the report still
        // captures every resolution, so callers wanting "see what
        // changed" don't need the clone-and-replace overhead.
        <Spec as MergeWithContext<()>>::merge_with_context(self, other, &mut ctx, &mut path);
        ctx.into()
    }
}

// ----- Top-level Spec -----

impl MergeWithContext<()> for Spec {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        // openapi version + info: under MergeInfo, full required-scalar
        // policy; otherwise the documented contract is "kept on base."
        // We still want `ErrorOnConflict` to catch a real mismatch and
        // we still want the conflict in the report (with
        // `Resolution::Base` to reflect that base actually won),
        // hence the explicit comparison + `record_kept_base_or_error`
        // helper rather than routing through `should_take_incoming`
        // (which embeds the default incoming-wins policy and would
        // record `Resolution::Incoming` even though no mutation
        // happened).
        if ctx.is_option(MergeOptions::MergeInfo) {
            merge_required_scalar(
                &mut self.openapi,
                other.openapi,
                ctx,
                path,
                ".openapi",
                ConflictKind::RequiredScalarOverridden,
            );
            {
                let mut guard = PathGuard::new(path, ".info");
                self.info
                    .merge_with_context(other.info, ctx, guard.path_mut());
            }
        } else {
            if self.openapi != other.openapi {
                record_kept_base_or_error(
                    ctx,
                    path,
                    ".openapi",
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
            &mut self.self_uri,
            other.self_uri,
            ctx,
            path,
            ".$self",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.json_schema_dialect,
            other.json_schema_dialect,
            ctx,
            path,
            ".jsonSchemaDialect",
            ConflictKind::ScalarOverridden,
        );
        merge_replace_list_when_nonempty(&mut self.servers, other.servers, ctx, path, ".servers");
        merge_opt_struct(&mut self.paths, other.paths, ctx, path, ".paths");
        merge_opt_struct(&mut self.webhooks, other.webhooks, ctx, path, ".webhooks");
        merge_opt_struct(
            &mut self.components,
            other.components,
            ctx,
            path,
            ".components",
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
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- Containers -----

impl MergeWithContext<()> for Components {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_map(
            &mut self.schemas,
            other.schemas,
            ctx,
            path,
            ".schemas",
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
            &mut self.parameters,
            other.parameters,
            ctx,
            path,
            ".parameters",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.request_bodies,
            other.request_bodies,
            ctx,
            path,
            ".requestBodies",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            path,
            ".headers",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.security_schemes,
            other.security_schemes,
            ctx,
            path,
            ".securitySchemes",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.links,
            other.links,
            ctx,
            path,
            ".links",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.callbacks,
            other.callbacks,
            ctx,
            path,
            ".callbacks",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.path_items,
            other.path_items,
            ctx,
            path,
            ".pathItems",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.media_types,
            other.media_types,
            ctx,
            path,
            ".mediaTypes",
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

impl MergeWithContext<()> for Callback {
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
        merge_opt_map(
            &mut self.operations,
            other.operations,
            ctx,
            path,
            // No outer segment — operations live directly under the
            // path item (e.g. `#.paths[/pets].get`).
            "",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.additional_operations,
            other.additional_operations,
            ctx,
            path,
            ".additionalOperations",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_replace_list_when_nonempty(&mut self.servers, other.servers, ctx, path, ".servers");
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
            // Per-status responses live directly under Responses
            // (e.g. `#.responses.200`).
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
        match (&mut self.request_body, other.request_body) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".requestBody");
                b.merge_with_context(i, ctx, guard.path_mut());
            }
        }
        merge_opt_struct(
            &mut self.responses,
            other.responses,
            ctx,
            path,
            ".responses",
        );
        merge_opt_map(
            &mut self.callbacks,
            other.callbacks,
            ctx,
            path,
            ".callbacks",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
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
        merge_replace_list_when_nonempty(&mut self.servers, other.servers, ctx, path, ".servers");
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
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
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        match (self, other) {
            (Parameter::Path(a), Parameter::Path(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Query(a), Parameter::Query(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Header(a), Parameter::Header(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Cookie(a), Parameter::Cookie(b)) => a.merge_with_context(*b, ctx, path),
            (Parameter::Querystring(a), Parameter::Querystring(b)) => {
                a.merge_with_context(*b, ctx, path)
            }
            (slot, incoming) => {
                if ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
                    *slot = incoming;
                }
            }
        }
    }
}

impl MergeWithContext<()> for InPath {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_required_scalar(
            &mut self.required,
            other.required,
            ctx,
            path,
            ".required",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            path,
            ".style",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            path,
            ".explode",
            ConflictKind::ScalarOverridden,
        );
        merge_schema_field(&mut self.schema, other.schema, ctx, path, "schema");
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            path,
            ".content",
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

impl MergeWithContext<()> for InQuery {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            path,
            ".required",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.allow_empty_value,
            other.allow_empty_value,
            ctx,
            path,
            ".allowEmptyValue",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            path,
            ".style",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            path,
            ".explode",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.allow_reserved,
            other.allow_reserved,
            ctx,
            path,
            ".allowReserved",
            ConflictKind::ScalarOverridden,
        );
        merge_schema_field(&mut self.schema, other.schema, ctx, path, "schema");
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            path,
            ".content",
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

impl MergeWithContext<()> for InHeader {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            path,
            ".required",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            path,
            ".style",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            path,
            ".explode",
            ConflictKind::ScalarOverridden,
        );
        merge_schema_field(&mut self.schema, other.schema, ctx, path, "schema");
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            path,
            ".content",
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

impl MergeWithContext<()> for InCookie {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            path,
            ".required",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            path,
            ".style",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            path,
            ".explode",
            ConflictKind::ScalarOverridden,
        );
        merge_schema_field(&mut self.schema, other.schema, ctx, path, "schema");
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            path,
            ".content",
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

impl MergeWithContext<()> for InQuerystring {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            path,
            ".required",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        // querystring `content` is a bare BTreeMap (required by the spec).
        for (k, incoming) in other.content {
            if let Some(b) = self.content.get_mut(&k) {
                let original_len = path.len();
                path.push_str(".content.");
                path.push_str(&k);
                b.merge_with_context(incoming, ctx, path);
                path.truncate(original_len);
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
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
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

/// Shared helper: merge an `Option<RefOr<Schema>>`. `field_name` is
/// the JSONPath segment (`schema`, `itemSchema`, `propertyNames`, …)
/// — without it, every collision was previously misreported as
/// `.schema`.
fn merge_schema_field(
    base: &mut Option<RefOr<Schema>>,
    other: Option<RefOr<Schema>>,
    ctx: &mut MergeContext<()>,
    path: &mut String,
    field_name: &str,
) {
    if ctx.errored {
        return;
    }
    match (base.as_mut(), other) {
        (_, None) => {}
        (None, Some(v)) => *base = Some(v),
        (Some(b), Some(i)) => {
            let original_len = path.len();
            path.push('.');
            path.push_str(field_name);
            b.merge_with_context(i, ctx, path);
            path.truncate(original_len);
        }
    }
}

// ----- MediaType / Encoding / Header / Response / RequestBody / Example / Link -----

impl MergeWithContext<()> for MediaType {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_schema_field(&mut self.schema, other.schema, ctx, path, "schema");
        merge_schema_field(
            &mut self.item_schema,
            other.item_schema,
            ctx,
            path,
            "itemSchema",
        );
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.encoding,
            other.encoding,
            ctx,
            path,
            ".encoding",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        // prefix_encoding is a positional list — replace when non-empty.
        merge_replace_list_when_nonempty(
            &mut self.prefix_encoding,
            other.prefix_encoding,
            ctx,
            path,
            ".prefixEncoding",
        );
        match (&mut self.item_encoding, other.item_encoding) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".itemEncoding");
                b.merge_with_context(i, ctx, guard.path_mut());
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

impl MergeWithContext<()> for Encoding {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.content_type,
            other.content_type,
            ctx,
            path,
            ".contentType",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            path,
            ".headers",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            path,
            ".style",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            path,
            ".explode",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.allow_reserved,
            other.allow_reserved,
            ctx,
            path,
            ".allowReserved",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.encoding,
            other.encoding,
            ctx,
            path,
            ".encoding",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_replace_list_when_nonempty(
            &mut self.prefix_encoding,
            other.prefix_encoding,
            ctx,
            path,
            ".prefixEncoding",
        );
        match (&mut self.item_encoding, other.item_encoding) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".itemEncoding");
                b.merge_with_context(*i, ctx, guard.path_mut());
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

impl MergeWithContext<()> for Header {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.required,
            other.required,
            ctx,
            path,
            ".required",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.style,
            other.style,
            ctx,
            path,
            ".style",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.explode,
            other.explode,
            ctx,
            path,
            ".explode",
            ConflictKind::ScalarOverridden,
        );
        merge_schema_field(&mut self.schema, other.schema, ctx, path, "schema");
        merge_opt_scalar(
            &mut self.example,
            other.example,
            ctx,
            path,
            ".example",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_map(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            path,
            ".content",
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
        merge_opt_map(
            &mut self.headers,
            other.headers,
            ctx,
            path,
            ".headers",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.content,
            other.content,
            ctx,
            path,
            ".content",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_map(
            &mut self.links,
            other.links,
            ctx,
            path,
            ".links",
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

impl MergeWithContext<()> for RequestBody {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        // content is a bare BTreeMap (required).
        for (k, incoming) in other.content {
            if let Some(b) = self.content.get_mut(&k) {
                let original_len = path.len();
                path.push_str(".content.");
                path.push_str(&k);
                b.merge_with_context(incoming, ctx, path);
                path.truncate(original_len);
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
            path,
            ".required",
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

impl MergeWithContext<()> for Example {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
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
        merge_opt_scalar(
            &mut self.value,
            other.value,
            ctx,
            path,
            ".value",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.serialized_value,
            other.serialized_value,
            ctx,
            path,
            ".serializedValue",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.data_value,
            other.data_value,
            ctx,
            path,
            ".dataValue",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.external_value,
            other.external_value,
            ctx,
            path,
            ".externalValue",
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

impl MergeWithContext<()> for Link {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.operation_ref,
            other.operation_ref,
            ctx,
            path,
            ".operationRef",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.operation_id,
            other.operation_id,
            ctx,
            path,
            ".operationId",
            ConflictKind::ScalarOverridden,
        );
        // Link.parameters is `Option<BTreeMap<String, serde_json::Value>>`
        // — treat like extensions.
        merge_extensions(
            &mut self.parameters,
            other.parameters,
            ctx,
            path,
            ".parameters",
        );
        merge_opt_scalar(
            &mut self.request_body,
            other.request_body,
            ctx,
            path,
            ".requestBody",
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
        merge_opt_struct(&mut self.server, other.server, ctx, path, ".server");
        merge_extensions(
            &mut self.extensions,
            other.extensions,
            ctx,
            path,
            ".extensions",
        );
    }
}

// ----- Leaves: Tag, Info, Contact, License, ExternalDocumentation,
//                Discriminator, XML, Server, ServerVariable -----

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
            &mut self.parent,
            other.parent,
            ctx,
            path,
            ".parent",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.kind,
            other.kind,
            ctx,
            path,
            ".kind",
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
            &mut self.identifier,
            other.identifier,
            ctx,
            path,
            ".identifier",
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

impl MergeWithContext<()> for Discriminator {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.property_name,
            other.property_name,
            ctx,
            path,
            ".propertyName",
            ConflictKind::RequiredScalarOverridden,
        );
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
                        if *existing != v {
                            let original_len = path.len();
                            path.push_str(".mapping.");
                            path.push_str(&k);
                            let take =
                                ctx.should_take_incoming(path, ConflictKind::ScalarOverridden);
                            path.truncate(original_len);
                            if take {
                                *existing = v;
                            }
                        }
                    }
                }
                // ErrorOnConflict: bail on the first collision; matches
                // the OAuth scopes loop and the documented "first real
                // collision triggers early error" contract.
                if ctx.errored {
                    return;
                }
            }
        }
        merge_opt_scalar(
            &mut self.default_mapping,
            other.default_mapping,
            ctx,
            path,
            ".defaultMapping",
            ConflictKind::ScalarOverridden,
        );
    }
}

impl MergeWithContext<()> for XML {
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
            &mut self.namespace,
            other.namespace,
            ctx,
            path,
            ".namespace",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.prefix,
            other.prefix,
            ctx,
            path,
            ".prefix",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.attribute,
            other.attribute,
            ctx,
            path,
            ".attribute",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.wrapped,
            other.wrapped,
            ctx,
            path,
            ".wrapped",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.node_type,
            other.node_type,
            ctx,
            path,
            ".nodeType",
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
            &mut self.name,
            other.name,
            ctx,
            path,
            ".name",
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
            &mut self.variables,
            other.variables,
            ctx,
            path,
            ".variables",
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

// ----- SecurityScheme -----

impl MergeWithContext<()> for SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
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
                if ctx.should_take_incoming(path, ConflictKind::ParameterVariantMismatch) {
                    *slot = incoming;
                }
            }
        }
    }
}

impl MergeWithContext<()> for ApiKeySecurityScheme {
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
        merge_required_scalar(
            &mut self.location,
            other.location,
            ctx,
            path,
            ".in",
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
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
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

impl MergeWithContext<()> for HttpSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.scheme,
            other.scheme,
            ctx,
            path,
            ".scheme",
            ConflictKind::RequiredScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.bearer_format,
            other.bearer_format,
            ctx,
            path,
            ".bearerFormat",
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
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
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

impl MergeWithContext<()> for MutualTLSSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
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

impl MergeWithContext<()> for OpenIdConnectSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_required_scalar(
            &mut self.open_id_connect_url,
            other.open_id_connect_url,
            ctx,
            path,
            ".openIdConnectUrl",
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
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
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

impl MergeWithContext<()> for OAuth2SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        {
            let mut guard = PathGuard::new(path, ".flows");
            self.flows
                .merge_with_context(other.flows, ctx, guard.path_mut());
        }
        merge_opt_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.oauth2_metadata_url,
            other.oauth2_metadata_url,
            ctx,
            path,
            ".oauth2MetadataUrl",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
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

impl MergeWithContext<()> for OAuth2Flows {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        merge_opt_struct(&mut self.implicit, other.implicit, ctx, path, ".implicit");
        merge_opt_struct(&mut self.password, other.password, ctx, path, ".password");
        merge_opt_struct(
            &mut self.client_credentials,
            other.client_credentials,
            ctx,
            path,
            ".clientCredentials",
        );
        merge_opt_struct(
            &mut self.authorization_code,
            other.authorization_code,
            ctx,
            path,
            ".authorizationCode",
        );
        merge_opt_struct(
            &mut self.device_authorization,
            other.device_authorization,
            ctx,
            path,
            ".deviceAuthorization",
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
                path: &mut String,
            ) {
                if ctx.errored { return; }
                $(
                    merge_required_scalar(
                        &mut self.$url_field,
                        other.$url_field,
                        ctx,
                        path,
                        concat!(".", $url_path),
                        ConflictKind::RequiredScalarOverridden,
                    );
                )*
                merge_opt_scalar(
                    &mut self.refresh_url,
                    other.refresh_url,
                    ctx,
                    path,
                    ".refreshUrl",
                    ConflictKind::ScalarOverridden,
                );
                // scopes is a bare BTreeMap<String, String>.
                for (k, v) in other.scopes {
                    match self.scopes.get_mut(&k) {
                        None => {
                            self.scopes.insert(k, v);
                        }
                        Some(existing) => {
                            if *existing != v {
                                let original_len = path.len();
                                path.push_str(".scopes.");
                                path.push_str(&k);
                                let take = ctx.should_take_incoming(
                                    path,
                                    ConflictKind::ScalarOverridden,
                                );
                                path.truncate(original_len);
                                if take {
                                    *existing = v;
                                }
                                if ctx.errored {
                                    return;
                                }
                            }
                        }
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
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<()>, path: &mut String) {
        if ctx.errored {
            return;
        }
        // ObjectSchema deep-merge is the one variant pairing that
        // recurses — everything else falls back to leaf-replace.
        //
        // Take ownership of `other` up front so the match is on owned
        // values, avoiding the borrow-then-re-destructure dance that
        // previously needed three `unreachable!()` guards. If the
        // variants don't match, the bound-once `incoming` is reused
        // for the leaf-replace fallback below.
        if ctx.is_option(MergeOptions::DeepMergeObjectSchemas)
            && let Schema::Single(self_single) = self
            && let SingleSchema::Object(self_obj) = self_single.as_mut()
        {
            match other {
                Schema::Single(other_single) => match *other_single {
                    SingleSchema::Object(other_obj) => {
                        self_obj.merge_with_context(other_obj, ctx, path);
                        return;
                    }
                    other_single_inner => {
                        // Re-box and fall through to leaf-replace.
                        return leaf_replace_schema(
                            self,
                            Schema::Single(Box::new(other_single_inner)),
                            ctx,
                            path,
                        );
                    }
                },
                non_single => {
                    return leaf_replace_schema(self, non_single, ctx, path);
                }
            }
        }
        leaf_replace_schema(self, other, ctx, path);
    }
}

fn leaf_replace_schema(base: &mut Schema, other: Schema, ctx: &mut MergeContext<()>, path: &str) {
    if *base != other && ctx.should_take_incoming(path, ConflictKind::SchemaLeafReplaced) {
        *base = other;
    }
}

/// Record a collision where the documented contract says "base is
/// kept" — used for `Spec.info` / `Spec.openapi` when `MergeInfo` is
/// off. Records `Resolution::Base` in the default / `BaseWins` modes
/// (reflecting what actually happened), and trips
/// `ErrorOnConflict` when set. Pushes `segment` onto `path` only
/// when actually recording, keeping the eager-allocation
/// regression off the non-conflict path.
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
        merge_opt_map(
            &mut self.pattern_properties,
            other.pattern_properties,
            ctx,
            path,
            ".patternProperties",
            |b, i, c, p| b.merge_with_context(i, c, p),
            key_str,
        );
        merge_opt_scalar(
            &mut self.default,
            other.default,
            ctx,
            path,
            ".default",
            ConflictKind::ScalarOverridden,
        );
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
        match (
            &mut self.unevaluated_properties,
            other.unevaluated_properties,
        ) {
            (_, None) => {}
            (slot @ None, Some(v)) => *slot = Some(v),
            (Some(b), Some(i)) => {
                let mut guard = PathGuard::new(path, ".unevaluatedProperties");
                b.merge_with_context(i, ctx, guard.path_mut());
            }
        }
        merge_schema_field(
            &mut self.property_names,
            other.property_names,
            ctx,
            path,
            "propertyNames",
        );
        merge_opt_vec_set_union(&mut self.required, other.required, ctx, path, ".required");
        merge_opt_scalar(
            &mut self.read_only,
            other.read_only,
            ctx,
            path,
            ".readOnly",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.write_only,
            other.write_only,
            ctx,
            path,
            ".writeOnly",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_scalar(
            &mut self.deprecated,
            other.deprecated,
            ctx,
            path,
            ".deprecated",
            ConflictKind::ScalarOverridden,
        );
        merge_opt_struct(&mut self.xml, other.xml, ctx, path, ".xml");
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
        merge_opt_scalar(
            &mut self.examples,
            other.examples,
            ctx,
            path,
            ".examples",
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
                    path: &mut String,
                ) {
                    if ctx.errored { return; }
                    if *self != other
                        && ctx.should_take_incoming(path, ConflictKind::SchemaLeafReplaced)
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
        let mut path = String::from("#.tags[pets]");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#.tags[0]");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#.op");
        base.merge_with_context(incoming, &mut ctx, &mut path);

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
        let mut path = String::from("#.op");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#.paths[/pets]");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#.s");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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
        let mut path = String::from("#.s");
        base.merge_with_context(incoming, &mut ctx, &mut path);
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

    #[test]
    fn discriminator_mapping_loop_breaks_on_error_on_conflict() {
        use crate::v3_2::discriminator::Discriminator;
        // Two mapping collisions in a row; under ErrorOnConflict only
        // the first should land in the report — the loop must `return`
        // after the first collision, not keep recording. Before the
        // early-break fix this test would see two recorded conflicts.
        let mut base = Discriminator {
            property_name: "kind".into(),
            mapping: Some(BTreeMap::from([
                ("a".to_owned(), "#/c/A".to_owned()),
                ("b".to_owned(), "#/c/B".to_owned()),
            ])),
            ..Default::default()
        };
        let incoming = Discriminator {
            property_name: "kind".into(),
            mapping: Some(BTreeMap::from([
                ("a".to_owned(), "#/c/AA".to_owned()),
                ("b".to_owned(), "#/c/BB".to_owned()),
            ])),
            ..Default::default()
        };
        let mut ctx: MergeContext<()> =
            MergeContext::new(&(), MergeOptions::ErrorOnConflict.only());
        let mut path = String::from("#.d");
        base.merge_with_context(incoming, &mut ctx, &mut path);
        assert!(ctx.errored, "first mapping collision must trip errored");
        assert_eq!(
            ctx.conflicts.len(),
            1,
            "loop must return after first collision, got {} conflicts",
            ctx.conflicts.len()
        );
    }

    #[test]
    fn spec_error_on_conflict_rolls_back_base_on_err() {
        // Base has tags=[A,B], a 200 response on /pets.get, and a
        // jsonSchemaDialect. Incoming adds a tag C (additive — no
        // conflict by itself), a 404 response (additive), and a
        // different jsonSchemaDialect that *will* collide. With
        // `ErrorOnConflict`, the additive bits that ran *before*
        // hitting the dialect conflict must not be observable on
        // `base` after the `Err` returns — the rollback contract.
        let mut base = Spec {
            tags: Some(vec![
                Tag {
                    name: "A".into(),
                    ..Default::default()
                },
                Tag {
                    name: "B".into(),
                    ..Default::default()
                },
            ]),
            paths: Some(Paths {
                paths: BTreeMap::from([(
                    "/pets".to_owned(),
                    PathItem {
                        operations: Some(BTreeMap::from([(
                            "get".to_owned(),
                            Operation {
                                responses: Some(Responses {
                                    responses: Some(BTreeMap::from([(
                                        "200".to_owned(),
                                        response_with("ok"),
                                    )])),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        )])),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            json_schema_dialect: Some("base-dialect".into()),
            ..Default::default()
        };
        let incoming = Spec {
            tags: Some(vec![Tag {
                name: "C".into(),
                ..Default::default()
            }]),
            paths: Some(Paths {
                paths: BTreeMap::from([(
                    "/pets".to_owned(),
                    PathItem {
                        operations: Some(BTreeMap::from([(
                            "get".to_owned(),
                            Operation {
                                responses: Some(Responses {
                                    responses: Some(BTreeMap::from([(
                                        "404".to_owned(),
                                        response_with("missing"),
                                    )])),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        )])),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            json_schema_dialect: Some("incoming-dialect".into()),
            ..Default::default()
        };
        let result = base.merge(incoming, MergeOptions::ErrorOnConflict.only());
        assert!(result.is_err());
        // Base is untouched: still 2 tags, still only the 200 response,
        // still the original dialect.
        let tag_names: Vec<_> = base
            .tags
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(tag_names, vec!["A", "B"]);
        let responses = base.paths.as_ref().unwrap().paths["/pets"]
            .operations
            .as_ref()
            .unwrap()["get"]
            .responses
            .as_ref()
            .unwrap()
            .responses
            .as_ref()
            .unwrap();
        assert!(responses.contains_key("200"));
        assert!(!responses.contains_key("404"), "404 must not have leaked");
        assert_eq!(base.json_schema_dialect.as_deref(), Some("base-dialect"));
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

    // ---- Coverage: ErrorOnConflict catches info mismatch even when base is kept ----

    #[test]
    fn spec_info_mismatch_trips_error_on_conflict_even_without_merge_info() {
        use crate::v3_2::info::Info;
        let mut base = Spec {
            info: Info {
                title: "Base".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let incoming = Spec {
            info: Info {
                title: "Incoming".into(),
                version: "2.0.0".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Without MergeInfo, default mode kept base silently before
        // the fix; with the fix, ErrorOnConflict trips on the info
        // mismatch.
        let err = base
            .merge(incoming, MergeOptions::ErrorOnConflict.only())
            .expect_err("info mismatch should error under ErrorOnConflict");
        assert!(err.conflicts.iter().any(|c| c.path.ends_with(".info")));
    }

    // ---- Coverage: itemSchema conflict path is correct ----

    #[test]
    fn media_type_item_schema_conflict_path_is_item_schema() {
        use crate::v3_2::media_type::MediaType;
        let s1 = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            required: Some(vec!["a".into()]),
            ..Default::default()
        })));
        let s2 = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            required: Some(vec!["b".into()]),
            ..Default::default()
        })));
        let mut base = MediaType {
            item_schema: Some(RefOr::new_item(s1)),
            ..Default::default()
        };
        let incoming = MediaType {
            item_schema: Some(RefOr::new_item(s2)),
            ..Default::default()
        };
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = String::from("#.mt");
        base.merge_with_context(incoming, &mut ctx, &mut path);
        // Conflict path should mention itemSchema, not schema.
        assert!(
            ctx.conflicts.iter().any(|c| c.path.contains("itemSchema")),
            "conflict paths: {:?}",
            ctx.conflicts.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
        assert!(
            !ctx.conflicts
                .iter()
                .any(|c| c.path.ends_with(".schema") && !c.path.contains("itemSchema")),
            "unexpected `.schema` path: {:?}",
            ctx.conflicts.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
    }

    // ---- Coverage: ServerVariable carries default/enum/description through merge ----

    #[test]
    fn server_variable_full_field_merge_through_server_variables_map() {
        use crate::v3_2::server::{Server, ServerVariable};
        // The variable fields are private, so build via serde round-trip.
        let base_var: ServerVariable = serde_json::from_value(serde_json::json!({
            "default": "v1",
            "description": "base desc"
        }))
        .unwrap();
        let incoming_var: ServerVariable = serde_json::from_value(serde_json::json!({
            "default": "v1",
            "enum": ["v1", "v2"],
            "description": "incoming desc"
        }))
        .unwrap();
        let mut base_server = Server {
            url: "https://api.example".into(),
            variables: Some(BTreeMap::from([("ver".to_owned(), base_var)])),
            ..Default::default()
        };
        let incoming_server = Server {
            url: "https://api.example".into(),
            variables: Some(BTreeMap::from([("ver".to_owned(), incoming_var)])),
            ..Default::default()
        };
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = String::from("#.srv");
        base_server.merge_with_context(incoming_server, &mut ctx, &mut path);
        let merged = base_server.variables.unwrap();
        let v = merged.get("ver").unwrap();
        // Round-trip through serde to inspect the private fields.
        let json = serde_json::to_value(v).unwrap();
        assert_eq!(json["description"], "incoming desc");
        assert_eq!(json["enum"], serde_json::json!(["v1", "v2"]));
        assert_eq!(json["default"], "v1");
    }

    // ---- Coverage: lazy paths balance correctly through deep recursion ----

    #[test]
    fn lazy_path_balances_after_deep_merge_of_identical_trees() {
        // Identical Spec subtrees: no conflicts should be recorded
        // and the `&mut String` path stack must end balanced. A
        // missed `path.truncate()` would either panic in truncate or
        // leak segments — the assertion that the empty conflict list
        // came back tells us neither happened.
        let mut base = Spec {
            components: Some(Components {
                schemas: Some(BTreeMap::from([(
                    "Pet".to_owned(),
                    RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                        ObjectSchema {
                            properties: Some(BTreeMap::from([(
                                "name".to_owned(),
                                RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                                    ObjectSchema {
                                        description: Some("base".into()),
                                        ..Default::default()
                                    },
                                )))),
                            )])),
                            ..Default::default()
                        },
                    )))),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let incoming = base.clone();
        let report = base
            .merge(
                incoming,
                MergeOptions::DeepMergeObjectSchemas | MergeOptions::MergeInfo,
            )
            .unwrap();
        assert!(
            report.conflicts.is_empty(),
            "identical trees should produce no conflicts: {:?}",
            report.conflicts
        );
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
