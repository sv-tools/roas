//! v3.0 Reference Object — `$ref`-only.
//!
//! OpenAPI 3.0.x's Reference Object has exactly one field, `$ref`. The shared
//! `crate::common::reference::Ref` carries `summary` and `description` which
//! were added in 3.1; using it here would let those v3.1-only fields leak
//! into 3.0 deserialize/serialize. This module pins v3.0 to the spec form.

use serde::{Deserialize, Serialize};

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::common::reference::{ResolveError, ResolveReference};
use crate::validation::Options;

/// v3.0 Reference Object — exactly `{ "$ref": "..." }`.
///
/// `#[serde(deny_unknown_fields)]` rejects extra v3.1-style `summary` /
/// `description` keys that are not part of the v3.0 spec.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ref {
    /// **Required** The reference identifier. MUST be a URI.
    #[serde(rename = "$ref")]
    pub reference: String,
}

impl Ref {
    pub fn new(reference: impl Into<String>) -> Self {
        Ref {
            reference: reference.into(),
        }
    }

    pub fn validate_with_context<T, D>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        if self.reference.is_empty() {
            ctx.error(path, ".$ref: must not be empty");
        }
    }
}

/// v3.0 RefOr<T> — either a `$ref` (v3.0 form) or an inline value of `T`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RefOr<T> {
    Ref(Ref),
    Item(T),
}

impl<D> RefOr<D> {
    pub fn new_ref(reference: impl Into<String>) -> Self {
        RefOr::Ref(Ref::new(reference))
    }

    pub fn new_item(item: D) -> Self {
        RefOr::Item(item)
    }

    pub fn get_item<'a, T>(&'a self, spec: &'a T) -> Result<&'a D, ResolveError>
    where
        T: ResolveReference<D>,
    {
        match self {
            RefOr::Item(d) => Ok(d),
            RefOr::Ref(r) => {
                if r.reference.starts_with("#/") {
                    match spec.resolve_reference(&r.reference) {
                        Some(d) => Ok(d),
                        None => Err(ResolveError::NotFound(r.reference.clone())),
                    }
                } else {
                    Err(ResolveError::ExternalUnsupported(r.reference.clone()))
                }
            }
        }
    }

    pub fn validate_with_context<T>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        match self {
            RefOr::Ref(r) => {
                r.validate_with_context(ctx, path.clone());
                if ctx.visit(r.reference.clone()) {
                    match self.get_item(ctx.spec) {
                        Ok(d) => {
                            d.validate_with_context(ctx, r.reference.clone());
                        }
                        Err(ResolveError::NotFound(reference)) => {
                            ctx.error(path, format_args!(".$ref: `{reference}` not found"));
                        }
                        Err(e @ ResolveError::ExternalUnsupported(_)) => {
                            if !ctx.is_option(Options::IgnoreExternalReferences) {
                                ctx.error(path, format_args!(".$ref: {e}"));
                            }
                        }
                    }
                }
            }
            RefOr::Item(d) => d.validate_with_context(ctx, path),
        }
    }
}

/// Resolve a `$ref` against a `BTreeMap<String, RefOr<D>>` field on the spec.
pub fn resolve_in_map<'a, T, D>(
    spec: &'a T,
    reference: &str,
    prefix: &str,
    map: &'a Option<std::collections::BTreeMap<String, RefOr<D>>>,
) -> Option<&'a D>
where
    T: ResolveReference<D>,
{
    map.as_ref()
        .and_then(|x| x.get(reference.trim_start_matches(prefix)))
        .and_then(move |x| x.get_item(spec).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
    struct Foo {
        pub foo: String,
    }

    #[test]
    fn ref_only_serializes_dollar_ref() {
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Foo");
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"$ref": "#/components/schemas/Foo"}),
        );
    }

    #[test]
    fn deserialize_rejects_v3_1_fields() {
        let r = serde_json::from_value::<RefOr<Foo>>(json!({
            "$ref": "#/components/schemas/Foo",
            "summary": "should be rejected",
        }));
        assert!(
            r.is_err(),
            "should not silently accept v3.1 summary on v3.0 Ref"
        );
    }

    #[test]
    fn deserialize_ref() {
        let r: RefOr<Foo> =
            serde_json::from_value(json!({"$ref": "#/components/schemas/Foo"})).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/schemas/Foo"));
    }
}
