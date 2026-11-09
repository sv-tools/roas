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
use roas::loader::{
    AsyncResourceFetcher, DocumentFetchFuture, FetchFuture, LoadedDocument, LoaderError,
    ResourceFetcher,
};
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
        Ok(self.fetch_document(uri)?.document)
    }

    fn fetch_document(&mut self, uri: &Url) -> Result<LoadedDocument, LoaderError> {
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

        let retrieval = response.url().clone();
        let bytes = response.bytes().map_err(|source| {
            fetch_error(uri.as_str().to_string(), HttpFetchError::Body { source })
        })?;

        parse_body(uri, &retrieval, content_type.as_deref(), &bytes)
            .map(|document| LoadedDocument::new(document, retrieval))
    }
}

impl AsyncResourceFetcher for Fetcher<AsyncClient> {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a> {
        Box::pin(async move { Ok(self.fetch_document(uri).await?.document) })
    }

    fn fetch_document<'a>(&'a mut self, uri: &'a Url) -> DocumentFetchFuture<'a> {
        let client = self.client.clone();
        Box::pin(async move {
            check_scheme(uri)?;
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

            let retrieval = response.url().clone();
            let bytes = response.bytes().await.map_err(|source| {
                fetch_error(uri.as_str().to_string(), HttpFetchError::Body { source })
            })?;

            parse_body(uri, &retrieval, content_type.as_deref(), &bytes)
                .map(|document| LoadedDocument::new(document, retrieval))
        })
    }
}

fn check_scheme(uri: &Url) -> Result<(), LoaderError> {
    match uri.scheme() {
        "http" | "https" => Ok(()),
        _ => Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_string())),
    }
}

