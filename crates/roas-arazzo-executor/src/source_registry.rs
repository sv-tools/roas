//! Complete documents, canonical identities and document-local source aliases.

use roas::{LoadedDocument, LoaderError};
use roas_arazzo::v1_1::{Description, SourceType};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

/// Stable, opaque document handle, local to the registry that created it.
/// Handles survive insertion; registries never remove or replace documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentId(usize);

/// Model family whose version grammar the document declares.
/// Recognition/loading does not imply validation or execution support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceVersion {
    /// Arazzo 1.0, upconverted to the 1.1 execution model.
    Arazzo1_0,
    /// Arazzo 1.1.
    Arazzo1_1,
    /// OpenAPI (Swagger) 2.0.
    OpenApi2,
    /// OpenAPI 3.0.
    OpenApi3_0,
    /// OpenAPI 3.1.
    OpenApi3_1,
    /// OpenAPI 3.2.
    OpenApi3_2,
    /// AsyncAPI 2.6; loading only, not broker execution.
    AsyncApi2_6,
    /// AsyncAPI 3.0; loading only, not broker execution.
    AsyncApi3_0,
    /// AsyncAPI 3.1; loading only, not broker execution.
    AsyncApi3_1,
}

impl SourceVersion {
    pub(crate) fn kind(self) -> SourceType {
        match self {
            Self::Arazzo1_0 | Self::Arazzo1_1 => SourceType::Arazzo,
            Self::OpenApi2 | Self::OpenApi3_0 | Self::OpenApi3_1 | Self::OpenApi3_2 => {
                SourceType::Openapi
            }
            Self::AsyncApi2_6 | Self::AsyncApi3_0 | Self::AsyncApi3_1 => SourceType::Asyncapi,
        }
    }
}

/// An immutable complete document. Arazzo is also deserialized in full; API
/// documents retain raw JSON and a checked version without structural validation.
#[derive(Debug)]
pub struct SourceDocument {
    loaded: Arc<LoadedDocument>,
    retrieval: Url,
    identity: Url,
    model: SourceVersion,
    version: String,
    arazzo: Option<Description>,
}

impl SourceDocument {
    /// Complete original JSON-compatible value; references are not rewritten.
    pub fn value(&self) -> &Value {
        &self.loaded.document
    }
    /// Actual retrieval location (the final redirect location when available).
    pub fn retrieval_uri(&self) -> &Url {
        &self.retrieval
    }
    /// Resolved Arazzo `$self`, or the retrieval URI if no identity is declared.
    pub fn identity(&self) -> &Url {
        &self.identity
    }
    /// Base for this document's references, never an API endpoint override.
    pub fn base_uri(&self) -> &Url {
        &self.identity
    }
    /// Recognized model family. This is not a structural-validation result.
    pub fn model(&self) -> SourceVersion {
        self.model
    }
    /// Version exactly as written in the source document.
    pub fn version(&self) -> &str {
        &self.version
    }
    /// Fully parsed Arazzo, upconverted from v1.0 where necessary.
    pub fn arazzo(&self) -> Option<&Description> {
        self.arazzo.as_ref()
    }
}

/// One owner's declared source edge. A missing target is not a successful load.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SourceLink {
    /// Alias local to the owning document.
    pub name: String,
    /// URI-reference exactly as declared in `sourceDescriptions`.
    pub declared_uri: String,
    /// URI after resolution against the owner's effective base, if valid.
    pub resolved_uri: Option<Url>,
    /// Linked document; cycles retain handles rather than expanding objects.
    pub target: Option<DocumentId>,
    pub(crate) kind: Option<SourceType>,
    pub(crate) index: usize,
}

