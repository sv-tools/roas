//! HTTP / HTTPS resource fetcher for [`roas`][roas]'s external-reference
//! loader.
//!
//! Registers a `reqwest::blocking::Client` as a [`ResourceFetcher`] so
//! external `$ref`s with `http://` or `https://` schemes can be resolved
//! by [`roas::loader::Loader`].
//!
//! ```no_run
//! use roas::loader::Loader;
//! use roas_http_fetcher::HttpFetcher;
//!
//! let mut loader = Loader::new();
//! loader.register_fetcher("https://", HttpFetcher::new());
//! ```
//!
//! Disabling certificate validation, attaching custom headers, etc. is
//! done by building a `reqwest::blocking::Client` yourself and handing
//! it to [`HttpFetcher::with_client`].
//!
//! [roas]: https://docs.rs/roas

use reqwest::blocking::Client;
use roas::loader::{LoaderError, ResourceFetcher};
use serde_json::Value;
use std::time::Duration;
use url::Url;

/// Default per-request timeout for `HttpFetcher::new()`. Picked
/// generously enough to clear a slow public schema host on the first
/// request, but low enough that an unreachable or unresponsive
/// upstream can't hang a validation pass. Callers wanting different
/// behaviour can build their own `reqwest::blocking::Client` and pass
/// it to [`HttpFetcher::with_client`].
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// `ResourceFetcher` that fetches JSON documents over HTTP/HTTPS via
/// `reqwest::blocking`.
///
/// The fetcher does not follow `$ref` chains itself — that's the
/// loader's job. Each `fetch` call issues a single GET against the
/// resource URL and parses the response body as JSON.
///
/// `HttpFetcher` is `Clone`; the underlying `reqwest::blocking::Client`
/// shares its connection pool across clones, so a single
/// `HttpFetcher::new()` cloned into separate `http://` and `https://`
/// registrations on a `Loader` reuses one pool for both schemes.
#[derive(Clone)]
pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    /// Build a fetcher with a default `reqwest::blocking::Client`
    /// (rustls TLS, no proxy beyond the system defaults, 30-second
    /// per-request timeout).
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("default reqwest::blocking::Client builder must succeed"),
        }
    }

    /// Build a fetcher from a pre-configured `reqwest::blocking::Client`.
    /// Use this to attach custom headers, set timeouts, configure
    /// proxies, disable cert validation, etc.
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned to [`LoaderError::Fetch`] from this fetcher.
/// Carries the HTTP status (if a response was actually received) and
/// the originating `reqwest` error chain.
#[derive(Debug, thiserror::Error)]
pub enum HttpFetchError {
    /// Transport-level failure (DNS, TCP, TLS, redirect loop, etc.) —
    /// the request never produced a response.
    #[error("HTTP request to `{uri}` failed: {source}")]
    Request {
        uri: String,
        #[source]
        source: reqwest::Error,
    },
    /// Non-2xx HTTP response.
    #[error("HTTP {status} from `{uri}`")]
    Status {
        uri: String,
        status: reqwest::StatusCode,
    },
    /// 2xx response but the body couldn't be read end-to-end.
    #[error("reading HTTP body from `{uri}` failed: {source}")]
    Body {
        uri: String,
        #[source]
        source: reqwest::Error,
    },
}

impl ResourceFetcher for HttpFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        let scheme = uri.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_owned()));
        }
        let uri_str = uri.as_str().to_owned();
        // Transport failure (DNS / TCP / TLS / redirect cycle / etc.).
        let response =
            self.client
                .get(uri.clone())
                .send()
                .map_err(|source| LoaderError::Fetch {
                    uri: uri_str.clone(),
                    source: Box::new(HttpFetchError::Request {
                        uri: uri_str.clone(),
                        source,
                    }),
                })?;
        let status = response.status();
        if !status.is_success() {
            return Err(LoaderError::Fetch {
                uri: uri_str.clone(),
                source: Box::new(HttpFetchError::Status {
                    uri: uri_str,
                    status,
                }),
            });
        }
        let body = response.bytes().map_err(|source| LoaderError::Fetch {
            uri: uri_str.clone(),
            source: Box::new(HttpFetchError::Body {
                uri: uri_str.clone(),
                source,
            }),
        })?;
        // Parse failures stay on `Parse` — they're genuinely about the
        // body's content, not the transport.
        serde_json::from_slice::<Value>(&body).map_err(|source| LoaderError::Parse {
            uri: uri_str,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fetcher_rejects_non_http_scheme() {
        let mut fetcher = HttpFetcher::new();
        let url = Url::parse("file:///tmp/x.json").unwrap();
        let err = fetcher
            .fetch(&url)
            .expect_err("non-http scheme must be rejected");
        assert!(matches!(err, LoaderError::UnsupportedFetcherUri(_)));
    }

    #[test]
    fn transport_failure_surfaces_as_loader_fetch_error() {
        // Point at a port that won't accept connections; the request
        // path should never get past the transport layer. Use a short
        // timeout so the test doesn't hang in CI.
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap();
        let mut fetcher = HttpFetcher::with_client(client);
        // 127.0.0.1:1 is a privileged port that's not listening; the
        // OS rejects the connect immediately.
        let url = Url::parse("http://127.0.0.1:1/openapi.json").unwrap();
        let err = fetcher
            .fetch(&url)
            .expect_err("transport failure must be surfaced");
        match err {
            LoaderError::Fetch { uri, source } => {
                assert_eq!(uri, "http://127.0.0.1:1/openapi.json");
                let detail = source.to_string();
                assert!(
                    detail.contains("HTTP request") || detail.contains("127.0.0.1:1"),
                    "expected request-failure detail, got: {detail}"
                );
            }
            other => panic!("expected LoaderError::Fetch, got {other:?}"),
        }
    }
}
