//! v2 Reference Object — `$ref`-only.
//!
//! OpenAPI 2.0's Reference Object has exactly one field, `$ref`. The shared
//! `crate::common::reference::Ref` carries `summary` and `description` for
//! v3.1 compatibility; using it in v2 would let those v3.1-only fields leak
//! into v2 deserialize/serialize. This module pins v2 to the spec form.

use serde::{Deserialize, Serialize};

use crate::common::reference::{ResolveError, ResolveReference};
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};

/// v2 Reference Object — exactly `{ "$ref": "..." }`.
///
/// `#[serde(deny_unknown_fields)]` rejects extra v3.1-style `summary` /
/// `description` keys that are not part of the v2 spec.
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

/// v2 RefOr<T> — either a `$ref` (v2 form) or an inline value of `T`.
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
        let r = RefOr::<Foo>::new_ref("#/definitions/Foo");
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"$ref": "#/definitions/Foo"}),
        );
    }

    #[test]
    fn item_serializes_inline() {
        let r = RefOr::new_item(Foo { foo: "bar".into() });
        assert_eq!(serde_json::to_value(&r).unwrap(), json!({"foo": "bar"}),);
    }

    #[test]
    fn deserialize_ref() {
        let r: RefOr<Foo> = serde_json::from_value(json!({"$ref": "#/definitions/Foo"})).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/definitions/Foo"));
    }

    #[test]
    fn deserialize_rejects_v3_1_fields() {
        // v3.1's `summary` / `description` on a Ref are not allowed in v2.
        let r = serde_json::from_value::<RefOr<Foo>>(json!({
            "$ref": "#/definitions/Foo",
            "summary": "should be rejected",
        }));
        // We expect the ref form to be rejected (deny_unknown_fields), and the
        // input to fall through to the Item form, which then also fails because
        // it lacks the required `foo` field.
        assert!(
            r.is_err(),
            "should not silently accept v3.1 summary on v2 Ref"
        );
    }

    use crate::v2::schema::{Schema, StringSchema};
    use crate::v2::spec::Spec;
    use crate::validation::Context;
    use crate::validation::Options;

    #[test]
    fn get_item_returns_inline() {
        let mut spec = Spec::default();
        let _ = spec
            .define_schema("Foo", Schema::from(StringSchema::default()))
            .unwrap();
        let r = RefOr::<Schema>::new_item(Schema::from(StringSchema::default()));
        let v = r.get_item(&spec).unwrap();
        assert!(matches!(v, Schema::String(_)));
    }

    #[test]
    fn get_item_resolves_internal_ref() {
        let mut spec = Spec::default();
        let _ = spec
            .define_schema("Foo", Schema::from(StringSchema::default()))
            .unwrap();
        let r = RefOr::<Schema>::new_ref("#/definitions/Foo");
        assert!(r.get_item(&spec).is_ok());
    }

    #[test]
    fn get_item_returns_not_found() {
        let spec = Spec::default();
        let r = RefOr::<Schema>::new_ref("#/definitions/Missing");
        let err = r.get_item(&spec).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[test]
    fn get_item_external_unsupported() {
        let spec = Spec::default();
        let r = RefOr::<Schema>::new_ref("https://example.com/foo.json");
        let err = r.get_item(&spec).unwrap_err();
        assert!(matches!(err, ResolveError::ExternalUnsupported(_)));
    }

    #[test]
    fn validate_with_context_inline_propagates() {
        let spec = Spec::default();
        let r = RefOr::<Schema>::new_item(Schema::from(StringSchema {
            pattern: Some("[".into()),
            ..Default::default()
        }));
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("pattern")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_with_context_ref_hit() {
        let mut spec = Spec::default();
        let _ = spec
            .define_schema(
                "Foo",
                Schema::from(StringSchema {
                    pattern: Some("[".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        let r = RefOr::<Schema>::new_ref("#/definitions/Foo");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "p".into());
        // The inner schema's pattern should be invalidated.
        assert!(
            ctx.errors.iter().any(|e| e.contains("pattern")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_with_context_ref_miss() {
        let spec = Spec::default();
        let r = RefOr::<Schema>::new_ref("#/definitions/Missing");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not found")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_with_context_external_unsupported() {
        let spec = Spec::default();
        let r = RefOr::<Schema>::new_ref("https://example.com/foo.json");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not supported") || e.contains("external")),
            "errors: {:?}",
            ctx.errors
        );

        // With option set, no error.
        let mut ctx = Context::new(&spec, Options::only(&Options::IgnoreExternalReferences));
        r.validate_with_context(&mut ctx, "p".into());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn validate_empty_ref_string() {
        let r = Ref::new("");
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        // Use Schema as the resolved type so ValidateWithContext bound is satisfied.
        r.validate_with_context::<Spec, Schema>(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn resolve_in_map_helper() {
        let mut spec = Spec::default();
        let _ = spec
            .define_schema("Foo", Schema::from(StringSchema::default()))
            .unwrap();
        // Using a built map of RefOr<Schema>, not the spec's `definitions` field.
        let mut map: std::collections::BTreeMap<String, RefOr<Schema>> = Default::default();
        map.insert(
            "X".into(),
            RefOr::new_item(Schema::from(StringSchema::default())),
        );
        let opt = Some(map);
        let v = resolve_in_map::<Spec, Schema>(&spec, "#/definitions/X", "#/definitions/", &opt);
        assert!(v.is_some());

        let v =
            resolve_in_map::<Spec, Schema>(&spec, "#/definitions/Missing", "#/definitions/", &opt);
        assert!(v.is_none());
    }
}
