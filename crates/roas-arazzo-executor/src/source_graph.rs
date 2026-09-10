//! Bounded source traversal; the caller owns all fetch policy and IO.

use crate::source_registry::{join, resource_uri};
use crate::{DocumentId, SourceDiagnostic, SourceError, SourceRegistry};
use roas::loader::{LoadedDocument, Loader, LoaderError};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use url::Url;

/// Loading policy, independent of workflow step/retry/call-depth limits.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SourceLoadOptions {
    /// Existing registry documents plus distinct fetch attempts allowed per load.
    /// Failed attempts and different retrieval aliases consume a slot too.
    pub max_documents: usize,
    /// Root depth is zero; a direct source has depth one. Known cycle/diamond
    /// targets do not expand again or consume an additional depth allowance.
    pub max_depth: usize,
    /// Only these root aliases are traversed, or all root aliases when absent.
    /// Sources of linked Arazzo documents are traversed in full.
    pub root_sources: Option<BTreeSet<String>>,
    /// Compatibility extension: allow an Arazzo retrieval URI instead of `$self`.
    /// False by default, following identity-based referencing.
    pub retrieval_aliases: bool,
}

impl Default for SourceLoadOptions {
    fn default() -> Self {
        Self {
            max_documents: 256,
            max_depth: 32,
            root_sources: None,
            retrieval_aliases: false,
        }
    }
}

/// An edge back into the active ancestry. Source cycles are representable data,
/// not execution permission or a reason to discard the readable documents.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SourceCycle {
    /// Document containing the back edge.
    pub owner: DocumentId,
    /// Local alias of the back edge.
    pub source_name: String,
    /// Ancestor document reached by this edge.
    pub target: DocumentId,
}

/// Loading results. Preparation, not this report, decides whether execution can
/// proceed with the available sources. A nonempty diagnostic list is not success.
#[derive(Debug)]
#[non_exhaustive]
pub struct SourceLoadReport {
    /// Located unresolved/invalid/budget-limited source edges.
    pub diagnostics: Vec<SourceDiagnostic>,
    /// Back edges, distinct from shared diamond dependencies.
    pub cycles: Vec<SourceCycle>,
    /// Distinct calls to the loader; its own cache may satisfy a call without IO.
    pub fetch_attempts: usize,
}

impl SourceRegistry {
    /// Load a selected source graph through an explicitly configured loader.
    /// Insert all supplied documents first. Failures retain other readable
    /// documents and are reported per edge after identity discovery completes.
    /// # Errors
    /// Invalid root/selection or an already-exceeded initial document budget.
    /// Source failures otherwise belong to the returned report.
    pub fn load_sources(
        &mut self,
        root: DocumentId,
        loader: &mut Loader,
        options: &SourceLoadOptions,
    ) -> Result<SourceLoadReport, SourceError> {
        let mut traversal = Traversal::new(self, root, options)?;
        while let Some(uri) = traversal.next(self) {
            let document = loader.load_document_shared(uri.as_str());
            traversal.accept(self, uri, document);
        }
        Ok(traversal.finish(self))
    }

    /// Async loading with exactly the same graph policy and diagnostics.
    /// Uses the loader's registered async fetchers on cache misses.
    pub async fn load_sources_async(
        &mut self,
        root: DocumentId,
        loader: &mut Loader,
        options: &SourceLoadOptions,
    ) -> Result<SourceLoadReport, SourceError> {
        let mut traversal = Traversal::new(self, root, options)?;
        while let Some(uri) = traversal.next(self) {
            let document = loader.load_document_shared_async(uri.as_str()).await;
            traversal.accept(self, uri, document);
        }
        Ok(traversal.finish(self))
    }
}

type Edge = (DocumentId, String);

struct Traversal<'a> {
    root: DocumentId,
    options: &'a SourceLoadOptions,
    initial_documents: usize,
    queue: VecDeque<(Edge, usize)>,
    seen: BTreeMap<DocumentId, usize>,
    edges: BTreeSet<Edge>,
    pending: BTreeMap<Edge, usize>,
    errors: BTreeMap<Edge, Arc<SourceError>>,
    attempted: BTreeSet<Url>,
    failed: BTreeMap<Url, Arc<SourceError>>,
    generation: usize,
    retried: usize,
}

impl<'a> Traversal<'a> {
    fn new(
        registry: &SourceRegistry,
        root: DocumentId,
        options: &'a SourceLoadOptions,
    ) -> Result<Self, SourceError> {
        registry.document(root)?;
        if registry.len() > options.max_documents {
            return Err(SourceError::Limit {
                kind: "document",
                limit: options.max_documents,
            });
        }
        if let Some(names) = &options.root_sources {
            for name in names {
                if registry.source(root, name).is_none() {
                    return Err(SourceError::Alias {
                        owner: root,
                        name: name.clone(),
                    });
                }
            }
        }
        let mut traversal = Self {
            root,
            options,
            initial_documents: registry.len(),
            queue: VecDeque::new(),
            seen: BTreeMap::new(),
            edges: BTreeSet::new(),
            pending: BTreeMap::new(),
            errors: BTreeMap::new(),
            attempted: BTreeSet::new(),
            failed: BTreeMap::new(),
            generation: 0,
            retried: 0,
        };
        traversal.enqueue(registry, root, 0);
        Ok(traversal)
    }

