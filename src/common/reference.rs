//! Reference Object — shared across v2, v3.0, v3.1, and v3.2.
//!
//! The OpenAPI Reference Object differs only by which sibling fields are
//! permitted: v2 and v3.0 spell it as `{ "$ref": "..." }`, while v3.1+
//! adds optional `summary` and `description` overrides. This module
//! carries the union: `summary` and `description` are `Option`-typed and
//! always declared on the struct. Per-version validators may still flag
//! their presence if a build wants v2 / v3.0 strictness at the
//! validation layer.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

use crate::loader::{Loader, LoaderError};
use crate::validation::{Context, Options, PushError, ValidateWithContext};

/// ResolveReference is a trait for resolving references.
pub trait ResolveReference<D> {
    fn resolve_reference(&self, reference: &str) -> Option<&D>;
}

/// ResolveError is an error type for resolving references.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// NotFound is returned when the reference is not found.
    #[error("reference `{0}` not found")]
    NotFound(String),

    /// ExternalUnsupported is returned by the loader-less `get_item` path
    /// when the reference targets a resource outside the current document.
    /// Callers that want the loader to fetch and parse the resource should
    /// use `get_item_with_loader`.
    #[error("resolving of an external reference `{0}` is not supported")]
    ExternalUnsupported(String),

    /// External is returned by `get_item_with_loader` when the loader was
    /// invoked but failed — no fetcher registered, fetch error, parse
    /// error, or missing JSON Pointer target. The underlying `LoaderError`
    /// is exposed as the error source.
    #[error("failed to resolve external reference `{reference}`")]
    External {
        reference: String,
        #[source]
        source: LoaderError,
    },
}

