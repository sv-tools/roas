//! Forward conversion from OpenAPI v3.1 to OpenAPI v3.2.
//!
//! Converts a [`crate::v3_1::spec::Spec`] into a
//! [`crate::v3_2::spec::Spec`] by reshaping the document on-the-fly
//! via `serde_json::Value`. The two versions are deeply compatible:
//! every 3.1 document is structurally valid 3.2, so the only required
//! migration is the `openapi` version bump
//! (`openapi: "3.1.x"` → `openapi: "3.2.0"`).
//!
//! On top of that, this module folds two Redoc-flavoured extensions
//! into the native 3.2 fields that supersede them, per the official
//! upgrade guide
//! (<https://learn.openapis.org/upgrading/v3.1-to-v3.2.html>):
//!
//! 1. On each `tags[*]`: `x-displayName: "<label>"` →
//!    `summary: "<label>"` (3.2 introduced a native `summary` field
//!    on tags).
//! 2. Top-level `x-tagGroups: [{name, tags: […]}]` →
//!    distribute as `parent: "<group>"` on each member tag, and add
//!    a synthesised tag `{name: "<group>", kind: "nav"}` for the
//!    group itself. `x-tagGroups` is then dropped. Membership rules:
//!    a member that's already declared in `tags` gains a `parent`
//!    pointer; a member that isn't declared is synthesised as a leaf
//!    tag.
//!
//! Everything else — server `name`, OAuth2 device-code flow,
//! `Discriminator.defaultMapping`, security-scheme `deprecated`,
//! Example `dataValue` — are pure 3.2 additions a user opts into,
//! not migrations from 3.1 shape, so they're not synthesised.
//!
//! The conversion serialises the v3.1 input with serde, runs the
//! transforms, and deserialises as a v3.2 spec. A valid 3.1 input
//! produces a structurally valid 3.2 document; semantic regressions
//! are surfaced by `Spec::validate`.

use crate::v3_1::spec::Spec as V31Spec;
use crate::v3_2::spec::Spec as V32Spec;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

impl From<V31Spec> for V32Spec {
    fn from(v31: V31Spec) -> Self {
        let mut value = serde_json::to_value(v31).expect("v3_1::Spec serializes");
        transform_spec(&mut value);
        serde_json::from_value(value).expect("transformed spec deserializes as v3_2::Spec")
    }
}

fn transform_spec(spec: &mut Value) {
    let Value::Object(obj) = spec else {
        return;
    };
    obj.insert("openapi".into(), Value::String("3.2.0".to_owned()));

    let tag_groups = obj.remove("x-tagGroups");
    if let Some(Value::Array(tags)) = obj.get_mut("tags") {
        for tag in tags.iter_mut() {
            if let Value::Object(t) = tag {
                migrate_x_display_name(t);
            }
        }
    }
    if let Some(Value::Array(groups)) = tag_groups {
        apply_tag_groups(obj, groups);
    }
}

/// `x-displayName` → `summary` on a tag. 3.2 introduced `summary` as
/// a first-class field; the Redoc-specific extension is the same
/// human-readable label.
fn migrate_x_display_name(tag: &mut Map<String, Value>) {
    if let Some(v) = tag.remove("x-displayName") {
        // Don't clobber a `summary` the user already declared.
        tag.entry("summary").or_insert(v);
    }
}

