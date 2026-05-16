//! Internal primitives for `Spec::merge` across the per-version trees.
//!
//! Every per-version `merge` reduces to three patterns:
//!   * **Map-merge**: for `Option<BTreeMap<K, V>>` containers (paths,
//!     `components.<bag>`, extensions). Per-key incoming wins; the base map
//!     is materialised lazily when only incoming has entries.
//!   * **Scalar-replace**: for `Option<T>` fields where any incoming `Some`
//!     should overwrite the base wholesale (`externalDocs`, v2's
//!     `host`/`basePath`).
//!   * **List-replace-when-non-empty**: for `Option<Vec<T>>` list fields
//!     where the contract is "wholesale incoming wins, but an empty
//!     incoming list doesn't wipe a populated base" (`servers`, `security`,
//!     v2's `schemes`/`consumes`/`produces`).
//!
//! `tags`-style ordered lists keyed by a name field have their own
//! per-version helper at the call site — the name field's type / lookup
//! differs by version and the call sites are short.

use std::collections::BTreeMap;

/// Merge `incoming` into `base` per key, with incoming entries replacing
/// base entries with the same key. `None` incoming is a no-op; a `Some`
/// incoming with no entries is harmless (extends with nothing).
pub(crate) fn merge_optional_map<K: Ord, V>(
    base: &mut Option<BTreeMap<K, V>>,
    incoming: Option<BTreeMap<K, V>>,
) {
    let Some(incoming) = incoming else { return };
    match base {
        Some(b) => b.extend(incoming),
        None => *base = Some(incoming),
    }
}

/// Replace `base` with `incoming` whenever `incoming` is `Some(_)`. Used
/// for scalar option fields where any present value should overwrite the
/// base wholesale.
pub(crate) fn merge_optional<T>(base: &mut Option<T>, incoming: Option<T>) {
    if incoming.is_some() {
        *base = incoming;
    }
}

/// Replace `base` with `incoming` when `incoming` is `Some` *and*
/// non-empty. An explicit `Some(vec![])` from incoming is treated the
/// same as `None`: we don't let an empty list wipe a populated base.
pub(crate) fn merge_optional_list<T>(base: &mut Option<Vec<T>>, incoming: Option<Vec<T>>) {
    match incoming {
        Some(v) if !v.is_empty() => *base = Some(v),
        _ => {}
    }
}

/// Per-key merge for a `Vec<T>` keyed by `key(&T)`. Incoming entries with
/// a key already present in base replace that slot in place; entries with
/// new keys are appended. A `None` incoming is a no-op.
///
/// Used for `tags`: OAS requires `Tag.name` uniqueness across the
/// document, so the natural merge axis is "incoming wins per name, new
/// names appended in incoming order".
pub(crate) fn merge_named_list<T, F>(base: &mut Option<Vec<T>>, incoming: Option<Vec<T>>, key: F)
where
    F: Fn(&T) -> &str,
{
    let Some(incoming) = incoming else { return };
    let target = base.get_or_insert_with(Vec::new);
    for item in incoming {
        let k = key(&item).to_owned();
        if let Some(slot) = target.iter_mut().find(|t| key(t) == k) {
            *slot = item;
        } else {
            target.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_optional_map_replaces_matching_keys_and_appends_new() {
        let mut base: Option<BTreeMap<String, i32>> =
            Some([("a".to_owned(), 1), ("b".to_owned(), 2)].into());
        let incoming: Option<BTreeMap<String, i32>> =
            Some([("b".to_owned(), 20), ("c".to_owned(), 30)].into());
        merge_optional_map(&mut base, incoming);
        let m = base.unwrap();
        assert_eq!(m.get("a"), Some(&1), "untouched key stays");
        assert_eq!(m.get("b"), Some(&20), "collision: incoming wins");
        assert_eq!(m.get("c"), Some(&30), "new key appended");
    }

    #[test]
    fn merge_optional_map_materialises_base_when_only_incoming_has_entries() {
        let mut base: Option<BTreeMap<String, i32>> = None;
        let incoming: Option<BTreeMap<String, i32>> = Some([("a".to_owned(), 1)].into());
        merge_optional_map(&mut base, incoming);
        assert_eq!(base.as_ref().unwrap().get("a"), Some(&1));
    }

    #[test]
    fn merge_optional_map_with_none_incoming_is_a_noop() {
        let mut base: Option<BTreeMap<String, i32>> = Some([("a".to_owned(), 1)].into());
        merge_optional_map::<String, i32>(&mut base, None);
        assert_eq!(base.as_ref().unwrap().get("a"), Some(&1));
    }

    #[test]
    fn merge_optional_replaces_only_when_incoming_is_some() {
        let mut base: Option<String> = Some("base".to_owned());
        merge_optional::<String>(&mut base, None);
        assert_eq!(base.as_deref(), Some("base"));
        merge_optional(&mut base, Some("incoming".to_owned()));
        assert_eq!(base.as_deref(), Some("incoming"));
    }

    #[test]
    fn merge_optional_list_keeps_base_when_incoming_is_none_or_empty() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2, 3]);
        merge_optional_list::<i32>(&mut base, None);
        assert_eq!(base.as_deref(), Some([1, 2, 3].as_slice()));
        merge_optional_list(&mut base, Some(Vec::<i32>::new()));
        assert_eq!(
            base.as_deref(),
            Some([1, 2, 3].as_slice()),
            "empty incoming must not wipe populated base",
        );
    }

    #[test]
    fn merge_optional_list_replaces_wholesale_when_incoming_is_non_empty() {
        let mut base: Option<Vec<i32>> = Some(vec![1, 2, 3]);
        merge_optional_list(&mut base, Some(vec![9, 8]));
        assert_eq!(base.as_deref(), Some([9, 8].as_slice()));
    }

    #[test]
    fn merge_named_list_replaces_matching_names_and_appends_new() {
        #[derive(Debug, PartialEq, Eq)]
        struct Named {
            name: String,
            v: i32,
        }
        let mut base: Option<Vec<Named>> = Some(vec![
            Named {
                name: "a".to_owned(),
                v: 1,
            },
            Named {
                name: "b".to_owned(),
                v: 2,
            },
        ]);
        let incoming: Option<Vec<Named>> = Some(vec![
            Named {
                name: "b".to_owned(),
                v: 20,
            },
            Named {
                name: "c".to_owned(),
                v: 30,
            },
        ]);
        merge_named_list(&mut base, incoming, |t| &t.name);
        let v = base.unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(
            v[0],
            Named {
                name: "a".to_owned(),
                v: 1
            }
        );
        assert_eq!(
            v[1],
            Named {
                name: "b".to_owned(),
                v: 20
            }
        );
        assert_eq!(
            v[2],
            Named {
                name: "c".to_owned(),
                v: 30
            }
        );
    }
}