/// Source-specific failures, separate from workflow execution errors/limits.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// Invalid retrieval URI, reference resolution or `$self` fragment.
    #[error("invalid document URI `{uri}`: {reason}")]
    InvalidUri {
        /// URI or URI-reference being processed.
        uri: String,
        /// Parse/base-resolution explanation.
        reason: String,
    },
    /// Arazzo deserialization or a recognized model's version grammar failed.
    #[error("cannot parse complete source document `{uri}`: {source}")]
    Parse {
        /// Document retrieval location.
        uri: String,
        /// Original model deserialization error.
        #[source]
        source: serde_json::Error,
    },
    /// No supported, unambiguous version discriminator at this URI.
    #[error("unsupported or missing document version in `{0}`")]
    Version(String),
    /// Nonidentical documents claim the same identity or retrieval location.
    #[error("different documents claim identity or retrieval URI `{0}`")]
    Conflict(String),
    /// A handle is outside this registry's document arena.
    #[error("document handle {0:?} is not in this registry")]
    UnknownDocument(DocumentId),
    /// An alias is undeclared or duplicated within its owner.
    #[error("document {owner:?} has no unique source alias `{name}`")]
    Alias {
        /// Owner (prospective handle when insertion rejects duplicate aliases).
        owner: DocumentId,
        /// Invalid local alias.
        name: String,
    },
    /// A linked document does not match the source's declared type.
    #[error("source `{name}` declares {expected:?}, but its document is {actual:?}")]
    Kind {
        /// Owner-local source name.
        name: String,
        /// Declared source type.
        expected: SourceType,
        /// Type detected from the document's version discriminator.
        actual: SourceType,
    },
    /// A noncanonical Arazzo retrieval alias needs explicit compatibility opt-in.
    #[error(
        "source reference `{reference}` names a retrieval alias, not the Arazzo identity `{identity}`"
    )]
    Identity {
        /// Resolved reference that named a retrieval location.
        reference: String,
        /// Document's resolved `$self` identity.
        identity: String,
    },
    /// A fetched reference still has no registered target.
    #[error("source reference `{0}` could not be resolved")]
    Unresolved(String),
    /// Graph loading exceeded its document-attempt or expansion-depth budget.
    #[error("source graph {kind} limit ({limit}) exceeded")]
    Limit {
        /// Budget name (`document` or `depth`).
        kind: &'static str,
        /// Configured maximum.
        limit: usize,
    },
    /// Failure from the caller-configured loader, retaining its source chain.
    #[error(transparent)]
    Load(#[from] LoaderError),
}

/// A loading diagnostic retains the owner and exact source field, while other
/// readable documents remain available for preparation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SourceDiagnostic {
    /// Owning Arazzo document, not the failed target.
    pub owner: DocumentId,
    /// Owner-local source name.
    pub source_name: String,
    /// Field location within that owner.
    pub path: String,
    /// Declared URI-reference, before base resolution.
    pub declared_uri: String,
    /// Typed failure shared by edges that attempted the same unavailable resource.
    pub error: Arc<SourceError>,
}

impl std::fmt::Display for SourceDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "document {:?} {} (`{}`): {}",
            self.owner, self.path, self.source_name, self.error
        )
    }
}

/// Registry of immutable documents and owner-scoped source/base-URL overrides.
/// Insert all supplied documents before loading/resolving links, so `$self`
/// identities can be found even when their retrieval locations differ.
#[derive(Debug, Default)]
pub struct SourceRegistry {
    pub(crate) documents: Vec<Arc<SourceDocument>>,
    identities: BTreeMap<Url, DocumentId>,
    retrievals: BTreeMap<Url, DocumentId>,
    pub(crate) links: BTreeMap<(DocumentId, String), SourceLink>,
    pub(crate) overrides: BTreeMap<(DocumentId, String), DocumentId>,
    pub(crate) base_urls: BTreeMap<(DocumentId, String), String>,
}

impl SourceRegistry {
    /// Empty registry; no fetching or ambient file/network policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a complete supplied document and register its identity atomically.
    /// Identical documents reuse a handle; conflicting identities are rejected.
    /// # Errors
    /// Invalid URI/version, malformed Arazzo, duplicate aliases or identity collision.
    pub fn insert(&mut self, retrieval: &str, value: Value) -> Result<DocumentId, SourceError> {
        let retrieval = resource_uri(retrieval)?;
        self.insert_document(Arc::new(LoadedDocument::new(value, retrieval)))
    }

