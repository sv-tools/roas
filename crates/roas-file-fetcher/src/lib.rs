//! Filesystem [`ResourceFetcher`] for the [`roas`] OpenAPI loader.
//!
//! [`FileFetcher`] is blocking and backed by [`std::fs::read`]. Non-`file://`
//! URIs are rejected with [`LoaderError::UnsupportedFetcherUri`]; I/O failures
//! surface as [`LoaderError::ReadFile`]; body parse failures as
//! [`LoaderError::Parse`].
//!
//! Optional features:
//!   * `async` — also expose `AsyncFileFetcher` for
//!     `roas::loader::Loader::register_async_fetcher`, backed by
//!     `tokio::fs::read`. Requires an active tokio runtime when the returned
//!     future is awaited. Off by default; enabling it pulls in `tokio` with
//!     `fs` + `rt` features.
//!   * `yaml` — parse YAML file bodies in addition to JSON. Selection is by
//!     file path extension (`.yaml` / `.yml`). Pulls in `serde-saphyr`.

#[cfg(feature = "async")]
use roas::loader::{AsyncResourceFetcher, FetchFuture};
use roas::loader::{LoaderError, ResourceFetcher};
#[cfg(feature = "yaml")]
use serde::de::Error as _;
use serde_json::Value;
use std::path::PathBuf;
use url::Url;

/// Blocking filesystem fetcher, suitable for
/// [`Loader::register_fetcher`](roas::loader::Loader::register_fetcher).
#[derive(Clone, Debug, Default)]
pub struct FileFetcher;

impl FileFetcher {
    /// Construct a blocking file fetcher.
    pub fn new() -> Self {
        Self
    }
}

/// Async filesystem fetcher, suitable for
/// [`Loader::register_async_fetcher`](roas::loader::Loader::register_async_fetcher).
/// A tokio runtime must be active when the returned future is awaited.
///
/// Available only with the `async` feature.
#[cfg(feature = "async")]
#[derive(Clone, Debug, Default)]
pub struct AsyncFileFetcher;

#[cfg(feature = "async")]
impl AsyncFileFetcher {
    /// Construct an async file fetcher.
    pub fn new() -> Self {
        Self
    }
}

impl ResourceFetcher for FileFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        check_scheme(uri)?;
        let path = uri_to_path(uri)?;
        let bytes =
            std::fs::read(&path).map_err(|source| LoaderError::ReadFile { path, source })?;
        parse_body(uri, &bytes)
    }
}

#[cfg(feature = "async")]
impl AsyncResourceFetcher for AsyncFileFetcher {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a> {
        Box::pin(async move {
            check_scheme(uri)?;
            let path = uri_to_path(uri)?;
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|source| LoaderError::ReadFile { path, source })?;
            parse_body(uri, &bytes)
        })
    }
}

fn check_scheme(uri: &Url) -> Result<(), LoaderError> {
    if uri.scheme() == "file" {
        Ok(())
    } else {
        Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_string()))
    }
}

fn uri_to_path(uri: &Url) -> Result<PathBuf, LoaderError> {
    uri.to_file_path()
        .map_err(|()| LoaderError::InvalidFileUri(uri.as_str().to_string()))
}

fn parse_body(uri: &Url, bytes: &[u8]) -> Result<Value, LoaderError> {
    if is_yaml(uri) {
        parse_yaml(uri, bytes)
    } else {
        serde_json::from_slice(bytes).map_err(|source| LoaderError::Parse {
            uri: uri.as_str().to_string(),
            source,
        })
    }
}

/// Decide whether to treat the file body as YAML.
///
/// With the `yaml` feature off this always returns `false`. With it on, the
/// URL path is checked against the `.yaml` / `.yml` extensions (case-insensitive).
#[allow(unused_variables)]
fn is_yaml(uri: &Url) -> bool {
    #[cfg(feature = "yaml")]
    {
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
/// - `017` and `1_000` stay strings rather than becoming numbers.
/// - `.inf` / `.nan` pass through (a `serde_json::Value` holds them as `null`).
/// - `<<: *anchor` merge keys are expanded.
/// - A duplicate mapping key is an error rather than a silent last-wins.
///
/// The default [`serde_saphyr::Budget`] stays on: it rejects alias bombs and
/// pathologically deep or large documents before they are materialised.
#[cfg(feature = "yaml")]
fn yaml_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
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

#[cfg(all(test, feature = "yaml"))]
mod yaml_tests {
    use super::*;
    use serde_json::json;

    fn parse(yaml: &str) -> Result<Value, String> {
        let uri = Url::parse("file:///spec.yaml").unwrap();
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
        let mut yaml = String::from("l0: &l0 [lol, lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
        for i in 1..10 {
            let prev = format!("*l{}", i - 1);
            yaml.push_str(&format!("l{i}: &l{i} [{}]\n", vec![prev; 10].join(", ")));
        }
        let err = parse(&yaml).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Helper: extract `&Path` from `LoaderError::ReadFile` so the error tests
    /// stay one line each.
    fn read_file_path(err: &LoaderError) -> Option<&Path> {
        match err {
            LoaderError::ReadFile { path, .. } => Some(path.as_path()),
            _ => None,
        }
    }

    #[test]
    fn file_fetcher_default_constructs() {
        let _: FileFetcher = Default::default();
        let _ = FileFetcher::new();
    }

    #[cfg(feature = "async")]
    #[test]
    fn async_file_fetcher_default_constructs() {
        let _: AsyncFileFetcher = Default::default();
        let _ = AsyncFileFetcher::new();
    }

    #[test]
    fn check_scheme_accepts_only_file() {
        check_scheme(&Url::parse("file:///tmp/x.json").unwrap()).unwrap();
        let err = check_scheme(&Url::parse("http://example.test/x.json").unwrap())
            .expect_err("http must be rejected");
        assert!(matches!(err, LoaderError::UnsupportedFetcherUri(s) if s.starts_with("http://")));
    }

    #[test]
    fn read_file_path_extracts_path_from_read_file_variant() {
        let err = LoaderError::ReadFile {
            path: PathBuf::from("/nope"),
            source: std::io::Error::other("missing"),
        };
        assert_eq!(read_file_path(&err), Some(Path::new("/nope")));
        let parse_err = LoaderError::Parse {
            uri: "x".into(),
            source: serde_json::from_str::<Value>("@").unwrap_err(),
        };
        assert_eq!(read_file_path(&parse_err), None);
    }
}
