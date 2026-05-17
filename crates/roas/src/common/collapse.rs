//! Shared building blocks for `Spec::collapse` across the per-version
//! type trees.
//!
//! Every version's collapse implementation needs the same machinery:
//! a canonical-JSON dedup map per component bag, a context-path-driven
//! naming scheme, and a generic "lift this inline `RefOr<T>` into a
//! component bag" routine that handles inline / internal-ref /
//! external-ref-with-loader cases uniformly. That code lives here.
//!
//! What stays per-version: the concrete `Spec` type, the per-bag
//! [`LiftableBag`] impls (which encode that version's tree recursion),
//! and the small "drive the collapser" entrypoint that owns the bags
//! and calls the generic [`lift_ref_or`] for each slot in the tree.
//!
//! Naming, dedup, error type, and loader integration are uniform
//! across versions; the trait surface only forces a version to
//! specify what *changes* — the concrete bag type, its ref prefix,
//! its tree-walking function, and an optional human-readable name
//! hint.

use std::collections::{BTreeMap, HashMap};
use std::mem;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::common::reference::{Ref, RefOr};
use crate::loader::{Loader, LoaderError};

/// Error returned by `Spec::collapse` for any OAS version.
///
/// Only fallible legs are loader-driven external-ref resolution and
/// JSON serialisation of a component for dedup; inline tree
/// rewriting itself never fails.
#[derive(Debug, thiserror::Error)]
pub enum CollapseError {
    /// The loader was invoked to resolve an external `$ref` and
    /// failed — no fetcher registered, fetch error, parse error,
    /// or missing JSON Pointer target. The underlying
    /// `LoaderError` is exposed as the error source.
    #[error("failed to resolve external reference `{reference}`")]
    External {
        reference: String,
        #[source]
        source: LoaderError,
    },

    /// A component couldn't be serialised to JSON for the dedup
    /// map. In practice every concrete component in this crate
    /// is `Serialize` so this only surfaces under custom serde
    /// error paths; it's exposed rather than panicked on so
    /// callers can decide their own fallback.
    #[error("failed to serialise component for dedup")]
    Serialize(#[from] serde_json::Error),
}

/// In-progress component bag plus its dedup map. Generic over the
/// component type so every version can reuse the same intern logic.
///
/// The bag is owned by the version's `Collapser` struct (one per
/// bag); a [`LiftableBag`] impl exposes `&mut Bag<Self>` via its
/// `bag` method so the generic [`lift_ref_or`] can intern into it.
pub struct Bag<T> {
    entries: BTreeMap<String, RefOr<T>>,
    /// Canonical-JSON-of-component → component name. Used so two
    /// structurally identical inline values collapse to the same
    /// component.
    seen: HashMap<String, String>,
}

impl<T> Default for Bag<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            seen: HashMap::new(),
        }
    }
}

impl<T> Bag<T> {
    /// True when this bag has no entries — caller uses this to skip
    /// writing back into `spec.components.<bag>` so a no-op collapse
    /// of an input without that bag doesn't materialise an empty one.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Consume the bag and yield the underlying `BTreeMap` for
    /// writing back into the spec.
    pub fn into_map(self) -> BTreeMap<String, RefOr<T>> {
        self.entries
    }
}

impl<T: Serialize> Bag<T> {
    /// Seed the bag from an existing `components.<bag>` map.
    /// Pre-existing entries keep their names; the dedup map is
    /// pre-populated so newly-lifted equivalents collapse onto
    /// them.
    pub fn seed(&mut self, initial: BTreeMap<String, RefOr<T>>) -> Result<(), CollapseError> {
        for (name, value) in initial {
            if let RefOr::Item(item) = &value {
                let canonical = serde_json::to_string(item)?;
                self.seen.entry(canonical).or_insert_with(|| name.clone());
            }
            self.entries.insert(name, value);
        }
        Ok(())
    }

    /// Insert `item` into the bag. If a structurally identical
    /// entry already exists (canonical-JSON equality), return the
    /// existing name and drop `item`. Otherwise generate a fresh
    /// name from `base` (with `_2` / `_3` suffix on collision) and
    /// insert.
    pub fn intern(&mut self, item: T, base: &str) -> Result<String, CollapseError> {
        let canonical = serde_json::to_string(&item)?;
        if let Some(existing) = self.seen.get(&canonical) {
            return Ok(existing.clone());
        }
        let name = unique_name(&self.entries, base);
        self.seen.insert(canonical, name.clone());
        self.entries.insert(name.clone(), RefOr::new_item(item));
        Ok(name)
    }

