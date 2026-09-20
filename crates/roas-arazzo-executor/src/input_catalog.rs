//! Discover schema names without compiling unused schemas or resolving their refs.

use crate::operation_document::decode_pointer;
use jsonschema::Draft;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

type Root = (Url, String);
pub(crate) type Documents = BTreeMap<Url, (Url, Value)>;

/// Selection is at schema-root granularity: a workflow input, reusable input,
/// OpenAPI component schema, or complete standalone schema document. All schema
/// children of a selected root remain intact, including definitions and dynamic
/// anchors; this is not assertion-level pruning or JSON Schema evaluation.
#[derive(Debug)]
pub(crate) struct Catalog {
    documents: Documents,
    roots: BTreeMap<Url, Vec<String>>,
    aliases: BTreeMap<Url, BTreeSet<Url>>,
    names: BTreeMap<Url, BTreeSet<Root>>,
    resources: BTreeMap<Url, BTreeSet<Root>>,
    claims: BTreeMap<Root, BTreeSet<Url>>,
}

impl Catalog {
    pub(crate) fn new(documents: Documents) -> Self {
        let mut catalog = Self {
            documents,
            roots: BTreeMap::new(),
            aliases: BTreeMap::new(),
            names: BTreeMap::new(),
            resources: BTreeMap::new(),
            claims: BTreeMap::new(),
        };
        for (uri, (base, document)) in &mut catalog.documents {
            for alias in [uri, base] {
                catalog
                    .aliases
                    .entry(alias.clone())
                    .or_default()
                    .insert(uri.clone());
            }
            let roots = roots(document);
            for pointer in &roots {
                let root = (uri.clone(), pointer.clone());
                let mut names = BTreeSet::new();
                if pointer.is_empty() {
                    names.insert(uri.clone());
                }
                discover(
                    document.pointer_mut(pointer).expect("indexed schema root"),
                    base,
                    &root,
                    &mut names,
                    &mut catalog.resources,
                );
                // Indexing is deliberately non-validating. Malformed identifiers
                // and refs are reported by normalization if their root is used.
                for name in &names {
                    catalog
                        .names
                        .entry(name.clone())
                        .or_default()
                        .insert(root.clone());
                }
                catalog.claims.insert(root, names);
            }
            catalog.roots.insert(uri.clone(), roots);
        }
        catalog
    }

    pub(crate) fn reachable(&self, entry: &Url) -> Documents {
        let mut documents = BTreeSet::new();
        let mut pending = self.targets(entry, &mut documents);
        let mut selected = BTreeSet::new();
        while let Some(root) = pending.pop_first() {
            if !selected.insert(root.clone()) {
                continue;
            }
            let (uri, pointer) = &root;
            documents.insert(uri.clone());
            let (base, document) = &self.documents[uri];
            // A document's retrieval and canonical aliases must expose the same
            // projection, even when different refs enter through each alias.
            if let Some(aliases) = self.aliases.get(base) {
                for alias in aliases {
                    let (alias_base, document) = &self.documents[alias];
                    if alias_base == base && document.pointer(pointer).is_some() {
                        pending.insert((alias.clone(), pointer.clone()));
                    }
                }
            }
            let schema = document.pointer(pointer).expect("indexed schema root");
            let mut references = BTreeSet::new();
            references_from(schema, &self.parent_scope(uri, pointer), &mut references);
            for reference in references {
                pending.extend(self.targets(&reference, &mut documents));
            }
            // Keep every claimant of a used identity. Normalization must reject
            // ambiguity instead of silently choosing whichever was indexed last.
            if let Some(claims) = self.claims.get(&root) {
                for name in claims {
                    pending.extend(self.names[name].iter().cloned());
                }
            }
        }
        documents
            .into_iter()
            .map(|uri| {
                let (base, document) = &self.documents[&uri];
                let mut document = document.clone();
                for pointer in &self.roots[&uri] {
                    if !selected.contains(&(uri.clone(), pointer.clone())) {
                        remove_root(&mut document, pointer);
                    }
                }
                (uri, (base.clone(), document))
            })
            .collect()
    }

    fn targets(&self, reference: &Url, documents: &mut BTreeSet<Url>) -> BTreeSet<Root> {
        let mut targets = self.names.get(reference).cloned().unwrap_or_default();
        let mut resource = reference.clone();
        resource.set_fragment(None);
        // A named schema resource may live inside a larger document. Preserve
        // that entire root even when the reference uses a pointer within it.
        if let Some(roots) = self.names.get(&resource) {
            targets.extend(roots.iter().cloned());
        }
        if let Ok(pointer) = decode_pointer(reference.fragment().unwrap_or_default())
            && let Some(resources) = self.resources.get(&resource)
        {
            for (uri, root) in resources {
                let pointer = format!("{root}{pointer}");
                if self.documents[uri].1.pointer(&pointer).is_some() {
                    targets.insert((uri.clone(), pointer));
                }
            }
        }
        if let Some(aliases) = self.aliases.get(&resource) {
            documents.extend(aliases.iter().cloned());
            for uri in aliases {
                let mut canonical = self.documents[uri].0.clone();
                canonical.set_fragment(reference.fragment());
                if let Some(roots) = self.names.get(&canonical) {
                    targets.extend(roots.iter().cloned());
                }
            }
            if let Ok(pointer) = decode_pointer(reference.fragment().unwrap_or_default()) {
                for uri in aliases {
                    for root in &self.roots[uri] {
                        if within(&pointer, root) || within(root, &pointer) {
                            targets.insert((uri.clone(), root.clone()));
                        }
                    }
                    // JSON Pointers may enter other schema-bearing positions
                    // (for example an inline OpenAPI parameter schema). Follow
                    // their refs too, without scanning arbitrary annotation data.
                    if self.documents[uri].1.pointer(&pointer).is_some() {
                        targets.insert((uri.clone(), pointer.clone()));
                    }
                }
            }
        }
        targets
    }

