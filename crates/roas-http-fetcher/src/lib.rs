//! HTTP/HTTPS [`ResourceFetcher`] / [`AsyncResourceFetcher`] for the [`roas`]
//! OpenAPI loader.
//!
//! The generic [`Fetcher<C>`](Fetcher) is parameterised over the underlying
//! `reqwest` client type. Two concrete aliases cover the common cases:
//!   * [`HttpFetcher`] (= `Fetcher<reqwest::blocking::Client>`) — synchronous;
//!     use with [`Loader::register_fetcher`](roas::loader::Loader::register_fetcher).
//!   * [`AsyncHttpFetcher`] (= `Fetcher<reqwest::Client>`) — async; use with
//!     [`Loader::register_async_fetcher`](roas::loader::Loader::register_async_fetcher).
//!     A tokio runtime must be active when the returned future is awaited.
//!
//! Both forms are `Clone` so a single fetcher can be registered for both
//! `http://` and `https://` prefixes, sharing one underlying connection pool.
//! Transport failures, non-2xx responses, and unreadable bodies are surfaced
//! through [`LoaderError::Fetch`] with a [`HttpFetchError`] source. Schemes
//! other than `http` / `https` are rejected with
//! [`LoaderError::UnsupportedFetcherUri`].
//!
//! With the `yaml` feature enabled, both forms parse YAML response bodies in
//! addition to JSON. Format selection sniffs the response `Content-Type`
//! header first and falls back to the URL path extension (`.yaml` / `.yml`).

use reqwest::Client as AsyncClient;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use roas::loader::{AsyncResourceFetcher, FetchFuture, LoaderError, ResourceFetcher};
#[cfg(feature = "yaml")]
use serde::de::Error as _;
use serde_json::Value;
use std::time::Duration;
use url::Url;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP/HTTPS fetcher generic over the underlying `reqwest` client.
///
/// Most callers should reach for one of the two concrete aliases —
/// [`HttpFetcher`] (blocking) or [`AsyncHttpFetcher`] (async) — rather than
/// naming `Fetcher<C>` directly. Naming the aliases lets Rust pick the right
/// inherent `new` / `try_new` impl at the call site without turbofish.
///
/// The struct is `Clone` (the underlying `reqwest` client is `Arc`-backed),
/// so a single fetcher can be registered for both `http://` and `https://`
/// prefixes on the same `Loader`, sharing one connection pool.
#[derive(Clone, Debug)]
pub struct Fetcher<C> {
    client: C,
}

/// Blocking HTTP/HTTPS fetcher, suitable for
/// [`Loader::register_fetcher`](roas::loader::Loader::register_fetcher).
pub type HttpFetcher = Fetcher<Client>;

/// Async HTTP/HTTPS fetcher, suitable for
/// [`Loader::register_async_fetcher`](roas::loader::Loader::register_async_fetcher).
/// A tokio runtime must be active when the returned future is awaited.
pub type AsyncHttpFetcher = Fetcher<AsyncClient>;

impl Fetcher<Client> {
    /// Build a blocking HTTP fetcher with a default `rustls-tls` client and a
    /// 30-second request timeout.
    ///
    /// Panics if the default `reqwest::blocking::Client` cannot be built.
    /// See [`try_new`](Self::try_new) for a fallible variant.
    pub fn new() -> Self {
        Self::try_new().expect("default reqwest::blocking::Client must build")
    }

    /// Fallible variant of [`new`](Self::new) that surfaces TLS / IO
    /// environment failures from the underlying `ClientBuilder`.
    pub fn try_new() -> Result<Self, reqwest::Error> {
        Ok(Self::with_client(
            Client::builder().timeout(DEFAULT_TIMEOUT).build()?,
        ))
    }

    /// Build a fetcher from a caller-provided `reqwest::blocking::Client`.
    /// Use this to override timeouts, redirect policy, TLS config, or proxy
    /// settings.
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }
}

