//! Public surface for applying an Overlay to a target JSON document.
//!
//! See [`Apply::apply`] for the entry point. The trait is implemented
//! for each per-version `Overlay` type ([`crate::v1_0::Overlay`] today;
//! `v1_1::Overlay` once that feature lands).

use enumset::{EnumSet, EnumSetType};
use std::fmt::{self, Display};

/// Apply an overlay document to a target JSON value in place.
///
/// On error the target is left untouched: implementors are expected
/// to operate on a clone and commit only on success.
pub trait Apply {
    fn apply(
        &self,
        target: &mut serde_json::Value,
        options: EnumSet<ApplyOptions>,
    ) -> Result<ApplyReport, ApplyError>;
}

/// Per-call apply toggles.
#[derive(EnumSetType, Debug)]
pub enum ApplyOptions {
    /// Treat a zero-match `target` JSONPath as an error rather than a
    /// no-op. Default behavior (option absent) follows
    /// [§4.4](https://spec.openapis.org/overlay/v1.0.0.html#action-object):
    /// "the action succeeds without changing the target document".
    ErrorOnZeroMatch,
    /// Reject `update` actions whose `target` selects nodes of mixed
    /// kind (some objects, some arrays). The v1.1 spec calls this out
    /// normatively; v1.0 doesn't, so this option lets v1.0 callers
    /// opt into the stricter check.
    ErrorOnMixedKindMatch,
}

#[cfg(feature = "clap")]
impl clap::ValueEnum for ApplyOptions {
    fn value_variants<'a>() -> &'a [Self] {
        &[
            ApplyOptions::ErrorOnZeroMatch,
            ApplyOptions::ErrorOnMixedKindMatch,
        ]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        let (name, help) = match self {
            ApplyOptions::ErrorOnZeroMatch => (
                "error-on-zero-match",
                "Fail when an action's `target` selects zero nodes",
            ),
            ApplyOptions::ErrorOnMixedKindMatch => (
                "error-on-mixed-kind-match",
                "Fail when `update` selects a mix of objects and arrays",
            ),
        };
        Some(clap::builder::PossibleValue::new(name).help(help))
    }
}

/// One entry per applied action, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ActionOutcome {
    pub index: usize,
    pub target: String,
    pub operation: Operation,
    pub matched: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Operation {
    Update,
    Remove,
    /// Overlay v1.1 only: source node located via the action's `copy`
    /// JSONPath was merged into each matched `target` node.
    Copy,
}

impl Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Operation::Update => "update",
            Operation::Remove => "remove",
            Operation::Copy => "copy",
        })
    }
}

/// Report returned by a successful [`Apply::apply`] call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ApplyReport {
    pub actions: Vec<ActionOutcome>,
}

/// Failure returned by [`Apply::apply`]. The `target` document is
/// guaranteed untouched on error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApplyError {
    pub action_index: usize,
    pub target: String,
    pub kind: ApplyErrorKind,
}

impl Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "actions[{}] (target {:?}): {}",
            self.action_index, self.target, self.kind
        )
    }
}

impl std::error::Error for ApplyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApplyErrorKind {
    /// `target` is not a syntactically valid RFC 9535 JSONPath query.
    InvalidJsonPath(String),
    /// No node matched and [`ApplyOptions::ErrorOnZeroMatch`] is set.
    ZeroMatch,
    /// Target matched nodes of mixed kinds and
    /// [`ApplyOptions::ErrorOnMixedKindMatch`] is set.
    MixedKindMatch,
    /// `target` resolves to a primitive or `null`. The spec
    /// [§4.4](https://spec.openapis.org/overlay/v1.0.0.html#action-object)
    /// requires action targets to be objects or arrays, for both
    /// `update` and `remove` actions.
    PrimitiveActionTarget,
    /// Overlay v1.1 only: the action's `copy` JSONPath is
    /// syntactically valid but matched no node in the working doc.
    CopySourceNotFound(String),
    /// Overlay v1.1 only: the action's `copy` JSONPath matched more
    /// than one node; the spec requires exactly one source.
    CopySourceMultiple(String),
    /// Overlay v1.1 only: the action set both `update` and `copy`,
    /// which the spec treats as mutually exclusive. Validation flags
    /// this; apply fails fast rather than silently dropping one.
    ConflictingMergeSources,
}

impl Display for ApplyErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyErrorKind::InvalidJsonPath(msg) => write!(f, "invalid JSONPath: {msg}"),
            ApplyErrorKind::ZeroMatch => {
                f.write_str("target matched zero nodes (error-on-zero-match)")
            }
            ApplyErrorKind::MixedKindMatch => f.write_str(
                "target matched nodes of mixed kind (objects and arrays) — \
                 error-on-mixed-kind-match",
            ),
            ApplyErrorKind::PrimitiveActionTarget => f.write_str(
                "action `target` must resolve to objects or arrays, \
                 not primitives or null",
            ),
            ApplyErrorKind::CopySourceNotFound(s) => {
                write!(f, "`copy` source {s:?} matched no node")
            }
            ApplyErrorKind::CopySourceMultiple(s) => write!(
                f,
                "`copy` source {s:?} matched multiple nodes; exactly one is required",
            ),
            ApplyErrorKind::ConflictingMergeSources => {
                f.write_str("action sets both `update` and `copy`; they are mutually exclusive")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_display_uses_lowercase_words() {
        assert_eq!(Operation::Update.to_string(), "update");
        assert_eq!(Operation::Remove.to_string(), "remove");
        assert_eq!(Operation::Copy.to_string(), "copy");
    }

    #[test]
    fn apply_error_display_includes_index_target_and_reason() {
        let e = ApplyError {
            action_index: 2,
            target: "$.foo".into(),
            kind: ApplyErrorKind::ZeroMatch,
        };
        let s = e.to_string();
        assert!(s.contains("actions[2]"));
        assert!(s.contains("$.foo"));
        assert!(s.contains("zero nodes"));
    }

    #[test]
    fn apply_error_kind_display_covers_every_variant() {
        let cases = [
            ApplyErrorKind::InvalidJsonPath("bad path".into()),
            ApplyErrorKind::ZeroMatch,
            ApplyErrorKind::MixedKindMatch,
            ApplyErrorKind::PrimitiveActionTarget,
            ApplyErrorKind::CopySourceNotFound("$.src".into()),
            ApplyErrorKind::CopySourceMultiple("$.src".into()),
            ApplyErrorKind::ConflictingMergeSources,
        ];
        for k in cases {
            assert!(
                !k.to_string().is_empty(),
                "Display impl for {k:?} produced empty string",
            );
        }
    }
}

#[cfg(all(test, feature = "clap"))]
mod clap_tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn apply_options_value_enum_round_trips_through_kebab_case() {
        for v in <ApplyOptions as ValueEnum>::value_variants() {
            let pv = v.to_possible_value().expect("possible value");
            let name = pv.get_name();
            let parsed = <ApplyOptions as ValueEnum>::from_str(name, false).expect("parses");
            assert_eq!(parsed, *v);
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "name `{name}` must be kebab-case",
            );
        }
    }
}
