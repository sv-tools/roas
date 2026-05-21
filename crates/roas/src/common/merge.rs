//! Structural merge helpers shared across versions.
//!
//! These don't force per-version components to share *types* — they're
//! generic over `V` and `T`, so each per-version impl block calls into
//! them with its own `V`. The dedup-policy decision (per-version files
//! stay duplicated except `reference.rs`) is preserved.

use std::collections::BTreeMap;

use crate::merge::{ConflictKind, MergeContext, MergeWithContext};

/// Merge two bare `BTreeMap`s by key. Used by `Paths`, `Callback`,
/// `Components.<bag>` (each bag is `Option<BTreeMap<...>>` — wrap with
/// [`merge_opt_map`] for those).
///
/// All helpers in this module no-op when `ctx.errored` is already
/// set, so callers do not need to interleave `if ctx.errored { return; }`
/// checks between successive calls. The first helper to record a
/// `Resolution::Errored` flips the flag and every subsequent helper
/// returns early.
pub(crate) fn merge_map<T, K, V>(
    base: &mut BTreeMap<K, V>,
    other: BTreeMap<K, V>,
    ctx: &mut MergeContext<T>,
    path: &str,
    mut recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, String),
    fmt_key: impl Fn(&K) -> String,
) where
    K: Ord,
{
    if ctx.errored {
        return;
    }
    for (k, incoming) in other {
        if let Some(base_v) = base.get_mut(&k) {
            let child_path = format!("{path}.{}", fmt_key(&k));
            recurse(base_v, incoming, ctx, child_path);
            if ctx.errored {
                return;
            }
        } else {
            base.insert(k, incoming);
        }
    }
}

/// Merge two `Option<BTreeMap<K, V>>` (the common shape for
/// `Components.<bag>` and the dozens of optional-map sub-fields).
pub(crate) fn merge_opt_map<T, K, V>(
    base: &mut Option<BTreeMap<K, V>>,
    other: Option<BTreeMap<K, V>>,
    ctx: &mut MergeContext<T>,
    path: &str,
    recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, String),
    fmt_key: impl Fn(&K) -> String,
) where
    K: Ord,
{
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    match base {
        Some(base_map) => merge_map(base_map, other, ctx, path, recurse, fmt_key),
        None => *base = Some(other),
    }
}

/// Merge an identity-keyed `Vec` (e.g. `tags` by name, `parameters` by
/// `(name, in)`). On collision the recursion callback decides what
/// happens; new keys are appended in incoming order.
pub(crate) fn merge_vec_by_key<T, V, K>(
    base: &mut Vec<V>,
    other: Vec<V>,
    ctx: &mut MergeContext<T>,
    path: &str,
    key_of: impl Fn(&V) -> K,
    mut recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, String),
    fmt_key: impl Fn(&K) -> String,
) where
    K: Ord + Clone,
{
    if ctx.errored {
        return;
    }
    use std::collections::BTreeMap as Map;
    let mut index: Map<K, usize> = Map::new();
    for (i, v) in base.iter().enumerate() {
        index.insert(key_of(v), i);
    }
    for incoming in other {
        let k = key_of(&incoming);
        if let Some(&i) = index.get(&k) {
            let child = format!("{path}[{}]", fmt_key(&k));
            recurse(&mut base[i], incoming, ctx, child);
            if ctx.errored {
                return;
            }
        } else {
            index.insert(k, base.len());
            base.push(incoming);
        }
    }
}

/// Merge an optional identity-keyed vec, treating `None` like an
/// empty list on either side.
pub(crate) fn merge_opt_vec_by_key<T, V, K>(
    base: &mut Option<Vec<V>>,
    other: Option<Vec<V>>,
    ctx: &mut MergeContext<T>,
    path: &str,
    key_of: impl Fn(&V) -> K,
    recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, String),
    fmt_key: impl Fn(&K) -> String,
) where
    K: Ord + Clone,
{
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    match base {
        Some(base_v) => merge_vec_by_key(base_v, other, ctx, path, key_of, recurse, fmt_key),
        None => *base = Some(other),
    }
}

/// Set-union an `Option<Vec<V>>` while preserving the base order
/// (incoming-only entries appended in incoming order). No conflict
/// is recorded — adding more tags / `required` entries is purely
/// additive.
///
/// Linear-scan dedup: `Vec::contains` against the grown base. Zero
/// allocations beyond the `Vec::push` itself — the previous
/// implementation built a `BTreeSet<V>` by cloning every base
/// element, then `insert`-cloned every incoming probe, paying
/// `2·|base| + |incoming|` clones. For the typical `tags` / `required`
/// list (handful of entries) the linear scan is faster anyway.
pub(crate) fn merge_opt_vec_set_union<T, V>(
    base: &mut Option<Vec<V>>,
    other: Option<Vec<V>>,
    ctx: &mut MergeContext<T>,
    _path: &str,
) where
    V: Eq,
{
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    match base {
        Some(base_v) => {
            for v in other {
                if !base_v.contains(&v) {
                    base_v.push(v);
                }
            }
        }
        None => *base = Some(other),
    }
}

/// Merge an optional scalar. `None × x` keeps `x`. `Some(a) × Some(b)`
/// where `a == b` is a no-op. Mismatching values run through the
/// three-mode policy.
pub(crate) fn merge_opt_scalar<T, V>(
    base: &mut Option<V>,
    other: Option<V>,
    ctx: &mut MergeContext<T>,
    path: &str,
    kind: ConflictKind,
) where
    V: PartialEq,
{
    if ctx.errored {
        return;
    }
    match (base.as_mut(), other) {
        (_, None) => {}
        (None, Some(v)) => *base = Some(v),
        (Some(a), Some(b)) => {
            if *a != b && ctx.should_take_incoming(path, kind) {
                *a = b;
            }
        }
    }
}

