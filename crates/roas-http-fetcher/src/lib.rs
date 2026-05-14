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
use url::Url;

/// `ResourceFetcher` that fetches JSON documents over HTTP/HTTPS via
/// `reqwest::blocking`.
///
/// The fetcher does not follow `$ref` chains itself — that's the
/// loader's job. Each `fetch` call issues a single GET against the
/// resource URL and parses the response body as JSON.
pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    /// Build a fetcher with a default `reqwest::blocking::Client`
    /// (rustls TLS, no proxy beyond the system defaults).
    pub fn new() -> Self {
        Self {
            client: Client::new(),
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

impl ResourceFetcher for HttpFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        let scheme = uri.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_owned()));
        }
        let response = self.client.get(uri.clone()).send().map_err(|source| {
            // The loader has no `Http`/`Network` variant on `LoaderError`,
            // so wrap the transport failure as a `Parse` error with the
            // network error rendered into the source. This keeps the
            // crate boundary clean without forcing a `LoaderError`
            // change upstream.
            LoaderError::Parse {
                uri: uri.as_str().to_owned(),
                source: serde_json::Error::io(std::io::Error::other(source.to_string())),
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(LoaderError::Parse {
                uri: uri.as_str().to_owned(),
                source: serde_json::Error::io(std::io::Error::other(format!("HTTP {status}"))),
            });
        }
        let body = response.bytes().map_err(|source| LoaderError::Parse {
            uri: uri.as_str().to_owned(),
            source: serde_json::Error::io(std::io::Error::other(source.to_string())),
        })?;
        serde_json::from_slice::<Value>(&body).map_err(|source| LoaderError::Parse {
            uri: uri.as_str().to_owned(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetcher_rejects_non_http_scheme() {
        let mut fetcher = HttpFetcher::new();
        let url = Url::parse("file:///tmp/x.json").unwrap();
        let err = fetcher
            .fetch(&url)
            .expect_err("non-http scheme must be rejected");
        assert!(matches!(err, LoaderError::UnsupportedFetcherUri(_)));
    }
}
