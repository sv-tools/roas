//! The Path Item Objects of a description, resolved once.
//!
//! A Path Item Object may be a `$ref`, and the specification is careful
//! about what that means: only a field appearing in **both** the local
//! object and the referenced one has
//! [undefined behavior](https://spec.openapis.org/oas/v3.2.0#path-item-object).
//! Fields that appear in one and not the other are simply present — so
//! a local `parameters` beside a `$ref` that carries the operations is a
//! Path Item Object with both, not a choice between them.
//!
//! Resolving happens once, when the validator is built, rather than per
//! request: this is middleware, and the answer never changes.

use std::collections::{BTreeMap, BTreeSet};

use roas::common::reference::ResolveReference;
use roas::v3_2::path_item::PathItem;
use roas::v3_2::spec::Spec;

/// Every Path Item Object of a description, with its `$ref` chain
/// followed and merged.
pub(crate) fn resolve(spec: &Spec) -> BTreeMap<String, PathItem> {
    let Some(paths) = &spec.paths else {
        return BTreeMap::new();
    };
    paths
        .iter()
        .map(|(template, item)| {
            let mut seen = BTreeSet::new();
            (template.clone(), resolve_one(spec, item, &mut seen))
        })
        .collect()
}

/// Follow one Path Item Object's reference, and its reference, and so
/// on — a component may point at another component.
///
/// When the chain cannot be finished — an external reference, a name
/// that resolves to nothing, or a cycle — the `reference` field is left
/// in place rather than cleared. That is what tells the validator the
/// Path Item Object is incomplete, so it can say so instead of treating
/// the missing half as absent.
fn resolve_one(spec: &Spec, item: &PathItem, seen: &mut BTreeSet<String>) -> PathItem {
    let Some(reference) = &item.reference else {
        return item.clone();
    };
    // External references are the loader's business, not this crate's.
    if !reference.starts_with("#/") {
        return item.clone();
    }
    // A chain that comes back to a reference it already followed would
    // otherwise never end.
    if !seen.insert(reference.clone()) {
        return item.clone();
    }
    let Some(referenced) = ResolveReference::<PathItem>::resolve_reference(spec, reference) else {
        return item.clone();
    };
    let referenced = resolve_one(spec, referenced, seen);
    let mut merged = merge(item, &referenced);
    // Only a chain that finished counts as followed.
    if referenced.reference.is_some() {
        merged.reference = item.reference.clone();
    }
    merged
}

/// A Path Item Object with the one it references filled in behind it.
///
/// Local wins wherever both define the same field — that case is
/// undefined, and preferring what is written at the call site is the
/// least surprising reading of it. Operations merge per method, since
/// each method is a field of the Path Item Object in its own right: a
/// local `get` beside a referenced `post` gives a path item with both.
fn merge(local: &PathItem, referenced: &PathItem) -> PathItem {
    PathItem {
        // Followed; `resolve_one` puts it back if the chain did not
        // actually finish.
        reference: None,
        summary: local.summary.clone().or_else(|| referenced.summary.clone()),
        description: local
            .description
            .clone()
            .or_else(|| referenced.description.clone()),
        operations: merge_operations(local.operations.as_ref(), referenced.operations.as_ref()),
        additional_operations: merge_operations(
            local.additional_operations.as_ref(),
            referenced.additional_operations.as_ref(),
        ),
        servers: local.servers.clone().or_else(|| referenced.servers.clone()),
        parameters: local
            .parameters
            .clone()
            .or_else(|| referenced.parameters.clone()),
        extensions: local
            .extensions
            .clone()
            .or_else(|| referenced.extensions.clone()),
    }
}

