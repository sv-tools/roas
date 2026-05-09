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
use std::collections::BTreeSet;

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
///
/// Deserialization routes by **presence of `$ref` in the input** rather than
/// by serde's untagged fallthrough. Inputs containing `$ref` MUST validate as
/// a `Ref` (which rejects 3.1-only sibling fields via `deny_unknown_fields`);
/// they will not be silently re-interpreted as an inline `T` if the `Ref`
/// form fails. This protects the v3.0 strictness guarantee even when `T`'s
/// own deserialization is permissive (for example, an ObjectSchema with a
/// defaulted `type` field would otherwise eat a stray `$ref` as an unknown
/// key).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RefOr<T> {
    Ref(Ref),
    Item(T),
}

impl<'de, T> Deserialize<'de> for RefOr<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Materialise the input as JSON Value so we can peek for `$ref` and
        // then try the appropriate variant. The single allocation is
        // acceptable for the deserialization path (and matches what other
        // OAS parsers do internally).
        let value = serde_json::Value::deserialize(deserializer)?;
        let has_ref = matches!(&value, serde_json::Value::Object(m) if m.contains_key("$ref"));
        if has_ref {
            Ref::deserialize(value)
                .map(RefOr::Ref)
                .map_err(serde::de::Error::custom)
        } else {
            T::deserialize(value)
                .map(RefOr::Item)
                .map_err(serde::de::Error::custom)
        }
    }
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
    let map = map.as_ref()?;
    let mut current = reference;
    let mut visited = BTreeSet::new();

    loop {
        let key = current.strip_prefix(prefix)?;
        let item = map.get(key)?;

        match item {
            RefOr::Item(d) => return Some(d),
            RefOr::Ref(r) => {
                if !r.reference.starts_with(prefix) {
                    return item.get_item(spec).ok();
                }
                if !visited.insert(r.reference.as_str()) {
                    return None;
                }
                current = &r.reference;
            }
        }
    }
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

    #[test]
    fn schema_with_no_type_parses_as_inline_object() {
        // Sanity: with the "missing type = object" relaxation in
        // ObjectSchema, an inline schema with no `$ref` and no `type`
        // still parses as `Item(ObjectSchema)`.
        let r: RefOr<crate::v3_0::schema::Schema> =
            serde_json::from_value(json!({"properties": {}})).expect("must parse");
        assert!(matches!(r, RefOr::Item(_)), "expected inline Item form");
    }

    #[test]
    fn schema_ref_with_extras_does_not_fall_back_to_inline() {
        // Even though ObjectSchema's `type` is now optional (a schema with
        // no `type` is treated as object), an input that *does* contain
        // `$ref` MUST validate as a Ref. Routing-by-`$ref`-presence
        // prevents `{"$ref": "...", "description": "..."}` from being
        // silently parsed as an inline ObjectSchema with the `$ref`
        // dropped.
        let r = serde_json::from_value::<RefOr<crate::v3_0::schema::Schema>>(json!({
            "$ref": "#/components/schemas/Foo",
            "description": "this v3.1 sibling is rejected",
        }));
        assert!(
            r.is_err(),
            "ref form must fail strictly when 3.1 sibling fields are present, even with permissive ObjectSchema"
        );
    }

    #[test]
    fn schema_ref_rejects_v3_1_sibling_fields() {
        let r = serde_json::from_value::<RefOr<crate::v3_0::schema::Schema>>(json!({
            "$ref": "#/components/schemas/Foo",
            "description": "should be rejected",
        }));
        assert!(
            r.is_err(),
            "schema refs must not fall back to inline schemas when v3.1 sibling fields are present"
        );
    }
}