fn parse_body(
    requested: &Url,
    retrieval: &Url,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, LoaderError> {
    // Preserve the original request's extension hint across redirects. Either
    // URL may add a YAML hint, but an explicit non-YAML media type wins over both.
    if is_yaml(content_type, requested) || is_yaml(content_type, retrieval) {
        parse_yaml(retrieval, bytes)
    } else {
        serde_json::from_slice(bytes).map_err(|source| LoaderError::Parse {
            uri: retrieval.as_str().to_string(),
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
    serde_saphyr::from_slice_with_options(bytes, yaml_options()).map_err(|yaml_err| {
        LoaderError::Parse {
            uri: uri.as_str().to_string(),
            source: serde_json::Error::custom(yaml_err.to_string()),
        }
    })
}

/// How every YAML body is read. The same settings are used by each roas
/// crate that reads YAML, so a document means the same thing to all of them.
///
/// - YAML 1.2 booleans only: `yes` / `on` / `no` / `off` stay strings.
/// - Integer spellings `017` and `1_000` stay strings rather than becoming numbers.
/// - `!!binary` scalars are read as their base64 text, not decoded.
/// - `.inf` / `.nan` pass through (a `serde_json::Value` holds them as `null`).
/// - `<<: *anchor` merge keys are expanded.
/// - A duplicate mapping key is an error rather than a silent last-wins.
///
/// A [`serde_saphyr::Budget`] stays on, raised so large specs fit: it rejects
/// alias bombs, nesting past 64 levels, documents past 10M nodes or
/// 64 MiB of scalar text, and an anchored node past 1M events.
#[cfg(feature = "yaml")]
fn yaml_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        // Same options in every roas crate; keep the copies in step
        // (`grep -rn 'strict_booleans: true' crates`). The budget is raised
        // from the defaults (250k nodes, 1M events, 50k aliases, 50k anchors,
        // 10k merge keys), which reject ordinary large or generated specs. Depth stays at 64: deeper nesting overflows a 2 MiB
        // thread stack in debug builds. The alias-to-anchor ratio check is off:
        // it rejects any document reusing one anchor 100+ times, a common
        // pattern in specs. Alias replay is raised to 10M events so a large
        // schema can be aliased thousands of times; the 1M cap on events
        // recorded under anchors still stops an alias bomb, and also caps
        // how large one anchored node can be. `!!binary` is
        // read as its text, as before, not decoded.
        budget: serde_saphyr::budget! {
            max_nodes: 10_000_000,
            max_events: 25_000_000,
            max_aliases: 1_000_000,
            max_anchors: 1_000_000,
            max_merge_keys: 1_000_000,
            enforce_alias_anchor_ratio: false,
        },
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: 10_000_000,
        },
        strict_booleans: true,
        ignore_binary_tag_for_string: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
    }
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

#[cfg(all(test, feature = "yaml"))]
mod yaml_tests {
    use super::*;
    use serde_json::json;

    fn parse(yaml: &str) -> Result<Value, String> {
        let uri = Url::parse("https://example.test/spec.yaml").unwrap();
        parse_yaml(&uri, yaml.as_bytes()).map_err(|e| match e {
            LoaderError::Parse { source, .. } => source.to_string(),
            other => panic!("unexpected error: {other}"),
        })
    }

    /// Each scalar whose meaning depends on the YAML options, next to what
    /// it must read as.
    #[test]
    fn yaml_scalars_read_as_yaml_1_2() {
        for (scalar, expected) in [
            ("yes", json!("yes")),
            ("on", json!("on")),
            ("no", json!("no")),
            ("off", json!("off")),
            ("true", json!(true)),
            ("False", json!(false)),
            ("017", json!("017")),
            ("1_000", json!("1_000")),
            ("17", json!(17)),
            ("0x11", json!(17)),
            ("1.5", json!(1.5)),
            (".inf", json!(null)),
            ("3.1.0", json!("3.1.0")),
            ("!!binary SGVsbG8=", json!("SGVsbG8=")),
            ("!!binary /wAB", json!("/wAB")),
        ] {
            assert_eq!(
                parse(&format!("v: {scalar}\n")).unwrap(),
                json!({ "v": expected }),
                "{scalar}",
            );
        }
    }

    #[test]
    fn yaml_merge_keys_expand_through_an_anchor() {
        let yaml = "base: &base { type: string, minLength: 1 }\n\
                    name:\n  <<: *base\n  minLength: 3\n";
        assert_eq!(
            parse(yaml).unwrap(),
            json!({
                "base": { "type": "string", "minLength": 1 },
                "name": { "type": "string", "minLength": 3 },
            }),
        );
    }

    #[test]
    fn yaml_duplicate_key_is_an_error() {
        let err = parse("a: 1\na: 2\n").unwrap_err();
        assert!(err.contains("duplicate mapping key"), "{err}");
    }

    #[test]
    fn yaml_alias_bomb_is_rejected_by_the_budget() {
        // Billion laughs: each level aliases the previous one ten times, so
        // full expansion is 10^7 scalars. The anchor-replay limits stop it.
        let mut yaml = String::from("l0: &l0 [lol, lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
        for i in 1..7 {
            let prev = format!("*l{}", i - 1);
            yaml.push_str(&format!("l{i}: &l{i} [{}]\n", vec![prev; 10].join(", ")));
        }
        let err = parse(&yaml).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }

    /// One shared anchor reused many times, as specs do for common
    /// responses, is not a bomb.
    #[test]
    fn yaml_anchor_reused_many_times_is_accepted() {
        // 40 properties aliased 6,000 times replays over a million events.
        let props: String = (0..40)
            .map(|i| format!("    p{i}: {{ type: string }}\n"))
            .collect();
        let mut yaml = format!("schema: &s\n  type: object\n  properties:\n{props}responses:\n");
        for i in 0..6_000 {
            yaml.push_str(&format!("  r{i}: *s\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(
            value["responses"]["r5999"]["properties"]["p39"]["type"],
            "string"
        );
    }

    /// Past the library's default caps of 50k aliases and 50k anchors,
    /// which generated specs (one anchor per shared object) can reach.
    #[test]
    fn yaml_many_aliases_and_anchors_fit_the_budget() {
        let mut yaml = String::from("shared: &s x\n");
        for i in 0..60_000 {
            yaml.push_str(&format!("a{i}: *s\nb{i}: &b{i} y\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(value["a59999"], "x");
        assert_eq!(value["b59999"], "y");
    }

    /// Past the library's default cap of 10k merge keys, which only applies
    /// because merge keys are expanded.
    #[test]
    fn yaml_many_merge_keys_fit_the_budget() {
        let mut yaml = String::from("base: &b { type: string }\n");
        for i in 0..10_001 {
            yaml.push_str(&format!("s{i}: {{ <<: *b, minLength: 1 }}\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(value["s10000"]["type"], "string");
    }

    #[test]
    fn yaml_nesting_is_capped_at_64_levels() {
        let nested = |depth: usize| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        parse(&nested(64)).unwrap();
        let err = parse(&nested(65)).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }

    /// Past the library's default budget of 250k nodes, which rejects
    /// ordinary large specs.
    #[test]
    fn yaml_large_document_fits_the_budget() {
        let yaml: String = (0..60_000).map(|i| format!("k{i}: [a, b, c]\n")).collect();
        let value = parse(&yaml).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 60_000);
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
