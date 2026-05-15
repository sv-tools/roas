#[cfg(feature = "async")]
use roas::loader::AsyncResourceFetcher;
use roas::loader::{LoaderError, ResourceFetcher};
#[cfg(feature = "async")]
use roas_file_fetcher::AsyncFileFetcher;
use roas_file_fetcher::FileFetcher;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use url::Url;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp-file path scoped to the test process so parallel tests don't
/// trample each other.
fn temp_file(suffix: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "roas-file-fetcher-{}-{}-{suffix}",
        std::process::id(),
        n,
    ))
}

struct TempFile(PathBuf);

impl TempFile {
    fn write(suffix: &str, body: &[u8]) -> Self {
        let path = temp_file(suffix);
        fs::write(&path, body).expect("write temp file");
        Self(path)
    }

    fn url(&self) -> Url {
        Url::from_file_path(&self.0).expect("file path to url")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn file_fetcher_reads_and_parses_json_body() {
    let file = TempFile::write("ok.json", br#"{"hello":"world"}"#);
    let mut fetcher = FileFetcher::new();
    let value = fetcher.fetch(&file.url()).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "hello": "world" }));
}

#[test]
fn file_fetcher_rejects_non_file_scheme_with_unsupported_fetcher_uri() {
    let mut fetcher = FileFetcher::new();
    let err = fetcher
        .fetch(&Url::parse("https://example.test/x.json").unwrap())
        .expect_err("https must be rejected");
    assert!(matches!(err, LoaderError::UnsupportedFetcherUri(_)));
}

#[test]
fn file_fetcher_surfaces_missing_file_as_read_file_error() {
    let url = Url::from_file_path(temp_file("missing.json")).unwrap();
    let mut fetcher = FileFetcher::new();
    let err = fetcher.fetch(&url).expect_err("missing file must error");
    assert!(matches!(err, LoaderError::ReadFile { .. }));
}

#[test]
fn file_fetcher_surfaces_invalid_json_body_as_parse_error() {
    let file = TempFile::write("bad.json", b"not json");
    let mut fetcher = FileFetcher::new();
    let err = fetcher
        .fetch(&file.url())
        .expect_err("invalid JSON must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[cfg(feature = "yaml")]
#[test]
fn file_fetcher_parses_yaml_when_path_extension_is_yaml() {
    let file = TempFile::write("ok.yaml", b"name: pet\ncount: 3\n");
    let mut fetcher = FileFetcher::new();
    let value = fetcher.fetch(&file.url()).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "name": "pet", "count": 3 }));
}

#[cfg(feature = "yaml")]
#[test]
fn file_fetcher_parses_yaml_when_path_extension_is_yml() {
    let file = TempFile::write("ok.yml", b"items:\n  - a\n  - b\n");
    let mut fetcher = FileFetcher::new();
    let value = fetcher.fetch(&file.url()).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "items": ["a", "b"] }));
}

#[cfg(feature = "yaml")]
#[test]
fn file_fetcher_yaml_parse_error_surfaces_as_loader_error_parse() {
    let file = TempFile::write("bad.yaml", b"key:\n\tvalue: oops\n");
    let mut fetcher = FileFetcher::new();
    let err = fetcher
        .fetch(&file.url())
        .expect_err("malformed YAML must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[cfg(not(feature = "yaml"))]
#[test]
fn file_fetcher_without_yaml_feature_parses_yaml_path_as_json_and_errors() {
    let file = TempFile::write("noyaml.yaml", b"name: pet\n");
    let mut fetcher = FileFetcher::new();
    let err = fetcher
        .fetch(&file.url())
        .expect_err("yaml body parsed as json must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_file_fetcher_reads_and_parses_json_body() {
    let file = TempFile::write("async-ok.json", br#"{"hello":"world"}"#);
    let mut fetcher = AsyncFileFetcher::new();
    let value = fetcher.fetch(&file.url()).await.expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "hello": "world" }));
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_file_fetcher_rejects_non_file_scheme_with_unsupported_fetcher_uri() {
    let mut fetcher = AsyncFileFetcher::new();
    let err = fetcher
        .fetch(&Url::parse("https://example.test/x.json").unwrap())
        .await
        .expect_err("https must be rejected");
    assert!(matches!(err, LoaderError::UnsupportedFetcherUri(_)));
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_file_fetcher_surfaces_missing_file_as_read_file_error() {
    let url = Url::from_file_path(temp_file("async-missing.json")).unwrap();
    let mut fetcher = AsyncFileFetcher::new();
    let err = fetcher
        .fetch(&url)
        .await
        .expect_err("missing file must error");
    assert!(matches!(err, LoaderError::ReadFile { .. }));
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_file_fetcher_surfaces_invalid_json_body_as_parse_error() {
    let file = TempFile::write("async-bad.json", b"not json");
    let mut fetcher = AsyncFileFetcher::new();
    let err = fetcher
        .fetch(&file.url())
        .await
        .expect_err("invalid JSON must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[cfg(all(feature = "async", feature = "yaml"))]
#[tokio::test]
async fn async_file_fetcher_parses_yaml_when_path_extension_is_yaml() {
    let file = TempFile::write("async-ok.yaml", b"name: pet\ncount: 3\n");
    let mut fetcher = AsyncFileFetcher::new();
    let value = fetcher.fetch(&file.url()).await.expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "name": "pet", "count": 3 }));
}

#[cfg(all(feature = "async", feature = "yaml"))]
#[tokio::test]
async fn async_file_fetcher_yaml_parse_error_surfaces_as_loader_error_parse() {
    let file = TempFile::write("async-bad.yaml", b"key:\n\tvalue: oops\n");
    let mut fetcher = AsyncFileFetcher::new();
    let err = fetcher
        .fetch(&file.url())
        .await
        .expect_err("malformed YAML must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}
