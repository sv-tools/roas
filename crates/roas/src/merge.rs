//! Public merge API and crate-internal recursion machinery.
//!
//! Sits parallel to [`crate::validation`]:
//!
//! * The public [`Merge`] trait is what callers reach for — `base.merge(incoming, opts)`.
//! * [`MergeWithContext<T>`] is the crate-internal recursive trait every
//!   component type implements. Implementors mutate `self` in place,
//!   record [`MergeConflict`]s into [`MergeContext::conflicts`], and
//!   recurse into children via each child's `merge_with_context`. The
//!   shape mirrors [`crate::validation::ValidateWithContext`] one-for-one.
//! * [`MergeOptions`] is an `EnumSet`-compatible flag enum. The default
//!   set is `EnumSet::empty()` (incoming wins, refs replace silently,
//!   schemas are leaves, info is preserved from base).

use enumset::{EnumSet, EnumSetType};
use std::collections::HashSet;
use std::fmt::{self, Display};
use thiserror::Error as ThisError;

/// Per-call flags toggling merge behavior.
///
/// All flags are *off* by default. The defaults are:
///
/// * **Conflict policy** — incoming wins.
/// * **Refs** — `Ref × Ref` with the same target merges metadata; with
///   different targets, incoming replaces. `Ref × Item` (in either
///   order) — incoming replaces. Every replacement is recorded as a
///   [`MergeConflict`] regardless.
/// * **Schemas** — treated as leaves (incoming replaces, recorded as
///   [`ConflictKind::SchemaLeafReplaced`]).
/// * **`info` / `openapi`** — kept from the base spec (the document
///   keeps its identity).
/// * **Lists like `servers` / `security`** — incoming replaces only
///   when non-empty.
#[derive(EnumSetType, Debug)]
pub enum MergeOptions {
    /// Reverse the default "incoming wins" policy: when both sides
    /// have a value at the same path, the base value is kept and the
    /// incoming value is dropped (still recorded as a conflict with
    /// [`Resolution::Base`]).
    ///
    /// Has no effect when only one side has a value, or for additive
    /// operations (new map keys, list-by-key new entries).
    BaseWins,

    /// Convert the first real collision into an early `Err`. The
    /// returned [`MergeError`] carries the conflicts collected up to
    /// that point (including the one that tripped the error).
    ///
    /// "Real" excludes additive merges: new map keys, identity-keyed
    /// list entries, and `Some` replacing `None`. Same-value
    /// collisions (where base and incoming compare equal) are also
    /// considered no-ops and do not trip this flag.
    ErrorOnConflict,

    /// Opt into deep-merging two object schemas. Without this flag,
    /// `Schema` collisions are always replace-incoming-wins (recorded
    /// as [`ConflictKind::SchemaLeafReplaced`]).
    ///
    /// When set, two `Schema::Single(SingleSchema::Object(_))` values
    /// merge their `properties`, `pattern_properties`, `required`
    /// (set-union), `additional_properties`, `property_names`, and
    /// scalar fields recursively. Other schema-variant pairings
    /// (`AllOf`, `OneOf`, `Bool`, `Multi`, `String`, etc.) still
    /// replace.
    DeepMergeObjectSchemas,

    /// Drop the "keep base `info` / `openapi`" guarantee and merge
    /// those fields like any other component. Off by default because
    /// the document's identity (title, version, OpenAPI version)
    /// usually belongs to the base.
    MergeInfo,

    /// Allow an empty incoming list to clear a populated base list.
    /// Off by default because most callers want a missing or empty
    /// list in the incoming document to mean "I have nothing to say
    /// about this," not "delete what's there."
    ///
    /// Applies to `Spec.servers`, `Spec.security`, `Operation.servers`,
    /// `Operation.security`, `PathItem.servers`.
    ReplaceListsWhenEmpty,
}

impl MergeOptions {
    /// Empty option set — the default behavior described in the
    /// [enum's doc comment](MergeOptions).
    pub fn new() -> EnumSet<MergeOptions> {
        EnumSet::empty()
    }

    /// Set containing only `self`.
    pub fn only(&self) -> EnumSet<MergeOptions> {
        EnumSet::only(*self)
    }
}

