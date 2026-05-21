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
use crate::v3_2::schema::{ObjectSchema, Schema, SingleSchema};
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
        let mut ctx: MergeContext = MergeContext::new(options);
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
            <Spec as MergeWithContext>::merge_with_context(
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
        <Spec as MergeWithContext>::merge_with_context(self, other, &mut ctx, &mut path);
        ctx.into()
    }
}

// ----- Top-level Spec -----

impl MergeWithContext for Spec {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Components {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Paths {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Callback {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for PathItem {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Responses {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Operation {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Parameter {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for InPath {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for InQuery {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for InHeader {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for InCookie {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for InQuerystring {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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
    ctx: &mut MergeContext,
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

impl MergeWithContext for MediaType {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Encoding {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Header {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Response {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for RequestBody {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Example {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Link {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Tag {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Info {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Contact {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for License {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for ExternalDocumentation {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Discriminator {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for XML {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for Server {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for ApiKeySecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for HttpSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for MutualTLSSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for OpenIdConnectSecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for OAuth2SecurityScheme {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

impl MergeWithContext for OAuth2Flows {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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
        impl MergeWithContext for $t {
            fn merge_with_context(
                &mut self,
                other: Self,
                ctx: &mut MergeContext,
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

impl MergeWithContext for Schema {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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

fn leaf_replace_schema(base: &mut Schema, other: Schema, ctx: &mut MergeContext, path: &str) {
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
    ctx: &mut MergeContext,
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

impl MergeWithContext for ObjectSchema {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext, path: &mut String) {
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
// don't need their own `MergeWithContext` impls — `Schema`'s impl
// handles the entire enum at the top level via `leaf_replace_schema`
// for any pairing other than `Single(Object(_))` × `Single(Object(_))`.
// Nothing in the codebase holds a `RefOr<StringSchema>` or
// `&mut SingleSchema` directly, so the per-variant impls would be
// dead code.

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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::DeepMergeObjectSchemas.only());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::ErrorOnConflict.only());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
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

    // ============================================================
    // Coverage tests — one collision per component impl. Each builds
    // two instances differing on a single field, merges, and asserts
    // either a conflict was recorded or a merge happened. Keeps each
    // case small; the structural correctness is already covered by
    // the targeted tests above.
    // ============================================================

    fn root_path() -> String {
        "#".to_owned()
    }

    fn run<S: MergeWithContext>(mut base: S, incoming: S, opts: EnumSet<MergeOptions>) -> S {
        let mut ctx: MergeContext = MergeContext::new(opts);
        let mut path = root_path();
        base.merge_with_context(incoming, &mut ctx, &mut path);
        base
    }

    fn report<S: MergeWithContext>(
        base: &mut S,
        incoming: S,
        opts: EnumSet<MergeOptions>,
    ) -> Vec<crate::merge::MergeConflict> {
        let mut ctx: MergeContext = MergeContext::new(opts);
        let mut path = root_path();
        base.merge_with_context(incoming, &mut ctx, &mut path);
        ctx.conflicts
    }

    fn mk_encoding(content_type: Option<&str>) -> crate::v3_2::media_type::Encoding {
        crate::v3_2::media_type::Encoding {
            content_type: content_type.map(str::to_owned),
            headers: None,
            style: None,
            explode: None,
            allow_reserved: None,
            encoding: None,
            prefix_encoding: None,
            item_encoding: None,
            extensions: None,
        }
    }

    // ---- ExternalDocumentation ----

    #[test]
    fn external_documentation_field_merges() {
        let mut base = ExternalDocumentation {
            url: "https://a".into(),
            description: Some("old".into()),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            ExternalDocumentation {
                url: "https://b".into(),
                description: Some("new".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.url, "https://b");
        assert_eq!(base.description.as_deref(), Some("new"));
        assert_eq!(conflicts.len(), 2);
    }

    // ---- Info / Contact / License ----

    #[test]
    fn contact_field_merges() {
        let mut base = Contact {
            name: Some("Alice".into()),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Contact {
                name: Some("Bob".into()),
                url: Some("https://b".into()),
                email: Some("b@x".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.name.as_deref(), Some("Bob"));
        assert_eq!(base.url.as_deref(), Some("https://b"));
        assert_eq!(base.email.as_deref(), Some("b@x"));
        assert_eq!(conflicts.len(), 1); // only `name` collided
    }

    #[test]
    fn license_required_name_with_optional_fields() {
        let mut base = License {
            name: "MIT".into(),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            License {
                name: "Apache-2.0".into(),
                identifier: Some("Apache-2.0".into()),
                url: Some("https://apache.org".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.name, "Apache-2.0");
        assert_eq!(base.identifier.as_deref(), Some("Apache-2.0"));
        assert_eq!(base.url.as_deref(), Some("https://apache.org"));
        assert_eq!(conflicts.len(), 1); // name required-scalar collision
        assert_eq!(conflicts[0].kind, ConflictKind::RequiredScalarOverridden);
    }

    #[test]
    fn info_terms_of_service_and_extensions() {
        use crate::v3_2::info::Info;
        let mut base = Info {
            title: "Base".into(),
            version: "1.0.0".into(),
            terms_of_service: Some("base tos".into()),
            extensions: Some(BTreeMap::from([("x-a".into(), serde_json::json!(1))])),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Info {
                title: "Base".into(),
                version: "1.0.0".into(),
                terms_of_service: Some("incoming tos".into()),
                extensions: Some(BTreeMap::from([("x-b".into(), serde_json::json!(2))])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.terms_of_service.as_deref(), Some("incoming tos"));
        let ext = base.extensions.unwrap();
        assert!(ext.contains_key("x-a"));
        assert!(ext.contains_key("x-b"));
        assert!(!conflicts.is_empty());
    }

    // ---- Discriminator ----

    #[test]
    fn discriminator_default_mapping_merges() {
        use crate::v3_2::discriminator::Discriminator;
        let mut base = Discriminator {
            property_name: "kind".into(),
            default_mapping: Some("a".into()),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Discriminator {
                property_name: "kind".into(),
                mapping: Some(BTreeMap::from([("a".into(), "#/c/A".into())])),
                default_mapping: Some("b".into()),
            },
            MergeOptions::new(),
        );
        assert_eq!(base.default_mapping.as_deref(), Some("b"));
        assert!(base.mapping.is_some());
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn discriminator_mapping_initially_none_takes_incoming() {
        use crate::v3_2::discriminator::Discriminator;
        let mut base = Discriminator {
            property_name: "kind".into(),
            mapping: None,
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Discriminator {
                property_name: "kind".into(),
                mapping: Some(BTreeMap::from([("a".into(), "#/c/A".into())])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert!(base.mapping.is_some());
        assert!(conflicts.is_empty());
    }

    // ---- XML ----

    #[test]
    fn xml_full_field_merge() {
        use crate::v3_2::xml::XML;
        let mut base = XML {
            name: Some("xa".into()),
            namespace: Some("ns".into()),
            ..Default::default()
        };
        let merged = run(
            base.clone(),
            XML {
                name: Some("xb".into()),
                prefix: Some("p".into()),
                attribute: Some(true),
                wrapped: Some(false),
                node_type: Some("element".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(merged.name.as_deref(), Some("xb"));
        assert_eq!(merged.namespace.as_deref(), Some("ns"));
        assert_eq!(merged.prefix.as_deref(), Some("p"));
        assert_eq!(merged.attribute, Some(true));
        let _ = &mut base;
    }

    // ---- Server / ServerVariable ----

    #[test]
    fn server_full_field_merge() {
        use crate::v3_2::server::Server;
        let mut base = Server {
            url: "https://api1".into(),
            name: Some("primary".into()),
            description: Some("old".into()),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Server {
                url: "https://api2".into(),
                name: Some("secondary".into()),
                description: Some("new".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.url, "https://api2");
        assert_eq!(base.name.as_deref(), Some("secondary"));
        assert_eq!(base.description.as_deref(), Some("new"));
        // url required-scalar + name + description optional-scalar
        assert!(conflicts.len() >= 3);
    }

    // ---- Tag with all optional fields ----

    #[test]
    fn tag_full_field_merge() {
        let mut base = Tag {
            name: "pets".into(),
            summary: Some("base sum".into()),
            description: Some("base desc".into()),
            external_docs: Some(ExternalDocumentation {
                url: "https://docs.a".into(),
                ..Default::default()
            }),
            parent: Some("animals".into()),
            kind: Some("entity".into()),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Tag {
                name: "pets".into(),
                summary: Some("inc sum".into()),
                description: Some("inc desc".into()),
                external_docs: Some(ExternalDocumentation {
                    url: "https://docs.b".into(),
                    ..Default::default()
                }),
                parent: Some("creatures".into()),
                kind: Some("group".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.summary.as_deref(), Some("inc sum"));
        assert_eq!(base.parent.as_deref(), Some("creatures"));
        assert_eq!(base.kind.as_deref(), Some("group"));
        assert!(!conflicts.is_empty());
    }

    // ---- MediaType + Encoding ----

    #[test]
    fn media_type_examples_and_encoding_merge() {
        use crate::v3_2::media_type::MediaType;
        let mut base = MediaType {
            description: Some("base".into()),
            example: Some(serde_json::json!({"a": 1})),
            encoding: Some(BTreeMap::from([(
                "field".to_owned(),
                mk_encoding(Some("application/json")),
            )])),
            prefix_encoding: Some(vec![mk_encoding(Some("text/csv"))]),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            MediaType {
                description: Some("incoming".into()),
                example: Some(serde_json::json!({"a": 2})),
                encoding: Some(BTreeMap::from([(
                    "field".to_owned(),
                    mk_encoding(Some("application/xml")),
                )])),
                prefix_encoding: Some(vec![mk_encoding(Some("text/plain"))]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.description.as_deref(), Some("incoming"));
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn media_type_item_encoding_merges_when_both_some() {
        use crate::v3_2::media_type::MediaType;
        let mut base = MediaType {
            item_encoding: Some(mk_encoding(Some("a"))),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            MediaType {
                item_encoding: Some(mk_encoding(Some("b"))),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(
            base.item_encoding.unwrap().content_type.as_deref(),
            Some("b")
        );
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn encoding_full_field_merge() {
        let mut base = mk_encoding(Some("a/b"));
        base.explode = Some(false);
        base.allow_reserved = Some(false);
        let mut incoming = mk_encoding(Some("c/d"));
        incoming.explode = Some(true);
        incoming.allow_reserved = Some(true);
        let conflicts = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(base.content_type.as_deref(), Some("c/d"));
        assert_eq!(base.explode, Some(true));
        assert_eq!(base.allow_reserved, Some(true));
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn encoding_item_encoding_merges_when_both_some() {
        let mut base = mk_encoding(None);
        base.item_encoding = Some(Box::new(mk_encoding(Some("a"))));
        let mut incoming = mk_encoding(None);
        incoming.item_encoding = Some(Box::new(mk_encoding(Some("b"))));
        let _ = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(
            base.item_encoding.unwrap().content_type.as_deref(),
            Some("b")
        );
    }

    // ---- Header ----

    #[test]
    fn header_full_field_merge() {
        use crate::v3_2::header::Header;
        let mut base = Header {
            description: Some("base".into()),
            required: Some(false),
            deprecated: Some(false),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Header {
                description: Some("incoming".into()),
                required: Some(true),
                deprecated: Some(true),
                explode: Some(true),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.description.as_deref(), Some("incoming"));
        assert_eq!(base.required, Some(true));
        assert!(!conflicts.is_empty());
    }

    // ---- Response ----

    #[test]
    fn response_full_field_merge() {
        use crate::v3_2::response::Response;
        let mut base = Response {
            summary: Some("base".into()),
            description: Some("d-base".into()),
            headers: Some(BTreeMap::from([(
                "X-Base".into(),
                RefOr::new_item(crate::v3_2::header::Header::default()),
            )])),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Response {
                summary: Some("incoming".into()),
                description: Some("d-incoming".into()),
                headers: Some(BTreeMap::from([(
                    "X-Inc".into(),
                    RefOr::new_item(crate::v3_2::header::Header::default()),
                )])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.summary.as_deref(), Some("incoming"));
        let h = base.headers.unwrap();
        assert!(h.contains_key("X-Base"));
        assert!(h.contains_key("X-Inc"));
        assert!(!conflicts.is_empty());
    }

    // ---- RequestBody ----

    #[test]
    fn request_body_content_and_required_merges() {
        use crate::v3_2::media_type::MediaType;
        use crate::v3_2::request_body::RequestBody;
        let mut base = RequestBody {
            description: Some("base".into()),
            content: BTreeMap::from([(
                "application/json".to_owned(),
                RefOr::new_item(MediaType {
                    description: Some("media-base".into()),
                    ..Default::default()
                }),
            )]),
            required: Some(false),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            RequestBody {
                description: Some("incoming".into()),
                content: BTreeMap::from([
                    (
                        "application/json".to_owned(),
                        RefOr::new_item(MediaType {
                            description: Some("media-inc".into()),
                            ..Default::default()
                        }),
                    ),
                    (
                        "application/xml".to_owned(),
                        RefOr::new_item(MediaType::default()),
                    ),
                ]),
                required: Some(true),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.description.as_deref(), Some("incoming"));
        assert_eq!(base.required, Some(true));
        assert!(base.content.contains_key("application/json"));
        assert!(base.content.contains_key("application/xml"));
        assert!(!conflicts.is_empty());
    }

    // ---- Example ----

    #[test]
    fn example_full_field_merge() {
        use crate::v3_2::example::Example;
        let mut base = Example {
            summary: Some("a".into()),
            description: Some("d-a".into()),
            value: Some(serde_json::json!(1)),
            serialized_value: Some("a-ser".into()),
            data_value: Some(serde_json::json!("d-a")),
            external_value: Some("https://a".into()),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Example {
                summary: Some("b".into()),
                description: Some("d-b".into()),
                value: Some(serde_json::json!(2)),
                serialized_value: Some("b-ser".into()),
                data_value: Some(serde_json::json!("d-b")),
                external_value: Some("https://b".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.summary.as_deref(), Some("b"));
        assert_eq!(base.external_value.as_deref(), Some("https://b"));
        assert!(!conflicts.is_empty());
    }

    // ---- Link ----

    #[test]
    fn link_full_field_merge() {
        use crate::v3_2::link::Link;
        let mut base = Link {
            operation_ref: Some("#/a".into()),
            description: Some("base".into()),
            request_body: Some(serde_json::json!({"r": 1})),
            parameters: Some(BTreeMap::from([("p1".into(), serde_json::json!("v1"))])),
            server: Some(Server {
                url: "https://srv1".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            Link {
                operation_id: Some("op2".into()),
                description: Some("incoming".into()),
                request_body: Some(serde_json::json!({"r": 2})),
                parameters: Some(BTreeMap::from([("p2".into(), serde_json::json!("v2"))])),
                server: Some(Server {
                    url: "https://srv1".into(),
                    description: Some("srv-desc".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.description.as_deref(), Some("incoming"));
        assert_eq!(base.operation_id.as_deref(), Some("op2"));
        let p = base.parameters.unwrap();
        assert!(p.contains_key("p1"));
        assert!(p.contains_key("p2"));
        assert!(!conflicts.is_empty());
    }

    // ---- SecurityScheme variants ----

    #[test]
    fn security_scheme_api_key_merges() {
        use crate::v3_2::security_scheme::{ApiKeyLocation, ApiKeySecurityScheme, SecurityScheme};
        let mut base = SecurityScheme::ApiKey(Box::new(ApiKeySecurityScheme {
            name: "api-key".into(),
            location: ApiKeyLocation::Header,
            description: Some("base".into()),
            ..Default::default()
        }));
        let _ = report(
            &mut base,
            SecurityScheme::ApiKey(Box::new(ApiKeySecurityScheme {
                name: "api-key".into(),
                location: ApiKeyLocation::Query,
                description: Some("incoming".into()),
                deprecated: Some(true),
                ..Default::default()
            })),
            MergeOptions::new(),
        );
        if let SecurityScheme::ApiKey(a) = base {
            assert_eq!(a.description.as_deref(), Some("incoming"));
            assert!(matches!(a.location, ApiKeyLocation::Query));
        } else {
            panic!("expected ApiKey");
        }
    }

    #[test]
    fn security_scheme_http_merges() {
        use crate::v3_2::security_scheme::{HttpSecurityScheme, SecurityScheme};
        let mut base = SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
            scheme: "basic".into(),
            description: Some("base".into()),
            ..Default::default()
        }));
        let _ = report(
            &mut base,
            SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
                scheme: "bearer".into(),
                bearer_format: Some("JWT".into()),
                description: Some("incoming".into()),
                deprecated: Some(true),
                ..Default::default()
            })),
            MergeOptions::new(),
        );
        if let SecurityScheme::HTTP(h) = base {
            assert_eq!(h.scheme, "bearer");
            assert_eq!(h.bearer_format.as_deref(), Some("JWT"));
        } else {
            panic!("expected HTTP");
        }
    }

    #[test]
    fn security_scheme_mutual_tls_merges() {
        use crate::v3_2::security_scheme::{MutualTLSSecurityScheme, SecurityScheme};
        let mut base = SecurityScheme::MutualTLS(Box::new(MutualTLSSecurityScheme {
            description: Some("base".into()),
            ..Default::default()
        }));
        let conflicts = report(
            &mut base,
            SecurityScheme::MutualTLS(Box::new(MutualTLSSecurityScheme {
                description: Some("incoming".into()),
                deprecated: Some(true),
                ..Default::default()
            })),
            MergeOptions::new(),
        );
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn security_scheme_openid_connect_merges() {
        use crate::v3_2::security_scheme::{OpenIdConnectSecurityScheme, SecurityScheme};
        let mut base = SecurityScheme::OpenIdConnect(Box::new(OpenIdConnectSecurityScheme {
            open_id_connect_url: "https://a/.well-known".into(),
            description: Some("base".into()),
            ..Default::default()
        }));
        let conflicts = report(
            &mut base,
            SecurityScheme::OpenIdConnect(Box::new(OpenIdConnectSecurityScheme {
                open_id_connect_url: "https://b/.well-known".into(),
                description: Some("incoming".into()),
                deprecated: Some(true),
                ..Default::default()
            })),
            MergeOptions::new(),
        );
        if let SecurityScheme::OpenIdConnect(o) = base {
            assert_eq!(o.open_id_connect_url, "https://b/.well-known");
        } else {
            panic!("expected OpenIdConnect");
        }
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn security_scheme_variant_mismatch_replaces() {
        use crate::v3_2::security_scheme::{
            ApiKeyLocation, ApiKeySecurityScheme, HttpSecurityScheme, SecurityScheme,
        };
        let mut base = SecurityScheme::ApiKey(Box::new(ApiKeySecurityScheme {
            name: "k".into(),
            location: ApiKeyLocation::Header,
            ..Default::default()
        }));
        let conflicts = report(
            &mut base,
            SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
                scheme: "basic".into(),
                ..Default::default()
            })),
            MergeOptions::new(),
        );
        assert!(matches!(base, SecurityScheme::HTTP(_)));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::ParameterVariantMismatch);
    }

    // ---- OAuth flows ----

    #[test]
    fn oauth2_flows_and_implicit_flow_merge() {
        use crate::v3_2::security_scheme::{
            ImplicitOAuth2Flow, OAuth2Flows, OAuth2SecurityScheme, SecurityScheme,
        };
        let mut flows = OAuth2Flows {
            implicit: Some(ImplicitOAuth2Flow {
                authorization_url: "https://auth/a".into(),
                refresh_url: Some("https://refresh/a".into()),
                scopes: BTreeMap::from([("read".into(), "Read".into())]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut base = SecurityScheme::OAuth2(Box::new(OAuth2SecurityScheme {
            flows: flows.clone(),
            description: Some("base".into()),
            ..Default::default()
        }));
        flows.implicit = Some(ImplicitOAuth2Flow {
            authorization_url: "https://auth/b".into(),
            refresh_url: Some("https://refresh/b".into()),
            scopes: BTreeMap::from([
                ("read".into(), "Updated Read".into()),
                ("write".into(), "Write".into()),
            ]),
            ..Default::default()
        });
        let conflicts = report(
            &mut base,
            SecurityScheme::OAuth2(Box::new(OAuth2SecurityScheme {
                flows,
                description: Some("incoming".into()),
                oauth2_metadata_url: Some("https://meta".into()),
                ..Default::default()
            })),
            MergeOptions::new(),
        );
        if let SecurityScheme::OAuth2(o) = &base {
            let i = o.flows.implicit.as_ref().unwrap();
            assert_eq!(i.authorization_url, "https://auth/b");
            assert!(i.scopes.contains_key("write"));
            assert_eq!(o.description.as_deref(), Some("incoming"));
            assert_eq!(o.oauth2_metadata_url.as_deref(), Some("https://meta"));
        } else {
            panic!("expected OAuth2");
        }
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn oauth2_password_flow_merges() {
        use crate::v3_2::security_scheme::PasswordOAuth2Flow;
        let mut base = PasswordOAuth2Flow {
            token_url: "https://a/token".into(),
            scopes: BTreeMap::from([("s1".into(), "S1".into())]),
            ..Default::default()
        };
        let conflicts = report(
            &mut base,
            PasswordOAuth2Flow {
                token_url: "https://b/token".into(),
                refresh_url: Some("https://b/refresh".into()),
                scopes: BTreeMap::from([
                    ("s1".into(), "S1-updated".into()),
                    ("s2".into(), "S2".into()),
                ]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.token_url, "https://b/token");
        assert!(base.scopes.contains_key("s2"));
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn oauth2_client_credentials_flow_merges() {
        use crate::v3_2::security_scheme::ClientCredentialsOAuth2Flow;
        let mut base = ClientCredentialsOAuth2Flow {
            token_url: "https://a".into(),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            ClientCredentialsOAuth2Flow {
                token_url: "https://b".into(),
                refresh_url: Some("https://b/r".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.token_url, "https://b");
    }

    #[test]
    fn oauth2_authorization_code_flow_merges() {
        use crate::v3_2::security_scheme::AuthorizationCodeOAuth2Flow;
        let mut base = AuthorizationCodeOAuth2Flow {
            authorization_url: "https://a/auth".into(),
            token_url: "https://a/token".into(),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            AuthorizationCodeOAuth2Flow {
                authorization_url: "https://b/auth".into(),
                token_url: "https://b/token".into(),
                refresh_url: Some("https://b/r".into()),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.authorization_url, "https://b/auth");
        assert_eq!(base.token_url, "https://b/token");
    }

    #[test]
    fn oauth2_device_authorization_flow_merges() {
        use crate::v3_2::security_scheme::DeviceAuthorizationOAuth2Flow;
        let mut base = DeviceAuthorizationOAuth2Flow {
            device_authorization_url: "https://a/da".into(),
            token_url: "https://a/token".into(),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            DeviceAuthorizationOAuth2Flow {
                device_authorization_url: "https://b/da".into(),
                token_url: "https://b/token".into(),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.device_authorization_url, "https://b/da");
    }

    // ---- Parameter variant mismatches ----

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

    // ---- Schema enum non-Single variants ----

    #[test]
    fn schema_all_of_leaf_replaces_under_deep_merge_option() {
        use crate::v3_2::schema::AllOfSchema;
        let mut base = Schema::AllOf(Box::new(AllOfSchema {
            all_of: vec![],
            ..Default::default()
        }));
        let conflicts = report(
            &mut base,
            Schema::AllOf(Box::new(AllOfSchema {
                all_of: vec![RefOr::new_item(Schema::Single(Box::new(
                    SingleSchema::Object(ObjectSchema::default()),
                )))],
                ..Default::default()
            })),
            MergeOptions::DeepMergeObjectSchemas.only(),
        );
        // AllOf doesn't deep-merge — falls back to leaf replace.
        if let Schema::AllOf(a) = &base {
            assert_eq!(a.all_of.len(), 1);
        }
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::SchemaLeafReplaced);
    }

    #[test]
    fn schema_single_string_replaces_under_deep_merge_object_only() {
        use crate::v3_2::schema::StringSchema;
        let mut base = Schema::Single(Box::new(SingleSchema::String(StringSchema {
            description: Some("base".into()),
            ..Default::default()
        })));
        let conflicts = report(
            &mut base,
            Schema::Single(Box::new(SingleSchema::String(StringSchema {
                description: Some("incoming".into()),
                ..Default::default()
            }))),
            MergeOptions::DeepMergeObjectSchemas.only(),
        );
        // StringSchema isn't ObjectSchema → leaf-replace.
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::SchemaLeafReplaced);
    }

    // ---- ObjectSchema deep merge — broader field coverage ----

    #[test]
    fn object_schema_deep_merge_full_fields() {
        use crate::common::bool_or::BoolOr;
        let mk_obj = |required: Vec<&str>, title: &str| {
            Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
                title: Some(title.into()),
                description: Some(format!("{title} desc")),
                required: Some(required.iter().map(|s| s.to_string()).collect()),
                pattern_properties: Some(BTreeMap::from([(
                    "^pat$".into(),
                    RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                        ObjectSchema::default(),
                    )))),
                )])),
                additional_properties: Some(BoolOr::Bool(true)),
                read_only: Some(false),
                write_only: Some(false),
                deprecated: Some(false),
                max_properties: Some(10),
                min_properties: Some(0),
                ..Default::default()
            })))
        };
        let mut base = mk_obj(vec!["a"], "Base");
        let incoming = mk_obj(vec!["b"], "Incoming");
        let _ = report(
            &mut base,
            incoming,
            MergeOptions::DeepMergeObjectSchemas.only(),
        );
        let Schema::Single(s) = &base else { panic!() };
        let SingleSchema::Object(o) = &**s else {
            panic!()
        };
        assert_eq!(o.title.as_deref(), Some("Incoming"));
        let req = o.required.as_ref().unwrap();
        assert!(req.contains(&"a".to_owned()) && req.contains(&"b".to_owned()));
    }

    #[test]
    fn object_schema_additional_properties_none_some_takes_incoming() {
        use crate::common::bool_or::BoolOr;
        let mut base = ObjectSchema::default();
        let incoming = ObjectSchema {
            additional_properties: Some(BoolOr::Bool(false)),
            ..Default::default()
        };
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
        let mut path = root_path();
        base.merge_with_context(incoming, &mut ctx, &mut path);
        assert!(matches!(
            base.additional_properties,
            Some(BoolOr::Bool(false))
        ));
    }

    #[test]
    fn object_schema_unevaluated_properties_merges() {
        use crate::common::bool_or::BoolOr;
        let mut base = ObjectSchema {
            unevaluated_properties: Some(BoolOr::Bool(true)),
            ..Default::default()
        };
        let incoming = ObjectSchema {
            unevaluated_properties: Some(BoolOr::Bool(false)),
            ..Default::default()
        };
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
        let mut path = root_path();
        base.merge_with_context(incoming, &mut ctx, &mut path);
        assert!(matches!(
            base.unevaluated_properties,
            Some(BoolOr::Bool(false))
        ));
    }

    // ---- Operation: callbacks, security, servers ----

    #[test]
    fn operation_callbacks_security_servers_merge() {
        use crate::v3_2::callback::Callback;
        let mut base = Operation {
            callbacks: Some(BTreeMap::from([(
                "cb1".to_owned(),
                RefOr::new_item(Callback::default()),
            )])),
            security: Some(vec![BTreeMap::from([("a".into(), vec!["read".into()])])]),
            servers: Some(vec![Server {
                url: "https://srv1".into(),
                ..Default::default()
            }]),
            deprecated: Some(false),
            extensions: Some(BTreeMap::from([("x-a".into(), serde_json::json!(1))])),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                callbacks: Some(BTreeMap::from([(
                    "cb2".to_owned(),
                    RefOr::new_item(Callback::default()),
                )])),
                security: Some(vec![BTreeMap::from([("b".into(), vec!["write".into()])])]),
                servers: Some(vec![Server {
                    url: "https://srv2".into(),
                    ..Default::default()
                }]),
                deprecated: Some(true),
                extensions: Some(BTreeMap::from([("x-b".into(), serde_json::json!(2))])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        // callbacks: new key appended → both present.
        let cb = base.callbacks.unwrap();
        assert!(cb.contains_key("cb1") && cb.contains_key("cb2"));
        // security: replace-when-non-empty.
        assert_eq!(base.security.unwrap().len(), 1);
        // deprecated overridden.
        assert_eq!(base.deprecated, Some(true));
    }

    // ---- PathItem: additional_operations + parameters dedup ----

    #[test]
    fn path_item_additional_operations_and_param_dedup() {
        let mut base = PathItem {
            additional_operations: Some(BTreeMap::from([(
                "trace".to_owned(),
                Operation::default(),
            )])),
            parameters: Some(vec![param_path("id")]),
            extensions: Some(BTreeMap::from([("x-a".into(), serde_json::json!(1))])),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            PathItem {
                additional_operations: Some(BTreeMap::from([(
                    "link".to_owned(),
                    Operation::default(),
                )])),
                parameters: Some(vec![param_path("id"), param_query("limit")]),
                extensions: Some(BTreeMap::from([("x-b".into(), serde_json::json!(2))])),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let ao = base.additional_operations.unwrap();
        assert!(ao.contains_key("trace") && ao.contains_key("link"));
        // Param dedup: id should not be duplicated.
        assert_eq!(base.parameters.unwrap().len(), 2);
        let ext = base.extensions.unwrap();
        assert!(ext.contains_key("x-a") && ext.contains_key("x-b"));
    }

    // ---- Components: every bag exercised ----

    #[test]
    fn components_every_bag_merges() {
        use crate::v3_2::callback::Callback;
        use crate::v3_2::components::Components;
        use crate::v3_2::example::Example;
        use crate::v3_2::header::Header;
        use crate::v3_2::link::Link;
        use crate::v3_2::media_type::MediaType;
        use crate::v3_2::request_body::RequestBody;
        use crate::v3_2::response::Response;
        use crate::v3_2::security_scheme::{ApiKeyLocation, ApiKeySecurityScheme, SecurityScheme};

        let make_components = |suffix: &str| Components {
            schemas: Some(BTreeMap::from([(
                format!("S{suffix}"),
                RefOr::new_item(Schema::Single(Box::new(SingleSchema::Object(
                    ObjectSchema::default(),
                )))),
            )])),
            responses: Some(BTreeMap::from([(
                format!("R{suffix}"),
                RefOr::new_item(Response::default()),
            )])),
            parameters: Some(BTreeMap::from([(
                format!("P{suffix}"),
                param_path(&format!("p{suffix}")),
            )])),
            examples: Some(BTreeMap::from([(
                format!("E{suffix}"),
                RefOr::new_item(Example::default()),
            )])),
            request_bodies: Some(BTreeMap::from([(
                format!("RB{suffix}"),
                RefOr::new_item(RequestBody::default()),
            )])),
            headers: Some(BTreeMap::from([(
                format!("H{suffix}"),
                RefOr::new_item(Header::default()),
            )])),
            security_schemes: Some(BTreeMap::from([(
                format!("SS{suffix}"),
                RefOr::new_item(SecurityScheme::ApiKey(Box::new(ApiKeySecurityScheme {
                    name: "k".into(),
                    location: ApiKeyLocation::Header,
                    ..Default::default()
                }))),
            )])),
            links: Some(BTreeMap::from([(
                format!("L{suffix}"),
                RefOr::new_item(Link::default()),
            )])),
            callbacks: Some(BTreeMap::from([(
                format!("CB{suffix}"),
                RefOr::new_item(Callback::default()),
            )])),
            path_items: Some(BTreeMap::from([(
                format!("PI{suffix}"),
                PathItem::default(),
            )])),
            media_types: Some(BTreeMap::from([(
                format!("MT{suffix}"),
                RefOr::new_item(MediaType::default()),
            )])),
            extensions: Some(BTreeMap::from([(
                format!("x-{suffix}"),
                serde_json::json!(1),
            )])),
        };
        let mut base = make_components("a");
        let _ = report(&mut base, make_components("b"), MergeOptions::new());
        // Every bag should now have both entries.
        assert_eq!(base.schemas.unwrap().len(), 2);
        assert_eq!(base.responses.unwrap().len(), 2);
        assert_eq!(base.parameters.unwrap().len(), 2);
        assert_eq!(base.examples.unwrap().len(), 2);
        assert_eq!(base.request_bodies.unwrap().len(), 2);
        assert_eq!(base.headers.unwrap().len(), 2);
        assert_eq!(base.security_schemes.unwrap().len(), 2);
        assert_eq!(base.links.unwrap().len(), 2);
        assert_eq!(base.callbacks.unwrap().len(), 2);
        assert_eq!(base.path_items.unwrap().len(), 2);
        assert_eq!(base.media_types.unwrap().len(), 2);
        assert_eq!(base.extensions.unwrap().len(), 2);
    }

    // ---- Spec: webhooks, security, external_docs ----

    #[test]
    fn spec_webhooks_security_external_docs() {
        let mut base = Spec {
            webhooks: Some(Paths {
                paths: BTreeMap::from([(
                    "/wh1".to_owned(),
                    PathItem {
                        summary: Some("wh1".into()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            security: Some(vec![BTreeMap::from([("base".into(), vec![])])]),
            external_docs: Some(ExternalDocumentation {
                url: "https://docs/a".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let incoming = Spec {
            webhooks: Some(Paths {
                paths: BTreeMap::from([(
                    "/wh2".to_owned(),
                    PathItem {
                        summary: Some("wh2".into()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            security: Some(vec![BTreeMap::from([("incoming".into(), vec![])])]),
            external_docs: Some(ExternalDocumentation {
                url: "https://docs/b".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        base.merge(incoming, MergeOptions::new()).unwrap();
        // webhooks merge per-key
        let wh = base.webhooks.unwrap().paths;
        assert!(wh.contains_key("/wh1") && wh.contains_key("/wh2"));
        // security: replace-when-non-empty
        assert_eq!(base.security.unwrap().len(), 1);
        // external_docs: deep merge
        assert_eq!(base.external_docs.unwrap().url, "https://docs/b");
    }

    // ---- None × Some additive paths for nested Option<RefOr<_>>s ----

    #[test]
    fn responses_default_none_takes_incoming() {
        use crate::v3_2::response::Responses;
        let mut base = Responses {
            default: None,
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Responses {
                default: Some(response_with("default")),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert!(base.default.is_some());
    }

    #[test]
    fn operation_request_body_none_takes_incoming() {
        use crate::v3_2::request_body::RequestBody;
        let mut base = Operation {
            request_body: None,
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                request_body: Some(RefOr::new_item(RequestBody::default())),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert!(base.request_body.is_some());
    }

    // ---- parameter_ref_key for non-Path variants (used in dedup) ----

    fn param_header(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Header(Box::new(InHeader {
            name: name.into(),
            description: None,
            required: None,
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

    fn param_cookie(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Cookie(Box::new(InCookie {
            name: name.into(),
            description: None,
            required: None,
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

    fn param_querystring(name: &str) -> RefOr<Parameter> {
        RefOr::new_item(Parameter::Querystring(Box::new(InQuerystring {
            name: name.into(),
            description: None,
            required: None,
            deprecated: None,
            content: BTreeMap::new(),
            example: None,
            examples: None,
            extensions: None,
        })))
    }

    #[test]
    fn parameter_dedup_covers_all_locations() {
        // Run a merge with each Parameter variant on both sides so
        // parameter_ref_key sees Header / Cookie / Querystring (and
        // ref) — covers the non-Path arms.
        let mut base = Operation {
            parameters: Some(vec![
                param_header("X-A"),
                param_cookie("c"),
                param_querystring("q"),
                RefOr::new_ref("#/components/parameters/P"),
            ]),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                parameters: Some(vec![
                    // Same keys — should dedup.
                    param_header("X-A"),
                    param_cookie("c"),
                    param_querystring("q"),
                    RefOr::new_ref("#/components/parameters/P"),
                ]),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        assert_eq!(base.parameters.unwrap().len(), 4, "no duplicates");
    }

    #[test]
    fn parameter_enum_each_same_variant_recurses() {
        // Cover each (Variant × Variant) match arm in Parameter.
        for (name, base, incoming) in [
            (
                "path",
                Parameter::Path(Box::new(mk_inpath("id", Some("a"), true))),
                Parameter::Path(Box::new(mk_inpath("id", Some("b"), true))),
            ),
            (
                "query",
                Parameter::Query(Box::new(InQuery {
                    name: "q".into(),
                    description: Some("a".into()),
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
                Parameter::Query(Box::new(InQuery {
                    name: "q".into(),
                    description: Some("b".into()),
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
            ),
            (
                "header",
                Parameter::Header(Box::new(InHeader {
                    name: "X-A".into(),
                    description: Some("a".into()),
                    required: None,
                    deprecated: None,
                    style: None,
                    explode: None,
                    schema: None,
                    example: None,
                    examples: None,
                    content: None,
                    extensions: None,
                })),
                Parameter::Header(Box::new(InHeader {
                    name: "X-A".into(),
                    description: Some("b".into()),
                    required: None,
                    deprecated: None,
                    style: None,
                    explode: None,
                    schema: None,
                    example: None,
                    examples: None,
                    content: None,
                    extensions: None,
                })),
            ),
            (
                "cookie",
                Parameter::Cookie(Box::new(InCookie {
                    name: "c".into(),
                    description: Some("a".into()),
                    required: None,
                    deprecated: None,
                    style: None,
                    explode: None,
                    schema: None,
                    example: None,
                    examples: None,
                    content: None,
                    extensions: None,
                })),
                Parameter::Cookie(Box::new(InCookie {
                    name: "c".into(),
                    description: Some("b".into()),
                    required: None,
                    deprecated: None,
                    style: None,
                    explode: None,
                    schema: None,
                    example: None,
                    examples: None,
                    content: None,
                    extensions: None,
                })),
            ),
            (
                "querystring",
                Parameter::Querystring(Box::new(InQuerystring {
                    name: "qs".into(),
                    description: Some("a".into()),
                    required: None,
                    deprecated: None,
                    content: BTreeMap::new(),
                    example: None,
                    examples: None,
                    extensions: None,
                })),
                Parameter::Querystring(Box::new(InQuerystring {
                    name: "qs".into(),
                    description: Some("b".into()),
                    required: None,
                    deprecated: None,
                    content: BTreeMap::new(),
                    example: None,
                    examples: None,
                    extensions: None,
                })),
            ),
        ] {
            let mut b = base;
            let _ = report(&mut b, incoming, MergeOptions::new());
            // After merge, description should be "b" for every variant.
            let desc = match &b {
                Parameter::Path(p) => p.description.as_deref(),
                Parameter::Query(p) => p.description.as_deref(),
                Parameter::Header(p) => p.description.as_deref(),
                Parameter::Cookie(p) => p.description.as_deref(),
                Parameter::Querystring(p) => p.description.as_deref(),
            };
            assert_eq!(desc, Some("b"), "{name}: incoming description must win");
        }
    }

    // ---- ErrorOnConflict success path (commits the working copy) ----

    #[test]
    fn spec_error_on_conflict_success_commits_working_copy() {
        // ErrorOnConflict set + no real collision → `Ok` with the
        // merged result observable on `self`. Exercises the
        // `*self = working` + Ok return in Spec::merge.
        let mut base = Spec::default();
        let incoming = Spec {
            json_schema_dialect: Some("https://example/dialect".into()),
            ..Default::default()
        };
        base.merge(incoming, MergeOptions::ErrorOnConflict.only())
            .unwrap();
        assert_eq!(
            base.json_schema_dialect.as_deref(),
            Some("https://example/dialect")
        );
    }

    // ---- Responses.default / Operation.request_body — both Some ----

    #[test]
    fn responses_default_both_some_recurses_into_response() {
        use crate::v3_2::response::Responses;
        let mut base = Responses {
            default: Some(response_with("base")),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Responses {
                default: Some(response_with("incoming")),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let RefOr::Item(r) = base.default.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(r.description.as_deref(), Some("incoming"));
    }

    #[test]
    fn operation_request_body_both_some_recurses() {
        use crate::v3_2::media_type::MediaType;
        use crate::v3_2::request_body::RequestBody;
        let mut base = Operation {
            request_body: Some(RefOr::new_item(RequestBody {
                description: Some("base".into()),
                content: BTreeMap::from([(
                    "application/json".to_owned(),
                    RefOr::new_item(MediaType::default()),
                )]),
                ..Default::default()
            })),
            ..Default::default()
        };
        let _ = report(
            &mut base,
            Operation {
                request_body: Some(RefOr::new_item(RequestBody {
                    description: Some("incoming".into()),
                    content: BTreeMap::from([(
                        "application/xml".to_owned(),
                        RefOr::new_item(MediaType::default()),
                    )]),
                    ..Default::default()
                })),
                ..Default::default()
            },
            MergeOptions::new(),
        );
        let RefOr::Item(rb) = base.request_body.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(rb.description.as_deref(), Some("incoming"));
        assert!(rb.content.contains_key("application/json"));
        assert!(rb.content.contains_key("application/xml"));
    }

    // ---- Helpers no-op when ctx.errored already set ----

    #[test]
    fn component_impls_no_op_when_already_errored() {
        // Sweep every component impl that has an entry-guard
        // `if ctx.errored { return; }`. Pre-flip the flag, call the
        // impl, and confirm nothing mutated. Covers the entry-guard
        // return branches that otherwise sit dead.
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::new());
        ctx.errored = true;
        let mut path = root_path();

        // PathItem
        {
            let mut pi = PathItem {
                summary: Some("base".into()),
                ..Default::default()
            };
            pi.merge_with_context(
                PathItem {
                    summary: Some("incoming".into()),
                    ..Default::default()
                },
                &mut ctx,
                &mut path,
            );
            assert_eq!(pi.summary.as_deref(), Some("base"));
        }
        // Responses
        {
            use crate::v3_2::response::Responses;
            let mut r = Responses::default();
            r.merge_with_context(
                Responses {
                    default: Some(response_with("incoming")),
                    ..Default::default()
                },
                &mut ctx,
                &mut path,
            );
            assert!(r.default.is_none());
        }
        // Operation
        {
            let mut op = Operation::default();
            op.merge_with_context(
                Operation {
                    summary: Some("incoming".into()),
                    ..Default::default()
                },
                &mut ctx,
                &mut path,
            );
            assert!(op.summary.is_none());
        }
        // Parameter
        {
            let mut p = Parameter::Path(Box::new(mk_inpath("x", None, true)));
            p.merge_with_context(
                Parameter::Query(Box::new(InQuery {
                    name: "x".into(),
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
                &mut ctx,
                &mut path,
            );
            assert!(matches!(p, Parameter::Path(_)));
        }
        // InPath / InQuery / InHeader / InCookie — entry guards.
        {
            let mut p = mk_inpath("a", Some("base"), true);
            p.merge_with_context(mk_inpath("a", Some("inc"), true), &mut ctx, &mut path);
            assert_eq!(p.description.as_deref(), Some("base"));
        }
    }

    // ---- Schema deep-merge fallback for Object × non-Object ----

    #[test]
    fn schema_object_vs_string_under_deep_merge_falls_back_to_replace() {
        use crate::v3_2::schema::StringSchema;
        let mut base = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            description: Some("base obj".into()),
            ..Default::default()
        })));
        let _ = report(
            &mut base,
            Schema::Single(Box::new(SingleSchema::String(StringSchema {
                description: Some("incoming str".into()),
                ..Default::default()
            }))),
            MergeOptions::DeepMergeObjectSchemas.only(),
        );
        // Falls into the `other_single_inner` arm → leaf-replace.
        assert!(matches!(
            base,
            Schema::Single(box_s) if matches!(*box_s, SingleSchema::String(_))
        ));
    }

    // ---- Parameter variants: collisions exercise field-level merges ----

    fn mk_inpath(name: &str, description: Option<&str>, required: bool) -> InPath {
        InPath {
            name: name.into(),
            description: description.map(str::to_owned),
            required,
            deprecated: None,
            style: None,
            explode: None,
            schema: None,
            example: None,
            examples: None,
            content: None,
            extensions: None,
        }
    }

    #[test]
    fn in_path_full_field_merge() {
        let mut base = mk_inpath("id", Some("base"), true);
        base.explode = Some(false);
        base.example = Some(serde_json::json!(1));
        let mut incoming = mk_inpath("id", Some("incoming"), false);
        incoming.deprecated = Some(true);
        incoming.explode = Some(true);
        incoming.example = Some(serde_json::json!(2));
        let conflicts = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(base.description.as_deref(), Some("incoming"));
        assert!(!base.required); // required-scalar collision
        assert_eq!(base.deprecated, Some(true));
        assert_eq!(base.explode, Some(true));
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn in_query_full_field_merge() {
        let mut base = InQuery {
            name: "q".into(),
            description: Some("base".into()),
            required: Some(false),
            deprecated: None,
            allow_empty_value: Some(false),
            style: None,
            explode: Some(false),
            allow_reserved: Some(false),
            schema: None,
            example: Some(serde_json::json!("a")),
            examples: None,
            content: None,
            extensions: None,
        };
        let incoming = InQuery {
            name: "q".into(),
            description: Some("incoming".into()),
            required: Some(true),
            deprecated: Some(true),
            allow_empty_value: Some(true),
            style: None,
            explode: Some(true),
            allow_reserved: Some(true),
            schema: None,
            example: Some(serde_json::json!("b")),
            examples: None,
            content: None,
            extensions: None,
        };
        let conflicts = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(base.required, Some(true));
        assert_eq!(base.explode, Some(true));
        assert_eq!(base.allow_reserved, Some(true));
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn in_header_full_field_merge() {
        let mut base = InHeader {
            name: "X-A".into(),
            description: Some("base".into()),
            required: Some(false),
            deprecated: None,
            style: None,
            explode: Some(false),
            schema: None,
            example: Some(serde_json::json!("a")),
            examples: None,
            content: None,
            extensions: None,
        };
        let incoming = InHeader {
            name: "X-A".into(),
            description: Some("incoming".into()),
            required: Some(true),
            deprecated: Some(true),
            style: None,
            explode: Some(true),
            schema: None,
            example: Some(serde_json::json!("b")),
            examples: None,
            content: None,
            extensions: None,
        };
        let _ = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(base.required, Some(true));
        assert_eq!(base.explode, Some(true));
    }

    #[test]
    fn in_cookie_full_field_merge() {
        let mut base = InCookie {
            name: "c".into(),
            description: Some("base".into()),
            required: Some(false),
            deprecated: None,
            style: None,
            explode: Some(false),
            schema: None,
            example: Some(serde_json::json!("a")),
            examples: None,
            content: None,
            extensions: None,
        };
        let incoming = InCookie {
            name: "c".into(),
            description: Some("incoming".into()),
            required: Some(true),
            deprecated: Some(true),
            style: None,
            explode: Some(true),
            schema: None,
            example: Some(serde_json::json!("b")),
            examples: None,
            content: None,
            extensions: None,
        };
        let _ = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(base.required, Some(true));
        assert_eq!(base.deprecated, Some(true));
    }

    #[test]
    fn in_querystring_full_field_merge_with_content_overlap() {
        use crate::v3_2::media_type::MediaType;
        let mut base = InQuerystring {
            name: "qs".into(),
            description: Some("base".into()),
            required: Some(false),
            deprecated: None,
            content: BTreeMap::from([(
                "application/json".to_owned(),
                RefOr::new_item(MediaType {
                    description: Some("media-base".into()),
                    ..Default::default()
                }),
            )]),
            example: Some(serde_json::json!("a")),
            examples: None,
            extensions: None,
        };
        let incoming = InQuerystring {
            name: "qs".into(),
            description: Some("incoming".into()),
            required: Some(true),
            deprecated: Some(true),
            content: BTreeMap::from([
                (
                    // Overlapping key — exercises the inner loop's
                    // recursive merge path.
                    "application/json".to_owned(),
                    RefOr::new_item(MediaType {
                        description: Some("media-inc".into()),
                        ..Default::default()
                    }),
                ),
                (
                    "text/plain".to_owned(),
                    RefOr::new_item(MediaType::default()),
                ),
            ]),
            example: Some(serde_json::json!("b")),
            examples: None,
            extensions: None,
        };
        let _ = report(&mut base, incoming, MergeOptions::new());
        assert_eq!(base.required, Some(true));
        assert!(base.content.contains_key("application/json"));
        assert!(base.content.contains_key("text/plain"));
    }

    // ---- Callback paths overlap exercises the inline loop ----

    #[test]
    fn callback_paths_overlap_merges_per_path_expression() {
        use crate::v3_2::callback::Callback;
        let mut base = Callback {
            paths: BTreeMap::from([(
                "{$request.body#/url}".to_owned(),
                PathItem {
                    summary: Some("base".into()),
                    ..Default::default()
                },
            )]),
            extensions: Some(BTreeMap::from([("x-a".into(), serde_json::json!(1))])),
        };
        let incoming = Callback {
            paths: BTreeMap::from([
                (
                    "{$request.body#/url}".to_owned(),
                    PathItem {
                        summary: Some("incoming".into()),
                        ..Default::default()
                    },
                ),
                ("{$response.body#/url}".to_owned(), PathItem::default()),
            ]),
            extensions: Some(BTreeMap::from([("x-b".into(), serde_json::json!(2))])),
        };
        let _ = report(&mut base, incoming, MergeOptions::new());
        // Overlapping path-expression — base summary replaced.
        assert_eq!(
            base.paths["{$request.body#/url}"].summary.as_deref(),
            Some("incoming")
        );
        // New path-expression appended.
        assert!(base.paths.contains_key("{$response.body#/url}"));
    }

    // ---- record_kept_base_or_error covers the openapi mismatch branch ----

    #[test]
    fn spec_openapi_mismatch_records_resolution_base_under_default() {
        // Forcing openapi to differ requires `Spec::default()`'s
        // V3_2_0 to be overridden — go through serde to build two
        // Specs with different openapi values.
        let mut base: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "T", "version": "1"},
        }))
        .unwrap();
        let incoming: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.2.1",
            "info": {"title": "T", "version": "1"},
        }))
        .unwrap();
        let report = base.merge(incoming, MergeOptions::new()).unwrap();
        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.path.ends_with(".openapi") && c.resolution == Resolution::Base)
        );
        // Base kept its openapi.
        let openapi_str = serde_json::to_value(&base.openapi).unwrap();
        assert_eq!(openapi_str, serde_json::json!("3.2.0"));
    }

    // ---- ReplaceListsWhenEmpty exercises that branch ----

    #[test]
    fn replace_lists_when_empty_option_drops_base_list() {
        let mut base = Spec {
            servers: Some(vec![Server {
                url: "https://srv1".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        base.merge(
            Spec {
                servers: Some(vec![]),
                ..Default::default()
            },
            MergeOptions::ReplaceListsWhenEmpty.only(),
        )
        .unwrap();
        // With the option set, incoming empty replaces base.
        let s = base.servers.unwrap();
        assert!(s.is_empty(), "incoming empty list should replace base");
    }

    // ---- BaseWins mode propagates through helpers ----

    #[test]
    fn base_wins_keeps_base_value_in_extensions() {
        let mut base: Option<BTreeMap<String, serde_json::Value>> =
            Some(BTreeMap::from([("x-a".into(), serde_json::json!(1))]));
        let mut ctx: MergeContext = MergeContext::new(MergeOptions::BaseWins.only());
        let mut path = root_path();
        crate::common::merge::merge_extensions(
            &mut base,
            Some(BTreeMap::from([("x-a".into(), serde_json::json!(2))])),
            &mut ctx,
            &mut path,
            ".ext",
        );
        let m = base.unwrap();
        assert_eq!(m.get("x-a"), Some(&serde_json::json!(1)));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].resolution, Resolution::Base);
    }
}
