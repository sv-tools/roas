//! v3.1 merge: per-component `MergeWithContext<Spec>` impls plus the
//! public `impl Merge for Spec`.
//!
//! Sits alongside `v3_1/validation.rs`. Every component type in v3.1
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

use crate::v3_1::callback::Callback;
use crate::v3_1::components::Components;
use crate::v3_1::discriminator::Discriminator;
use crate::v3_1::example::Example;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::header::Header;
use crate::v3_1::info::{Contact, Info, License, Logo};
use crate::v3_1::link::Link;
use crate::v3_1::media_type::{Encoding, MediaType};
use crate::v3_1::operation::{CodeSample, Operation};
use crate::v3_1::parameter::{InCookie, InHeader, InPath, InQuery, Parameter};
use crate::v3_1::path_item::{PathItem, Paths};
use crate::v3_1::request_body::RequestBody;
use crate::v3_1::response::{Response, Responses};
use crate::v3_1::schema::{ObjectSchema, Schema, SingleSchema};
use crate::v3_1::security_scheme::{
    ApiKeySecurityScheme, AuthorizationCodeOAuth2Flow, ClientCredentialsOAuth2Flow,
    HttpSecurityScheme, ImplicitOAuth2Flow, MutualTLSSecurityScheme, OAuth2Flows,
    OAuth2SecurityScheme, OpenIdConnectSecurityScheme, PasswordOAuth2Flow, SecurityScheme,
};
use crate::v3_1::server::Server;
use crate::v3_1::spec::{Spec, TagGroup};
use crate::v3_1::tag::Tag;
use crate::v3_1::xml::XML;

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
        // tags is a Vec<String> — set-union semantics.
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
        merge_opt_vec_set_union(&mut self.x_tags, other.x_tags, ctx, path, ".x-tags");
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
            &mut self.encoding,
            other.encoding,
            ctx,
            path,
            ".encoding",
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
        // v3.1 `description` is a required String (vs v3.2's Option<String>).
        merge_required_scalar(
            &mut self.description,
            other.description,
            ctx,
            path,
            ".description",
            ConflictKind::RequiredScalarOverridden,
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
        // v3.1 Server has no `name` (added in v3.2).
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

// Schema sub-types (StringSchema, IntegerSchema, …, SingleSchema)
// don't need their own `MergeWithContext<()>` impls — `Schema`'s impl
// handles the entire enum at the top level via `leaf_replace_schema`
// for any pairing other than `Single(Object(_))` × `Single(Object(_))`.
// Nothing in the codebase holds a `RefOr<StringSchema>` or
// `&mut SingleSchema` directly, so the per-variant impls would be
// dead code.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::{ConflictKind, MergeOptions, Resolution};
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
            description: description.into(),
            ..Default::default()
        })
    }

    // ---- Spec roll-up ----

    #[test]
    fn spec_default_incoming_wins() {
        let mut base = Spec {
            json_schema_dialect: Some("base".into()),
            ..Default::default()
        };
        let report = base
            .merge(
                Spec {
                    json_schema_dialect: Some("incoming".into()),
                    ..Default::default()
                },
                MergeOptions::new(),
            )
            .unwrap();
        assert_eq!(base.json_schema_dialect.as_deref(), Some("incoming"));
        assert!(!report.conflicts.is_empty());
    }

    #[test]
    fn spec_error_on_conflict_returns_err_and_rolls_back() {
        let mut base = Spec {
            json_schema_dialect: Some("base".into()),
            ..Default::default()
        };
        let result = base.merge(
            Spec {
                json_schema_dialect: Some("incoming".into()),
                ..Default::default()
            },
            MergeOptions::ErrorOnConflict.only(),
        );
        assert!(result.is_err());
        assert_eq!(base.json_schema_dialect.as_deref(), Some("base"));
    }

    #[test]
    fn spec_base_wins_records_resolution_base() {
        let mut base = Spec {
            json_schema_dialect: Some("base".into()),
            ..Default::default()
        };
        let report = base
            .merge(
                Spec {
                    json_schema_dialect: Some("incoming".into()),
                    ..Default::default()
                },
                MergeOptions::BaseWins.only(),
            )
            .unwrap();
        assert_eq!(base.json_schema_dialect.as_deref(), Some("base"));
        assert_eq!(report.conflicts[0].resolution, Resolution::Base);
    }

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
        base.merge(
            Spec {
                paths: Some(Paths {
                    paths: BTreeMap::from([("/pets".to_owned(), mk_pi("post"))]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MergeOptions::new(),
        )
        .unwrap();
        let pi = &base.paths.unwrap().paths["/pets"];
        let ops = pi.operations.as_ref().unwrap();
        assert!(ops.contains_key("get"));
        assert!(ops.contains_key("post"));
    }

    #[test]
    fn spec_info_kept_on_base_by_default() {
        let mut base = Spec {
            info: Info {
                title: "Base API".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let report = base
            .merge(
                Spec {
                    info: Info {
                        title: "Incoming".into(),
                        version: "2.0.0".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                MergeOptions::new(),
            )
            .unwrap();
        assert_eq!(base.info.title, "Base API");
        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.path.ends_with(".info") && c.resolution == Resolution::Base)
        );
    }

    #[test]
    fn spec_info_merged_under_merge_info_option() {
        let mut base = Spec {
            info: Info {
                title: "Base".into(),
                version: "1.0.0".into(),
                description: Some("base desc".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        base.merge(
            Spec {
                info: Info {
                    title: "Base".into(),
                    version: "1.0.0".into(),
                    summary: Some("from incoming".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            MergeOptions::MergeInfo.only(),
        )
        .unwrap();
        assert_eq!(base.info.description.as_deref(), Some("base desc"));
        assert_eq!(base.info.summary.as_deref(), Some("from incoming"));
    }

    // ---- v3.1-specific: x_tag_groups ----

    #[test]
    fn spec_x_tag_groups_dedup_by_name() {
        let mut base = Spec {
            x_tag_groups: Some(vec![TagGroup {
                name: "auth".into(),
                tags: vec!["login".into(), "logout".into()],
                ..Default::default()
            }]),
            ..Default::default()
        };
        base.merge(
            Spec {
                x_tag_groups: Some(vec![
                    TagGroup {
                        name: "auth".into(),
                        tags: vec!["refresh".into()],
                        ..Default::default()
                    },
                    TagGroup {
                        name: "users".into(),
                        tags: vec!["create".into()],
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            MergeOptions::new(),
        )
        .unwrap();
        let groups = base.x_tag_groups.unwrap();
        assert_eq!(groups.len(), 2);
        let auth = groups.iter().find(|g| g.name == "auth").unwrap();
        // tags union: login, logout, refresh
        assert_eq!(auth.tags.len(), 3);
    }

    // ---- v3.1-specific: x_logo on Info ----

    #[test]
    fn info_x_logo_merges_when_both_some() {
        let mut base = Info {
            title: "T".into(),
            version: "1".into(),
            x_logo: Some(Logo {
                url: "https://a.png".into(),
                alt_text: Some("base alt".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Info {
                title: "T".into(),
                version: "1".into(),
                x_logo: Some(Logo {
                    url: "https://b.png".into(),
                    href: Some("https://b".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let logo = base.x_logo.unwrap();
        assert_eq!(logo.url, "https://b.png");
        assert_eq!(logo.alt_text.as_deref(), Some("base alt"));
        assert_eq!(logo.href.as_deref(), Some("https://b"));
    }

    // ---- v3.1-specific: x_code_samples on Operation ----

    #[test]
    fn operation_x_code_samples_dedup_by_lang_label() {
        let mut base = Operation {
            x_code_samples: Some(vec![CodeSample {
                lang: "curl".into(),
                label: Some("basic".into()),
                source: "curl https://a".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                x_code_samples: Some(vec![
                    // Same (lang, label) — dedup recurses + replaces source.
                    CodeSample {
                        lang: "curl".into(),
                        label: Some("basic".into()),
                        source: "curl https://b".into(),
                        ..Default::default()
                    },
                    // Different label — appended.
                    CodeSample {
                        lang: "curl".into(),
                        label: Some("auth".into()),
                        source: "curl -H ...".into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let samples = base.x_code_samples.unwrap();
        assert_eq!(samples.len(), 2);
        let basic = samples
            .iter()
            .find(|s| s.label.as_deref() == Some("basic"))
            .unwrap();
        assert_eq!(basic.source, "curl https://b");
    }

    #[test]
    fn operation_x_tags_set_union() {
        let mut base = Operation {
            x_tags: Some(vec!["a".into(), "b".into()]),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                x_tags: Some(vec!["b".into(), "c".into()]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let tags = base.x_tags.unwrap();
        assert_eq!(tags, vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
    }

    // ---- v3.1-specific: Tag.x_display_name ----

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

    // ---- RefOr ----

    #[test]
    fn refor_item_item_recurses() {
        let mut base: RefOr<Tag> = RefOr::new_item(Tag {
            name: "pets".into(),
            description: Some("base".into()),
            ..Default::default()
        });
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        base.merge_with_context(
            RefOr::new_item(Tag {
                name: "pets".into(),
                description: Some("inc".into()),
                ..Default::default()
            }),
            &mut ctx,
            &mut path,
        );
        match base {
            RefOr::Item(t) => assert_eq!(t.description.as_deref(), Some("inc")),
            _ => panic!(),
        }
    }

    // ---- Operation: responses, parameters dedup, method coexistence ----

    #[test]
    fn operation_responses_keeps_base_status_codes() {
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
        let _ = report(
            &mut base,
            Operation {
                responses: Some(Responses {
                    responses: Some(BTreeMap::from([
                        ("200".into(), response_with("ok updated")),
                        ("500".into(), response_with("err")),
                    ])),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let r = base.responses.unwrap().responses.unwrap();
        assert!(r.contains_key("200"));
        assert!(r.contains_key("404"));
        assert!(r.contains_key("500"));
    }

    #[test]
    fn operation_parameters_dedup_by_name_in() {
        let mut base = Operation {
            parameters: Some(vec![param_path("id"), param_query("limit")]),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                parameters: Some(vec![param_path("id"), param_query("filter")]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.parameters.unwrap().len(), 3);
    }

    #[test]
    fn path_item_get_and_post_coexist() {
        let mut base = PathItem {
            operations: Some(BTreeMap::from([("get".to_owned(), Operation::default())])),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            PathItem {
                operations: Some(BTreeMap::from([("post".to_owned(), Operation::default())])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let ops = base.operations.unwrap();
        assert!(ops.contains_key("get") && ops.contains_key("post"));
    }

    // ---- Schema deep merge ----

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
        let _ = report(
            &mut base,
            Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
                properties: Some(BTreeMap::from([(
                    "b".to_owned(),
                    RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                        ObjectSchema::default(),
                    )))),
                )])),
                required: Some(vec!["b".into()]),
                ..Default::default()
            }))),
            MergeOptions::DeepMergeObjectSchemas.only(),
        );
        let Schema::Single(box_s) = &base else {
            panic!()
        };
        let SingleSchema::Object(obj) = &**box_s else {
            panic!()
        };
        let props = obj.properties.as_ref().unwrap();
        assert!(props.contains_key("a") && props.contains_key("b"));
        let req = obj.required.as_ref().unwrap();
        assert!(req.contains(&"a".to_owned()) && req.contains(&"b".to_owned()));
    }

    // ---- Parameter variant mismatch ----

    #[test]
    fn parameter_variant_mismatch_replaces() {
        let mut base = Parameter::Path(Box::new(InPath {
            name: "id".into(),
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
        }));
        let conflicts = report(
            &mut base,
            Parameter::Query(Box::new(InQuery {
                name: "id".into(),
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
            })),
            MergeOptions::new(),
        );
        assert!(matches!(base, Parameter::Query(_)));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::ParameterVariantMismatch);
    }
}