impl Default for Fetcher<Client> {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher<AsyncClient> {
    /// Build an async HTTP fetcher with a default `rustls-tls` client and a
    /// 30-second request timeout.
    ///
    /// Panics if the default `reqwest::Client` cannot be built. See
    /// [`try_new`](Self::try_new) for a fallible variant.
    pub fn new() -> Self {
        Self::try_new().expect("default reqwest::Client must build")
    }

    /// Fallible variant of [`new`](Self::new) that surfaces TLS / IO
    /// environment failures from the underlying `ClientBuilder`.
    pub fn try_new() -> Result<Self, reqwest::Error> {
        Ok(Self::with_client(
            AsyncClient::builder().timeout(DEFAULT_TIMEOUT).build()?,
        ))
    }

    /// Build a fetcher from a caller-provided `reqwest::Client`. Use this to
    /// override timeouts, redirect policy, TLS config, or proxy settings.
    pub fn with_client(client: AsyncClient) -> Self {
        Self { client }
    }
}

impl Default for Fetcher<AsyncClient> {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceFetcher for Fetcher<Client> {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        check_scheme(uri)?;
        let response = self.client.get(uri.as_str()).send().map_err(|source| {
            fetch_error(uri.as_str().to_string(), HttpFetchError::Request { source })
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(fetch_error(
                uri.as_str().to_string(),
                HttpFetchError::Status { status },
            ));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let bytes = response.bytes().map_err(|source| {
            fetch_error(uri.as_str().to_string(), HttpFetchError::Body { source })
        })?;

        parse_body(uri, content_type.as_deref(), &bytes)
    }
}

impl AsyncResourceFetcher for Fetcher<AsyncClient> {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a> {
        let client = self.client.clone();
        let uri = uri.clone();
        Box::pin(async move {
            check_scheme(&uri)?;
            let response = client.get(uri.as_str()).send().await.map_err(|source| {
                fetch_error(uri.as_str().to_string(), HttpFetchError::Request { source })
            })?;

            let status = response.status();
            if !status.is_success() {
                return Err(fetch_error(
                    uri.as_str().to_string(),
                    HttpFetchError::Status { status },
                ));
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let bytes = response.bytes().await.map_err(|source| {
                fetch_error(uri.as_str().to_string(), HttpFetchError::Body { source })
            })?;

            parse_body(&uri, content_type.as_deref(), &bytes)
        })
    }
}

fn check_scheme(uri: &Url) -> Result<(), LoaderError> {
    match uri.scheme() {
        "http" | "https" => Ok(()),
        _ => Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_string())),
    }
}

fn parse_body(uri: &Url, content_type: Option<&str>, bytes: &[u8]) -> Result<Value, LoaderError> {
    if is_yaml(content_type, uri) {
        parse_yaml(uri, bytes)
    } else {
        serde_json::from_slice(bytes).map_err(|source| LoaderError::Parse {
            uri: uri.as_str().to_string(),
            source,
        })
    }
}

/// Decide whether to treat the response body as YAML.
///
/// With the `yaml` feature off this always returns `false`. With it on, the
/// decision is `Content-Type`-first, URL-extension-second:
///   1. `Content-Type` containing `yaml` (covers `application/yaml`,
///      `application/x-yaml`, `text/yaml`, `text/x-yaml`, etc.).
///   2. URL path ending in `.yaml` or `.yml`.
#[allow(unused_variables)]
fn is_yaml(content_type: Option<&str>, uri: &Url) -> bool {
    #[cfg(feature = "yaml")]
    {
        if let Some(ct) = content_type {
            let mime = ct
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if mime.contains("yaml") {
                return true;
            }
            if !mime.is_empty() && mime != "application/octet-stream" {
                return false;
            }
        }
        let path = uri.path().to_ascii_lowercase();
        path.ends_with(".yaml") || path.ends_with(".yml")
    }
    #[cfg(not(feature = "yaml"))]
    {
        false
    }
}

#[cfg(feature = "yaml")]
fn parse_yaml(uri: &Url, bytes: &[u8]) -> Result<Value, LoaderError> {
    serde_yaml_ng::from_slice(bytes).map_err(|yaml_err| LoaderError::Parse {
        uri: uri.as_str().to_string(),
        source: serde_json::Error::custom(yaml_err.to_string()),
    })
}

#[cfg(not(feature = "yaml"))]
#[allow(dead_code)]
fn parse_yaml(_uri: &Url, _bytes: &[u8]) -> Result<Value, LoaderError> {
    unreachable!("parse_yaml is only reached when the `yaml` feature is enabled")
}

/// Transport-layer failure exposed by [`HttpFetcher`].
///
/// `HttpFetchError` is what the boxed `source` of [`LoaderError::Fetch`]
/// carries when produced by this crate. Downstream code that needs the
/// structured detail (e.g. a status-code-specific retry) can downcast via
/// `std::error::Error::source` and `downcast_ref::<HttpFetchError>()`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpFetchError {
    /// The underlying request could not be dispatched: DNS lookup failed, the
    /// connection was refused, the request timed out, etc.
    #[error("HTTP request failed")]
    Request {
        #[source]
        source: reqwest::Error,
    },

