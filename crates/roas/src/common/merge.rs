//! Structural merge helpers shared across versions.
//!
//! Generic over `V` and `T`, so each per-version impl block calls into
//! them with its own `V`. The dedup-policy decision (per-version files
//! stay duplicated except `reference.rs`) is preserved.
//!
//! All helpers in this module no-op when `ctx.errored` is already set,
//! so callers do not need to interleave `if ctx.errored { return; }`
//! checks between successive calls. The first helper to record a
//! `Resolution::Errored` flips the flag and every subsequent helper
//! returns early.
//!
//! Path traversal uses a `&mut String` stack: each helper pushes its
//! own segment before recording a conflict and truncates back when
//! it returns. The non-conflict path therefore performs zero String
//! allocations (the original implementation `format!`'d a fresh
//! `String` for every field of every node, conflict or not). The
//! [`push_segment`] / [`PathGuard`] pair encapsulates the RAII
//! push/truncate dance so call sites stay readable.

use std::collections::BTreeMap;

use crate::merge::{ConflictKind, MergeContext, MergeWithContext};

/// RAII guard that pushes a segment onto `path` on construction and
/// truncates it back to the original length on drop. Holding the
/// guard for the duration of a child call keeps the path valid for
/// any conflict recording that happens inside.
pub(crate) struct PathGuard<'a> {
    path: &'a mut String,
    original_len: usize,
}

impl<'a> PathGuard<'a> {
    pub(crate) fn new(path: &'a mut String, segment: &str) -> Self {
        let original_len = path.len();
        path.push_str(segment);
        PathGuard { path, original_len }
    }

    #[allow(dead_code)]
    pub(crate) fn as_str(&self) -> &str {
        self.path
    }

    pub(crate) fn path_mut(&mut self) -> &mut String {
        self.path
    }
}

impl Drop for PathGuard<'_> {
    fn drop(&mut self) {
        self.path.truncate(self.original_len);
    }
}

/// Convenience for ad-hoc pushes that don't immediately go into a
/// helper that already does its own push (e.g. when recording a
/// conflict directly from a component impl).
#[inline]
pub(crate) fn with_segment<R>(
    path: &mut String,
    segment: &str,
    f: impl FnOnce(&mut String) -> R,
) -> R {
    let mut guard = PathGuard::new(path, segment);
    f(guard.path_mut())
}

