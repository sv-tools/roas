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

#[test]
fn file_fetcher_returns_the_text_a_document_was_parsed_from() {
    let body = "{ \"title\": \"caf\u{e9}\" }\n";
    let file = TempFile::write("text.json", body.as_bytes());
    let (loaded, text) = FileFetcher::new()
        .fetch_document_text(&file.url())
        .expect("fetch ok");
    assert_eq!(loaded.document, serde_json::json!({ "title": "caf\u{e9}" }));
    assert_eq!(loaded.retrieval_uri, file.url());
    assert_eq!(text, body);

    let missing = temp_file("missing.json");
    let err = FileFetcher::new()
        .fetch_document_text(&Url::from_file_path(&missing).unwrap())
        .expect_err("a missing file is an error");
    assert!(matches!(err, LoaderError::ReadFile { .. }), "{err}");
    let err = FileFetcher::new()
        .fetch_document_text(&Url::parse("https://example.test/x.json").unwrap())
        .expect_err("https must be rejected");
    assert!(matches!(err, LoaderError::UnsupportedFetcherUri(_)));
    let file = TempFile::write("broken.json", b"{");
    let err = FileFetcher::new()
        .fetch_document_text(&file.url())
        .expect_err("a broken body is an error");
    assert!(matches!(err, LoaderError::Parse { .. }), "{err}");
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_file_fetcher_returns_the_text_a_document_was_parsed_from() {
    let body = "{ \"title\": \"caf\u{e9}\" }\n";
    let file = TempFile::write("async-text.json", body.as_bytes());
    let (loaded, text) = AsyncFileFetcher::new()
        .fetch_document_text(&file.url())
        .await
        .expect("fetch ok");
    assert_eq!(loaded.document, serde_json::json!({ "title": "caf\u{e9}" }));
    assert_eq!(loaded.retrieval_uri, file.url());
    assert_eq!(text, body);

    let missing = temp_file("async-missing.json");
    let err = AsyncFileFetcher::new()
        .fetch_document_text(&Url::from_file_path(&missing).unwrap())
        .await
        .expect_err("a missing file is an error");
    assert!(matches!(err, LoaderError::ReadFile { .. }), "{err}");
    let file = TempFile::write("async-broken.json", b"{");
    let err = AsyncFileFetcher::new()
        .fetch_document_text(&file.url())
        .await
        .expect_err("a broken body is an error");
    assert!(matches!(err, LoaderError::Parse { .. }), "{err}");
}

#[test]
fn file_fetcher_rejects_remote_host_uri_non_regular_file_oversize_and_root_escape() {
    let mut fetcher = FileFetcher::new();
    let unc = Url::parse("file://evil.example/share/spec.json").unwrap();
    assert!(matches!(
        fetcher.fetch(&unc),
        Err(LoaderError::InvalidFileUri(_))
    ));

    // Directory is not a regular file.
    let dir_url = Url::from_directory_path(std::env::temp_dir()).unwrap();
    assert!(matches!(
        fetcher.fetch(&dir_url),
        Err(LoaderError::ReadFile { .. })
    ));

    // Byte cap rejects oversize files.
    let big = TempFile::write("oversize.json", br#"{"key":"1234567890"}"#);
    let mut capped = FileFetcher::new().with_max_bytes(8);
    assert!(matches!(
        capped.fetch(&big.url()),
        Err(LoaderError::ReadFile { .. })
    ));

    // Root confinement allows files inside root and rejects files outside.
    let sub = temp_file("confined-root");
    fs::create_dir_all(&sub).unwrap();
    let inside_path = sub.join("ok.json");
    fs::write(&inside_path, br#"{"inside":true}"#).unwrap();
    let outside = TempFile::write("outside.json", br#"{"inside":false}"#);
    let mut confined = FileFetcher::new().with_root(&sub);
    assert_eq!(
        confined
            .fetch(&Url::from_file_path(&inside_path).unwrap())
            .unwrap(),
        serde_json::json!({ "inside": true })
    );
    assert!(matches!(
        confined.fetch(&outside.url()),
        Err(LoaderError::ReadFile { .. })
    ));
    let _ = fs::remove_file(&inside_path);
    let _ = fs::remove_dir(&sub);
}

#[cfg(unix)]
#[test]
fn file_fetcher_follow_symlinks_false_rejects_symlink() {
    let target = TempFile::write("symlink-target.json", br#"{"ok":true}"#);
    let link_path = temp_file("symlink-link.json");
    std::os::unix::fs::symlink(&target.0, &link_path).unwrap();
    let link_url = Url::from_file_path(&link_path).unwrap();

    // Default follows symlinks.
    assert_eq!(
        FileFetcher::new().fetch(&link_url).unwrap(),
        serde_json::json!({ "ok": true })
    );
    // `follow_symlinks(false)` rejects them.
    assert!(matches!(
        FileFetcher::new().follow_symlinks(false).fetch(&link_url),
        Err(LoaderError::ReadFile { .. })
    ));
    let _ = fs::remove_file(&link_path);
}

#[cfg(feature = "yaml")]
#[test]
fn file_fetcher_handles_percent_encoded_yaml_extension_and_fetch_document_text() {
    let file = TempFile::write("encoded.yaml", b"name: pet\n");
    let raw = file.url().as_str().replace(".yaml", "%2eyaml");
    let encoded_url = Url::parse(&raw).unwrap();
    let (loaded, text) = FileFetcher::new()
        .fetch_document_text(&encoded_url)
        .expect("percent-encoded .yaml must parse as YAML");
    assert_eq!(loaded.document, serde_json::json!({ "name": "pet" }));
    assert_eq!(text, "name: pet\n");
}

#[cfg(feature = "async")]
#[tokio::test]
async fn async_file_fetcher_enforces_root_and_max_bytes_and_symlink_policy() {
    let big = TempFile::write("async-oversize.json", br#"{"key":"1234567890"}"#);
    let mut capped = AsyncFileFetcher::new()
        .with_max_bytes(8)
        .with_root(std::env::temp_dir())
        .follow_symlinks(true);
    assert!(matches!(
        capped.fetch(&big.url()).await,
        Err(LoaderError::ReadFile { .. })
    ));
}
