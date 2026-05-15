//! HTTP/HTTPS [`ResourceFetcher`] for the [`roas`] OpenAPI loader.
//!
//! See the crate-level README for a usage example. Transport failures, non-2xx
//! responses, and unreadable bodies are surfaced through [`LoaderError::Fetch`]
//! with a [`HttpFetchError`] source.
//!
//! With the `yaml` feature enabled, the fetcher parses YAML response bodies in
//! addition to JSON. Format selection sniffs the response `Content-Type`
//! header first and falls back to the URL path extension (`.yaml` / `.yml`).

use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use roas::loader::{LoaderError, ResourceFetcher};
#[cfg(feature = "yaml")]
use serde::de::Error as _;
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

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let bytes = response
            .bytes()
            .map_err(|source| fetch_error(uri_string.clone(), HttpFetchError::Body { source }))?;

        parse_body(&uri_string, uri, content_type.as_deref(), &bytes)
    }
}

fn parse_body(
    uri_string: &str,
    uri: &Url,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, LoaderError> {
    if is_yaml(content_type, uri) {
        parse_yaml(uri_string, bytes)
    } else {
        serde_json::from_slice(bytes).map_err(|source| LoaderError::Parse {
            uri: uri_string.to_string(),
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
fn parse_yaml(uri_string: &str, bytes: &[u8]) -> Result<Value, LoaderError> {
    serde_yaml_ng::from_slice(bytes).map_err(|yaml_err| LoaderError::Parse {
        uri: uri_string.to_string(),
        source: serde_json::Error::custom(yaml_err.to_string()),
    })
}

#[cfg(not(feature = "yaml"))]
#[allow(dead_code)]
fn parse_yaml(_uri_string: &str, _bytes: &[u8]) -> Result<Value, LoaderError> {
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
