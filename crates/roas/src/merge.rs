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
    ///
    /// **Caveat:** unlike [`Self::ErrorOnConflict`], `BaseWins` does
    /// **not** roll back additive mutations. If the caller inspects
    /// `self` after a `BaseWins` merge, new map keys / new tags / new
    /// status codes from the incoming spec will be present — only the
    /// overwrite-of-existing path is suppressed. Pair with
    /// [`Self::ErrorOnConflict`] (which dominates — see below) if you
    /// need atomicity.
    BaseWins,

    /// Convert the first real collision into an early `Err`. The
    /// returned [`MergeError`] carries the conflicts collected up to
    /// that point (including the one that tripped the error).
    ///
    /// "Real" excludes additive merges: new map keys, identity-keyed
    /// list entries, and `Some` replacing `None`. Same-value
    /// collisions (where base and incoming compare equal) are also
    /// considered no-ops and do not trip this flag.
    ///
    /// **Dominates [`Self::BaseWins`]:** when both are set, the first
    /// real mismatch returns `Err` rather than silently keeping base.
    /// The base spec is left untouched on error
    /// ([`Merge::merge`] clones internally before mutating in this
    /// mode), so `BaseWins`-style "keep what I have" is implicitly
    /// upheld by the rollback.
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
/// [`crate::validation::ValidateWithContext`], except the JSONPath
/// locator is a `&mut String` push/truncate stack rather than an
/// owned `String` rebuilt at every recursion level. Implementors
/// push their child segments before recursing and truncate back
/// when they return — concretely:
///
/// ```ignore
/// let len = path.len();
/// path.push_str(".myField");
/// inner.merge_with_context(other, ctx, path);
/// path.truncate(len);
/// ```
///
/// The helpers in [`crate::common::merge`] do this internally so
/// most call sites don't have to. This turns the previous
/// O(nodes × fields) `format!` allocations into O(conflicts) —
/// only the conflict-recording path materialises an owned `String`.
pub(crate) trait MergeWithContext<T> {
    fn merge_with_context(&mut self, other: Self, ctx: &mut MergeContext<T>, path: &mut String);
}

/// Merge context — carries the spec being merged into, the options,
/// and accumulated conflicts.
///
/// The `spec` back-reference is currently unused by every concrete
/// `MergeWithContext` impl (merge recurses through owned subtrees, so
/// there's nothing to resolve against the parent spec). It's kept on
/// the type for symmetry with `validation::Context<T>` and so v2 /
/// v3.0 / v3.1 ports can introduce ref-following merge later without
/// reshaping every signature. If it stays unused once all four
/// versions land, drop `T` / `spec` in a follow-up.
pub(crate) struct MergeContext<'a, T> {
    pub spec: &'a T,
    pub options: EnumSet<MergeOptions>,
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

    #[test]
    fn merge_options_only_helper_returns_single_set() {
        let only = MergeOptions::BaseWins.only();
        assert!(only.contains(MergeOptions::BaseWins));
        assert!(!only.contains(MergeOptions::ErrorOnConflict));
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn merge_options_new_is_empty() {
        assert!(MergeOptions::new().is_empty());
    }

    #[test]
    fn merge_report_default_is_empty_and_len_zero() {
        let r = MergeReport::default();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn merge_report_display_renders_count_and_bullets() {
        let r = MergeReport {
            conflicts: vec![
                MergeConflict {
                    path: "#.a".into(),
                    kind: ConflictKind::ScalarOverridden,
                    resolution: Resolution::Incoming,
                },
                MergeConflict {
                    path: "#.b".into(),
                    kind: ConflictKind::RefReplaced,
                    resolution: Resolution::Base,
                },
            ],
        };
        let s = r.to_string();
        assert!(s.starts_with("2 merge conflicts:"));
        assert!(s.contains("- #.a: scalar overridden (Incoming)"));
        assert!(s.contains("- #.b: ref replaced (Base)"));
    }

    #[test]
    fn merge_report_display_empty_still_renders_header() {
        let s = MergeReport::default().to_string();
        assert!(s.starts_with("0 merge conflicts:"));
    }

    #[test]
    fn merge_conflict_display_renders_path_kind_resolution() {
        let c = MergeConflict {
            path: "#.foo".into(),
            kind: ConflictKind::ListReplaced,
            resolution: Resolution::Errored,
        };
        assert_eq!(c.to_string(), "#.foo: list replaced (Errored)");
    }

    #[test]
    fn conflict_kind_display_covers_every_variant() {
        for (k, expected) in [
            (ConflictKind::ScalarOverridden, "scalar overridden"),
            (
                ConflictKind::RequiredScalarOverridden,
                "required scalar overridden",
            ),
            (ConflictKind::RefReplaced, "ref replaced"),
            (ConflictKind::RefVsValue, "ref/value mismatch"),
            (ConflictKind::SchemaLeafReplaced, "schema leaf replaced"),
            (ConflictKind::ListReplaced, "list replaced"),
            (
                ConflictKind::ParameterVariantMismatch,
                "parameter variant mismatch",
            ),
        ] {
            assert_eq!(k.to_string(), expected);
        }
    }

    #[test]
    fn merge_error_display_shows_message() {
        let err = MergeError {
            conflicts: vec![MergeConflict {
                path: "#".into(),
                kind: ConflictKind::ScalarOverridden,
                resolution: Resolution::Errored,
            }],
        };
        assert_eq!(err.to_string(), "merge aborted on conflict");
    }

    #[test]
    fn merge_context_debug_includes_state() {
        let ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let s = format!("{ctx:?}");
        assert!(s.contains("MergeContext"));
        assert!(s.contains("conflicts: []"));
        assert!(s.contains("errored: false"));
    }

    #[test]
    fn merge_context_visit_inserts_path() {
        // (`visit` was removed; the visited HashSet is gone too.)
        // This test confirms the public surface no longer carries
        // either by reaching into Debug — defensive against accidental
        // re-introduction.
        let ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        assert!(!format!("{ctx:?}").contains("visited"));
    }
}