    /// Phase-2a primitives. Rather than a closure-based
    /// `recurse_existing` (which would force the closure to capture
    /// `&mut Collapser` while we already hold `&mut Bag`, creating
    /// aliasing), each version composes its phase-2a loop from
    /// three primitives: snapshot the inline keys, pull an item out
    /// by name, put it back under the same name. The walk between
    /// take and put has full access to the Collapser without
    /// aliasing because the item lives on the caller's stack.
    pub fn inline_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|(name, value)| match value {
                RefOr::Item(_) => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Take a pre-existing inline entry out of the bag. Returns
    /// `None` if the name is missing or refers to a `RefOr::Ref`
    /// (no inline body to recurse into). The `Ref` branch is
    /// unreachable when paired with [`Self::inline_names`] (which
    /// filters refs out), but if a caller bypasses that filter the
    /// ref is put back so the bag isn't left short an entry.
    pub fn take_inline(&mut self, name: &str) -> Option<T> {
        match self.entries.remove(name)? {
            RefOr::Item(item) => Some(item),
            r @ RefOr::Ref(_) => {
                self.entries.insert(name.to_owned(), r);
                None
            }
        }
    }

    /// Put an entry back under its original name and refresh the
    /// dedup map with its current canonical form (children may have
    /// been lifted, changing the canonical JSON).
    pub fn put_inline(&mut self, name: String, item: T) -> Result<(), CollapseError> {
        let canonical = serde_json::to_string(&item)?;
        self.seen.entry(canonical).or_insert_with(|| name.clone());
        self.entries.insert(name, RefOr::new_item(item));
        Ok(())
    }
}

/// Picks the first non-colliding name in `bag` starting from `base`.
/// On collision, appends `_2`, `_3`, …. Shared by every bag's intern
/// path.
pub fn unique_name<V>(bag: &BTreeMap<String, V>, base: &str) -> String {
    if !bag.contains_key(base) {
        return base.to_owned();
    }
    for i in 2..u32::MAX {
        let candidate = format!("{base}_{i}");
        if !bag.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("exhausted u32 suffixes for `{base}`");
}

/// Context-path accumulator. Carries the chain of segments through
/// the spec tree (e.g., `["getPets", "responses", "200", "content",
/// "application/json", "schema"]`) so `derive_name` can flatten it
/// into a valid component name when no [`LiftableBag::name_hint`]
/// fires.
#[derive(Clone)]
pub struct NameContext {
    parts: Vec<String>,
}

impl NameContext {
    pub fn new<I, S>(parts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            parts: parts.into_iter().map(Into::into).collect(),
        }
    }

    pub fn push(&self, part: &str) -> Self {
        let mut next = self.clone();
        next.parts.push(part.to_owned());
        next
    }

    pub fn derive_name(&self) -> String {
        sanitize_component_name(self.parts.join("_"))
    }

    /// Derive a name for a component fetched via an external
    /// `$ref`. If the reference has a JSON Pointer fragment, use
    /// the last segment (e.g., `external.json#/components/schemas/
    /// Pet` → `Pet`); else fall back to the surrounding context.
    pub fn from_external_ref(reference: &str, fallback: &NameContext) -> Self {
        if let Some((_, fragment)) = reference.split_once('#')
            && let Some(last) = fragment.rsplit('/').next()
            && !last.is_empty()
        {
            return NameContext::new([last.to_owned()]);
        }
        fallback.clone()
    }
}

/// Normalise a candidate name to OAS component-name format
/// (`^[a-zA-Z0-9.\-_]+$`). Replaces invalid chars with `_`, collapses
/// runs of `_`, trims leading/trailing `_`. Empty input falls back
/// to the literal `"Schema"`.
pub fn sanitize_component_name(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let trimmed = out.trim_matches('_').to_owned();
    if trimmed.is_empty() {
        "Schema".to_owned()
    } else {
        trimmed
    }
}

/// A reference is internal when it starts with `#` (a JSON Pointer
/// fragment-only ref like `#/components/schemas/Pet`); anything else
/// names an external resource.
pub fn is_internal_ref(reference: &str) -> bool {
    reference.starts_with('#')
}

/// Hook for the [`lift_ref_or`] generic to reach the
/// version-specific `Collapser`'s loader. Versions implement this
/// with a one-liner — `self.loader.as_deref_mut()`.
pub trait HasLoader {
    fn loader_mut(&mut self) -> Option<&mut Loader>;
}

/// A component type that participates in collapse. Each version
/// implements this for each concrete component type it has
/// (`Schema`, `Parameter`, `Response`, …), against its own
/// `Collapser` struct. The trait surface only forces the version
/// to spell out what's *different* per type:
///
/// * `PREFIX`: the `#/components/<bag>/` prefix used to build refs.
/// * `bag`: how to reach this type's bag inside the Collapser.
/// * `walk`: the per-type tree recursion (call [`lift_ref_or`] on
///   every nested component slot).
/// * `name_hint` (optional): a per-type human-readable name hint
///   (e.g. a `Parameter`'s `<name><In>` or a `Schema`'s `title`).
///
/// The generic [`lift_ref_or`] uses this trait to perform the
/// uniform inline / internal-ref / external-ref-with-loader logic
/// against any concrete component type.
// `Clone` is required transitively by `Loader::resolve_reference_as<T>`,
// which clones cached values out of its typed cache — see `lift_ref_or`.
pub trait LiftableBag<C>: Sized + Clone + Serialize + DeserializeOwned + 'static {
    /// The `#/components/<bag>/` prefix. Used to build internal
    /// `$ref` targets.
    const PREFIX: &'static str;

    /// Borrow this type's bag mutably out of the Collapser.
    fn bag(c: &mut C) -> &mut Bag<Self>;