fn merge_operations<T: Clone>(
    local: Option<&BTreeMap<String, T>>,
    referenced: Option<&BTreeMap<String, T>>,
) -> Option<BTreeMap<String, T>> {
    if local.is_none() && referenced.is_none() {
        return None;
    }
    let mut merged = referenced.cloned().unwrap_or_default();
    if let Some(local) = local {
        // Local wins per method, which is the field-level granularity
        // the specification talks about.
        merged.extend(
            local
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(spec: serde_json::Value) -> BTreeMap<String, PathItem> {
        let spec: Spec = serde_json::from_value(spec).expect("the description must parse");
        resolve(&spec)
    }

    fn shared(local: serde_json::Value) -> PathItem {
        let mut paths = resolved(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" },
            "paths": { "/x": local },
            "components": { "pathItems": { "Shared": {
                "summary": "shared",
                "servers": [{ "url": "/v2" }],
                "parameters": [
                    { "name": "X-Ref", "in": "header", "required": true,
                      "schema": { "type": "string" } }
                ],
                "get": { "operationId": "referencedGet" },
                "post": { "operationId": "referencedPost" }
            } } }
        }));
        paths.remove("/x").expect("the path must be present")
    }

    fn operation_ids(item: &PathItem) -> Vec<(String, String)> {
        item.operations
            .iter()
            .flatten()
            .map(|(key, operation)| {
                (
                    key.clone(),
                    operation.operation_id.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    #[test]
    fn a_plain_path_item_is_itself() {
        let item = shared(json!({ "get": { "operationId": "local" } }));
        assert_eq!(
            operation_ids(&item),
            [("get".to_owned(), "local".to_owned())]
        );
        assert!(item.parameters.is_none());
    }

    #[test]
    fn a_bare_reference_is_what_it_names() {
        let item = shared(json!({ "$ref": "#/components/pathItems/Shared" }));
        assert_eq!(item.summary.as_deref(), Some("shared"));
        assert_eq!(item.parameters.as_ref().map(Vec::len), Some(1));
        assert_eq!(
            operation_ids(&item),
            [
                ("get".to_owned(), "referencedGet".to_owned()),
                ("post".to_owned(), "referencedPost".to_owned()),
            ],
        );
        // Followed, so nothing follows it twice.
        assert!(item.reference.is_none());
    }

    #[test]
    fn a_field_beside_a_reference_is_kept_rather_than_dropped() {
        // The reason this matters: a required parameter written at the
        // call site must not vanish because the operations came from a
        // component.
        let item = shared(json!({
            "$ref": "#/components/pathItems/Shared",
            "parameters": [
                { "name": "X-Local", "in": "header", "required": true,
                  "schema": { "type": "string" } }
            ]
        }));
        assert_eq!(item.parameters.as_ref().map(Vec::len), Some(1));
        assert_eq!(operation_ids(&item).len(), 2);
    }

    #[test]
    fn operations_merge_per_method() {
        let item = shared(json!({
            "$ref": "#/components/pathItems/Shared",
            "get": { "operationId": "localGet" }
        }));
        assert_eq!(
            operation_ids(&item),
            [
                // Local wins for `get` — the one field defined twice.
                ("get".to_owned(), "localGet".to_owned()),
                // And `post` still arrives from the reference.
                ("post".to_owned(), "referencedPost".to_owned()),
            ],
        );
    }

    #[test]
    fn a_local_server_wins_over_the_referenced_one() {
        let item = shared(json!({
            "$ref": "#/components/pathItems/Shared",
            "servers": [{ "url": "/v9" }]
        }));
        assert_eq!(
            item.servers.as_ref().map(|servers| servers[0].url.clone()),
            Some("/v9".to_owned()),
        );
    }

    #[test]
    fn a_reference_chain_is_followed_to_its_end() {
        let mut paths = resolved(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" },
            "paths": { "/x": { "$ref": "#/components/pathItems/A" } },
            "components": { "pathItems": {
                "A": { "$ref": "#/components/pathItems/B" },
                "B": { "get": { "operationId": "deep" } }
            } }
        }));
        let item = paths.remove("/x").expect("the path must be present");
        assert_eq!(
            operation_ids(&item),
            [("get".to_owned(), "deep".to_owned())]
        );
        assert!(item.reference.is_none());
    }

    #[test]
    fn a_reference_cycle_ends_rather_than_recurring_forever() {
        let mut paths = resolved(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" },
            "paths": { "/x": { "$ref": "#/components/pathItems/A" } },
            "components": { "pathItems": {
                "A": { "$ref": "#/components/pathItems/B" },
                "B": { "$ref": "#/components/pathItems/A" }
            } }
        }));
        let item = paths.remove("/x").expect("the path must be present");
        // Unfinished, so the reference stays for the caller to report.
        assert_eq!(item.reference.as_deref(), Some("#/components/pathItems/A"));
    }

    #[test]
    fn a_chain_that_cannot_finish_keeps_its_reference() {
        let mut paths = resolved(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" },
            "paths": { "/x": { "$ref": "#/components/pathItems/A" } },
            "components": { "pathItems": {
                "A": { "$ref": "#/components/pathItems/Gone" }
            } }
        }));
        let item = paths.remove("/x").expect("the path must be present");
        assert_eq!(item.reference.as_deref(), Some("#/components/pathItems/A"));
    }

    #[test]
    fn a_reference_that_names_nothing_leaves_the_path_item_as_written() {
        let item = shared(json!({
            "$ref": "#/components/pathItems/Gone",
            "get": { "operationId": "local" }
        }));
        assert_eq!(
            operation_ids(&item),
            [("get".to_owned(), "local".to_owned())]
        );
    }

    #[test]
    fn an_external_reference_is_left_for_the_loader() {
        let item = shared(json!({ "$ref": "other.yaml#/paths/~1x" }));
        assert_eq!(item.reference.as_deref(), Some("other.yaml#/paths/~1x"));
    }

    #[test]
    fn a_description_without_paths_resolves_to_nothing() {
        let empty = resolved(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" }
        }));
        assert!(empty.is_empty());
    }
}