/// Convert v3.1's `x-tagGroups: [{name, tags: […]}]` into native 3.2
/// hierarchical tags. Each group becomes a synthesised tag with
/// `kind: "nav"`; each member is rewritten to carry
/// `parent: "<group>"`. Members that aren't yet declared in
/// `tags[*]` are added as leaf tags so the parent reference resolves.
fn apply_tag_groups(spec: &mut Map<String, Value>, groups: Vec<Value>) {
    // Pull existing tags into a name-indexed map for in-place
    // mutation while keeping a stable insertion order. Track whether
    // the source had a `tags` field at all so an explicit empty list
    // (`"tags": []`) round-trips even when the group list adds
    // nothing.
    let had_tags = spec.contains_key("tags");
    let mut tags: Vec<Value> = match spec.remove("tags") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    for (i, tag) in tags.iter().enumerate() {
        if let Some(name) = tag.get("name").and_then(|v| v.as_str()) {
            // First-declared wins for duplicate names so the
            // migration's "preserve existing X" guarantees are
            // stable.
            by_name.entry(name.to_owned()).or_insert(i);
        }
    }

    for group in groups {
        let Value::Object(mut g) = group else {
            continue;
        };
        let Some(group_name) = g.remove("name").and_then(string) else {
            continue;
        };
        let members: Vec<String> = g
            .remove("tags")
            .and_then(|v| match v {
                Value::Array(arr) => Some(arr),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(string)
            .collect();

        // Synthesise (or update) the group tag itself with `kind: nav`.
        // Any leftover keys on the group object (typically `x-*`
        // Specification Extensions; v3.1's `TagGroup` flattens them
        // via serde) migrate onto the group tag so vendor metadata
        // round-trips through `v3_2::Tag.extensions`. Existing keys
        // on the target tag win.
        let group_idx = ensure_tag(&mut tags, &mut by_name, &group_name);
        if let Value::Object(t) = &mut tags[group_idx] {
            t.entry("kind").or_insert(Value::String("nav".to_owned()));
            for (k, v) in g {
                t.entry(k).or_insert(v);
            }
        }

        // Add `parent: <group_name>` to each member, synthesising the
        // member tag if it wasn't declared. Skip self-references —
        // a group whose `tags` list contains its own name would
        // otherwise produce a self-parent that fails v3.2 tag-hierarchy
        // validation.
        for member_name in members {
            if member_name == group_name {
                continue;
            }
            let idx = ensure_tag(&mut tags, &mut by_name, &member_name);
            if let Value::Object(t) = &mut tags[idx] {
                // Don't overwrite an existing parent — first declared
                // wins (typical for ambiguous source data).
                t.entry("parent")
                    .or_insert(Value::String(group_name.clone()));
            }
        }
    }

    if !tags.is_empty() || had_tags {
        spec.insert("tags".into(), Value::Array(tags));
    }
}

fn ensure_tag(tags: &mut Vec<Value>, by_name: &mut BTreeMap<String, usize>, name: &str) -> usize {
    if let Some(&idx) = by_name.get(name) {
        return idx;
    }
    let mut new_tag = Map::new();
    new_tag.insert("name".into(), Value::String(name.to_owned()));
    let idx = tags.len();
    tags.push(Value::Object(new_tag));
    by_name.insert(name.to_owned(), idx);
    idx
}

fn string(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_1::spec::Spec as V31Spec;
    use crate::v3_2::spec::Spec as V32Spec;
    use crate::validation::{IGNORE_UNUSED, Options, Validate};

    fn convert(raw: &str) -> Value {
        let v31: V31Spec = serde_json::from_str(raw).expect("v3.1 spec parses");
        let v32: V32Spec = v31.into();
        serde_json::to_value(&v32).unwrap()
    }

    #[test]
    fn openapi_version_lifted() {
        let raw = r##"{
            "openapi": "3.1.2",
            "info": { "title": "t", "version": "1" },
            "paths": {}
        }"##;
        let value = convert(raw);
        assert_eq!(value["openapi"], "3.2.0");
    }

    #[test]
    fn x_display_name_on_tag_becomes_summary() {
        let raw = r##"{
            "openapi": "3.1.2",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "tags": [
                {"name": "pets", "x-displayName": "Pets"}
            ]
        }"##;
        let value = convert(raw);
        let tag = &value["tags"][0];
        assert_eq!(tag["name"], "pets");
        assert_eq!(tag["summary"], "Pets");
        assert!(tag.get("x-displayName").is_none());
    }

    #[test]
    fn x_display_name_does_not_clobber_existing_summary() {
        // The typed v3.1 `Tag` drops fields it doesn't declare
        // (`summary`, `parent`, `kind` are all v3.2 additions), so
        // this "preserve existing X" case can't reach the converter
        // through the `From<V31Spec>` path. Exercise the migration
        // directly on hand-built JSON to pin down the defensive
        // behaviour: `x-displayName` does NOT overwrite an existing
        // `summary`.
        let mut tag = serde_json::json!({
            "name": "pets",
            "summary": "kept",
            "x-displayName": "dropped"
        });
        super::migrate_x_display_name(tag.as_object_mut().unwrap());
        assert_eq!(tag["summary"], "kept");
        assert!(tag.get("x-displayName").is_none());
    }

    #[test]
    fn x_tag_groups_distributes_parents_and_synthesises_groups() {
        // Two pre-declared member tags get `parent` set. The group
        // tag itself is synthesised with `kind: nav`. `x-tagGroups`
        // is dropped.
        let raw = r##"{
            "openapi": "3.1.2",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "tags": [
                {"name": "books"},
                {"name": "magazines"}
            ],
            "x-tagGroups": [
                {"name": "Products", "tags": ["books", "magazines"]}
            ]
        }"##;
        let value = convert(raw);
        assert!(value.get("x-tagGroups").is_none(), "x-tagGroups dropped");
        let tags = value["tags"].as_array().unwrap();
        let by_name: BTreeMap<&str, &Value> = tags
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t))
            .collect();
        assert_eq!(by_name["books"]["parent"], "Products");
        assert_eq!(by_name["magazines"]["parent"], "Products");
        assert_eq!(by_name["Products"]["kind"], "nav");
    }

    #[test]
    fn x_tag_groups_synthesises_missing_member_tags() {
        // A tag named only in `x-tagGroups[*].tags` (no entry in
        // `tags`) is created as a leaf with `parent`.
        let raw = r##"{
            "openapi": "3.1.2",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "x-tagGroups": [
                {"name": "Core", "tags": ["pets"]}
            ]
        }"##;
        let value = convert(raw);
        let tags = value["tags"].as_array().unwrap();
        let by_name: BTreeMap<&str, &Value> = tags
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t))
            .collect();
        assert_eq!(by_name["pets"]["parent"], "Core");
        assert_eq!(by_name["Core"]["kind"], "nav");
    }

    #[test]
    fn x_tag_groups_extension_fields_migrate_onto_group_tag() {
        // v3.1's `TagGroup` flattens `x-*` extensions; when we
        // collapse the group into a native v3.2 tag those vendor
        // fields must survive — copy them onto the synthesised
        // group tag.
        let raw = r##"{
            "openapi": "3.1.2",
            "info": { "title": "t", "version": "1" },
            "paths": {},
            "x-tagGroups": [
                {
                    "name": "Products",
                    "tags": ["books"],
                    "x-icon": "shopping-bag",
                    "x-order": 1
                }
            ]
        }"##;
        let value = convert(raw);
        let tags = value["tags"].as_array().unwrap();
        let by_name: BTreeMap<&str, &Value> = tags
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t))
            .collect();
        let products = by_name["Products"];
        assert_eq!(products["kind"], "nav");
        assert_eq!(products["x-icon"], "shopping-bag");
        assert_eq!(products["x-order"], 1);
    }

    #[test]
    fn group_extension_does_not_clobber_existing_tag_extension() {
        // If the source already declared the group tag with the same
        // `x-*` key, that one wins — first-declared semantics carry
        // over to extension migration too.
        let mut spec = serde_json::json!({
            "tags": [
                {"name": "Products", "x-icon": "first"}
            ]
        });
        let groups = vec![serde_json::json!({
            "name": "Products",
            "tags": ["books"],
            "x-icon": "second"
        })];
        super::apply_tag_groups(spec.as_object_mut().unwrap(), groups);
        let tags = spec["tags"].as_array().unwrap();
        let products = tags.iter().find(|t| t["name"] == "Products").unwrap();
        assert_eq!(products["x-icon"], "first");
    }

    #[test]
    fn group_member_matching_group_name_is_skipped() {
        // `{name: "Products", tags: ["Products"]}` would otherwise
        // turn the group tag into its own parent, failing v3.2's
        // tag-hierarchy validation.
        let mut spec = serde_json::json!({});
        let groups = vec![serde_json::json!({
            "name": "Products",
            "tags": ["Products", "books"]
        })];
        super::apply_tag_groups(spec.as_object_mut().unwrap(), groups);
        let tags = spec["tags"].as_array().unwrap();
        let by_name: BTreeMap<&str, &Value> = tags
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t))
            .collect();
        // Products tag is `kind: nav` and has NO `parent` (no
        // self-reference).
        assert_eq!(by_name["Products"]["kind"], "nav");
        assert!(by_name["Products"].get("parent").is_none());
        // The legitimate member still gets parented.
        assert_eq!(by_name["books"]["parent"], "Products");
    }

    #[test]
    fn duplicate_tag_names_resolve_to_first_declared() {
        // The migration's "preserve existing X" guarantees should
        // target the first-declared tag, not the last. This matters
        // for any tag-hierarchy decision the conversion makes for
        // ambiguous source data.
        let mut spec = serde_json::json!({
            "tags": [
                {"name": "Products", "kind": "audience"},
                {"name": "Products", "kind": "badge"}
            ]
        });
        let groups = vec![serde_json::json!({
            "name": "Products",
            "tags": []
        })];
        super::apply_tag_groups(spec.as_object_mut().unwrap(), groups);
        let tags = spec["tags"].as_array().unwrap();
        // The first-declared tag keeps its `kind: audience` (the
        // migration doesn't clobber). The second-declared tag is
        // unchanged because the index lookup pointed at the first.
        assert_eq!(tags[0]["kind"], "audience");
        assert_eq!(tags[1]["kind"], "badge");
    }

    #[test]
    fn empty_tags_array_is_preserved_when_x_tag_groups_is_empty() {
        // If the source had `tags: []` and `x-tagGroups: []` (no
        // groups to expand), we must keep the explicit empty array
        // rather than dropping the field entirely.
        let mut spec = serde_json::json!({
            "tags": []
        });
        let groups: Vec<Value> = Vec::new();
        super::apply_tag_groups(spec.as_object_mut().unwrap(), groups);
        assert_eq!(spec["tags"], serde_json::json!([]));
    }

    #[test]
    fn x_tag_groups_preserves_existing_kind_on_group_tag() {
        // The typed v3.1 `Tag` drops `kind` (v3.2 addition), so a
        // pre-declared `kind` on the source can't reach the converter
        // through the public API. Exercise `apply_tag_groups`
        // directly with hand-built JSON to pin the defensive
        // behaviour down.
        let mut spec = serde_json::json!({
            "tags": [{"name": "Products", "kind": "audience"}]
        });
        let groups = vec![serde_json::json!({
            "name": "Products",
            "tags": ["books"]
        })];
        super::apply_tag_groups(spec.as_object_mut().unwrap(), groups);
        let tags = spec["tags"].as_array().unwrap();
        let products = tags.iter().find(|t| t["name"] == "Products").unwrap();
        assert_eq!(products["kind"], "audience");
    }

    #[test]
    fn x_tag_groups_keeps_existing_parent() {
        // Same constraint — `parent` is a v3.2 addition. Drive the
        // migration directly to confirm first-declared parent wins.
        let mut spec = serde_json::json!({
            "tags": [{"name": "fiction", "parent": "Books"}]
        });
        let groups = vec![serde_json::json!({
            "name": "Products",
            "tags": ["fiction"]
        })];
        super::apply_tag_groups(spec.as_object_mut().unwrap(), groups);
        let tags = spec["tags"].as_array().unwrap();
        let fiction = tags.iter().find(|t| t["name"] == "fiction").unwrap();
        assert_eq!(fiction["parent"], "Books");
    }

    /// Sweep every checked-in v3.1 fixture; each should convert and
    /// validate clean as v3.2.
    #[test]
    fn all_v3_1_fixtures_convert_to_valid_v3_2() {
        let fixtures: &[(&str, &str)] = &[
            (
                "petstore",
                include_str!("../../tests/v3_1_data/petstore.json"),
            ),
            (
                "petstore-expanded",
                include_str!("../../tests/v3_1_data/petstore-expanded.json"),
            ),
            (
                "non-oauth-scopes",
                include_str!("../../tests/v3_1_data/non-oauth-scopes.json"),
            ),
            (
                "tictactoe",
                include_str!("../../tests/v3_1_data/tictactoe.json"),
            ),
            (
                "api-with-examples",
                include_str!("../../tests/v3_1_data/api-with-examples.json"),
            ),
            (
                "callback-example",
                include_str!("../../tests/v3_1_data/callback-example.json"),
            ),
            (
                "link-example",
                include_str!("../../tests/v3_1_data/link-example.json"),
            ),
            (
                "oas-3-1-features",
                include_str!("../../tests/v3_1_data/oas-3-1-features.json"),
            ),
            ("uspto", include_str!("../../tests/v3_1_data/uspto.json")),
        ];
        let opts = Options::new() | Options::IgnoreMissingTags | IGNORE_UNUSED;
        for (name, raw) in fixtures {
            let v31: V31Spec =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{name}: parse: {e}"));
            let v32: V32Spec = v31.into();
            assert_eq!(v32.openapi.as_str(), "3.2.0", "{name} openapi version");
            if let Err(e) = v32.validate(opts, None) {
                panic!("{name}: converted spec did not validate cleanly:\n{e}");
            }
        }
    }
}