    /// The server returned a non-success status code.
    #[error("non-success HTTP response: {status}")]
    Status { status: StatusCode },

    /// The response headers came back fine but the body could not be read.
    #[error("failed to read response body")]
    Body {
        #[source]
        source: reqwest::Error,
    },
}

fn fetch_error(uri: String, source: HttpFetchError) -> LoaderError {
    LoaderError::Fetch {
        uri,
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_fetcher_default_constructs() {
        let _ = HttpFetcher::default();
        let _ = HttpFetcher::new();
        let _ = AsyncHttpFetcher::default();
        let _ = AsyncHttpFetcher::new();
    }

    #[test]
    fn http_fetcher_try_new_succeeds_for_default_config() {
        HttpFetcher::try_new().expect("blocking client must build");
        AsyncHttpFetcher::try_new().expect("async client must build");
    }

    #[test]
    fn http_fetcher_is_clone_and_shares_pool() {
        // Reqwest's clients are Arc-backed internally, so cloning is cheap and
        // shares the connection pool. Exercising clone covers the contract.
        let fetcher = HttpFetcher::new();
        let _second = fetcher.clone();
        let async_fetcher = AsyncHttpFetcher::new();
        let _async_second = async_fetcher.clone();
    }

    #[test]
    fn fetch_error_helper_boxes_into_loader_error_fetch() {
        let inner = HttpFetchError::Status {
            status: StatusCode::NOT_FOUND,
        };
        let err = fetch_error("https://example.test/x.json".into(), inner);
        match err {
            LoaderError::Fetch { uri, source } => {
                assert_eq!(uri, "https://example.test/x.json");
                let downcast = source
                    .downcast_ref::<HttpFetchError>()
                    .expect("source must downcast to HttpFetchError");
                assert!(matches!(
                    downcast,
                    HttpFetchError::Status {
                        status: StatusCode::NOT_FOUND
                    }
                ));
            }
            other => panic!("expected LoaderError::Fetch, got {other:?}"),
        }
    }

    #[test]
    fn check_scheme_accepts_http_and_https() {
        check_scheme(&Url::parse("http://example.test/x.json").unwrap()).unwrap();
        check_scheme(&Url::parse("https://example.test/x.json").unwrap()).unwrap();
    }

    #[test]
    fn check_scheme_rejects_file_uri_with_unsupported_fetcher_uri() {
        let err = check_scheme(&Url::parse("file:///tmp/x.json").unwrap())
            .expect_err("file:// must be rejected");
        assert!(matches!(err, LoaderError::UnsupportedFetcherUri(s) if s.starts_with("file://")));
    }
}