    pub(crate) fn insert_document(
        &mut self,
        loaded: Arc<LoadedDocument>,
    ) -> Result<DocumentId, SourceError> {
        let retrieval = resource_uri(loaded.retrieval_uri.as_str())?;
        let value = &loaded.document;
        if let Some(id) = self.retrievals.get(&retrieval).copied() {
            return if self.documents[id.0].value() == value {
                Ok(id)
            } else {
                Err(SourceError::Conflict(retrieval.to_string()))
            };
        }
        let (model, version, arazzo) = parse_document(value, &retrieval)?;
        let identity = match arazzo
            .as_ref()
            .and_then(|document| document.self_.as_deref())
        {
            Some(self_) => {
                let identity = join(&retrieval, self_)?;
                if identity.fragment().is_some() {
                    return Err(SourceError::InvalidUri {
                        uri: self_.into(),
                        reason: "`$self` must not contain a fragment".into(),
                    });
                }
                identity
            }
            None => retrieval.clone(),
        };
        if let Some(id) = self.identities.get(&identity).copied() {
            if self.documents[id.0].value() != value {
                return Err(SourceError::Conflict(identity.to_string()));
            }
            self.add_retrieval_alias(id, retrieval.as_str())?;
            return Ok(id);
        }
        // Never let a new canonical identity replace another document's alias.
        if self.retrievals.contains_key(&identity) || self.identities.contains_key(&retrieval) {
            return Err(SourceError::Conflict(identity.to_string()));
        }
        let id = DocumentId(self.documents.len());
        let mut names = BTreeSet::new();
        if let Some(description) = &arazzo {
            for source in &description.source_descriptions {
                if !names.insert(source.name.as_str()) {
                    return Err(SourceError::Alias {
                        owner: id,
                        name: source.name.clone(),
                    });
                }
            }
            for (index, source) in description.source_descriptions.iter().enumerate() {
                self.links.insert(
                    (id, source.name.clone()),
                    SourceLink {
                        name: source.name.clone(),
                        declared_uri: source.url.clone(),
                        resolved_uri: join(&identity, &source.url).ok(),
                        target: None,
                        kind: source.type_,
                        index,
                    },
                );
            }
        }
        self.identities.insert(identity.clone(), id);
        self.retrievals.insert(retrieval.clone(), id);
        self.documents.push(Arc::new(SourceDocument {
            loaded,
            retrieval,
            identity,
            model,
            version,
            arazzo,
        }));
        Ok(id)
    }

    /// Retain a requested URI in addition to a fetcher's final retrieval URI.
    /// This records a location alias; it does not change canonical identity.
    pub fn add_retrieval_alias(&mut self, id: DocumentId, uri: &str) -> Result<(), SourceError> {
        self.document(id)?;
        let uri = resource_uri(uri)?;
        if self
            .retrievals
            .get(&uri)
            .or_else(|| self.identities.get(&uri))
            .is_some_and(|existing| *existing != id)
        {
            return Err(SourceError::Conflict(uri.to_string()));
        }
        self.retrievals.insert(uri, id);
        Ok(())
    }