/// Merge a required scalar (one that's always present, like
/// `info.title`). Same policy as [`merge_opt_scalar`] for the
/// `Some × Some` arm.
pub(crate) fn merge_required_scalar<T, V>(
    base: &mut V,
    other: V,
    ctx: &mut MergeContext<T>,
    path: &str,
    kind: ConflictKind,
) where
    V: PartialEq,
{
    if ctx.errored {
        return;
    }
    if *base != other && ctx.should_take_incoming(path, kind) {
        *base = other;
    }
}

/// Replace `base` with `other` only when `other` is non-empty.
/// Records [`ConflictKind::ListReplaced`] when an actual replacement
/// occurs. Used for `servers`, `security`, etc.
pub(crate) fn merge_replace_list_when_nonempty<T, V>(
    base: &mut Option<Vec<V>>,
    other: Option<Vec<V>>,
    ctx: &mut MergeContext<T>,
    path: &str,
) {
    if ctx.errored {
        return;
    }
    use crate::merge::MergeOptions;
    let replace_when_empty = ctx.is_option(MergeOptions::ReplaceListsWhenEmpty);
    let Some(other) = other else { return };
    if other.is_empty() && !replace_when_empty {
        return;
    }
    match base {
        Some(b) if !b.is_empty() => {
            if ctx.should_take_incoming(path, ConflictKind::ListReplaced) {
                *b = other;
            }
        }
        _ => *base = Some(other),
    }
}

/// Merge `Option<BTreeMap<String, serde_json::Value>>` extensions
/// per-key. Identical JSON values are no-ops; differing values go
/// through the policy.
pub(crate) fn merge_extensions<T>(
    base: &mut Option<BTreeMap<String, serde_json::Value>>,
    other: Option<BTreeMap<String, serde_json::Value>>,
    ctx: &mut MergeContext<T>,
    path: &str,
) {
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    let base_map = base.get_or_insert_with(BTreeMap::new);
    for (k, incoming) in other {
        match base_map.get_mut(&k) {
            None => {
                base_map.insert(k, incoming);
            }
            Some(existing) => {
                if *existing != incoming {
                    let child = format!("{path}.{k}");
                    if ctx.should_take_incoming(&child, ConflictKind::ScalarOverridden) {
                        *existing = incoming;
                    }
                }
            }
        }
        if ctx.errored {
            return;
        }
    }
}

/// Merge two `Option<S>` where `S: MergeWithContext<T>`. Mirrors the
/// scalar shape but recurses on `Some × Some`.
pub(crate) fn merge_opt_struct<T, S>(
    base: &mut Option<S>,
    other: Option<S>,
    ctx: &mut MergeContext<T>,
    path: &str,
) where
    S: MergeWithContext<T>,
{
    if ctx.errored {
        return;
    }
    match (base.as_mut(), other) {
        (_, None) => {}
        (None, Some(v)) => *base = Some(v),
        (Some(a), Some(b)) => a.merge_with_context(b, ctx, path.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::{ConflictKind, MergeOptions};

    #[test]
    fn merge_opt_scalar_none_incoming_no_op() {
        let mut base: Option<String> = Some("a".into());
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_opt_scalar(
            &mut base,
            None,
            &mut ctx,
            "#.x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("a"));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_opt_scalar_none_base_takes_incoming() {
        let mut base: Option<String> = None;
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_opt_scalar(
            &mut base,
            Some("b".into()),
            &mut ctx,
            "#.x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("b"));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_opt_scalar_same_value_no_conflict() {
        let mut base: Option<String> = Some("a".into());
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_opt_scalar(
            &mut base,
            Some("a".into()),
            &mut ctx,
            "#.x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("a"));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_opt_scalar_differing_takes_incoming_by_default() {
        let mut base: Option<String> = Some("a".into());
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_opt_scalar(
            &mut base,
            Some("b".into()),
            &mut ctx,
            "#.x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("b"));
        assert_eq!(ctx.conflicts.len(), 1);
    }

    #[test]
    fn merge_opt_vec_set_union_dedupes_and_preserves_base_order() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2]);
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_opt_vec_set_union(&mut base, Some(vec![2, 3]), &mut ctx, "#.x");
        assert_eq!(base.unwrap(), vec![1, 2, 3]);
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_extensions_new_key_inserts() {
        let mut base: Option<BTreeMap<String, serde_json::Value>> = Some({
            let mut m = BTreeMap::new();
            m.insert("x-a".into(), serde_json::json!(1));
            m
        });
        let mut other = BTreeMap::new();
        other.insert("x-b".into(), serde_json::json!(2));
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_extensions(&mut base, Some(other), &mut ctx, "#.x");
        let b = base.unwrap();
        assert_eq!(b.get("x-a"), Some(&serde_json::json!(1)));
        assert_eq!(b.get("x-b"), Some(&serde_json::json!(2)));
    }

    #[test]
    fn merge_replace_list_keeps_populated_base_when_incoming_empty() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2]);
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_replace_list_when_nonempty(&mut base, Some(vec![]), &mut ctx, "#.x");
        assert_eq!(base, Some(vec![1, 2]));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_replace_list_replaces_when_both_non_empty() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2]);
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        merge_replace_list_when_nonempty(&mut base, Some(vec![9, 10]), &mut ctx, "#.x");
        assert_eq!(base, Some(vec![9, 10]));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::ListReplaced);
    }
}