    /// Scope inherited by a pointer target. Only schema-valued ancestors can
    /// change scope; an `$id` in annotation data is not an enclosing resource.
    fn parent_scope(&self, uri: &Url, pointer: &str) -> Url {
        let (base, document) = &self.documents[uri];
        let Some(root) = self.roots[uri].iter().find(|root| within(pointer, root)) else {
            return base.clone();
        };
        if root == pointer {
            return base.clone();
        }
        let mut schema = document.pointer(root).expect("indexed schema root");
        let mut base = scope(schema, base);
        for (end, _) in pointer
            .match_indices('/')
            .filter(|(end, _)| *end > root.len())
        {
            let ancestor = document
                .pointer(&pointer[..end])
                .expect("ancestor of a resolved pointer");
            if Draft::Draft202012
                .subresources_of(schema)
                .any(|child| std::ptr::eq(child, ancestor))
            {
                base = scope(ancestor, &base);
                schema = ancestor;
            }
        }
        base
    }
}

fn within(pointer: &str, parent: &str) -> bool {
    pointer == parent
        || pointer
            .strip_prefix(parent)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn roots(document: &Value) -> Vec<String> {
    let mut roots = Vec::new();
    let reusable = if document.get("arazzo").is_some() {
        if let Some(workflows) = document.get("workflows").and_then(Value::as_array) {
            for (index, workflow) in workflows.iter().enumerate() {
                if workflow.get("inputs").is_some() {
                    roots.push(format!("/workflows/{index}/inputs"));
                }
            }
        }
        "/components/inputs"
    } else if document.get("openapi").is_some() {
        "/components/schemas"
    } else {
        return vec![String::new()];
    };
    if let Some(schemas) = document.pointer(reusable).and_then(Value::as_object) {
        roots.extend(
            schemas
                .keys()
                .map(|name| format!("{reusable}/{}", name.replace('~', "~0").replace('/', "~1"))),
        );
    }
    roots
}

fn scope(schema: &Value, parent: &Url) -> Url {
    declared_scope(schema, parent).unwrap_or_else(|| parent.clone())
}

fn declared_scope(schema: &Value, parent: &Url) -> Option<Url> {
    let mut uri = parent.join(schema.get("$id")?.as_str()?).ok()?;
    if uri.fragment() == Some("") {
        uri.set_fragment(None);
    }
    Some(uri)
}

fn discover(
    schema: &mut Value,
    parent: &Url,
    location: &Root,
    names: &mut BTreeSet<Url>,
    resources: &mut BTreeMap<Url, BTreeSet<Root>>,
) {
    let base = scope(schema, parent);
    if let Some(uri) = declared_scope(schema, parent) {
        resources
            .entry(uri.clone())
            .or_default()
            .insert(location.clone());
        names.insert(uri);
    }
    for keyword in ["$anchor", "$dynamicAnchor"] {
        if let Some(anchor) = schema.get(keyword).and_then(Value::as_str) {
            let mut uri = base.clone();
            uri.set_fragment(Some(anchor));
            names.insert(uri);
        }
    }
    if let Some(object) = schema.as_object_mut() {
        for (path, child) in children(object) {
            discover(
                child,
                &base,
                &(location.0.clone(), format!("{}{path}", location.1)),
                names,
                resources,
            );
        }
    }
}

fn references_from(schema: &Value, parent: &Url, references: &mut BTreeSet<Url>) {
    let base = scope(schema, parent);
    for keyword in ["$ref", "$dynamicRef"] {
        if let Some(reference) = schema.get(keyword).and_then(Value::as_str)
            && let Ok(uri) = base.join(reference)
        {
            references.insert(uri);
        }
    }
    for child in Draft::Draft202012.subresources_of(schema) {
        references_from(child, &base, references);
    }
}

fn remove_root(document: &mut Value, pointer: &str) {
    let Some((parent, name)) = pointer.rsplit_once('/') else {
        return;
    };
    if let Some(object) = document.pointer_mut(parent).and_then(Value::as_object_mut) {
        object.remove(&name.replace("~1", "/").replace("~0", "~"));
    }
}

/// Schema-valued keyword positions, never arbitrary JSON objects or annotations.
/// Shared by identity discovery and normalization so their traversal stays aligned.
pub(crate) fn children(object: &mut serde_json::Map<String, Value>) -> Vec<(String, &mut Value)> {
    let mut children = Vec::new();
    for (name, value) in object {
        match name.as_str() {
            "additionalProperties"
            | "contains"
            | "contentSchema"
            | "else"
            | "if"
            | "items"
            | "not"
            | "propertyNames"
            | "then"
            | "unevaluatedItems"
            | "unevaluatedProperties" => {
                children.push((format!("/{name}"), value));
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(values) = value.as_array_mut() {
                    children.extend(
                        values
                            .iter_mut()
                            .enumerate()
                            .map(|(index, value)| (format!("/{name}/{index}"), value)),
                    );
                }
            }
            "$defs" | "definitions" | "dependentSchemas" | "patternProperties" | "properties" => {
                if let Some(values) = value.as_object_mut() {
                    children.extend(values.iter_mut().map(|(key, value)| {
                        (
                            format!("/{name}/{}", key.replace('~', "~0").replace('/', "~1")),
                            value,
                        )
                    }));
                }
            }
            _ => {}
        }
    }
    children
}
