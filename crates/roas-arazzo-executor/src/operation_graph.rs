//! Load only Path Item references, using the source traversal's IO budgets.

use crate::operation_document::{Document, Version, escape};
use crate::{DocumentId, SourceError, SourceLoadOptions, SourceRegistry};
use roas::{LoadedDocument, LoaderError};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use url::Url;

/// Located failure while loading a document needed by a Path Item reference.
/// Readable sources remain available; preparation decides if this blocks a run.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ReferenceDiagnostic {
    /// Identity/location of the document containing the reference.
    pub document: String,
    /// JSON Pointer to the referring field.
    pub pointer: String,
    /// Reference as written (empty for an invalid root Path Item).
    pub reference: String,
    /// Original loader, budget, document or pointer error.
    pub error: Arc<SourceError>,
}

impl std::fmt::Display for ReferenceDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}#{} (`{}`): {}",
            self.document, self.pointer, self.reference, self.error
        )
    }
}

#[derive(Clone)]
struct Node {
    document: Url,
    pointer: String,
    depth: usize,
    version: Version,
    origin: String,
    at: String,
    reference: String,
}

type Key = (String, String, String);
impl Node {
    fn key(&self) -> Key {
        (self.origin.clone(), self.at.clone(), self.reference.clone())
    }
}

#[derive(Default)]
pub(crate) struct References {
    queue: VecDeque<Node>,
    seeded: BTreeMap<DocumentId, usize>,
    seen: BTreeMap<(String, String), usize>,
    depths: BTreeMap<String, usize>,
    pending: BTreeMap<Key, Node>,
    errors: BTreeMap<Key, ReferenceDiagnostic>,
}

impl References {
    pub(crate) fn seed(
        &mut self,
        registry: &SourceRegistry,
        documents: &BTreeMap<DocumentId, usize>,
    ) {
        for (&id, &depth) in documents {
            if self
                .seeded
                .get(&id)
                .is_some_and(|previous| *previous <= depth)
            {
                continue;
            }
            self.seeded.insert(id, depth);
            let source = registry.document(id).expect("traversal document");
            if source.arazzo().is_some() || source.value().get("asyncapi").is_some() {
                continue;
            }
            let Ok(document) = Document::new(
                source.value(),
                Some(source.retrieval_uri().clone()),
                source.identity().to_string(),
                Version::Legacy,
            ) else {
                continue;
            };
            self.depths.insert(document.key().into(), depth);
            if let Some(paths) = source
                .value()
                .get("paths")
                .and_then(serde_json::Value::as_object)
            {
                for path in paths.keys().filter(|path| path.starts_with('/')) {
                    let pointer = format!("/paths/{}", escape(path));
                    self.queue.push_back(Node {
                        document: source.identity().clone(),
                        origin: document.key().into(),
                        at: pointer.clone(),
                        pointer,
                        depth,
                        version: document.version,
                        reference: String::new(),
                    });
                }
            }
        }
    }

    fn reject(&mut self, node: Node, error: Arc<SourceError>) {
        self.errors.insert(
            node.key(),
            ReferenceDiagnostic {
                document: node.origin.clone(),
                pointer: node.at.clone(),
                reference: node.reference.clone(),
                error,
            },
        );
        self.pending.insert(node.key(), node);
    }

    pub(crate) fn retry(&mut self) {
        self.queue
            .extend(std::mem::take(&mut self.pending).into_values());
    }

    pub(crate) fn next(
        &mut self,
        registry: &SourceRegistry,
        options: &SourceLoadOptions,
        initial_documents: usize,
        attempted: &mut BTreeSet<Url>,
        failed: &BTreeMap<Url, Arc<SourceError>>,
    ) -> Option<Url> {
        while let Some(node) = self.queue.pop_front() {
            let loaded = registry.operation_document(&node.document);
            let document = loaded.map(|(value, retrieval)| {
                Document::new(
                    value,
                    Some(retrieval.clone()),
                    node.document.to_string(),
                    node.version,
                )
            });
            let known_depth = document
                .as_ref()
                .and_then(|document| document.as_ref().ok())
                .and_then(|document| self.depths.get(document.key()))
                .copied();
            if node.depth > options.max_depth && known_depth.is_none() {
                self.reject(
                    node,
                    Arc::new(SourceError::Limit {
                        kind: "depth",
                        limit: options.max_depth,
                    }),
                );
                continue;
            }
            let document = match document {
                Some(Ok(document)) => document,
                Some(Err(error)) => {
                    self.reject(node, Arc::new(error.into()));
                    continue;
                }
                None => {
                    if let Some(error) = failed.get(&node.document) {
                        self.reject(node, Arc::clone(error));
                    } else if attempted.contains(&node.document) {
                        let error = SourceError::Unresolved(node.document.to_string());
                        self.reject(node, Arc::new(error));
                    } else if attempted.len()
                        >= options.max_documents.saturating_sub(initial_documents)
                    {
                        self.reject(
                            node,
                            Arc::new(SourceError::Limit {
                                kind: "document",
                                limit: options.max_documents,
                            }),
                        );
                    } else {
                        let uri = node.document.clone();
                        attempted.insert(uri.clone());
                        self.queue.push_front(node);
                        return Some(uri);
                    }
                    continue;
                }
            };
            self.errors.remove(&node.key());
            self.pending.remove(&node.key());
            let depth = known_depth.map_or(node.depth, |known| known.min(node.depth));
            let key = (document.key().to_owned(), node.pointer.clone());
            if self
                .seen
                .get(&key)
                .is_some_and(|previous| *previous <= depth)
            {
                continue;
            }
            self.depths.insert(document.key().into(), depth);
            let Some(item) = document
                .value
                .pointer(&node.pointer)
                .and_then(serde_json::Value::as_object)
            else {
                let error = document.error(
                    &node.pointer,
                    &node.reference,
                    "Path Item target is missing or is not an object",
                );
                self.reject(node, Arc::new(error.into()));
                continue;
            };
            let Some(reference) = item.get("$ref") else {
                self.seen.insert(key, depth);
                continue;
            };
            let at = format!("{}/$ref", node.pointer);
            let result = reference
                .as_str()
                .ok_or_else(|| document.error(&at, "", "$ref must be a URI string"))
                .and_then(|reference| document.reference(reference, &at));
            match result {
                Ok((Some(target), pointer)) => {
                    self.seen.insert(key, depth);
                    let target_depth = if document.base.as_ref() == Some(&target) {
                        depth
                    } else {
                        depth.saturating_add(1)
                    };
                    self.queue.push_back(Node {
                        document: target,
                        pointer,
                        depth: target_depth,
                        version: document.version,
                        origin: document.key().into(),
                        at,
                        reference: reference.as_str().expect("checked URI string").into(),
                    });
                }
                Ok((None, _)) => unreachable!("registry documents always have a reference base"),
                Err(error) => self.reject(node, Arc::new(error.into())),
            }
        }
        None
    }

    pub(crate) fn accept(
        registry: &mut SourceRegistry,
        uri: Url,
        loaded: Result<Arc<LoadedDocument>, LoaderError>,
        failed: &mut BTreeMap<Url, Arc<SourceError>>,
    ) {
        let result = loaded.map_err(SourceError::Load).and_then(|loaded| {
            let retrieval = loaded.retrieval_uri.clone();
            registry.insert_reference_loaded(loaded)?;
            registry.reference_alias(uri.clone(), retrieval)
        });
        if let Err(error) = result {
            failed.insert(uri, Arc::new(error));
        }
    }

    pub(crate) fn finish(self) -> Vec<ReferenceDiagnostic> {
        self.errors.into_values().collect()
    }
}