/// Merge two bare `BTreeMap`s by key. Used by `Paths`, `Callback`,
/// `Components.<bag>` (each bag is `Option<BTreeMap<...>>` — wrap with
/// [`merge_opt_map`] for those).
pub(crate) fn merge_map<T, K, V>(
    base: &mut BTreeMap<K, V>,
    other: BTreeMap<K, V>,
    ctx: &mut MergeContext<T>,
    path: &mut String,
    mut recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, &mut String),
    fmt_key: impl Fn(&K, &mut String),
) where
    K: Ord,
{
    if ctx.errored {
        return;
    }
    for (k, incoming) in other {
        if let Some(base_v) = base.get_mut(&k) {
            let original_len = path.len();
            path.push('.');
            fmt_key(&k, path);
            recurse(base_v, incoming, ctx, path);
            path.truncate(original_len);
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
/// Pushes `segment` onto `path` only when there's something to do.
pub(crate) fn merge_opt_map<T, K, V>(
    base: &mut Option<BTreeMap<K, V>>,
    other: Option<BTreeMap<K, V>>,
    ctx: &mut MergeContext<T>,
    path: &mut String,
    segment: &str,
    recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, &mut String),
    fmt_key: impl Fn(&K, &mut String),
) where
    K: Ord,
{
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    match base {
        Some(base_map) => {
            let mut guard = PathGuard::new(path, segment);
            merge_map(base_map, other, ctx, guard.path_mut(), recurse, fmt_key);
        }
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
    path: &mut String,
    key_of: impl Fn(&V) -> K,
    mut recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, &mut String),
    fmt_key: impl Fn(&K, &mut String),
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
            let original_len = path.len();
            path.push('[');
            fmt_key(&k, path);
            path.push(']');
            recurse(&mut base[i], incoming, ctx, path);
            path.truncate(original_len);
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
#[allow(clippy::too_many_arguments)] // 8 args; this is a structural plumbing helper.
pub(crate) fn merge_opt_vec_by_key<T, V, K>(
    base: &mut Option<Vec<V>>,
    other: Option<Vec<V>>,
    ctx: &mut MergeContext<T>,
    path: &mut String,
    segment: &str,
    key_of: impl Fn(&V) -> K,
    recurse: impl FnMut(&mut V, V, &mut MergeContext<T>, &mut String),
    fmt_key: impl Fn(&K, &mut String),
) where
    K: Ord + Clone,
{
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    match base {
        Some(base_v) => {
            let mut guard = PathGuard::new(path, segment);
            merge_vec_by_key(
                base_v,
                other,
                ctx,
                guard.path_mut(),
                key_of,
                recurse,
                fmt_key,
            );
        }
        None => *base = Some(other),
    }
}

/// Set-union an `Option<Vec<V>>` while preserving the base order
/// (incoming-only entries appended in incoming order). No conflict
/// is recorded — adding more tags / `required` entries is purely
/// additive.
///
/// Linear-scan dedup: `Vec::contains` against the grown base.
pub(crate) fn merge_opt_vec_set_union<T, V>(
    base: &mut Option<Vec<V>>,
    other: Option<Vec<V>>,
    ctx: &mut MergeContext<T>,
    _path: &mut String,
    _segment: &str,
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
///
/// `segment` is appended onto `path` only when a real conflict is
/// recorded — the non-conflict path never grows `path`.
pub(crate) fn merge_opt_scalar<T, V>(
    base: &mut Option<V>,
    other: Option<V>,
    ctx: &mut MergeContext<T>,
    path: &mut String,
    segment: &str,
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
            if *a != b {
                let take = with_segment(path, segment, |p| ctx.should_take_incoming(p, kind));
                if take {
                    *a = b;
                }
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
    path: &mut String,
    segment: &str,
    kind: ConflictKind,
) where
    V: PartialEq,
{
    if ctx.errored {
        return;
    }
    if *base != other {
        let take = with_segment(path, segment, |p| ctx.should_take_incoming(p, kind));
        if take {
            *base = other;
        }
    }
}

/// Replace `base` with `other` only when `other` is non-empty.
/// Records [`ConflictKind::ListReplaced`] when an actual replacement
/// occurs. Used for `servers`, `security`, etc.
pub(crate) fn merge_replace_list_when_nonempty<T, V>(
    base: &mut Option<Vec<V>>,
    other: Option<Vec<V>>,
    ctx: &mut MergeContext<T>,
    path: &mut String,
    segment: &str,
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
            let take = with_segment(path, segment, |p| {
                ctx.should_take_incoming(p, ConflictKind::ListReplaced)
            });
            if take {
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
    path: &mut String,
    segment: &str,
) {
    if ctx.errored {
        return;
    }
    let Some(other) = other else { return };
    let base_map = base.get_or_insert_with(BTreeMap::new);
    let mut guard = PathGuard::new(path, segment);
    for (k, incoming) in other {
        match base_map.get_mut(&k) {
            None => {
                base_map.insert(k, incoming);
            }
            Some(existing) => {
                if *existing != incoming {
                    let take = with_segment(guard.path_mut(), ".", |p| {
                        p.push_str(&k);
                        ctx.should_take_incoming(p, ConflictKind::ScalarOverridden)
                    });
                    if take {
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
    path: &mut String,
    segment: &str,
) where
    S: MergeWithContext<T>,
{
    if ctx.errored {
        return;
    }
    match (base.as_mut(), other) {
        (_, None) => {}
        (None, Some(v)) => *base = Some(v),
        (Some(a), Some(b)) => {
            let mut guard = PathGuard::new(path, segment);
            a.merge_with_context(b, ctx, guard.path_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::{ConflictKind, MergeOptions};

    fn root_path() -> String {
        "#".to_owned()
    }

    #[test]
    fn merge_opt_scalar_none_incoming_no_op() {
        let mut base: Option<String> = Some("a".into());
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_opt_scalar(
            &mut base,
            None,
            &mut ctx,
            &mut path,
            ".x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("a"));
        assert!(ctx.conflicts.is_empty());
        assert_eq!(path, "#", "path stack must be balanced");
    }

    #[test]
    fn merge_opt_scalar_none_base_takes_incoming() {
        let mut base: Option<String> = None;
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_opt_scalar(
            &mut base,
            Some("b".into()),
            &mut ctx,
            &mut path,
            ".x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("b"));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_opt_scalar_same_value_no_conflict() {
        let mut base: Option<String> = Some("a".into());
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_opt_scalar(
            &mut base,
            Some("a".into()),
            &mut ctx,
            &mut path,
            ".x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("a"));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_opt_scalar_differing_takes_incoming_by_default() {
        let mut base: Option<String> = Some("a".into());
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_opt_scalar(
            &mut base,
            Some("b".into()),
            &mut ctx,
            &mut path,
            ".x",
            ConflictKind::ScalarOverridden,
        );
        assert_eq!(base.as_deref(), Some("b"));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].path, "#.x", "conflict path materialised");
        assert_eq!(path, "#", "path stack balanced after conflict");
    }

    #[test]
    fn merge_opt_vec_set_union_dedupes_and_preserves_base_order() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2]);
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_opt_vec_set_union(&mut base, Some(vec![2, 3]), &mut ctx, &mut path, ".x");
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
        let mut path = root_path();
        merge_extensions(&mut base, Some(other), &mut ctx, &mut path, ".ext");
        let b = base.unwrap();
        assert_eq!(b.get("x-a"), Some(&serde_json::json!(1)));
        assert_eq!(b.get("x-b"), Some(&serde_json::json!(2)));
        assert_eq!(path, "#", "path balanced");
    }

    #[test]
    fn merge_replace_list_keeps_populated_base_when_incoming_empty() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2]);
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_replace_list_when_nonempty(&mut base, Some(vec![]), &mut ctx, &mut path, ".servers");
        assert_eq!(base, Some(vec![1, 2]));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn merge_replace_list_replaces_when_both_non_empty() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2]);
        let mut ctx: MergeContext<()> = MergeContext::new(&(), MergeOptions::new());
        let mut path = root_path();
        merge_replace_list_when_nonempty(
            &mut base,
            Some(vec![9, 10]),
            &mut ctx,
            &mut path,
            ".servers",
        );
        assert_eq!(base, Some(vec![9, 10]));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::ListReplaced);
        assert_eq!(ctx.conflicts[0].path, "#.servers");
    }

    #[test]
    fn path_guard_restores_path_on_drop() {
        let mut path = "#.foo".to_owned();
        {
            let guard = PathGuard::new(&mut path, ".bar");
            assert_eq!(guard.as_str(), "#.foo.bar");
        }
        assert_eq!(path, "#.foo");
    }

    #[test]
    fn path_guard_restores_path_on_drop_with_nested_guards() {
        let mut path = "#".to_owned();
        {
            let mut g1 = PathGuard::new(&mut path, ".a");
            {
                let g2 = PathGuard::new(g1.path_mut(), ".b");
                assert_eq!(g2.as_str(), "#.a.b");
            }
            assert_eq!(g1.as_str(), "#.a");
        }
        assert_eq!(path, "#");
    }
}