/// Resolution applied to a single collision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// Incoming value was taken — the default policy.
    Incoming,
    /// Base value was kept — under [`MergeOptions::BaseWins`].
    Base,
    /// [`MergeOptions::ErrorOnConflict`] tripped on this collision.
    /// The conflict is the last entry in the returned
    /// [`MergeError::conflicts`].
    Errored,
}

/// What kind of collision was resolved at a given path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    /// An optional scalar was set on both sides with different values.
    ScalarOverridden,

    /// A required scalar (e.g. `info.title`) was set on both sides
    /// with different values, and the merge policy kept one of them.
    RequiredScalarOverridden,

    /// `Ref × Ref` with different target strings.
    RefReplaced,

    /// `Ref × Item` (in either direction). The replacement is silent
    /// — neither side knows whether the ref resolves to a structurally
    /// compatible value.
    RefVsValue,

    /// Two `Schema` values collided and the leaf-replace policy was
    /// applied (default behavior, or applied because the variants
    /// don't match the [`MergeOptions::DeepMergeObjectSchemas`] gate).
    SchemaLeafReplaced,

    /// A whole `Option<Vec<T>>` was replaced wholesale (non-keyed
    /// lists like `servers` / `security`).
    ListReplaced,

    /// A `Parameter` of one location collided with one of a different
    /// location (e.g. `InPath` × `InQuery`). The location is the
    /// identity, so the merge is a replace; deep-merge would not make
    /// sense.
    ParameterVariantMismatch,
}

impl Display for ConflictKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConflictKind::ScalarOverridden => "scalar overridden",
            ConflictKind::RequiredScalarOverridden => "required scalar overridden",
            ConflictKind::RefReplaced => "ref replaced",
            ConflictKind::RefVsValue => "ref/value mismatch",
            ConflictKind::SchemaLeafReplaced => "schema leaf replaced",
            ConflictKind::ListReplaced => "list replaced",
            ConflictKind::ParameterVariantMismatch => "parameter variant mismatch",
        })
    }
}

/// One recorded resolution. Built by [`MergeContext::should_take_incoming`]
/// and the helpers in [`crate::common::merge`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MergeConflict {
    pub path: String,
    pub kind: ConflictKind,
    pub resolution: Resolution,
}

impl Display for MergeConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({:?})", self.path, self.kind, self.resolution)
    }
}

/// The successful return value of [`Merge::merge`]. Carries every
/// collision that the merge resolved, including same-value collisions
/// the caller might want to verify.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MergeReport {
    pub conflicts: Vec<MergeConflict>,
}

impl MergeReport {
    pub fn is_empty(&self) -> bool {
        self.conflicts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.conflicts.len()
    }
}

impl Display for MergeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} merge conflicts:", self.conflicts.len())?;
        for c in &self.conflicts {
            writeln!(f, "- {c}")?;
        }
        Ok(())
    }
}

/// Returned when [`MergeOptions::ErrorOnConflict`] is set and a real
/// collision is hit. Carries every conflict collected up to and
/// including the one that tripped the error.
#[derive(Debug, Clone, PartialEq, ThisError)]
#[error("merge aborted on conflict")]
pub struct MergeError {
    pub conflicts: Vec<MergeConflict>,
}

/// Public entry point for merging. Implemented for each per-version
/// `Spec` type; v3.2 ships first, v2 / v3.0 / v3.1 follow.
pub trait Merge: Sized {
    /// Merge `other` into `self` in place, following the rules
    /// determined by `options`.
    ///
    /// Returns a [`MergeReport`] listing every collision (even ones
    /// resolved silently). Returns [`MergeError`] only when
    /// [`MergeOptions::ErrorOnConflict`] is set and a real collision
    /// was hit.
    fn merge(
        &mut self,
        other: Self,
        options: EnumSet<MergeOptions>,
    ) -> Result<MergeReport, MergeError>;
}

/// Crate-internal recursive merge trait. Mirrors
/// [`crate::validation::ValidateWithContext`].
pub(crate) trait MergeWithContext<T> {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<T>, path: String);
}

/// Merge context — carries the spec being merged into, the options,
/// accumulated conflicts, and a `visited` set for cycle safety
/// during nested-ref / nested-schema recursion.
pub(crate) struct MergeContext<'a, T> {
    pub spec: &'a T,
    pub options: EnumSet<MergeOptions>,
    pub visited: HashSet<String>,
    pub conflicts: Vec<MergeConflict>,
    /// Set by [`Self::should_take_incoming`] when
    /// [`MergeOptions::ErrorOnConflict`] is engaged and a real
    /// collision has been recorded. Implementors check this after
    /// each potentially-conflicting step and short-circuit further
    /// merging.
    pub errored: bool,
}

