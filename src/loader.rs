//! External resource loader for OpenAPI references.
//!
//! A loader is responsible for fetching and caching external resources. It
//! does not fetch anything by default. Callers opt in by registering fetchers
//! for URI prefixes, for example `file://` or `https://`.

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use thiserror::Error;
use url::Url;

/// Boxed future returned by resource fetchers.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, LoaderError>> + 'a>>;

/// Fetches and parses resources for the loader.
///
/// Fetchers receive the resource URL without its fragment and return a parsed document.
/// They do not manage the loader cache.
pub trait ResourceFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError>;
}

/// Asynchronously fetches and parses resources for the loader.
///
/// Async fetchers receive the resource URL without its fragment and return a parsed
/// document. They do not manage the loader cache.
pub trait AsyncResourceFetcher {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a>;
}

/// JSON file-system fetcher.
///
/// This fetcher is not registered by default. Register it explicitly for the
/// `file://` prefix to allow loading JSON documents from the file system.
#[derive(Clone, Debug, Default)]
pub struct JsonFileFetcher;

impl ResourceFetcher for JsonFileFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        if uri.scheme() != "file" {
            return Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_string()));
        }

        let path = uri
            .to_file_path()
            .map_err(|()| LoaderError::InvalidFileUri(uri.as_str().to_string()))?;
        let bytes = fs::read(&path).map_err(|source| LoaderError::ReadFile { path, source })?;
        serde_json::from_slice(&bytes).map_err(|source| LoaderError::Parse {
            uri: uri.as_str().to_string(),
            source,
        })
    }
}

/// External resource loading errors.
#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("no fetcher registered for `{uri}`")]
    NoFetcherRegistered { uri: String },

    #[error("fetcher does not support `{0}`")]
    UnsupportedFetcherUri(String),

    #[error("invalid file URI `{0}`")]
    InvalidFileUri(String),

    #[error(
        "reference `{0}` has no base resource — loader cannot resolve internal-only `#/...` references"
    )]
    MissingBaseUri(String),

    #[error("invalid URI `{uri}`")]
    InvalidUri {
        uri: String,
        #[source]
        source: url::ParseError,
    },

    #[error("failed to read `{path}`")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse `{uri}`")]
    Parse {
        uri: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("reference `{reference}` not found in `{uri}`")]
    PointerNotFound { uri: String, reference: String },

    #[error("invalid URI fragment in `{0}`")]
    InvalidFragment(String),
}

/// External resource loader with a fetcher registry and document cache.
///
/// Two layers of caching are maintained: the raw `Value` cache keyed by
/// resource URI (so the same file/URL is fetched once), and a typed
/// cache keyed by `(reference, TypeId)` (so a `$ref` deserialized into
/// some concrete `T` is parsed only once across the run, regardless of
/// how many places point to it).
pub struct Loader {
    fetchers: BTreeMap<String, Box<dyn ResourceFetcher>>,
    async_fetchers: BTreeMap<String, Box<dyn AsyncResourceFetcher>>,
    cache: BTreeMap<Url, Value>,
    typed_cache: BTreeMap<(String, TypeId), Box<dyn Any>>,
}

impl Loader {
    /// Create a loader with no registered fetchers.
    pub fn new() -> Self {
        Self {
            fetchers: BTreeMap::new(),
            async_fetchers: BTreeMap::new(),
            cache: BTreeMap::new(),
            typed_cache: BTreeMap::new(),
        }
    }

    /// Register a fetcher for a URI prefix.
    ///
    /// The longest matching prefix wins. For example, callers can register a
    /// general `https://` fetcher and a narrower
    /// `https://schemas.example.test/` fetcher for a specific host.
    pub fn register_fetcher(
        &mut self,
        prefix: impl Into<String>,
        fetcher: impl ResourceFetcher + 'static,
    ) -> Option<Box<dyn ResourceFetcher>> {
        self.fetchers.insert(prefix.into(), Box::new(fetcher))
    }

    /// Register an async fetcher for a URI prefix.
    ///
    /// Async loading uses the longest matching prefix among **async**
    /// fetchers only — sync fetchers are not consulted on async cache
    /// misses (see [`Self::load_resource_async`] and
    /// [`Self::resolve_reference_async`]). Sync loading symmetrically
    /// uses only sync fetchers.
    pub fn register_async_fetcher(
        &mut self,
        prefix: impl Into<String>,
        fetcher: impl AsyncResourceFetcher + 'static,
    ) -> Option<Box<dyn AsyncResourceFetcher>> {
        self.async_fetchers.insert(prefix.into(), Box::new(fetcher))
    }