    fn enqueue(&mut self, registry: &SourceRegistry, owner: DocumentId, depth: usize) {
        if self
            .seen
            .get(&owner)
            .is_some_and(|previous| *previous <= depth)
        {
            return;
        }
        self.seen.insert(owner, depth);
        for source in registry
            .sources(owner)
            .expect("registered traversal document")
        {
            if owner == self.root
                && self
                    .options
                    .root_sources
                    .as_ref()
                    .is_some_and(|names| !names.contains(&source.name))
            {
                continue;
            }
            let edge = (owner, source.name.clone());
            self.edges.insert(edge.clone());
            self.queue.push_back((edge, depth.saturating_add(1)));
        }
    }

    fn reject(&mut self, edge: Edge, depth: usize, error: Arc<SourceError>) {
        self.pending.insert(edge.clone(), depth);
        self.errors.insert(edge, error);
    }

    fn next(&mut self, registry: &mut SourceRegistry) -> Option<Url> {
        loop {
            let Some((edge, _)) = self.queue.pop_front() else {
                // A later document may supply an earlier reference's identity.
                // Reconcile after complete discovery, without refetching failures.
                if self.retried != self.generation {
                    self.retried = self.generation;
                    self.queue.extend(std::mem::take(&mut self.pending));
                    continue;
                }
                return None;
            };
            let depth = self.seen[&edge.0].saturating_add(1);
            let link = registry.links.get(&edge).expect("declared edge").clone();
            registry.links.get_mut(&edge).expect("declared edge").target = None;
            let target = registry.overrides.get(&edge).copied().map_or_else(
                || registry.resolve(edge.0, &link.declared_uri, self.options.retrieval_aliases),
                |id| Ok(Some(id)),
            );
            let target = match target {
                Ok(target) => target,
                Err(error) => {
                    self.reject(edge, depth, Arc::new(error));
                    continue;
                }
            };
            if depth > self.options.max_depth
                && target.is_none_or(|id| !self.seen.contains_key(&id))
            {
                self.reject(
                    edge,
                    depth,
                    Arc::new(SourceError::Limit {
                        kind: "depth",
                        limit: self.options.max_depth,
                    }),
                );
                continue;
            }
            if let Some(target) = target {
                if let Err(error) = registry.check_kind(&link, target) {
                    self.reject(edge, depth, Arc::new(error));
                    continue;
                }
                registry.links.get_mut(&edge).expect("declared edge").target = Some(target);
                self.pending.remove(&edge);
                self.errors.remove(&edge);
                self.enqueue(registry, target, depth);
                continue;
            }
            let uri = join(
                registry.document(edge.0).expect("owner").base_uri(),
                &link.declared_uri,
            )
            .and_then(|uri| resource_uri(uri.as_str()));
            let uri = match uri {
                Ok(uri) => uri,
                Err(error) => {
                    self.reject(edge, depth, Arc::new(error));
                    continue;
                }
            };
            if let Some(error) = self.failed.get(&uri) {
                self.reject(edge, depth, Arc::clone(error));
                continue;
            }
            if self.attempted.contains(&uri) {
                self.reject(
                    edge,
                    depth,
                    Arc::new(SourceError::Unresolved(uri.to_string())),
                );
                continue;
            }
            if self.attempted.len() >= self.options.max_documents - self.initial_documents {
                self.reject(
                    edge,
                    depth,
                    Arc::new(SourceError::Limit {
                        kind: "document",
                        limit: self.options.max_documents,
                    }),
                );
                continue;
            }
            self.attempted.insert(uri.clone());
            self.queue.push_front((edge, depth));
            return Some(uri);
        }
    }

    fn accept(
        &mut self,
        registry: &mut SourceRegistry,
        uri: Url,
        result: Result<Arc<LoadedDocument>, LoaderError>,
    ) {
        let result = result.map_err(SourceError::Load).and_then(|loaded| {
            let id = registry.insert_document(loaded)?;
            registry.add_retrieval_alias(id, uri.as_str())?;
            Ok(id)
        });
        match result {
            Ok(_) => self.generation += 1,
            Err(error) => {
                self.failed.insert(uri, Arc::new(error));
            }
        }
    }

    fn finish(self, registry: &SourceRegistry) -> SourceLoadReport {
        let mut diagnostics = self
            .errors
            .into_iter()
            .map(|((owner, source_name), error)| {
                let link = registry
                    .source(owner, &source_name)
                    .expect("declared source");
                SourceDiagnostic {
                    owner,
                    source_name,
                    path: format!("#.sourceDescriptions[{}].url", link.index),
                    declared_uri: link.declared_uri.clone(),
                    error,
                }
            })
            .collect::<Vec<_>>();
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.owner,
                registry
                    .source(diagnostic.owner, &diagnostic.source_name)
                    .expect("source")
                    .index,
            )
        });
        SourceLoadReport {
            diagnostics,
            cycles: cycles(registry, self.root, &self.edges),
            fetch_attempts: self.attempted.len(),
        }
    }
}

fn cycles(registry: &SourceRegistry, root: DocumentId, edges: &BTreeSet<Edge>) -> Vec<SourceCycle> {
    let mut cycles = Vec::new();
    let mut active = BTreeSet::from([root]);
    let mut done = BTreeSet::new();
    let mut stack = vec![(root, 0)];
    while let Some((owner, next)) = stack.last_mut() {
        let sources = registry.sources(*owner).expect("registered graph document");
        let Some(link) = sources.get(*next) else {
            done.insert(*owner);
            active.remove(owner);
            stack.pop();
            continue;
        };
        *next += 1;
        if !edges.contains(&(*owner, link.name.clone())) {
            continue;
        }
        if let Some(target) = link.target {
            if active.contains(&target) {
                cycles.push(SourceCycle {
                    owner: *owner,
                    source_name: link.name.clone(),
                    target,
                });
            } else if !done.contains(&target) {
                active.insert(target);
                stack.push((target, 0));
            }
        }
    }
    cycles
}