    /// Read a document by its stable registry-local handle.
    pub fn document(&self, id: DocumentId) -> Result<&SourceDocument, SourceError> {
        self.documents
            .get(id.0)
            .map(Arc::as_ref)
            .ok_or(SourceError::UnknownDocument(id))
    }
    /// Number of unique documents registered, not the number of aliases.
    pub fn len(&self) -> usize {
        self.documents.len()
    }
    /// Whether this registry has no documents.
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
    /// Sources declared by one owner, in document order.
    pub fn sources(&self, owner: DocumentId) -> Result<Vec<&SourceLink>, SourceError> {
        self.document(owner)?;
        let mut sources = self
            .links
            .iter()
            .filter_map(|((id, _), link)| (*id == owner).then_some(link))
            .collect::<Vec<_>>();
        sources.sort_by_key(|link| link.index);
        Ok(sources)
    }
    /// Lookup a local alias; names from other documents cannot collide.
    pub fn source(&self, owner: DocumentId, name: &str) -> Option<&SourceLink> {
        self.links.get(&(owner, name.into()))
    }
    /// Resolve a document reference without IO. Canonical identities win.
    /// Retrieval aliases for Arazzo documents with `$self` require explicit opt-in.
    pub fn resolve(
        &self,
        owner: DocumentId,
        reference: &str,
        retrieval_aliases: bool,
    ) -> Result<Option<DocumentId>, SourceError> {
        let mut uri = join(self.document(owner)?.base_uri(), reference)?;
        uri.set_fragment(None);
        if let Some(id) = self.identities.get(&uri) {
            return Ok(Some(*id));
        }
        if let Some(id) = self.retrievals.get(&uri) {
            let document = self.document(*id)?;
            if !retrieval_aliases && document.arazzo.as_ref().is_some_and(|d| d.self_.is_some()) {
                return Err(SourceError::Identity {
                    reference: uri.to_string(),
                    identity: document.identity.to_string(),
                });
            }
            return Ok(Some(*id));
        }
        Ok(None)
    }
    /// Explicit caller override of one owner's source. This is a deliberate
    /// override, not implicit identity-based resolution. Type must still match.
    pub fn override_source(
        &mut self,
        owner: DocumentId,
        name: &str,
        target: DocumentId,
    ) -> Result<(), SourceError> {
        let link = self.source(owner, name).ok_or_else(|| SourceError::Alias {
            owner,
            name: name.into(),
        })?;
        self.check_kind(link, target)?;
        self.overrides.insert((owner, name.into()), target);
        self.links
            .get_mut(&(owner, name.into()))
            .expect("checked alias")
            .target = Some(target);
        Ok(())
    }
    /// Scope an API endpoint override to one document's alias. Explicit Options
    /// overrides take precedence when adapting the registry for execution.
    pub fn override_base_url(
        &mut self,
        owner: DocumentId,
        name: &str,
        url: impl Into<String>,
    ) -> Result<(), SourceError> {
        if self.source(owner, name).is_none() {
            return Err(SourceError::Alias {
                owner,
                name: name.into(),
            });
        }
        self.base_urls.insert((owner, name.into()), url.into());
        Ok(())
    }
    pub(crate) fn check_kind(
        &self,
        link: &SourceLink,
        target: DocumentId,
    ) -> Result<(), SourceError> {
        let actual = self.document(target)?.model.kind();
        if let Some(expected) = link.kind
            && expected != actual
        {
            return Err(SourceError::Kind {
                name: link.name.clone(),
                expected,
                actual,
            });
        }
        Ok(())
    }
    pub(crate) fn shared(&self, id: DocumentId) -> Result<Arc<SourceDocument>, SourceError> {
        self.documents
            .get(id.0)
            .cloned()
            .ok_or(SourceError::UnknownDocument(id))
    }
}

pub(crate) fn resource_uri(uri: &str) -> Result<Url, SourceError> {
    let mut url = Url::parse(uri).map_err(|error| SourceError::InvalidUri {
        uri: uri.into(),
        reason: error.to_string(),
    })?;
    url.set_fragment(None);
    Ok(url)
}

pub(crate) fn join(base: &Url, reference: &str) -> Result<Url, SourceError> {
    base.join(reference)
        .map_err(|error| SourceError::InvalidUri {
            uri: reference.into(),
            reason: if base.cannot_be_a_base() && matches!(Url::parse(reference), Err(url::ParseError::RelativeUrlWithoutBase)) {
                format!("base `{base}` is not hierarchical — give the source an absolute URL, or the Arazzo description a hierarchical `$self`")
            } else {
                format!("resolving against `{base}`: {error}")
            },
        })
}

