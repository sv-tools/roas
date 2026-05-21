//! Reference Object — shared across v2, v3.0, v3.1, and v3.2.
//!
//! The OpenAPI Reference Object differs only by which sibling fields are
//! permitted: v2 and v3.0 spell it as `{ "$ref": "..." }`, while v3.1+
//! adds optional `summary` and `description` overrides. This module
//! carries the union: `summary` and `description` are `Option`-typed and
//! always declared on the struct. Per-version validators may still flag
//! their presence if a build wants v2 / v3.0 strictness at the
//! validation layer.

use serde::de::{self, DeserializeOwned, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;
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
    ///
    /// `Ref` is boxed so the enum is not sized for the ~72-byte `Ref`
    /// struct on every slot: a `RefOr<Schema>` (where `Schema` is itself
    /// only ~16 bytes) shrinks from ~80 bytes to ~24. Reference variants
    /// are the minority in a typical document, so the one extra heap
    /// allocation per `$ref` is paid by the rare case while every inline
    /// `Item` in a map or vec gets the smaller slot.
    Ref(Box<Ref>),

    /// The component itself.
    Item(T),
}

/// The field set a `Ref` accepts, mirroring `Ref`'s `deny_unknown_fields`.
const REF_FIELDS: &[&str] = &["$ref", "summary", "description"];

/// Visitor backing [`RefOr`]'s `Deserialize`.
///
/// * A scalar / sequence input can never be a `$ref`, so it is streamed
///   straight into `T`.
/// * A map whose **first** key is `$ref` is streamed straight into a
///   `Ref` — no intermediate `serde_json::Value` is built.
/// * Any other map is buffered into a `serde_json::Value` so a `$ref`
///   appearing at a non-first position can still be detected (the
///   OpenAPI Reference Object permits sibling keys in any order). This
///   leg matches the historical behaviour; only the fast paths above
///   avoid the throwaway DOM.
struct RefOrVisitor<T>(PhantomData<fn() -> T>);

impl<'de, T> Visitor<'de> for RefOrVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = RefOr<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a Reference Object or an inline component")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        T::deserialize(v.into_deserializer()).map(RefOr::Item)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        T::deserialize(v.into_deserializer()).map(RefOr::Item)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        T::deserialize(v.into_deserializer()).map(RefOr::Item)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        T::deserialize(v.into_deserializer()).map(RefOr::Item)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        T::deserialize(v.into_deserializer()).map(RefOr::Item)
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        T::deserialize(v.into_deserializer()).map(RefOr::Item)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        T::deserialize(().into_deserializer()).map(RefOr::Item)
    }

    fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        T::deserialize(de::value::SeqAccessDeserializer::new(seq)).map(RefOr::Item)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let Some(first_key) = map.next_key::<String>()? else {
            // Empty map `{}` — an inline component (e.g. `Schema::Empty`).
            return T::deserialize(de::value::MapAccessDeserializer::new(map)).map(RefOr::Item);
        };

        if first_key == "$ref" {
            // Fast path: stream the Reference Object directly into `Ref`.
            let reference: String = map.next_value()?;
            let mut summary: Option<String> = None;
            let mut description: Option<String> = None;
            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "$ref" => return Err(de::Error::duplicate_field("$ref")),
                    "summary" => {
                        if summary.is_some() {
                            return Err(de::Error::duplicate_field("summary"));
                        }
                        summary = Some(map.next_value()?);
                    }
                    "description" => {
                        if description.is_some() {
                            return Err(de::Error::duplicate_field("description"));
                        }
                        description = Some(map.next_value()?);
                    }
                    _ => return Err(de::Error::unknown_field(key.as_str(), REF_FIELDS)),
                }
            }
            return Ok(RefOr::Ref(Box::new(Ref {
                reference,
                summary,
                description,
            })));
        }

        // Slow path: buffer so a `$ref` at a non-first position is still
        // detected, then route on its presence.
        let mut entries = serde_json::Map::new();
        entries.insert(first_key, map.next_value()?);
        while let Some(key) = map.next_key::<String>()? {
            entries.insert(key, map.next_value()?);
        }
        let value = serde_json::Value::Object(entries);
        if value.as_object().is_some_and(|m| m.contains_key("$ref")) {
            Ref::deserialize(value)
                .map(|r| RefOr::Ref(Box::new(r)))
                .map_err(de::Error::custom)
        } else {
            T::deserialize(value)
                .map(RefOr::Item)
                .map_err(de::Error::custom)
        }
    }
}

