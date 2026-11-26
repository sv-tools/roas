//! Version-agnostic apply helpers: JSON merging per the Overlay
//! [§4.4.3.1 merging rules](https://spec.openapis.org/overlay/v1.0.0.html#merging-rules)
//! and a thin wrapper around [`serde_json_path`].

use std::cmp::Ordering;
use std::collections::HashSet;

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json_path::JsonPath;

/// Deserialize a field that is present as `Some`, `null` included, so an
/// explicit `update: null` is told apart from a missing `update`; pair with
/// `#[serde(default)]` for the missing case.
pub(crate) fn present_value<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(de).map(Some)
}

/// Compile a JSONPath query string into a typed [`JsonPath`]. Returns
/// the underlying parser error as a string so callers can surface it
/// in [`ApplyError`](crate::apply::ApplyError) without leaking the
/// `serde_json_path` types from their public API.
pub fn compile_path(s: &str) -> Result<JsonPath, String> {
    JsonPath::parse(s).map_err(|e| e.to_string())
}

/// Evaluate a compiled JSONPath against `doc` and return matched
/// locations as RFC 6901 JSON Pointers, in document order.
///
/// The returned pointers are owned strings; the caller is free to
/// keep mutating `doc` afterwards without lifetime conflicts with the
/// borrow taken by `query_located`.
pub fn locate(doc: &Value, path: &JsonPath) -> Vec<String> {
    path.query_located(doc)
        .into_iter()
        .map(|n| n.location().to_json_pointer())
        .collect()
}