fn parse_document(
    value: &Value,
    uri: &Url,
) -> Result<(SourceVersion, String, Option<Description>), SourceError> {
    let parse_error = |source| SourceError::Parse {
        uri: uri.to_string(),
        source,
    };
    // Reject ambiguous discriminators rather than choosing a document kind silently.
    if ["arazzo", "swagger", "openapi", "asyncapi"]
        .iter()
        .filter(|key| value.get(**key).is_some())
        .count()
        != 1
    {
        return Err(SourceError::Version(uri.to_string()));
    }
    if let Some(version) = value.get("arazzo").and_then(Value::as_str) {
        if version.starts_with("1.0.") {
            let document = serde_json::from_value::<roas_arazzo::v1_0::Description>(value.clone())
                .map_err(parse_error)?;
            return Ok((
                SourceVersion::Arazzo1_0,
                version.into(),
                Some(document.into()),
            ));
        }
        if version.starts_with("1.1.") {
            let document = serde_json::from_value(value.clone()).map_err(parse_error)?;
            return Ok((SourceVersion::Arazzo1_1, version.into(), Some(document)));
        }
    }
    macro_rules! version {
        ($key:literal, $prefix:literal, $type:ty, $model:ident) => {
            if let Some(written) = value.get($key).and_then(Value::as_str)
                && (written == $prefix || written.starts_with(concat!($prefix, ".")))
            {
                serde_json::from_value::<$type>(value[$key].clone()).map_err(parse_error)?;
                return Ok((SourceVersion::$model, written.into(), None));
            }
        };
    }
    version!("swagger", "2", roas::v2::spec::Version, OpenApi2);
    version!("openapi", "3.0", roas::v3_0::spec::Version, OpenApi3_0);
    version!("openapi", "3.1", roas::v3_1::spec::Version, OpenApi3_1);
    version!("openapi", "3.2", roas::v3_2::spec::Version, OpenApi3_2);
    version!("asyncapi", "2.6", roas_asyncapi::v2_6::Version, AsyncApi2_6);
    version!("asyncapi", "3.0", roas_asyncapi::v3_0::Version, AsyncApi3_0);
    version!("asyncapi", "3.1", roas_asyncapi::v3_1::Version, AsyncApi3_1);
    Err(SourceError::Version(uri.to_string()))
}

impl crate::Options {
    /// Adapt readable sources from one registry owner into executor options.
    /// Existing `source`/`base_url` entries are explicit overrides and win.
    /// Unresolved graph links remain absent, so checked preparation still rejects
    /// a missing required source (or an unprovable bare operation ID).
    /// # Errors
    /// An invalid registry handle. Loading diagnostics remain in SourceLoadReport.
    pub fn source_registry(
        mut self,
        registry: &SourceRegistry,
        owner: DocumentId,
    ) -> Result<Self, SourceError> {
        for link in registry.sources(owner)? {
            if let Some(target) = link.target {
                let document = registry.shared(target)?;
                self.sources
                    .entry(link.name.clone())
                    .or_insert_with(|| crate::operation::Source {
                        url: link.declared_uri.clone(),
                        data: crate::operation::SourceData::Registry(document),
                    });
            }
            if let Some(url) = registry.base_urls.get(&(owner, link.name.clone())) {
                self.base_urls
                    .entry(link.name.clone())
                    .or_insert_with(|| url.clone());
            }
        }
        Ok(self)
    }

    /// Original document metadata for a registry-backed source. Legacy
    /// `Options::source` values have no invented retrieval URI or identity.
    pub fn source_document(&self, name: &str) -> Option<&SourceDocument> {
        match &self.sources.get(name)?.data {
            crate::operation::SourceData::Registry(document) => Some(document),
            crate::operation::SourceData::Owned(_) => None,
        }
    }
}