    /// Preload a parsed document into the cache.
    ///
    /// The cache key is the resource part of `uri`, without any fragment. For
    /// example, `content.json#/Pet` is stored under `content.json`.
    ///
    /// If a document was already cached for the same resource, the typed
    /// cache is wiped so subsequent `resolve_reference_as` calls re-run
    /// `serde_json::from_value` against the new document. Partial-key
    /// invalidation (clearing only entries that targeted this resource)
    /// would require parsing every cached `(reference, TypeId)` key, so
    /// we clear the typed cache wholesale on overwrite — the tradeoff
    /// is acceptable because re-preloading is a configuration-time
    /// operation, not a hot path.
    pub fn preload_resource(
        &mut self,
        uri: impl AsRef<str>,
        document: Value,
    ) -> Result<Option<Value>, LoaderError> {
        let (key, _) = parse_reference(uri.as_ref())?;
        let previous = self.cache.insert(key, document);
        if previous.is_some() {
            self.typed_cache.clear();
        }
        Ok(previous)
    }

    /// Load a resource, returning the cached parsed document.
    ///
    /// The same resource URI is fetched only once.
    pub fn load_resource(&mut self, uri: &str) -> Result<&Value, LoaderError> {
        let (key, _) = parse_reference(uri)?;
        self.load_resource_by_key(key)
    }

    fn load_resource_by_key(&mut self, key: Url) -> Result<&Value, LoaderError> {
        if !self.cache.contains_key(&key) {
            let fetcher_key = best_fetcher_key(&self.fetchers, key.as_str()).ok_or_else(|| {
                LoaderError::NoFetcherRegistered {
                    uri: key.as_str().to_string(),
                }
            })?;
            let parsed = self
                .fetchers
                .get_mut(&fetcher_key)
                .expect("fetcher key came from the registry")
                .fetch(&key)?;

            self.cache.insert(key.clone(), parsed);
        }

        Ok(self
            .cache
            .get(&key)
            .expect("resource was inserted into the cache"))
    }

    /// Asynchronously load a resource, returning the cached parsed document.
    ///
    /// The same resource URI is fetched only once. Async loading uses async
    /// fetchers only.
    pub async fn load_resource_async(&mut self, uri: &str) -> Result<&Value, LoaderError> {
        let (key, _) = parse_reference(uri)?;
        self.load_resource_by_key_async(key).await
    }

    async fn load_resource_by_key_async(&mut self, key: Url) -> Result<&Value, LoaderError> {
        if !self.cache.contains_key(&key) {
            let fetcher_key =
                best_fetcher_key(&self.async_fetchers, key.as_str()).ok_or_else(|| {
                    LoaderError::NoFetcherRegistered {
                        uri: key.as_str().to_string(),
                    }
                })?;
            let parsed = self
                .async_fetchers
                .get_mut(&fetcher_key)
                .expect("async fetcher key came from the registry")
                .fetch(&key)
                .await?;

            self.cache.insert(key.clone(), parsed);
        }

        Ok(self
            .cache
            .get(&key)
            .expect("resource was inserted into the cache"))
    }

    /// Resolve a reference and return the referenced JSON value.
    ///
    /// `reference` must include a resource (`common.json#/Pet`,
    /// `file:///tmp/common.json#/Pet`,
    /// `https://example.test/openapi.json#/Pet`).
    pub fn resolve_reference(&mut self, reference: &str) -> Result<&Value, LoaderError> {
        let (key, fragment) = parse_reference(reference)?;
        if key.as_str() == "relative:" {
            return Err(LoaderError::MissingBaseUri(reference.to_string()));
        }
        let pointer = decode_fragment(&fragment)
            .map_err(|()| LoaderError::InvalidFragment(reference.to_string()))?;
        let document = self.load_resource_by_key(key.clone())?;

        if pointer.is_empty() {
            return Ok(document);
        }

        match document.pointer(&pointer) {
            Some(value) => Ok(value),
            None => Err(LoaderError::PointerNotFound {
                uri: key.as_str().to_string(),
                reference: reference.to_string(),
            }),
        }
    }