/// Recursively merge `update` into `target`, per Overlay
/// [§4.4.3.1](https://spec.openapis.org/overlay/v1.0.0.html#merging-rules):
///
/// - Both objects → per-key recursive merge (keys only in `update`
///   inserted, keys only in `target` kept, shared keys recurse).
/// - Both arrays  → concatenate (`update` items appended to `target`).
/// - Anything else (shape mismatch, both primitives, `null` on either
///   side) → replace `target` with a clone of `update`.
pub fn merge_json(target: &mut Value, update: &Value) {
    match (target, update) {
        (Value::Object(t), Value::Object(u)) => {
            for (k, v) in u {
                match t.get_mut(k) {
                    Some(existing) => merge_json(existing, v),
                    None => {
                        t.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (Value::Array(t), Value::Array(u)) => {
            t.extend(u.iter().cloned());
        }
        (slot, other) => {
            *slot = other.clone();
        }
    }
}

/// Remove the value at `pointer` from `doc` (RFC 6901). Returns
/// `true` if the value was removed, `false` if the pointer didn't
/// resolve to a removable child (root, missing key, out-of-range
/// index).
pub fn remove_at(doc: &mut Value, pointer: &str) -> bool {
    if pointer.is_empty() {
        // Removing the document root is undefined; treat as no-op so
        // a malformed action doesn't accidentally clobber the input.
        return false;
    }
    let (parent_ptr, last_segment) = match pointer.rsplit_once('/') {
        Some(pair) => pair,
        None => return false,
    };
    let key = unescape_pointer_segment(last_segment);
    let Some(parent) = doc.pointer_mut(parent_ptr) else {
        return false;
    };
    match parent {
        Value::Object(map) => map.remove(&key).is_some(),
        Value::Array(vec) => {
            let Ok(idx) = key.parse::<usize>() else {
                return false;
            };
            if idx >= vec.len() {
                return false;
            }
            vec.remove(idx);
            true
        }
        _ => false,
    }
}

/// Remove duplicate RFC 6901 pointers while preserving first-seen order.
///
/// RFC 9535 JSONPath queries with overlapping union selectors (e.g.
/// `$.items[0, 0]` or `$.items[*, 0]`) can select the same node more
/// than once; deduplicating ensures each matched node is mutated or
/// removed at most once.
pub(crate) fn dedup_pointers(pointers: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::with_capacity(pointers.len());
    pointers
        .into_iter()
        .filter(|ptr| seen.insert(ptr.clone()))
        .collect()
}

/// Order RFC 6901 pointers so that removing each one in sequence never
/// shifts or invalidates a pointer that has not been removed yet:
/// deepest paths first, and among equal-depth paths, higher array
/// indices before lower ones (compared numerically, not
/// lexicographically).
pub(crate) fn sort_for_remove(pointers: &mut [String]) {
    pointers.sort_by(|a, b| compare_remove_order(a, b));
}

fn compare_remove_order(a: &str, b: &str) -> Ordering {
    let depth_a = a.bytes().filter(|&byte| byte == b'/').count();
    let depth_b = b.bytes().filter(|&byte| byte == b'/').count();
    // Deeper paths first so descendants are removed before any ancestor
    // or ancestor-sibling array element can shift them. Within equal
    // depth, numeric segments precede non-numeric ones (forming a
    // strict weak order), sorted descending numerically and tie-broken
    // by raw string.
    depth_b.cmp(&depth_a).then_with(|| {
        for (sa, sb) in a.split('/').skip(1).zip(b.split('/').skip(1)) {
            let ord = match (sa.parse::<usize>(), sb.parse::<usize>()) {
                (Ok(ia), Ok(ib)) => ib.cmp(&ia).then_with(|| sb.cmp(sa)),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => sb.cmp(sa),
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    })
}

/// Unescape a single JSON Pointer segment per
/// [RFC 6901 §4](https://www.rfc-editor.org/rfc/rfc6901#section-4):
/// `~1` → `/`, `~0` → `~`. The order matters — unescape `~1` first.
pub(crate) fn unescape_pointer_segment(seg: &str) -> String {
    seg.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_objects_recurse_per_key() {
        let mut target = json!({ "a": 1, "nested": { "x": "old", "kept": true } });
        let update = json!({ "b": 2, "nested": { "x": "new", "added": 42 } });
        merge_json(&mut target, &update);
        assert_eq!(
            target,
            json!({
                "a": 1,
                "b": 2,
                "nested": { "x": "new", "kept": true, "added": 42 }
            }),
        );
    }

    #[test]
    fn merge_arrays_concatenate() {
        let mut target = json!([1, 2, 3]);
        merge_json(&mut target, &json!([4, 5]));
        assert_eq!(target, json!([1, 2, 3, 4, 5]));
    }

    #[test]
    fn merge_primitive_replaces() {
        let mut target = json!("old");
        merge_json(&mut target, &json!("new"));
        assert_eq!(target, json!("new"));
    }

    #[test]
    fn merge_shape_mismatch_replaces() {
        let mut target = json!({ "a": 1 });
        merge_json(&mut target, &json!([1, 2]));
        assert_eq!(target, json!([1, 2]));

        let mut target = json!([1, 2]);
        merge_json(&mut target, &json!({ "a": 1 }));
        assert_eq!(target, json!({ "a": 1 }));
    }

    #[test]
    fn merge_null_replaces() {
        let mut target = json!({ "a": 1 });
        merge_json(&mut target, &json!(null));
        assert_eq!(target, json!(null));
    }

    #[test]
    fn locate_returns_pointers_in_document_order() {
        let doc = json!({
            "info": { "title": "x" },
            "paths": {
                "/pets": { "get": { "summary": "list" } },
                "/users": { "get": { "summary": "list" } }
            }
        });
        let path = compile_path("$.paths.*.get").unwrap();
        let mut ptrs = locate(&doc, &path);
        ptrs.sort();
        assert_eq!(ptrs, vec!["/paths/~1pets/get", "/paths/~1users/get"]);
    }

    #[test]
    fn compile_path_returns_error_for_invalid_syntax() {
        let err = compile_path("not a path").unwrap_err();
        assert!(!err.is_empty(), "expected non-empty parser error");
    }

    #[test]
    fn remove_at_object_key_returns_true() {
        let mut doc = json!({ "a": 1, "b": 2 });
        assert!(remove_at(&mut doc, "/a"));
        assert_eq!(doc, json!({ "b": 2 }));
    }

    #[test]
    fn remove_at_array_index_returns_true_and_shifts() {
        let mut doc = json!([10, 20, 30]);
        assert!(remove_at(&mut doc, "/1"));
        assert_eq!(doc, json!([10, 30]));
    }

    #[test]
    fn remove_at_handles_escaped_segments() {
        let mut doc = json!({ "paths": { "/pets": { "get": {} } } });
        assert!(remove_at(&mut doc, "/paths/~1pets"));
        assert_eq!(doc, json!({ "paths": {} }));
    }

    #[test]
    fn remove_at_returns_false_for_unknown_pointer() {
        let mut doc = json!({ "a": 1 });
        assert!(!remove_at(&mut doc, "/missing"));
        assert!(!remove_at(&mut doc, "/a/deeper"));
        assert!(!remove_at(&mut doc, "")); // root
    }

    #[test]
    fn remove_at_out_of_range_array_index_returns_false() {
        let mut doc = json!([1, 2, 3]);
        assert!(!remove_at(&mut doc, "/9"));
        assert!(!remove_at(&mut doc, "/notnum"));
    }

    #[test]
    fn sort_for_remove_orders_deepest_first_and_numeric_indices_descending() {
        let mut ptrs = vec![
            "/items/2".to_owned(),
            "/items/10".to_owned(),
            "/items/0".to_owned(),
            "/items/0/children/0".to_owned(),
            "/items/0/children/2".to_owned(),
        ];
        sort_for_remove(&mut ptrs);
        assert_eq!(
            ptrs,
            vec![
                "/items/0/children/2",
                "/items/0/children/0",
                "/items/10",
                "/items/2",
                "/items/0",
            ]
        );

        // Mixed numeric and non-numeric keys at the same depth must form a
        // strict weak order (no transitivity cycle).
        let mut mixed = vec![
            "/1a".to_owned(),
            "/2".to_owned(),
            "/10".to_owned(),
            "/01".to_owned(),
            "/1".to_owned(),
            "/0a".to_owned(),
        ];
        sort_for_remove(&mut mixed);
        assert_eq!(mixed, vec!["/10", "/2", "/1", "/01", "/1a", "/0a"]);
    }

    #[test]
    fn dedup_pointers_preserves_first_seen_order() {
        let ptrs = vec![
            "/items/2".to_owned(),
            "/items/0".to_owned(),
            "/items/2".to_owned(),
            "/items/1".to_owned(),
        ];
        assert_eq!(
            dedup_pointers(ptrs),
            vec!["/items/2", "/items/0", "/items/1"]
        );
    }
}
