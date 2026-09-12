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
//!
//! Which inline schemas are *worth* lifting is decided here too, in
//! [`schema_lift_decision`], and applies to every version at once:
//! a bare `{"type": "string"}` gains nothing from a machine-derived
//! name and a `$ref`, so it stays where it is. See
//! [`LiftDecision`] for the rule.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::mem;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::common::reference::{Ref, RefOr, ReferenceObject};
use crate::loader::{Loader, LoaderError};

/// Error returned by `Spec::collapse` for any OAS version.
///
/// Only fallible legs are loader-driven external-ref resolution and
/// JSON serialization of a component for dedup; inline tree
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

    /// A component couldn't be serialized to JSON for the dedup
    /// map. In practice every concrete component in this crate
    /// is `Serialize` so this only surfaces under custom serde
    /// error paths; it's exposed rather than panicked on so
    /// callers can decide their own fallback.
    #[error("failed to serialize component for dedup")]
    Serialize(#[from] serde_json::Error),
}

/// In-progress component bag plus its dedup map. Generic over the
/// component type so every version can reuse the same intern logic.
///
/// The bag is owned by the version's `Collapser` struct (one per
/// bag); a [`LiftableBag`] impl exposes `&mut Bag<Self>` via its
/// `bag` method so the generic [`lift_ref_or`] can intern into it.
pub struct Bag<T, R = Ref> {
    entries: BTreeMap<String, RefOr<T, R>>,
    /// Digest of a component's canonical JSON → candidate component
    /// names. Storing a 64-bit digest instead of the full canonical
    /// JSON keeps the dedup map small regardless of component size;
    /// the (astronomically rare) digest collision is resolved by
    /// re-serializing each candidate and comparing bytes, so two
    /// structurally identical inline values still collapse to the
    /// same component without ever trusting the digest alone.
    seen: HashMap<u64, Vec<String>>,
}

/// 64-bit digest of a component's canonical JSON, used as the dedup
/// key in [`Bag::seen`].
fn digest(canonical: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.hash(&mut hasher);
    hasher.finish()
}

impl<T, R> Default for Bag<T, R> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            seen: HashMap::new(),
        }
    }
}

impl<T, R> Bag<T, R> {
    /// True when this bag has no entries — caller uses this to skip
    /// writing back into `spec.components.<bag>` so a no-op collapse
    /// of an input without that bag doesn't materialise an empty one.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Consume the bag and yield the underlying `BTreeMap` for
    /// writing back into the spec.
    pub fn into_map(self) -> BTreeMap<String, RefOr<T, R>> {
        self.entries
    }
}