    /// Asynchronously resolve a reference and return the referenced JSON value.
    pub async fn resolve_reference_async(
        &mut self,
        reference: &str,
    ) -> Result<&Value, LoaderError> {
        let (key, fragment) = parse_reference(reference)?;
        if key.as_str() == "relative:" {
            return Err(LoaderError::MissingBaseUri(reference.to_string()));
        }
        let pointer = decode_fragment(&fragment)
            .map_err(|()| LoaderError::InvalidFragment(reference.to_string()))?;
        let document = self.load_resource_by_key_async(key.clone()).await?;

        if pointer.is_empty() {
            return Ok(document);
        }

        match document.pointer(&pointer) {
            Some(value) => Ok(value),
            None => Err(LoaderError::PointerNotFound {
                uri: key.as_str().to_string(),
                reference: reference.to_string(),
            }),
        }
    }

    /// Resolve and deserialize a reference into the requested type.
    ///
    /// Subsequent calls with the same `reference` and `T` return a clone
    /// of the cached deserialized value rather than re-running serde over
    /// the underlying `Value`.
    pub fn resolve_reference_as<T>(&mut self, reference: &str) -> Result<T, LoaderError>
    where
        T: 'static + Clone + DeserializeOwned,
    {
        let cache_key = (reference.to_string(), TypeId::of::<T>());
        if let Some(entry) = self.typed_cache.get(&cache_key)
            && let Some(cached) = entry.downcast_ref::<T>()
        {
            return Ok(cached.clone());
        }
        let (key, _) = parse_reference(reference)?;
        let uri = key.to_string();
        let value = self.resolve_reference(reference)?;
        let parsed: T = serde_json::from_value(value.clone())
            .map_err(|source| LoaderError::Parse { uri, source })?;
        self.typed_cache.insert(cache_key, Box::new(parsed.clone()));
        Ok(parsed)
    }

    /// Asynchronously resolve and deserialize a reference into the requested type.
    ///
    /// Shares the typed cache with [`Self::resolve_reference_as`].
    pub async fn resolve_reference_as_async<T>(&mut self, reference: &str) -> Result<T, LoaderError>
    where
        T: 'static + Clone + DeserializeOwned,
    {
        let cache_key = (reference.to_string(), TypeId::of::<T>());
        if let Some(entry) = self.typed_cache.get(&cache_key)
            && let Some(cached) = entry.downcast_ref::<T>()
        {
            return Ok(cached.clone());
        }
        let (key, _) = parse_reference(reference)?;
        let uri = key.to_string();
        let value = self.resolve_reference_async(reference).await?;
        let parsed: T = serde_json::from_value(value.clone())
            .map_err(|source| LoaderError::Parse { uri, source })?;
        self.typed_cache.insert(cache_key, Box::new(parsed.clone()));
        Ok(parsed)
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

fn best_fetcher_key<T: ?Sized>(fetchers: &BTreeMap<String, Box<T>>, uri: &str) -> Option<String> {
    fetchers
        .keys()
        .filter(|prefix| uri.starts_with(prefix.as_str()))
        .max_by_key(|prefix| prefix.len())
        .cloned()
}

fn parse_reference(reference: &str) -> Result<(Url, String), LoaderError> {
    match Url::parse(reference) {
        Ok(url) => Ok(split_url_fragment(url)),
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let relative = format!("relative:{reference}");
            Url::parse(&relative)
                .map_err(|source| LoaderError::InvalidUri {
                    uri: reference.to_string(),
                    source,
                })
                .map(split_url_fragment)
        }
        Err(source) => Err(LoaderError::InvalidUri {
            uri: reference.to_string(),
            source,
        }),
    }
}

fn split_url_fragment(mut url: Url) -> (Url, String) {
    let fragment = url.fragment().unwrap_or_default().to_string();
    url.set_fragment(None);
    (url, fragment)
}

fn decode_fragment(fragment: &str) -> Result<String, ()> {
    if fragment.is_empty() {
        return Ok(String::new());
    }

    let bytes = fragment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(());
            }
            let hi = hex(bytes[i + 1])?;
            let lo = hex(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn hex(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::future::Future;
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    #[derive(Clone, Default)]
    struct StaticFetcher {
        count: Rc<Cell<usize>>,
    }

    impl ResourceFetcher for StaticFetcher {
        fn fetch(&mut self, _uri: &Url) -> Result<Value, LoaderError> {
            self.count.set(self.count.get() + 1);
            Ok(pet_document())
        }
    }

    #[derive(Clone, Default)]
    struct AsyncStaticFetcher {
        count: Rc<Cell<usize>>,
    }

    impl AsyncResourceFetcher for AsyncStaticFetcher {
        fn fetch<'a>(&'a mut self, _uri: &'a Url) -> FetchFuture<'a> {
            Box::pin(async move {
                self.count.set(self.count.get() + 1);
                Ok(pet_document())
            })
        }
    }

    fn pet_document() -> Value {
        serde_json::json!({
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object"
                    }
                }
            }
        })
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = pin!(future);

        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn default_loader_has_no_fetchers() {
        let mut loader = Loader::new();
        let err = loader
            .load_resource("Cargo.toml")
            .expect_err("file loading should not happen without a fetcher");
        assert!(matches!(err, LoaderError::NoFetcherRegistered { .. }));
    }

