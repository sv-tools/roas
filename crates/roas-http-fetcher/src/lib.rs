//! HTTP/HTTPS [`ResourceFetcher`] for the [`roas`] OpenAPI loader.
//!
//! See the crate-level README for a usage example. Transport failures, non-2xx
//! responses, and unreadable bodies are surfaced through [`LoaderError::Fetch`]
//! with a [`HttpFetchError`] source.

use reqwest::StatusCode;
use reqwest::blocking::Client;
use roas::loader::{LoaderError, ResourceFetcher};
use serde_json::Value;
use std::time::Duration;
use url::Url;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP/HTTPS fetcher backed by `reqwest::blocking::Client`.
///
/// `HttpFetcher` is `Clone` so a single underlying connection pool can be
/// shared across multiple `Loader` registrations (e.g. one for `http://` and
/// one for `https://`).
#[derive(Clone, Debug)]
pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    /// Build an HTTP fetcher with a default `rustls-tls` client and a 30-second
    /// request timeout.
    pub fn new() -> Self {
        Self::with_client(
            Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("default reqwest::blocking::Client must build"),
        )
    }

    /// Build a fetcher from a caller-provided `reqwest::blocking::Client`. Use
    /// this to override timeouts, redirect policy, TLS config, or proxy
    /// settings.
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceFetcher for HttpFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        let uri_string = uri.as_str().to_string();
        let response = self.client.get(uri.clone()).send().map_err(|source| {
            fetch_error(uri_string.clone(), HttpFetchError::Request { source })
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(fetch_error(uri_string, HttpFetchError::Status { status }));
        }

        let bytes = response
            .bytes()
            .map_err(|source| fetch_error(uri_string.clone(), HttpFetchError::Body { source }))?;

        serde_json::from_slice(&bytes).map_err(|source| LoaderError::Parse {
            uri: uri_string,
            source,
        })
    }
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
    }

    #[test]
    fn http_fetcher_is_clone_and_shares_pool() {
        // Reqwest's blocking Client is Arc-backed internally, so cloning is
        // cheap and shares the connection pool. The contract we care about
        // here is that the wrapper is Clone — exercising it suffices.
        let fetcher = HttpFetcher::new();
        let _second = fetcher.clone();
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
}