    /// Walk into an instance, lifting every nested component slot.
    /// After this returns, the instance is ready for canonical-JSON
    /// dedup (its children are refs, not inline values).
    fn walk(item: &mut Self, ctx: &NameContext, c: &mut C) -> Result<(), CollapseError>;

    /// Optional human-readable name hint. Returning `Some(name)`
    /// shadows the context-derived name on first-seen interning.
    /// Default: `None`.
    fn name_hint(_item: &Self) -> Option<String> {
        None
    }
}

/// The "lift one `RefOr<T>` into the bag of T" generic. Used by
/// every per-version walker — each `lift_ref_or_T` collapses to a
/// single call here, regardless of T.
///
/// Handles:
/// * `RefOr::Ref` with an internal ref (`#/...`): no-op.
/// * `RefOr::Ref` with an external ref and a loader: fetch + recurse
///   + intern + rewrite the slot to a local ref.
/// * `RefOr::Ref` with an external ref and no loader: no-op.
/// * `RefOr::Item`: recurse + intern + rewrite the slot to a local
///   ref.
pub fn lift_ref_or<T, C>(
    slot: &mut RefOr<T>,
    ctx: NameContext,
    c: &mut C,
) -> Result<(), CollapseError>
where
    T: LiftableBag<C>,
    C: HasLoader,
{
    match slot {
        RefOr::Ref(r) => {
            if is_internal_ref(&r.reference) {
                return Ok(());
            }
            let reference = r.reference.clone();
            let Some(loader) = c.loader_mut() else {
                return Ok(());
            };
            let mut fetched: T = loader.resolve_reference_as(&reference).map_err(|source| {
                CollapseError::External {
                    reference: reference.clone(),
                    source,
                }
            })?;
            let derived_ctx = NameContext::from_external_ref(&reference, &ctx);
            T::walk(&mut fetched, &derived_ctx, c)?;
            let name = intern(c, fetched, &derived_ctx)?;
            *slot = RefOr::new_ref(format!("{}{name}", T::PREFIX));
            Ok(())
        }
        RefOr::Item(_) => {
            // Take ownership out of the slot so we can recurse +
            // intern without aliasing.
            let placeholder = RefOr::Ref(Ref::new(String::new()));
            let owned = mem::replace(slot, placeholder);
            let RefOr::Item(mut item) = owned else {
                unreachable!("matched RefOr::Item above");
            };
            T::walk(&mut item, &ctx, c)?;
            let name = intern(c, item, &ctx)?;
            *slot = RefOr::new_ref(format!("{}{name}", T::PREFIX));
            Ok(())
        }
    }
}

fn intern<T, C>(c: &mut C, item: T, ctx: &NameContext) -> Result<String, CollapseError>
where
    T: LiftableBag<C>,
{
    let base = match T::name_hint(&item) {
        Some(h) if !h.is_empty() => sanitize_component_name(h),
        _ => ctx.derive_name(),
    };
    T::bag(c).intern(item, &base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_name_appends_suffix_against_existing_keys() {
        let mut bag: BTreeMap<String, ()> = BTreeMap::new();
        assert_eq!(unique_name(&bag, "foo"), "foo");
        bag.insert("foo".to_owned(), ());
        assert_eq!(unique_name(&bag, "foo"), "foo_2");
        bag.insert("foo_2".to_owned(), ());
        assert_eq!(unique_name(&bag, "foo"), "foo_3");
    }

    #[test]
    fn sanitize_component_name_handles_edge_cases() {
        assert_eq!(sanitize_component_name("Pet"), "Pet");
        assert_eq!(
            sanitize_component_name("paths./pets[0].schema"),
            "paths._pets_0_.schema"
        );
        assert_eq!(sanitize_component_name("/foo/"), "foo");
        assert_eq!(sanitize_component_name("Hello World"), "Hello_World");
        assert_eq!(sanitize_component_name("///"), "Schema");
        assert_eq!(sanitize_component_name(""), "Schema");
    }

    #[test]
    fn name_context_from_external_ref_uses_last_pointer_segment() {
        let fallback = NameContext::new(["fallback"]);
        let ctx =
            NameContext::from_external_ref("external.json#/components/schemas/Pet", &fallback);
        assert_eq!(ctx.derive_name(), "Pet");
    }

    #[test]
    fn name_context_from_external_ref_falls_back_on_empty_fragment() {
        let fallback = NameContext::new(["fallback"]);
        let ctx = NameContext::from_external_ref("external.json", &fallback);
        assert_eq!(ctx.derive_name(), "fallback");
        let ctx = NameContext::from_external_ref("external.json#", &fallback);
        assert_eq!(ctx.derive_name(), "fallback");
        let ctx = NameContext::from_external_ref("external.json#/", &fallback);
        assert_eq!(ctx.derive_name(), "fallback");
    }

    #[test]
    fn is_internal_ref_routes_by_leading_hash() {
        assert!(is_internal_ref("#/components/schemas/Pet"));
        assert!(!is_internal_ref("external.json#/Pet"));
        assert!(!is_internal_ref("https://example.com/spec#/Pet"));
    }
}