impl<T: Serialize, R: ReferenceObject> Bag<T, R> {
    /// Seed the bag from an existing `components.<bag>` map.
    /// Pre-existing entries keep their names; the dedup map is
    /// pre-populated so newly-lifted equivalents collapse onto
    /// them.
    pub fn seed(&mut self, initial: BTreeMap<String, RefOr<T, R>>) -> Result<(), CollapseError> {
        for (name, value) in initial {
            if let RefOr::Item(item) = &value {
                let d = digest(&serde_json::to_string(item)?);
                self.seen.entry(d).or_default().push(name.clone());
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
        let d = digest(&canonical);
        if let Some(candidates) = self.seen.get(&d) {
            // A digest hit is almost always a real match; confirm by
            // re-serializing the candidate so a digest collision can
            // never silently merge two distinct components.
            for name in candidates {
                if let Some(RefOr::Item(existing)) = self.entries.get(name)
                    && serde_json::to_string(existing)? == canonical
                {
                    return Ok(name.clone());
                }
            }
        }
        let name = unique_name(&self.entries, base);
        self.seen.entry(d).or_default().push(name.clone());
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
        let d = digest(&serde_json::to_string(&item)?);
        let candidates = self.seen.entry(d).or_default();
        if !candidates.contains(&name) {
            candidates.push(name.clone());
        }
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
///
/// Stored as a shared leaf-to-root cons-list so [`Self::push`] — called
/// once per node on a traversal-heavy collapse — is O(1) (one small
/// allocation plus a refcount bump) instead of cloning the whole
/// segment vector at every descent.
#[derive(Clone)]
pub struct NameContext {
    node: Option<Rc<NameNode>>,
}

struct NameNode {
    part: String,
    parent: Option<Rc<NameNode>>,
}

impl NameContext {
    pub fn new<I, S>(parts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut ctx = NameContext { node: None };
        for part in parts {
            ctx = ctx.push_owned(part.into());
        }
        ctx
    }

    fn push_owned(&self, part: String) -> Self {
        NameContext {
            node: Some(Rc::new(NameNode {
                part,
                parent: self.node.clone(),
            })),
        }
    }

    pub fn push(&self, part: &str) -> Self {
        self.push_owned(part.to_owned())
    }

    /// Collect the path segments in root-to-leaf order.
    fn segments(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut cur = self.node.as_deref();
        while let Some(node) = cur {
            out.push(node.part.as_str());
            cur = node.parent.as_deref();
        }
        out.reverse();
        out
    }

    pub fn derive_name(&self) -> String {
        sanitize_component_name(self.segments().join("_"))
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

/// What [`lift_ref_or`] should do with one inline schema.
///
/// Collapse exists to give reusable structures a name and a `$ref`.
/// A schema that is nothing but its type earns neither: replacing
/// `{"type": "string"}` with a `$ref` to a generated
/// `components_schemas_Order_properties.quantity` makes the document
/// bigger and harder to read. So inline schemas are sorted into
/// three classes, uniformly for v2 / v3.0 / v3.1 / v3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftDecision {
    /// Structural shapes — objects, `allOf` / `anyOf` / `oneOf` /
    /// `not`, and anything carrying `enum` values. These are what a
    /// name and a `$ref` genuinely serve, so they always lift.
    ///
    /// A `title` counts as structural too, for a different reason:
    /// it is the one annotation collapse turns into the component
    /// name (see `LiftableBag::name_hint`). A titled schema gets the
    /// author's own name rather than a generated one, so lifting it
    /// produces `components.schemas.EmailAddress`, not
    /// `components_schemas_User_properties.email`.
    Always,

    /// A schema carrying nothing beyond its type plus pure
    /// annotations (`description`, `example`, `examples`,
    /// `deprecated`, `readOnly`, `writeOnly`), plus the boolean and
    /// empty schemas. Never lifted — inline *is* the readable form.
    Never,

    /// Everything in between: constrained scalars (`format`,
    /// `pattern`, `maxLength`, `default`, `xml`, …) and thin array
    /// wrappers. More than a bare type, but a generated name for a
    /// single-use one is still noise — so these lift only when the
    /// identical schema occurs more than once in the document, where
    /// dedup pays for the name.
    IfRepeated,
}

/// Keys that make a schema structural: worth a name of its own.
const STRUCTURAL_KEYS: &[&str] = &[
    // Not structure, but an author-given name: see `LiftDecision`.
    "title",
    "properties",
    "patternProperties",
    "additionalProperties",
    "unevaluatedProperties",
    "propertyNames",
    "required",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "enum",
    "discriminator",
];

/// Keys that describe a schema without constraining it. A schema
/// built only from `type` plus these is a bare type with prose
/// attached.
const ANNOTATION_KEYS: &[&str] = &[
    "description",
    "example",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// Sort one schema, in its serialized form, into a [`LiftDecision`].
///
/// Works on JSON rather than on a version's `Schema` enum so the
/// rule is written once and every version inherits it — the four
/// type trees differ, the JSON keys they emit do not.
pub fn schema_lift_decision(value: &serde_json::Value) -> LiftDecision {
    // A boolean schema (`true` / `false`) or the empty schema `{}`
    // has nothing to name.
    let Some(map) = value.as_object() else {
        return LiftDecision::Never;
    };
    if map.is_empty() {
        return LiftDecision::Never;
    }
    if map.keys().any(|k| STRUCTURAL_KEYS.contains(&k.as_str())) {
        return LiftDecision::Always;
    }
    if map
        .keys()
        .all(|k| k == "type" || ANNOTATION_KEYS.contains(&k.as_str()))
    {
        return LiftDecision::Never;
    }
    LiftDecision::IfRepeated
}

/// How many document slots hold each [`LiftDecision::IfRepeated`]
/// shape.
///
/// Collapse runs twice: a census pass over a throwaway clone of the
/// spec fills this in, then the real pass consults it. Both passes
/// are the *same* walk, so a shape is counted at exactly the slots
/// that could lift it — no separate notion of "where a schema might
/// live", and nothing counted from an `example` payload or an
/// extension that merely resembles a schema. Two slots that never
/// lift are counted anyway, because an inline schema matching
/// either of them can collapse onto an already-named component: a
/// schema reached through an external `$ref`, and a pre-existing
/// `components.<bag>` entry.
///
/// Slots are keyed by a 64-bit digest of the schema's canonical JSON
/// *before* its children are walked — the form both passes see at
/// that slot, since a subtree is only rewritten after its own slot
/// has been weighed. A digest collision can only make a single-use
/// schema lift, which is what this crate did for every schema until
/// 0.20, so the digest is trusted here without a confirmation pass
/// (unlike [`Bag::seen`], where a collision would merge two
/// components).
#[derive(Default)]
pub struct SchemaRepeats {
    counts: HashMap<u64, u32>,
    /// True during the census pass: [`Self::weigh`] records a slot
    /// and reports "not repeated" so the census walk rewrites the
    /// clone exactly as the real pass will rewrite the spec.
    counting: bool,
}

impl SchemaRepeats {
    /// Start a census pass. Feed the result of [`Self::finish`] to
    /// the real pass.
    pub fn census() -> Self {
        Self {
            counts: HashMap::new(),
            counting: true,
        }
    }

    /// Close the census and return the counts for the real pass.
    pub fn finish(self) -> Self {
        Self {
            counts: self.counts,
            counting: false,
        }
    }

    /// Weigh one repeat-eligible slot: during the census, record it
    /// and answer "not repeated"; afterwards, answer whether more
    /// than one slot held this shape.
    fn weigh(&mut self, shape: u64) -> bool {
        if self.counting {
            self.note(shape);
            return false;
        }
        self.counts.get(&shape).is_some_and(|n| *n > 1)
    }

    /// Record a slot without asking about it — used for schemas
    /// pulled in through an external `$ref`, which lift either way
    /// but still make an identical inline schema worth lifting onto.
    fn note(&mut self, shape: u64) {
        if self.counting {
            *self.counts.entry(shape).or_default() += 1;
        }
    }
}

/// Hook for the [`lift_ref_or`] generic to reach the
/// version-specific `Collapser`'s loader and its slot counts.
/// Versions implement this with two one-liners.
pub trait CollapseState {
    fn loader_mut(&mut self) -> Option<&mut Loader>;

    fn schema_repeats(&mut self) -> &mut SchemaRepeats;
}

/// A component type that participates in collapse. Each version
/// implements this for each concrete component type it has
/// (`Schema`, `Parameter`, `Response`, …), against its own
/// `Collapser` struct. The trait surface only forces the version
/// to spell out what's *different* per type:
///
/// * `PREFIX`: the `#/components/<bag>/` prefix used to build refs.
/// * `IS_SCHEMA`: whether the triviality filter applies to this bag.
/// * `bag`: how to reach this type's bag inside the Collapser.
/// * `walk`: the per-type tree recursion (call [`lift_ref_or`] on
///   every nested component slot).
/// * `name_hint` (optional): a per-type human-readable name hint
///   (e.g. a `Parameter`'s `<name><In>` or a `Schema`'s `title`).
///
/// The generic [`lift_ref_or`] uses this trait to perform the
/// uniform inline / internal-ref / external-ref-with-loader logic
/// against any concrete component type.
///
/// The second parameter is the reference payload of the slots this type
/// lives in — [`Ref`] for every component except the 3.1 / 3.2 schema,
/// whose `$ref` may carry sibling keywords.
pub trait LiftableBag<C, R: ReferenceObject = Ref>:
    Sized + Serialize + DeserializeOwned + 'static
{
    /// The `#/components/<bag>/` prefix. Used to build internal
    /// `$ref` targets.
    const PREFIX: &'static str;

    /// Whether this bag holds schemas. Only schemas go through the
    /// [`schema_lift_decision`] filter — every other component type
    /// (parameters, responses, headers, …) is named in the document
    /// already or has no trivial form worth keeping inline, so it
    /// always lifts.
    const IS_SCHEMA: bool = false;

    /// Borrow this type's bag mutably out of the Collapser.
    fn bag(c: &mut C) -> &mut Bag<Self, R>;

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

    /// Walk into a reference payload's own nested slots. A plain
    /// Reference Object has none; a 3.1+ schema `$ref` may carry
    /// sibling keywords with inline or external schemas inside them.
    /// Default: no-op.
    fn walk_ref(_reference: &mut R, _ctx: &NameContext, _c: &mut C) -> Result<(), CollapseError> {
        Ok(())
    }
}

/// The "lift one `RefOr<T>` into the bag of T" generic. Used by
/// every per-version walker — each `lift_ref_or_T` collapses to a
/// single call here, regardless of T.
///
/// Handles:
/// * `RefOr::Ref` with an internal ref (`#/...`): no-op.
/// * `RefOr::Ref` with an external ref and a loader: fetch, recurse,
///   intern, rewrite the slot to a local ref. A schema written as a
///   `$ref` was already named by its author, so the triviality
///   filter doesn't apply to it.
/// * `RefOr::Ref` with an external ref and no loader: no-op.
/// * `RefOr::Item`: recurse, then — for schemas — consult
///   [`schema_lift_decision`]: lift (intern + rewrite the slot to a
///   local ref) or leave the (now child-lifted) schema inline.
pub fn lift_ref_or<T, R, C>(
    slot: &mut RefOr<T, R>,
    ctx: NameContext,
    c: &mut C,
) -> Result<(), CollapseError>
where
    // `Clone` is required by `Loader::resolve_reference_as<T>` (it
    // clones cached values out of its typed cache); pinned here at
    // the call site rather than on the trait so the trait surface
    // stays minimal.
    T: LiftableBag<C, R> + Clone,
    R: ReferenceObject,
    C: CollapseState,
{
    match slot {
        RefOr::Ref(r) => {
            // The payload's own nested slots first — for a schema `$ref`
            // with siblings, an external `$ref` inside `properties` is
            // lifted whether or not the target is.
            T::walk_ref(r, &ctx, c)?;
            if is_internal_ref(r.reference()) {
                return Ok(());
            }
            let reference = r.reference().to_owned();
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
            if T::IS_SCHEMA {
                // An external schema always lifts, but note its shape
                // so an identical inline one collapses onto it rather
                // than staying inline beside it.
                note_schema_shape(&fetched, c)?;
            }
            T::walk(&mut fetched, &derived_ctx, c)?;
            let name = intern(c, fetched, &derived_ctx)?;
            // Rewrite the target in place rather than replacing the
            // slot: a schema `$ref` may carry sibling keywords, and
            // they belong to this use site, not to the fetched
            // component.
            r.set_reference(format!("{}{name}", T::PREFIX));
            Ok(())
        }
        RefOr::Item(_) => {
            // Take ownership out of the slot so we can recurse +
            // intern without aliasing.
            let placeholder = RefOr::new_ref(String::new());
            let owned = mem::replace(slot, placeholder);
            let RefOr::Item(mut item) = owned else {
                unreachable!("matched RefOr::Item above");
            };
            // Weigh the pre-walk form: walking rewrites this schema's
            // children into `$ref`s, and the census pass weighed the
            // same slot before walking it too.
            let lift = if T::IS_SCHEMA {
                should_lift_schema(&item, c)?
            } else {
                true
            };
            T::walk(&mut item, &ctx, c)?;
            if !lift {
                *slot = RefOr::new_item(item);
                return Ok(());
            }
            let name = intern(c, item, &ctx)?;
            *slot = RefOr::new_ref(format!("{}{name}", T::PREFIX));
            Ok(())
        }
    }
}

/// Apply [`schema_lift_decision`] to one not-yet-walked schema,
/// weighing the [`LiftDecision::IfRepeated`] case against the census.
fn should_lift_schema<T: Serialize, C: CollapseState>(
    item: &T,
    c: &mut C,
) -> Result<bool, CollapseError> {
    let value = serde_json::to_value(item)?;
    Ok(match schema_lift_decision(&value) {
        LiftDecision::Always => true,
        LiftDecision::Never => false,
        LiftDecision::IfRepeated => c.schema_repeats().weigh(shape_digest(&value)),
    })
}

/// Note a pre-existing `components.<bag>` entry in the census.
///
/// Such an entry keeps its own name and is never lifted, but it is
/// still a slot holding a shape: an inline schema that matches it
/// should collapse onto it, which costs no generated name at all
/// because the author already named this one. Counting the entry is
/// what lets the repeat rule see that. A no-op for bags that don't
/// hold schemas.
pub fn note_existing_component<T, R, C>(item: &T, c: &mut C) -> Result<(), CollapseError>
where
    T: LiftableBag<C, R>,
    R: ReferenceObject,
    C: CollapseState,
{
    if T::IS_SCHEMA {
        note_schema_shape(item, c)?;
    }
    Ok(())
}

/// Record a schema slot in the census without weighing it.
fn note_schema_shape<T: Serialize, C: CollapseState>(
    item: &T,
    c: &mut C,
) -> Result<(), CollapseError> {
    let value = serde_json::to_value(item)?;
    if schema_lift_decision(&value) == LiftDecision::IfRepeated {
        c.schema_repeats().note(shape_digest(&value));
    }
    Ok(())
}

/// Digest of a schema's canonical JSON, the census key.
fn shape_digest(value: &serde_json::Value) -> u64 {
    digest(&value.to_string())
}

fn intern<T, R, C>(c: &mut C, item: T, ctx: &NameContext) -> Result<String, CollapseError>
where
    T: LiftableBag<C, R>,
    R: ReferenceObject,
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
    fn schema_lift_decision_sorts_schemas_by_shape() {
        use serde_json::json;
        for value in [
            json!({"type": "object", "properties": {"a": {"type": "string"}}}),
            json!({"type": "object", "required": ["a"]}),
            json!({"type": "object", "additionalProperties": false}),
            json!({"allOf": [{"type": "object"}]}),
            json!({"anyOf": [{"type": "string"}]}),
            json!({"oneOf": [{"type": "string"}]}),
            json!({"not": {"type": "string"}}),
            json!({"type": "string", "enum": ["a", "b"]}),
            json!({"title": "Email", "type": "string"}),
        ] {
            assert_eq!(
                schema_lift_decision(&value),
                LiftDecision::Always,
                "expected a lift for {value}",
            );
        }

        for value in [
            json!(true),
            json!(false),
            json!({}),
            json!({"type": "string"}),
            json!({"type": "object"}),
            json!({
                "type": "string",
                "description": "d",
                "example": "e",
                "examples": ["e"],
                "deprecated": true,
                "readOnly": true,
                "writeOnly": false,
            }),
        ] {
            assert_eq!(
                schema_lift_decision(&value),
                LiftDecision::Never,
                "expected no lift for {value}",
            );
        }

        for value in [
            json!({"type": "string", "format": "date-time"}),
            json!({"type": "string", "pattern": "^a$", "maxLength": 8}),
            json!({"type": "array", "items": {"type": "string"}}),
            json!({"type": "string", "xml": {"name": "photoUrl"}}),
        ] {
            assert_eq!(
                schema_lift_decision(&value),
                LiftDecision::IfRepeated,
                "expected a dedup-dependent lift for {value}",
            );
        }
    }

    #[test]
    fn schema_repeats_counts_slots_across_the_census_pass() {
        let uuid = serde_json::json!({"type": "string", "format": "uuid"});
        let date = serde_json::json!({"type": "string", "format": "date-time"});

        let mut census = SchemaRepeats::census();
        // The census answers "not repeated" for every slot, so its
        // walk rewrites the clone exactly as the real pass will.
        assert!(!census.weigh(shape_digest(&uuid)));
        assert!(!census.weigh(shape_digest(&uuid)));
        assert!(!census.weigh(shape_digest(&date)));

        let mut repeats = census.finish();
        assert!(repeats.weigh(shape_digest(&uuid)), "two slots held it");
        assert!(!repeats.weigh(shape_digest(&date)), "one slot held it");
        // Weighing after the census never changes a count.
        assert!(!repeats.weigh(shape_digest(&date)));
        // Nor does a late `note`.
        repeats.note(shape_digest(&date));
        assert!(!repeats.weigh(shape_digest(&date)));
    }

    #[test]
    fn schema_repeats_notes_external_shapes_alongside_inline_slots() {
        let uuid = shape_digest(&serde_json::json!({"type": "string", "format": "uuid"}));
        let mut census = SchemaRepeats::census();
        // One inline slot plus one schema pulled in through an
        // external `$ref` is two slots holding the same shape.
        assert!(!census.weigh(uuid));
        census.note(uuid);
        assert!(census.finish().weigh(uuid));
    }

    #[test]
    fn schema_repeats_default_reports_nothing_repeated() {
        let mut repeats = SchemaRepeats::default();
        assert!(!repeats.weigh(shape_digest(&serde_json::json!({"type": "string"}))));
    }

    #[test]
    fn is_internal_ref_routes_by_leading_hash() {
        assert!(is_internal_ref("#/components/schemas/Pet"));
        assert!(!is_internal_ref("external.json#/Pet"));
        assert!(!is_internal_ref("https://example.com/spec#/Pet"));
    }
}