/// RefOr is a simple object to allow storing a reference to another component or a component itself.
///
/// Deserialization routes by **presence of `$ref` in the input** rather than
/// by serde's untagged fallthrough. Inputs containing `$ref` MUST validate as
/// a `Ref` (which rejects unknown siblings via `deny_unknown_fields`); they
/// will not be silently re-interpreted as an inline `T` if the `Ref` form
/// fails. This prevents `{"$ref": "...", "typo": "..."}` from being parsed
/// as an inline `T` with the `$ref` dropped.
///
/// Example:
///
/// ```rust
/// use serde::{Deserialize, Serialize};
/// use roas::common::reference::RefOr;
///
/// #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
/// struct Foo {
///     pub value: String,
/// }
///
/// #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
/// struct Bar {
///     pub foo: Option<RefOr<Foo>>,
/// }
/// ```
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum RefOr<T> {
    /// A reference to another component.
    Ref(Ref),

    /// The component itself.
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
        // Materialise the input as JSON Value so we can peek for `$ref`
        // and then route to the appropriate variant. The single
        // allocation is acceptable for the deserialization path (and
        // matches what other OAS parsers do internally).
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

/// Ref is a simple object to allow referencing other components in the OpenAPI document,
/// internally and externally.
/// The $ref string value contains a URI [RFC3986](https://www.rfc-editor.org/rfc/rfc3986),
/// which identifies the location of the value being referenced.
/// See the rules for resolving Relative References.
///
/// Specification example:
///
/// ```yaml
/// $ref: '#/components/schemas/Pet'
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Ref {
    /// **Required** The reference identifier.
    /// This MUST be in the form of a URI.
    #[serde(rename = "$ref")]
    pub reference: String,

    /// A short summary which by default SHOULD override that of the referenced component.
    /// If the referenced object-type does not allow a summary field, then this field has no effect.
    /// Added in OpenAPI 3.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A description which by default SHOULD override that of the referenced component.
    /// CommonMark syntax MAY be used for rich text representation.
    /// If the referenced object-type does not allow a description field, then this field has no effect.
    /// Added in OpenAPI 3.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl<D> RefOr<D> {
    /// Validate this `RefOr<D>` in the surrounding context.
    ///
    /// The extra `D: 'static + Clone + DeserializeOwned` bounds are
    /// required even when validating an inline `Item` or an internal
    /// `#/...` ref with no loader attached, because the same body must
    /// be statically callable for the loader-driven external path that
    /// invokes `Loader::resolve_reference_as::<D>` under the hood. In
    /// practice every concrete component type in this crate (`Schema`,
    /// `Parameter`, `Header`, `Response`, etc.) satisfies these bounds
    /// already; the constraint only bites downstream code that
    /// parameterises `RefOr<D>` over a custom `D` lacking `Clone` or
    /// `DeserializeOwned`.
    pub fn validate_with_context<T>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T> + 'static + Clone + DeserializeOwned,
    {
        match self {
            RefOr::Ref(r) => {
                r.validate_with_context::<T, D>(ctx, path.clone());
                if !ctx.visit(r.reference.clone()) {
                    return;
                }
                if r.reference.starts_with("#/") {
                    match ctx.spec.resolve_reference(&r.reference) {
                        Some(d) => d.validate_with_context(ctx, r.reference.clone()),
                        None => ctx.error(path, format_args!(".$ref: `{}` not found", r.reference)),
                    }
                    return;
                }
                // External reference: route through the loader if one is
                // attached to the context. `as_deref_mut` keeps the
                // mutable borrow on `ctx.loader` scoped to this expression
                // so the resolved owned `D` (or `LoaderError`) outlives
                // the borrow and lets us re-use `ctx` to validate it.
                let resolved = ctx
                    .loader
                    .as_deref_mut()
                    .map(|l| l.resolve_reference_as::<D>(&r.reference));
                match resolved {
                    Some(Ok(d)) => d.validate_with_context(ctx, r.reference.clone()),
                    Some(Err(source)) => {
                        if !ctx.is_option(Options::IgnoreExternalReferences) {
                            ctx.error(
                                path,
                                format_args!(
                                    ".$ref: failed to resolve external reference `{}`: {source}",
                                    r.reference,
                                ),
                            );
                        }
                    }
                    None => {
                        if !ctx.is_option(Options::IgnoreExternalReferences) {
                            ctx.error(
                                path,
                                format_args!(
                                    ".$ref: resolving of an external reference `{}` is not supported",
                                    r.reference,
                                ),
                            );
                        }
                    }
                }
            }
            RefOr::Item(d) => {
                d.validate_with_context(ctx, path);
            }
        }
    }

    /// Create a new RefOr with a reference.
    pub fn new_ref(reference: impl Into<String>) -> Self {
        RefOr::Ref(Ref::new(reference))
    }

    /// Create a new RefOr with an item.
    pub fn new_item(item: D) -> Self {
        RefOr::Item(item)
    }

    /// Get the item from the RefOr by returning the Item or resolving a reference.
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

    /// Resolve the reference using the spec for internal `#/` pointers and
    /// the provided `Loader` for external ones.
    ///
    /// Internal refs are returned as `Cow::Borrowed` from the spec. External
    /// refs are deserialized through `Loader::resolve_reference_as` and
    /// returned as `Cow::Owned`. Loader failures (no fetcher registered,
    /// fetch / parse error, missing JSON Pointer target) surface as
    /// `ResolveError::External` with the underlying `LoaderError` as source.
    pub fn get_item_with_loader<'a, T>(
        &'a self,
        spec: &'a T,
        loader: &mut Loader,
    ) -> Result<Cow<'a, D>, ResolveError>
    where
        T: ResolveReference<D>,
        D: 'static + Clone + DeserializeOwned,
    {
        match self {
            RefOr::Item(d) => Ok(Cow::Borrowed(d)),
            RefOr::Ref(r) => {
                if r.reference.starts_with("#/") {
                    spec.resolve_reference(&r.reference)
                        .map(Cow::Borrowed)
                        .ok_or_else(|| ResolveError::NotFound(r.reference.clone()))
                } else {
                    loader
                        .resolve_reference_as::<D>(&r.reference)
                        .map(Cow::Owned)
                        .map_err(|source| ResolveError::External {
                            reference: r.reference.clone(),
                            source,
                        })
                }
            }
        }
    }
}