    #[test]
    fn file_resource_is_fetched_once_and_cached() {
        let dir = std::env::temp_dir();
        let file = dir.join(format!(
            "roas-loader-test-{}-{}.json",
            std::process::id(),
            "cache"
        ));
        fs::write(
            &file,
            br#"{"components":{"schemas":{"Pet":{"type":"object"}}}}"#,
        )
        .unwrap();

        let mut loader = Loader::new();
        loader.register_fetcher("file://", JsonFileFetcher);

        // Build the `file://` URL via `Url::from_file_path` so paths
        // with spaces / non-ASCII / Windows separators stay valid.
        let mut url = Url::from_file_path(&file).unwrap();
        url.set_fragment(Some("/components/schemas/Pet"));
        let reference = url.to_string();
        assert!(loader.resolve_reference(&reference).is_ok());

        // Delete the file. The second resolve must still succeed —
        // proving the resource came out of the cache rather than the
        // fetcher, since the fetcher would now fail on a `ReadFile`
        // error.
        fs::remove_file(&file).unwrap();
        assert!(
            loader.resolve_reference(&reference).is_ok(),
            "second resolve should hit the cache, not re-fetch the deleted file"
        );
    }

    #[test]
    fn relative_reference_uses_preloaded_document() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "common.json",
                serde_json::json!({
                    "Pet": {
                        "type": "object"
                    }
                }),
            )
            .unwrap();
        let value = loader.resolve_reference("common.json#/Pet/type").unwrap();
        assert_eq!(value, "object");
    }

    #[test]
    fn registered_uri_fetcher_is_opt_in_and_cached() {
        let fetch_count = Rc::new(Cell::new(0));
        let mut loader = Loader::new();
        loader.register_fetcher(
            "https://",
            StaticFetcher {
                count: fetch_count.clone(),
            },
        );
        let reference = "https://example.test/openapi.json#/components/schemas/Pet";

        assert!(loader.resolve_reference(reference).is_ok());
        assert!(loader.resolve_reference(reference).is_ok());
        assert_eq!(fetch_count.get(), 1);
    }

    #[test]
    fn registered_async_fetcher_is_opt_in_and_cached() {
        let fetch_count = Rc::new(Cell::new(0));
        let mut loader = Loader::new();
        loader.register_async_fetcher(
            "https://",
            AsyncStaticFetcher {
                count: fetch_count.clone(),
            },
        );
        let reference = "https://example.test/openapi.json#/components/schemas/Pet";

        assert!(block_on(loader.resolve_reference_async(reference)).is_ok());
        assert!(block_on(loader.resolve_reference_async(reference)).is_ok());
        assert_eq!(fetch_count.get(), 1);
    }

    #[test]
    fn longest_fetcher_prefix_wins() {
        let broad_count = Rc::new(Cell::new(0));
        let narrow_count = Rc::new(Cell::new(0));
        let mut loader = Loader::new();
        loader.register_fetcher(
            "https://",
            StaticFetcher {
                count: broad_count.clone(),
            },
        );
        loader.register_fetcher(
            "https://schemas.example.test/",
            StaticFetcher {
                count: narrow_count.clone(),
            },
        );

        let reference = "https://schemas.example.test/openapi.json#/components/schemas/Pet";
        assert!(loader.resolve_reference(reference).is_ok());
        assert_eq!(broad_count.get(), 0);
        assert_eq!(narrow_count.get(), 1);
    }

    #[test]
    fn async_loader_uses_async_fetchers_only() {
        let sync_count = Rc::new(Cell::new(0));
        let async_count = Rc::new(Cell::new(0));
        let mut loader = Loader::new();
        loader.register_fetcher(
            "https://schemas.example.test/",
            StaticFetcher {
                count: sync_count.clone(),
            },
        );
        loader.register_async_fetcher(
            "https://",
            AsyncStaticFetcher {
                count: async_count.clone(),
            },
        );

        let reference = "https://schemas.example.test/openapi.json#/components/schemas/Pet";
        assert!(block_on(loader.resolve_reference_async(reference)).is_ok());
        assert_eq!(sync_count.get(), 0);
        assert_eq!(async_count.get(), 1);
    }

    #[test]
    fn async_loader_ignores_sync_fetchers_when_cache_misses() {
        let sync_count = Rc::new(Cell::new(0));
        let mut loader = Loader::new();
        loader.register_fetcher(
            "https://",
            StaticFetcher {
                count: sync_count.clone(),
            },
        );

        let reference = "https://example.test/openapi.json#/components/schemas/Pet";
        let err = block_on(loader.resolve_reference_async(reference))
            .expect_err("async loading should require an async fetcher");
        assert!(
            matches!(err, LoaderError::NoFetcherRegistered { uri } if uri == "https://example.test/openapi.json")
        );
        assert_eq!(sync_count.get(), 0);
    }

    #[test]
    fn relative_reference_without_preload_uses_resource_as_cache_key() {
        let mut loader = Loader::new();
        loader.register_fetcher("https://", StaticFetcher::default());

        let err = loader
            .resolve_reference("../common.json#/components/schemas/Pet")
            .expect_err("relative external refs must be preloaded explicitly");
        assert!(
            matches!(err, LoaderError::NoFetcherRegistered { uri } if uri == "relative:../common.json")
        );
    }

    #[test]
    fn query_only_reference_uses_query_as_cache_key() {
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "?v=2",
                serde_json::json!({
                    "Pet": {
                        "type": "object"
                    }
                }),
            )
            .unwrap();

        let value = loader.resolve_reference("?v=2#/Pet/type").unwrap();
        assert_eq!(value, "object");
    }

    #[test]
    fn parse_reference_returns_resource_url_and_fragment() {
        let (key, fragment) =
            parse_reference("https://example.test/document.json#/foo/bar").unwrap();
        assert_eq!(key.as_str(), "https://example.test/document.json");
        assert_eq!(fragment, "/foo/bar");

        let (key, fragment) = parse_reference("file://content.json#/foo/bar").unwrap();
        assert_eq!(key.as_str(), "file://content.json/");
        assert_eq!(fragment, "/foo/bar");

        let (key, fragment) = parse_reference("content.json#/foo/bar").unwrap();
        assert_eq!(key.scheme(), "relative");
        assert_eq!(key.as_str(), "relative:content.json");
        assert_eq!(fragment, "/foo/bar");
    }

    #[test]
    fn internal_reference_without_resource_is_rejected() {
        let mut loader = Loader::new();
        let err = loader
            .resolve_reference("#/components/schemas/Pet")
            .expect_err("loader only resolves external references");
        assert!(matches!(err, LoaderError::MissingBaseUri(_)));
    }

    #[test]
    fn preload_overwrite_invalidates_typed_cache() {
        #[derive(Clone, serde::Deserialize, PartialEq, Debug)]
        struct Pet {
            name: String,
        }

        let mut loader = Loader::new();
        loader
            .preload_resource("pets.json", serde_json::json!({ "Pet": { "name": "rex" } }))
            .unwrap();

        let first: Pet = loader.resolve_reference_as("pets.json#/Pet").unwrap();
        assert_eq!(first.name, "rex");

        // Overwrite the document with a different Pet under the same key.
        loader
            .preload_resource(
                "pets.json",
                serde_json::json!({ "Pet": { "name": "buddy" } }),
            )
            .unwrap();

        let second: Pet = loader.resolve_reference_as("pets.json#/Pet").unwrap();
        assert_eq!(
            second.name, "buddy",
            "typed cache must be invalidated when the underlying document is re-preloaded"
        );
    }

    #[test]
    fn json_file_fetcher_rejects_non_file_scheme() {
        let url = Url::parse("https://example.test/foo.json").unwrap();
        let err = JsonFileFetcher
            .fetch(&url)
            .expect_err("non-file must error");
        assert!(matches!(err, LoaderError::UnsupportedFetcherUri(u) if u.contains("https")));
    }

    #[test]
    fn json_file_fetcher_surfaces_missing_file_as_read_error() {
        let url = Url::parse("file:///does/not/exist/roas-loader-test.json").unwrap();
        let err = JsonFileFetcher
            .fetch(&url)
            .expect_err("missing file must error");
        assert!(
            matches!(err, LoaderError::ReadFile { path, .. } if path.to_string_lossy().contains("does/not/exist"))
        );
    }

    #[test]
    fn json_file_fetcher_surfaces_invalid_json_as_parse_error() {
        let file = std::env::temp_dir().join(format!(
            "roas-loader-test-{}-invalid.json",
            std::process::id()
        ));
        fs::write(&file, b"not valid json").unwrap();
        let url = Url::from_file_path(&file).unwrap();
        let err = JsonFileFetcher
            .fetch(&url)
            .expect_err("invalid JSON must error");
        assert!(matches!(err, LoaderError::Parse { .. }));
        fs::remove_file(file).unwrap();
    }

    #[test]
    fn resolve_reference_propagates_pointer_not_found() {
        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "Pet": { "name": "x" } }))
            .unwrap();
        let err = loader
            .resolve_reference("doc.json#/Missing")
            .expect_err("nonexistent pointer must error");
        assert!(matches!(err, LoaderError::PointerNotFound { .. }));
    }

    #[test]
    fn resolve_reference_with_invalid_fragment_errors() {
        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "ok": true }))
            .unwrap();
        let err = loader
            .resolve_reference("doc.json#%ZZ")
            .expect_err("invalid percent-encoding in fragment must error");
        assert!(matches!(err, LoaderError::InvalidFragment(_)));
    }

    #[test]
    fn resolve_reference_with_empty_fragment_returns_full_document() {
        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "ok": true }))
            .unwrap();
        let value = loader.resolve_reference("doc.json").unwrap();
        assert_eq!(value, &serde_json::json!({ "ok": true }));
    }

    #[test]
    fn resolve_reference_as_uses_typed_cache_on_repeat_calls() {
        #[derive(Clone, serde::Deserialize, PartialEq, Debug)]
        struct Pet {
            name: String,
        }

        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "Pet": { "name": "rex" } }))
            .unwrap();

        let first: Pet = loader.resolve_reference_as("doc.json#/Pet").unwrap();
        // Mutate the underlying Value cache directly — the typed cache
        // entry from the first call must still win.
        let mut url = Url::parse("relative:doc.json").unwrap();
        url.set_fragment(None);
        loader
            .cache
            .insert(url, serde_json::json!({ "Pet": { "name": "different" } }));

        let second: Pet = loader.resolve_reference_as("doc.json#/Pet").unwrap();
        assert_eq!(first, second, "typed cache must keep the parsed value");
    }

    fn block_on_simple<F: Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn load_resource_async_returns_preloaded_value_without_fetcher() {
        // Async resolution can reuse the preloaded sync cache; the
        // async-fetcher registry is only consulted on cache miss.
        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "ok": true }))
            .unwrap();
        let value = block_on_simple(loader.load_resource_async("doc.json")).unwrap();
        assert_eq!(value, &serde_json::json!({ "ok": true }));
    }

    #[test]
    fn resolve_reference_async_pointer_into_preloaded_document() {
        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "Pet": { "name": "rex" } }))
            .unwrap();
        let value = block_on_simple(loader.resolve_reference_async("doc.json#/Pet/name")).unwrap();
        assert_eq!(value, &serde_json::json!("rex"));
    }

    #[test]
    fn resolve_reference_async_rejects_internal_only_refs() {
        let mut loader = Loader::new();
        let err = block_on_simple(loader.resolve_reference_async("#/components/schemas/Pet"))
            .expect_err("internal refs are not loader resolvable");
        assert!(matches!(err, LoaderError::MissingBaseUri(_)));
    }

    #[test]
    fn resolve_reference_as_async_uses_shared_typed_cache() {
        #[derive(Clone, serde::Deserialize, PartialEq, Debug)]
        struct Pet {
            name: String,
        }

        let mut loader = Loader::new();
        loader
            .preload_resource("doc.json", serde_json::json!({ "Pet": { "name": "rex" } }))
            .unwrap();

        let sync_first: Pet = loader.resolve_reference_as("doc.json#/Pet").unwrap();
        // Async call after a sync call should hit the typed cache and
        // return the same value, even if the underlying Value is then
        // mutated.
        let mut url = Url::parse("relative:doc.json").unwrap();
        url.set_fragment(None);
        loader
            .cache
            .insert(url, serde_json::json!({ "Pet": { "name": "different" } }));

        let async_second: Pet =
            block_on_simple(loader.resolve_reference_as_async("doc.json#/Pet")).unwrap();
        assert_eq!(
            sync_first, async_second,
            "sync and async share the typed cache"
        );
    }
}