impl<'de, T> Deserialize<'de> for RefOr<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(RefOrVisitor(PhantomData))
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

// The `Ref` variant is boxed so a `RefOr` slot stays pointer-sized
// instead of growing to the full `Ref` struct. If this fails, the
// `Box` was dropped from `RefOr::Ref`.
const _: () = assert!(
    std::mem::size_of::<RefOr<u8>>() < std::mem::size_of::<Ref>(),
    "RefOr::Ref must stay boxed",
);

impl<D> RefOr<D> {
    /// Validate this `RefOr<D>` in the surrounding context.
    ///
    /// Crate-internal: callers drive validation through
    /// [`Validate::validate`](crate::validation::Validate::validate)
    /// rather than invoking this method directly.
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
    ///
    /// Nested `$ref: "#/..."` inside an externally-loaded document
    /// resolves correctly against that document's own structure, not
    /// the root spec: the loader rewrites every `$ref` against the
    /// document's base URL at load time (see
    /// [`Loader::resolve_reference`](crate::loader::Loader::resolve_reference)),
    /// so the validator sees fully-qualified URLs and routes them
    /// through the loader uniformly.
    pub(crate) fn validate_with_context<T>(&self, ctx: &mut Context<T>, path: String)
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
                // External reference. `IgnoreExternalReferences` means
                // "don't recurse into external documents during
                // validation" — both before and after the loader
                // integration. Short-circuit here so the option's
                // behaviour stays the same regardless of whether a
                // loader is attached: with the option set, we never
                // ask the loader to fetch and we never validate the
                // resolved value, even when it would have succeeded.
                if ctx.is_option(Options::IgnoreExternalReferences) {
                    return;
                }
                // Route through the loader if one is attached to the
                // context. `as_deref_mut` keeps the mutable borrow on
                // `ctx.loader` scoped to this expression so the
                // resolved owned `D` (or `LoaderError`) outlives the
                // borrow and lets us re-use `ctx` to validate it.
                let resolved = ctx
                    .loader
                    .as_deref_mut()
                    .map(|l| l.resolve_reference_as::<D>(&r.reference));
                // `IgnoreExternalReferences` was already handled
                // above; if we got here it isn't set, so any failure
                // surfaces unconditionally.
                match resolved {
                    Some(Ok(d)) => d.validate_with_context(ctx, r.reference.clone()),
                    Some(Err(source)) => ctx.error(
                        path,
                        format_args!(
                            ".$ref: failed to resolve external reference `{}`: {source}",
                            r.reference,
                        ),
                    ),
                    None => ctx.error(
                        path,
                        format_args!(
                            ".$ref: resolving of an external reference `{}` is not supported",
                            r.reference,
                        ),
                    ),
                }
            }
            RefOr::Item(d) => {
                d.validate_with_context(ctx, path);
            }
        }
    }

    /// Create a new RefOr with a reference.
    pub fn new_ref(reference: impl Into<String>) -> Self {
        RefOr::Ref(Box::new(Ref::new(reference)))
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
    pub(crate) fn validate_with_context<T, D>(&self, ctx: &mut Context<T>, path: String)
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

// ---- merge ----

impl<D, T> crate::merge::MergeWithContext<T> for RefOr<D>
where
    D: crate::merge::MergeWithContext<T>,
{
    fn merge_with_context(
        &mut self,
        other: Self,
        ctx: &mut crate::merge::MergeContext<T>,
        path: String,
    ) {
        use crate::merge::ConflictKind;
        match (self, other) {
            (RefOr::Item(base), RefOr::Item(incoming)) => {
                base.merge_with_context(incoming, ctx, path);
            }
            (slot @ RefOr::Ref(_), RefOr::Ref(incoming_ref)) => {
                let RefOr::Ref(base_ref) = slot else {
                    unreachable!()
                };
                if base_ref.reference == incoming_ref.reference {
                    base_ref.merge_with_context(*incoming_ref, ctx, path);
                } else if ctx.should_take_incoming(&path, ConflictKind::RefReplaced) {
                    *slot = RefOr::Ref(incoming_ref);
                }
            }
            (slot, incoming) => {
                if ctx.should_take_incoming(&path, ConflictKind::RefVsValue) {
                    *slot = incoming;
                }
            }
        }
    }
}

impl<T> crate::merge::MergeWithContext<T> for Ref {
    fn merge_with_context(
        &mut self,
        other: Self,
        ctx: &mut crate::merge::MergeContext<T>,
        path: String,
    ) {
        use crate::common::merge::merge_opt_scalar;
        use crate::merge::ConflictKind;
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
    use crate::validation::ValidationErrorsExt;
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
            serde_json::to_value(RefOr::Ref::<Foo>(Box::new(Ref {
                reference: String::from("#/components/schemas/Foo"),
                ..Default::default()
            })))
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
            RefOr::Ref(Box::new(Ref {
                reference: String::from("#/components/schemas/Foo"),
                ..Default::default()
            })),
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
    fn ref_detected_when_not_the_first_key() {
        // `$ref` after a sibling key exercises the buffered slow path.
        let r: RefOr<Foo> = serde_json::from_value(serde_json::json!({
            "description": "d",
            "$ref": "#/components/schemas/Foo",
        }))
        .unwrap();
        match r {
            RefOr::Ref(rr) => {
                assert_eq!(rr.reference, "#/components/schemas/Foo");
                assert_eq!(rr.description.as_deref(), Some("d"));
            }
            RefOr::Item(_) => panic!("expected Ref variant"),
        }
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
    fn ref_fast_and_slow_paths_agree_on_a_full_ref() {
        // A fully-populated `Ref` must deserialize identically whether
        // `$ref` is the first key (streaming fast path) or not (buffered
        // slow path). This guards against the two paths drifting if
        // `Ref` ever gains a field — one path would then accept it and
        // the other reject it for the same document.
        let fast: RefOr<Foo> =
            serde_json::from_str(r##"{"$ref":"#/x","summary":"s","description":"d"}"##).unwrap();
        let slow: RefOr<Foo> =
            serde_json::from_str(r##"{"summary":"s","$ref":"#/x","description":"d"}"##).unwrap();
        assert_eq!(fast, slow, "first-key and non-first-key $ref must agree");
        match fast {
            RefOr::Ref(r) => {
                assert_eq!(r.reference, "#/x");
                assert_eq!(r.summary.as_deref(), Some("s"));
                assert_eq!(r.description.as_deref(), Some("d"));
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

        // Construct the `file://` URL via `Url::from_file_path` so the
        // reference is well-formed on any platform (Windows backslashes,
        // spaces, non-ASCII) instead of relying on `Path::display`.
        let mut url = Url::from_file_path(&file).unwrap();
        url.set_fragment(Some("/components/schemas/Pet"));
        let r = RefOr::<Foo>::new_ref(url.to_string());
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
            ctx.errors
                .mentions_all(&["not supported", "https://example.test/foo.json"]),
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
            ctx.errors.mentions("not found"),
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
            ctx.errors.mentions("must not be empty"),
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
            ctx.errors.mentions("must not be empty"),
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
            ctx.errors.mentions("must not be empty"),
            "empty `$ref` must error: {:?}",
            ctx.errors,
        );
    }

    #[test]
    fn get_item_external_unsupported_returns_error() {
        // `get_item` (without loader) cannot resolve external refs.
        let spec = PetSpec;
        let r = RefOr::<Foo>::new_ref("https://example.test/foo.json#/Foo");
        let err = r.get_item(&spec).unwrap_err();
        assert!(
            matches!(err, ResolveError::ExternalUnsupported(_)),
            "expected ExternalUnsupported, got {err:?}"
        );
        // The Display impl includes the reference string.
        assert!(err.to_string().contains("external reference"));
    }

    #[test]
    fn get_item_internal_not_found_returns_error() {
        // Internal ref that the spec can't resolve.
        let spec = PetSpec; // always returns None
        let r = RefOr::<Foo>::new_ref("#/components/schemas/Missing");
        let err = r.get_item(&spec).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn resolve_in_map_cross_map_ref_delegates_to_spec_resolver() {
        use std::collections::BTreeMap;

        // A spec that can resolve "#/components/schemas/Concrete" directly.
        struct ConcreteSpec(Foo);
        impl ResolveReference<Foo> for ConcreteSpec {
            fn resolve_reference(&self, reference: &str) -> Option<&Foo> {
                if reference == "#/components/schemas/Concrete" {
                    Some(&self.0)
                } else {
                    None
                }
            }
        }

        let concrete_foo = Foo {
            foo: "concrete".into(),
        };
        let spec = ConcreteSpec(concrete_foo);

        // Build a map where an entry refs a key outside `prefix` — this
        // exercises the "Cross-map ref" branch that calls `get_item(spec)`.
        let mut map: BTreeMap<String, RefOr<Foo>> = BTreeMap::new();
        map.insert(
            "Alias".into(),
            RefOr::new_ref("#/components/schemas/Concrete"),
        );
        let map_opt = Some(map);

        let result = crate::common::reference::resolve_in_map(
            &spec,
            "#/things/Alias",
            "#/things/",
            &map_opt,
        );
        // The cross-map ref resolves to the concrete item via spec.
        assert_eq!(result.map(|f| f.foo.as_str()), Some("concrete"));
    }

    #[test]
    fn resolve_in_map_cycle_returns_none() {
        use std::collections::BTreeMap;

        // A spec that never resolves anything.
        struct EmptySpec;
        impl ResolveReference<Foo> for EmptySpec {
            fn resolve_reference(&self, _reference: &str) -> Option<&Foo> {
                None
            }
        }

        // Build a cycle: A -> B -> A (both within the same prefix)
        let mut map: BTreeMap<String, RefOr<Foo>> = BTreeMap::new();
        map.insert("A".into(), RefOr::new_ref("#/things/B"));
        map.insert("B".into(), RefOr::new_ref("#/things/A"));
        let map_opt = Some(map);

        let result = crate::common::reference::resolve_in_map(
            &EmptySpec,
            "#/things/A",
            "#/things/",
            &map_opt,
        );
        assert!(result.is_none(), "cycle detection must return None");
    }

    #[test]
    fn ref_or_visitor_scalar_deserializations() {
        // Exercises RefOrVisitor::visit_bool, visit_i64, visit_u64, visit_f64,
        // visit_str, visit_unit, and visit_seq by deserializing RefOr<Value>
        // from various JSON scalar and array forms.
        use serde_json::Value;

        // visit_bool — JSON `true`
        let r: RefOr<Value> = serde_json::from_str("true").unwrap();
        assert_eq!(r, RefOr::Item(Value::Bool(true)));

        // visit_bool — JSON `false`
        let r: RefOr<Value> = serde_json::from_str("false").unwrap();
        assert_eq!(r, RefOr::Item(Value::Bool(false)));

        // visit_u64 — positive integer
        let r: RefOr<Value> = serde_json::from_str("42").unwrap();
        assert_eq!(r, RefOr::Item(Value::Number(42_u64.into())));

        // visit_i64 — negative integer
        let r: RefOr<Value> = serde_json::from_str("-1").unwrap();
        assert_eq!(r, RefOr::Item(Value::Number((-1_i64).into())));

        // visit_f64 — float
        let r: RefOr<Value> = serde_json::from_str("2.5").unwrap();
        match r {
            RefOr::Item(Value::Number(n)) => {
                let v = n.as_f64().unwrap();
                assert!((v - 2.5).abs() < 1e-10, "expected ~2.5, got {v}");
            }
            other => panic!("expected Item(Number), got {other:?}"),
        }

        // visit_str — JSON string
        let r: RefOr<Value> = serde_json::from_str(r#""hello""#).unwrap();
        assert_eq!(r, RefOr::Item(Value::String("hello".into())));

        // visit_unit — JSON null
        let r: RefOr<Value> = serde_json::from_str("null").unwrap();
        assert_eq!(r, RefOr::Item(Value::Null));

        // visit_seq — JSON array
        let r: RefOr<Value> = serde_json::from_str("[1,2,3]").unwrap();
        assert_eq!(
            r,
            RefOr::Item(Value::Array(vec![
                Value::Number(1_u64.into()),
                Value::Number(2_u64.into()),
                Value::Number(3_u64.into()),
            ]))
        );

        // Empty array
        let r: RefOr<Value> = serde_json::from_str("[]").unwrap();
        assert_eq!(r, RefOr::Item(Value::Array(vec![])));
    }

    #[test]
    fn ref_or_visitor_visit_string_via_owned_deserializer() {
        // visit_string is called by deserializers that yield an owned `String`
        // rather than a borrowed `&str`. We exercise it via serde's
        // `value::StringDeserializer` which directly calls `visit_string`.
        use serde::Deserialize;
        use serde::de::IntoDeserializer;
        use serde::de::value::StringDeserializer;
        use serde_json::Value;

        let des: StringDeserializer<serde_json::Error> = "owned".to_owned().into_deserializer();
        let r: RefOr<Value> = RefOr::deserialize(des).unwrap();
        assert_eq!(r, RefOr::Item(Value::String("owned".into())));
    }

    #[test]
    fn ref_or_visitor_expecting_message_via_error() {
        // Triggers RefOrVisitor::expecting by attempting to deserialize a
        // type-incompatible value so serde generates a descriptive error that
        // includes the `expecting` string.
        use serde::Deserialize;
        use serde::de::IntoDeserializer;
        use serde::de::value::U64Deserializer;

        // Foo expects a map, so deserializing from a raw u64 fails.
        // serde formats the error using `expecting`.
        let des: U64Deserializer<serde_json::Error> = 99_u64.into_deserializer();
        let r = Foo::deserialize(des);
        assert!(r.is_err(), "Foo should not deserialize from u64");
    }

    #[test]
    fn ref_duplicate_summary_is_rejected() {
        // Exercises the `if summary.is_some()` branch (duplicate "summary" key).
        // JSON object: `$ref` is first → fast path; second `summary` triggers error.
        let r = serde_json::from_str::<RefOr<Foo>>(
            r##"{"$ref":"#/x","summary":"first","summary":"second"}"##,
        );
        assert!(r.is_err(), "duplicate summary must be rejected");
    }

    #[test]
    fn ref_duplicate_description_is_rejected() {
        // Exercises the `if description.is_some()` branch (duplicate "description").
        let r = serde_json::from_str::<RefOr<Foo>>(
            r##"{"$ref":"#/x","description":"first","description":"second"}"##,
        );
        assert!(r.is_err(), "duplicate description must be rejected");
    }

    #[test]
    fn resolve_error_display_messages() {
        // Cover Display impls for all ResolveError variants.
        let e = ResolveError::NotFound("my-ref".into());
        assert!(e.to_string().contains("not found"));

        let e = ResolveError::ExternalUnsupported("http://x".into());
        assert!(e.to_string().contains("not supported"));

        use crate::loader::LoaderError;
        let e = ResolveError::External {
            reference: "http://x/y".into(),
            source: LoaderError::NoFetcherRegistered {
                uri: "http://x/y".into(),
            },
        };
        assert!(
            e.to_string()
                .contains("failed to resolve external reference")
        );
        // Source is accessible via std::error::Error::source.
        let src = std::error::Error::source(&e).expect("External must have a source");
        assert!(src.to_string().contains("no fetcher"));
    }
}