impl Ref {
    pub fn validate_with_context<T, D>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        if self.reference.is_empty() {
            ctx.error(path, ".$ref: must not be empty");
        }
    }

    pub fn new(reference: impl Into<String>) -> Self {
        Ref {
            reference: reference.into(),
            ..Default::default()
        }
    }
}

/// Resolve a `$ref` against a `Components`-style map, following alias
/// chains iteratively with cycle detection.
///
/// Behavior:
/// * `reference` MUST start with `prefix` — if not, the lookup returns
///   `None` rather than silently using the unstripped string (this is
///   stricter than the previous `trim_start_matches` behavior, which
///   could fold a wrong-prefix `#/components/parameters/X` reference into
///   the schemas map and produce confusing results).
/// * If the matched entry is itself a `RefOr::Ref` whose target lies in
///   the same map (`prefix`-anchored), the chain is followed iteratively.
/// * A `BTreeSet` of already-visited references prevents infinite
///   recursion when the chain loops (`A → B → A`); on a cycle the lookup
///   returns `None` and the caller surfaces the usual "not found" error.
/// * Cross-map `$ref`s (e.g. a parameter that refs a schema) are resolved
///   via `RefOr::get_item`'s spec-wide path, so their cycle handling falls
///   to the spec resolver.
pub fn resolve_in_map<'a, T, D>(
    spec: &'a T,
    reference: &str,
    prefix: &str,
    map: &'a Option<BTreeMap<String, RefOr<D>>>,
) -> Option<&'a D>
where
    T: ResolveReference<D>,
{
    let map = map.as_ref()?;
    let mut current = reference;
    let mut visited: BTreeSet<&str> = BTreeSet::new();

    loop {
        let key = current.strip_prefix(prefix)?;
        let item = map.get(key)?;

        match item {
            RefOr::Item(d) => return Some(d),
            RefOr::Ref(r) => {
                if !r.reference.starts_with(prefix) {
                    // Cross-map ref: hand off to spec-wide resolver.
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
    use crate::loader::{JsonFileFetcher, ResourceFetcher};
    use serde_json::Value;
    use std::fs;
    use url::Url;

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
    struct Foo {
        pub foo: String,
    }

    struct PetSpec;

    impl ResolveReference<Foo> for PetSpec {
        fn resolve_reference(&self, _reference: &str) -> Option<&Foo> {
            None
        }
    }

    #[test]
    fn test_ref_or_foo_serialize() {
        assert_eq!(
            serde_json::to_value(RefOr::new_item(Foo {
                foo: String::from("bar"),
            }))
            .unwrap(),
            serde_json::json!({
                "foo": "bar"
            }),
            "serialize item",
        );
        assert_eq!(
            serde_json::to_value(RefOr::Ref::<Foo>(Ref {
                reference: String::from("#/components/schemas/Foo"),
                ..Default::default()
            }))
            .unwrap(),
            serde_json::json!({
                "$ref": "#/components/schemas/Foo"
            }),
            "serialize ref",
        );
    }

    #[test]
    fn test_ref_or_foo_deserialize() {
        assert_eq!(
            serde_json::from_value::<RefOr<Foo>>(serde_json::json!({
                "foo":"bar",
            }))
            .unwrap(),
            RefOr::new_item(Foo {
                foo: String::from("bar"),
            }),
            "deserialize item",
        );

        assert_eq!(
            serde_json::from_value::<RefOr<Foo>>(serde_json::json!({
                "$ref":"#/components/schemas/Foo",
            }))
            .unwrap(),
            RefOr::Ref(Ref {
                reference: String::from("#/components/schemas/Foo"),
                ..Default::default()
            }),
            "deserialize ref",
        );
    }

    #[test]
    fn ref_with_unknown_sibling_is_rejected() {
        let r = serde_json::from_value::<RefOr<Foo>>(serde_json::json!({
            "$ref": "#/components/schemas/Foo",
            "typo": "unexpected sibling",
        }));
        assert!(
            r.is_err(),
            "$ref form must fail strictly when unknown siblings are present"
        );
    }

    #[test]
    fn ref_with_summary_and_description_is_accepted() {
        let r: RefOr<Foo> = serde_json::from_value(serde_json::json!({
            "$ref": "#/components/schemas/Foo",
            "summary": "s",
            "description": "d",
        }))
        .unwrap();
        match r {
            RefOr::Ref(rr) => {
                assert_eq!(rr.reference, "#/components/schemas/Foo");
                assert_eq!(rr.summary.as_deref(), Some("s"));
                assert_eq!(rr.description.as_deref(), Some("d"));
            }
            RefOr::Item(_) => panic!("expected Ref variant"),
        }
    }

    #[test]
    fn get_item_with_loader_external_resolves_via_loader() {
        let dir = std::env::temp_dir().join(format!(
            "roas-refor-loader-{}-{}",
            std::process::id(),
            "external"
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pets.json");
        fs::write(
            &file,
            br#"{"components":{"schemas":{"Pet":{"foo":"bar"}}}}"#,
        )
        .unwrap();

        let mut loader = Loader::new();
        loader.register_fetcher("file://", JsonFileFetcher);

        let r = RefOr::<Foo>::new_ref(format!("file://{}#/components/schemas/Pet", file.display()));
        let spec = PetSpec;
        let resolved = r.get_item_with_loader(&spec, &mut loader).unwrap();
        match resolved {
            Cow::Owned(foo) => assert_eq!(foo.foo, "bar"),
            Cow::Borrowed(_) => panic!("expected owned value from loader"),
        }

        fs::remove_file(file).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[derive(Clone, Default)]
    struct FailingFetcher;

    impl ResourceFetcher for FailingFetcher {
        fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
            Err(LoaderError::Parse {
                uri: uri.as_str().to_string(),
                source: serde_json::from_str::<Value>("not json").unwrap_err(),
            })
        }
    }

    #[test]
    fn get_item_with_loader_propagates_loader_error() {
        let mut loader = Loader::new();
        loader.register_fetcher("https://", FailingFetcher);
        let r = RefOr::<Foo>::new_ref("https://example.test/pets.json#/Foo");
        let spec = PetSpec;
        let err = r.get_item_with_loader(&spec, &mut loader).unwrap_err();
        match err {
            ResolveError::External { reference, source } => {
                assert_eq!(reference, "https://example.test/pets.json#/Foo");
                assert!(matches!(source, LoaderError::Parse { .. }));
            }
            other => panic!("expected External, got {other:?}"),
        }
    }

    #[test]
    fn get_item_with_loader_internal_uses_spec() {
        struct InlineSpec(Foo);
        impl ResolveReference<Foo> for InlineSpec {
            fn resolve_reference(&self, reference: &str) -> Option<&Foo> {
                if reference == "#/components/schemas/Foo" {
                    Some(&self.0)
                } else {
                    None
                }
            }
        }
        let mut loader = Loader::new();
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Foo");
        let spec = InlineSpec(Foo {
            foo: "from-spec".into(),
        });
        let resolved = r.get_item_with_loader(&spec, &mut loader).unwrap();
        match resolved {
            Cow::Borrowed(foo) => assert_eq!(foo.foo, "from-spec"),
            Cow::Owned(_) => panic!("expected borrowed value from spec"),
        }
    }

    #[test]
    fn get_item_with_loader_inline_item_returns_borrowed() {
        let mut loader = Loader::new();
        let inline = Foo {
            foo: "inline".into(),
        };
        let r = RefOr::<Foo>::new_item(inline);
        let spec = PetSpec;
        let resolved = r.get_item_with_loader(&spec, &mut loader).unwrap();
        match resolved {
            Cow::Borrowed(foo) => assert_eq!(foo.foo, "inline"),
            Cow::Owned(_) => panic!("inline item must come back borrowed"),
        }
    }

    #[test]
    fn get_item_with_loader_internal_ref_missing_in_spec_is_not_found() {
        let mut loader = Loader::new();
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Missing");
        let spec = PetSpec; // always returns None
        let err = r.get_item_with_loader(&spec, &mut loader).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[derive(Default)]
    struct FooSpec {
        foo: Option<Foo>,
    }

    impl ResolveReference<Foo> for FooSpec {
        fn resolve_reference(&self, reference: &str) -> Option<&Foo> {
            if reference == "#/components/schemas/Foo" {
                self.foo.as_ref()
            } else {
                None
            }
        }
    }

    impl ValidateWithContext<FooSpec> for Foo {
        fn validate_with_context(&self, ctx: &mut Context<FooSpec>, path: String) {
            if self.foo.is_empty() {
                ctx.error(path, "foo must not be empty");
            }
        }
    }

    #[test]
    fn validate_with_context_loaderless_external_ref_emits_not_supported_error() {
        let spec = FooSpec::default();
        let r = RefOr::<Foo>::new_ref("https://example.test/foo.json#/Foo");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "#.x".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not supported")
                && e.contains("https://example.test/foo.json")),
            "expected `not supported` error, got: {:?}",
            ctx.errors,
        );
    }

    #[test]
    fn validate_with_context_loaderless_external_ref_is_silenced_under_ignore() {
        let spec = FooSpec::default();
        let r = RefOr::<Foo>::new_ref("https://example.test/foo.json#/Foo");
        let mut ctx = Context::new(&spec, Options::IgnoreExternalReferences.only());
        r.validate_with_context(&mut ctx, "#.x".into());
        assert!(
            ctx.errors.is_empty(),
            "IgnoreExternalReferences must silence loader-less external errors: {:?}",
            ctx.errors,
        );
    }

    #[test]
    fn validate_with_context_internal_not_found_emits_error() {
        let spec = FooSpec::default(); // no Foo defined
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Foo");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "#.x".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not found")),
            "expected `not found` error, got: {:?}",
            ctx.errors,
        );
    }

    #[test]
    fn validate_with_context_internal_hit_recurses_into_resolved_value() {
        // Resolved Foo has an empty `foo` field, so the inner validator
        // should produce a "must not be empty" error against the
        // reference path.
        let spec = FooSpec {
            foo: Some(Foo { foo: String::new() }),
        };
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Foo");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "#.x".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "expected recursive `must not be empty` error, got: {:?}",
            ctx.errors,
        );
    }

    #[test]
    fn validate_with_context_inline_item_recurses() {
        let spec = FooSpec::default();
        let r = RefOr::<Foo>::new_item(Foo { foo: String::new() });
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "#.x".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "inline item validation must propagate, got: {:?}",
            ctx.errors,
        );
    }

    #[test]
    fn validate_with_context_visited_ref_is_not_revisited() {
        let spec = FooSpec {
            foo: Some(Foo { foo: String::new() }),
        };
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Foo");
        let mut ctx = Context::new(&spec, Options::new());
        r.validate_with_context(&mut ctx, "#.x".into());
        let first = ctx.errors.len();
        // Second visit must be skipped — the visited set has already
        // recorded the reference.
        r.validate_with_context(&mut ctx, "#.y".into());
        assert_eq!(
            ctx.errors.len(),
            first,
            "second walk of the same ref must not add new errors"
        );
    }

    #[test]
    fn ref_validate_with_context_emits_must_not_be_empty_for_empty_ref() {
        let spec = FooSpec::default();
        let r = Ref::new("");
        let mut ctx = Context::new(&spec, Options::new());
        Ref::validate_with_context::<FooSpec, Foo>(&r, &mut ctx, "#.x".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "empty `$ref` must error: {:?}",
            ctx.errors,
        );
    }
}