impl<T> MergeContext<'_, T> {
    pub fn is_option(&self, o: MergeOptions) -> bool {
        self.options.contains(o)
    }

    #[allow(dead_code)]
    pub fn visit(&mut self, path: String) -> bool {
        self.visited.insert(path)
    }

    pub fn record(&mut self, path: String, kind: ConflictKind, resolution: Resolution) {
        self.conflicts.push(MergeConflict {
            path,
            kind,
            resolution,
        });
    }

    /// Apply the three-mode policy to a single collision. Records the
    /// conflict and returns `true` when the caller should overwrite
    /// `base` with `incoming`, `false` when the caller should keep
    /// `base`.
    ///
    /// Under `ErrorOnConflict` this sets [`Self::errored`] and returns
    /// `false` so the caller leaves `base` alone — the top-level
    /// `Spec::merge` checks `errored` and converts the report into a
    /// [`MergeError`] before returning.
    pub fn should_take_incoming(&mut self, path: &str, kind: ConflictKind) -> bool {
        if self.is_option(MergeOptions::ErrorOnConflict) {
            self.record(path.to_owned(), kind, Resolution::Errored);
            self.errored = true;
            false
        } else if self.is_option(MergeOptions::BaseWins) {
            self.record(path.to_owned(), kind, Resolution::Base);
            false
        } else {
            self.record(path.to_owned(), kind, Resolution::Incoming);
            true
        }
    }
}

impl<T> MergeContext<'_, T> {
    pub(crate) fn new<'a>(spec: &'a T, options: EnumSet<MergeOptions>) -> MergeContext<'a, T> {
        MergeContext {
            spec,
            options,
            visited: HashSet::new(),
            conflicts: Vec::new(),
            errored: false,
        }
    }
}

impl<'a, T> From<MergeContext<'a, T>> for Result<MergeReport, MergeError> {
    fn from(ctx: MergeContext<'a, T>) -> Self {
        if ctx.errored {
            Err(MergeError {
                conflicts: ctx.conflicts,
            })
        } else {
            Ok(MergeReport {
                conflicts: ctx.conflicts,
            })
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for MergeContext<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MergeContext")
            .field("spec", &self.spec)
            .field("options", &self.options)
            .field("visited", &self.visited)
            .field("conflicts", &self.conflicts)
            .field("errored", &self.errored)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_incoming_default_takes_incoming_and_records() {
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let took = ctx.should_take_incoming("#.x", ConflictKind::ScalarOverridden);
        assert!(took);
        assert!(!ctx.errored);
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].resolution, Resolution::Incoming);
    }

    #[test]
    fn should_take_incoming_base_wins_keeps_base() {
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::BaseWins.only());
        let took = ctx.should_take_incoming("#.x", ConflictKind::ScalarOverridden);
        assert!(!took);
        assert!(!ctx.errored);
        assert_eq!(ctx.conflicts[0].resolution, Resolution::Base);
    }

    #[test]
    fn should_take_incoming_error_mode_marks_errored() {
        let mut ctx: MergeContext<()> =
            MergeContext::new(&(), MergeOptions::ErrorOnConflict.only());
        let took = ctx.should_take_incoming("#.x", ConflictKind::ScalarOverridden);
        assert!(!took);
        assert!(ctx.errored);
        assert_eq!(ctx.conflicts[0].resolution, Resolution::Errored);
    }

    #[test]
    fn context_into_result_returns_err_when_errored() {
        let mut ctx: MergeContext<()> =
            MergeContext::new(&(), MergeOptions::ErrorOnConflict.only());
        ctx.should_take_incoming("#.x", ConflictKind::ScalarOverridden);
        let res: Result<MergeReport, MergeError> = ctx.into();
        assert!(res.is_err());
    }

    #[test]
    fn context_into_result_returns_ok_with_report() {
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        ctx.should_take_incoming("#.x", ConflictKind::ScalarOverridden);
        let res: Result<MergeReport, MergeError> = ctx.into();
        let report = res.expect("ok");
        assert_eq!(report.len(), 1);
    }
}
