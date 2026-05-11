//! External resource loader for OpenAPI references.
//!
//! A loader is responsible for fetching and caching external resources. It
//! does not fetch anything by default. Callers opt in by registering fetchers
//! for URI prefixes, for example `file://` or `https://`.

use serde::de::DeserializeOwned;
use serde_json::Value;
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

    #[error("external reference `{0}` needs a base resource")]
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
pub struct Loader {
    fetchers: BTreeMap<String, Box<dyn ResourceFetcher>>,
    async_fetchers: BTreeMap<String, Box<dyn AsyncResourceFetcher>>,
    cache: BTreeMap<Url, Value>,
}

impl Loader {
    /// Create a loader with no registered fetchers.
    pub fn new() -> Self {
        Self {
            fetchers: BTreeMap::new(),
            async_fetchers: BTreeMap::new(),
            cache: BTreeMap::new(),
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
    /// Async loading uses the longest matching prefix across both async and
    /// sync fetchers. Sync loading only uses sync fetchers.
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
    pub fn preload_resource(
        &mut self,
        uri: impl AsRef<str>,
        document: Value,
    ) -> Result<Option<Value>, LoaderError> {
        let (key, _) = parse_reference(uri.as_ref())?;
        Ok(self.cache.insert(key, document))
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
    pub fn resolve_reference_as<T>(&mut self, reference: &str) -> Result<T, LoaderError>
    where
        T: DeserializeOwned,
    {
        let (key, _) = parse_reference(reference)?;
        let uri = key.to_string();
        let value = self.resolve_reference(reference)?;
        serde_json::from_value(value.clone()).map_err(|source| LoaderError::Parse { uri, source })
    }

    /// Asynchronously resolve and deserialize a reference into the requested type.
    pub async fn resolve_reference_as_async<T>(&mut self, reference: &str) -> Result<T, LoaderError>
    where
        T: DeserializeOwned,
    {
        let (key, _) = parse_reference(reference)?;
        let uri = key.to_string();
        let value = self.resolve_reference_async(reference).await?;
        serde_json::from_value(value.clone()).map_err(|source| LoaderError::Parse { uri, source })
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

        let reference = format!("file://{}#/components/schemas/Pet", file.display());
        assert!(loader.resolve_reference(&reference).is_ok());
        assert!(loader.resolve_reference(&reference).is_ok());

        fs::remove_file(file).unwrap();
    }

    #[test]
    fn relative_reference_uses_preloaded_document() {
        let dir = std::env::temp_dir().join(format!("roas-loader-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("openapi.json");
        fs::write(&root, br#"{"openapi":"3.2.0"}"#).unwrap();

        let mut loader = Loader::new();
        loader.register_fetcher("file://", JsonFileFetcher);
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

        fs::remove_file(dir.join("openapi.json")).unwrap();
        fs::remove_dir(dir).unwrap();
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
}
