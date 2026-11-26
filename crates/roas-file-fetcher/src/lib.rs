//! Filesystem [`ResourceFetcher`] for the [`roas`] OpenAPI loader.
//!
//! [`FileFetcher`] is blocking and reads regular files up to a configurable
//! byte limit ([`DEFAULT_MAX_BYTES`], 64 MiB), rejecting non-`file://` URIs
//! with [`LoaderError::UnsupportedFetcherUri`], invalid or remote-host
//! `file://` URIs (e.g. UNC shares) with [`LoaderError::InvalidFileUri`],
//! I/O or policy failures (non-regular files, size limits, root escapes,
//! refused symlinks) with [`LoaderError::ReadFile`], and body parse failures
//! with [`LoaderError::Parse`].
//!
//! Optional features:
//!   * `async` — also expose `AsyncFileFetcher` for
//!     `roas::loader::Loader::register_async_fetcher`, running filesystem I/O
//!     and JSON/YAML parsing on Tokio's blocking thread pool via
//!     `tokio::task::spawn_blocking`. Requires an active tokio runtime when
//!     the returned future is awaited. Off by default; enabling it pulls in
//!     `tokio` with the `rt` feature.
//!   * `yaml` — parse YAML file bodies in addition to JSON. Selection is by
//!     decoded file path extension (`.yaml` / `.yml`). Pulls in
//!     `serde-saphyr`.

#[cfg(feature = "async")]
use roas::loader::{AsyncResourceFetcher, FetchFuture};
use roas::loader::{LoadedDocument, LoaderError, ResourceFetcher};
#[cfg(feature = "yaml")]
use serde::de::Error as _;
use serde_json::Value;
use std::fs::File;
use std::io::{Error as IoError, ErrorKind, Read};
use std::path::{Path, PathBuf};
use url::Url;

/// Default maximum file size read by [`FileFetcher`] and `AsyncFileFetcher`
/// (64 MiB), matching the YAML scalar budget.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct FileConfig {
    root: Option<PathBuf>,
    follow_symlinks: bool,
    max_bytes: usize,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            root: None,
            follow_symlinks: true,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// Blocking filesystem fetcher, suitable for
/// [`Loader::register_fetcher`](roas::loader::Loader::register_fetcher).
#[derive(Clone, Debug, Default)]
pub struct FileFetcher {
    config: FileConfig,
}

impl FileFetcher {
    /// Construct a blocking file fetcher with default limits
    /// ([`DEFAULT_MAX_BYTES`], 64 MiB).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Confine all reads to files residing under `root` (verified after
    /// canonicalization so `..` segments and escaping symlinks are rejected).
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.config.root = Some(root.into());
        self
    }

    /// Configure whether a symbolic link at the target path may be followed
    /// (default: `true`). When `false`, a symlink at the final path component
    /// is rejected with `PermissionDenied`. Combine with [`with_root`](Self::with_root)
    /// to confine canonical paths (including intermediate directory symlinks)
    /// beneath a trusted root directory.
    #[must_use]
    pub fn follow_symlinks(mut self, allow: bool) -> Self {
        self.config.follow_symlinks = allow;
        self
    }

    /// Override the maximum file size in bytes (default: [`DEFAULT_MAX_BYTES`],
    /// 64 MiB).
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.config.max_bytes = max_bytes;
        self
    }

    /// [`ResourceFetcher::fetch_document`], also returning the text the
    /// document was parsed from, for callers that report where in it
    /// something is.
    pub fn fetch_document_text(
        &mut self,
        uri: &Url,
    ) -> Result<(LoadedDocument, String), LoaderError> {
        let (path, bytes) = read(uri, &self.config)?;
        with_text(uri, &path, bytes)
    }
}

/// Async filesystem fetcher, suitable for
/// [`Loader::register_async_fetcher`](roas::loader::Loader::register_async_fetcher).
/// A tokio runtime must be active when the returned future is awaited.
///
/// Filesystem I/O and CPU-bound JSON/YAML parsing run on Tokio's blocking
/// pool (`tokio::task::spawn_blocking`) so async worker threads are not
/// stalled.
///
/// Available only with the `async` feature.
#[cfg(feature = "async")]
#[derive(Clone, Debug, Default)]
pub struct AsyncFileFetcher {
    config: FileConfig,
}

#[cfg(feature = "async")]
impl AsyncFileFetcher {
    /// Construct an async file fetcher with default limits
    /// ([`DEFAULT_MAX_BYTES`], 64 MiB).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Confine all reads to files residing under `root` (verified after
    /// canonicalization so `..` segments and escaping symlinks are rejected).
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.config.root = Some(root.into());
        self
    }

    /// Configure whether a symbolic link at the target path may be followed
    /// (default: `true`). When `false`, a symlink at the final path component
    /// is rejected with `PermissionDenied`. Combine with [`with_root`](Self::with_root)
    /// to confine canonical paths (including intermediate directory symlinks)
    /// beneath a trusted root directory.
    #[must_use]
    pub fn follow_symlinks(mut self, allow: bool) -> Self {
        self.config.follow_symlinks = allow;
        self
    }

    /// Override the maximum file size in bytes (default: [`DEFAULT_MAX_BYTES`],
    /// 64 MiB).
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.config.max_bytes = max_bytes;
        self
    }

    /// [`AsyncResourceFetcher::fetch_document`], also returning the text
    /// the document was parsed from, for callers that report where in it
    /// something is.
    pub async fn fetch_document_text(
        &mut self,
        uri: &Url,
    ) -> Result<(LoadedDocument, String), LoaderError> {
        let uri = uri.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            let (path, bytes) = read(&uri, &config)?;
            with_text(&uri, &path, bytes)
        })
        .await
        .map_err(|err| LoaderError::ReadFile {
            path: PathBuf::new(),
            source: IoError::other(err),
        })?
    }
}

impl ResourceFetcher for FileFetcher {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        let (path, bytes) = read(uri, &self.config)?;
        parse_body(uri, &path, &bytes)
    }
}

#[cfg(feature = "async")]
impl AsyncResourceFetcher for AsyncFileFetcher {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a> {
        let uri = uri.clone();
        let config = self.config.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let (path, bytes) = read(&uri, &config)?;
                parse_body(&uri, &path, &bytes)
            })
            .await
            .map_err(|err| LoaderError::ReadFile {
                path: PathBuf::new(),
                source: IoError::other(err),
            })?
        })
    }
}

fn read(uri: &Url, config: &FileConfig) -> Result<(PathBuf, Vec<u8>), LoaderError> {
    check_scheme(uri)?;
    let path = uri_to_path(uri)?;
    let bytes = read_bounded(&path, config)?;
    Ok((path, bytes))
}

fn size_limit_error(path: &Path, max_bytes: usize) -> LoaderError {
    LoaderError::ReadFile {
        path: path.to_path_buf(),
        source: IoError::new(
            ErrorKind::InvalidData,
            format!("file exceeds maximum size limit of {max_bytes} bytes"),
        ),
    }
}

fn read_limited(
    reader: impl Read,
    path: &Path,
    max_bytes: usize,
    hint_len: usize,
) -> Result<Vec<u8>, LoaderError> {
    let mut bytes = Vec::with_capacity(hint_len.min(max_bytes));
    let cap = (max_bytes as u64).saturating_add(1);
    reader
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|source| LoaderError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > max_bytes {
        return Err(size_limit_error(path, max_bytes));
    }
    Ok(bytes)
}

fn read_bounded(path: &Path, config: &FileConfig) -> Result<Vec<u8>, LoaderError> {
    let io_err = |source| LoaderError::ReadFile {
        path: path.to_path_buf(),
        source,
    };
    let meta = if !config.follow_symlinks {
        let sym = std::fs::symlink_metadata(path).map_err(io_err)?;
        if sym.file_type().is_symlink() {
            return Err(io_err(IoError::new(
                ErrorKind::PermissionDenied,
                "refusing to follow symbolic link",
            )));
        }
        sym
    } else {
        std::fs::metadata(path).map_err(io_err)?
    };
    if let Some(root) = &config.root {
        let canon_root = std::fs::canonicalize(root).map_err(io_err)?;
        let canon_path = std::fs::canonicalize(path).map_err(io_err)?;
        if !canon_path.starts_with(&canon_root) {
            return Err(io_err(IoError::new(
                ErrorKind::PermissionDenied,
                "file path escapes configured root directory",
            )));
        }
    }
    // Check file type before opening so FIFOs, sockets, devices, or
    // directories cannot block or fail unexpectedly.
    if !meta.file_type().is_file() {
        return Err(io_err(IoError::new(
            ErrorKind::InvalidInput,
            "refusing to read non-regular file",
        )));
    }
    if meta.len() > config.max_bytes as u64 {
        return Err(size_limit_error(path, config.max_bytes));
    }
    let file = File::open(path).map_err(io_err)?;
    let opened = file.metadata().map_err(io_err)?;
    if !opened.file_type().is_file() {
        return Err(io_err(IoError::new(
            ErrorKind::InvalidInput,
            "refusing to read non-regular file",
        )));
    }
    if opened.len() > config.max_bytes as u64 {
        return Err(size_limit_error(path, config.max_bytes));
    }
    read_limited(file, path, config.max_bytes, opened.len() as usize)
}

/// The document `bytes` hold, and the bytes as text.
fn with_text(
    uri: &Url,
    path: &Path,
    bytes: Vec<u8>,
) -> Result<(LoadedDocument, String), LoaderError> {
    let document = parse_body(uri, path, &bytes)?;
    // Neither parser accepts anything but UTF-8, so this never fails.
    let text = String::from_utf8(bytes).map_err(|error| LoaderError::Parse {
        uri: uri.as_str().to_string(),
        source: <serde_json::Error as serde::de::Error>::custom(error),
    })?;
    Ok((LoadedDocument::new(document, uri.clone()), text))
}

fn check_scheme(uri: &Url) -> Result<(), LoaderError> {
    if uri.scheme() == "file" {
        Ok(())
    } else {
        Err(LoaderError::UnsupportedFetcherUri(uri.as_str().to_string()))
    }
}

fn uri_to_path(uri: &Url) -> Result<PathBuf, LoaderError> {
    // Reject non-local hosts (e.g. `file://attacker/share/spec.json`, which
    // triggers outbound SMB on Windows).
    if uri
        .host_str()
        .is_some_and(|host| !host.is_empty() && !host.eq_ignore_ascii_case("localhost"))
    {
        return Err(LoaderError::InvalidFileUri(uri.as_str().to_string()));
    }
    uri.to_file_path()
        .map_err(|()| LoaderError::InvalidFileUri(uri.as_str().to_string()))
}

fn parse_body(uri: &Url, path: &Path, bytes: &[u8]) -> Result<Value, LoaderError> {
    if is_yaml(path) {
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
/// decoded filesystem path extension is checked against `yaml` / `yml`
/// (case-insensitive), so percent-encoded extensions such as `spec%2eyaml`
/// are recognized.
#[allow(unused_variables)]
fn is_yaml(path: &Path) -> bool {
    #[cfg(feature = "yaml")]
    {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
    }
    #[cfg(not(feature = "yaml"))]
    {
        false
    }
}

#[cfg(feature = "yaml")]
mod yaml_value;

#[cfg(feature = "yaml")]
fn parse_yaml(uri: &Url, bytes: &[u8]) -> Result<Value, LoaderError> {
    serde_saphyr::from_slice_with_options::<yaml_value::JsonValue>(bytes, yaml_options())
        .map(yaml_value::JsonValue::into_inner)
        .map_err(|yaml_err| LoaderError::Parse {
            uri: uri.as_str().to_string(),
            source: serde_json::Error::custom(yaml_err.to_string()),
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
            ("!!binary SGVsbG8=", json!("SGVsbG8=")),
            ("!!binary /wAB", json!("/wAB")),
            ("1e400", json!(null)),
            ("!include other.yaml", json!("other.yaml")),
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

    /// `200` and `"200"` are different YAML keys but the same JSON one.
    /// Through a `<<` merge the inherited entry would otherwise overwrite the
    /// explicit one, in either direction.
    #[test]
    fn yaml_keys_colliding_as_json_are_an_error() {
        for yaml in [
            "base: &b { 200: { description: inherited } }\n\
             responses:\n  <<: *b\n  \"200\": { description: explicit }\n",
            "base: &b { \"200\": { description: inherited } }\n\
             responses:\n  <<: *b\n  200: { description: explicit }\n",
            "responses:\n  200: a\n  \"200\": b\n",
        ] {
            let err = parse(yaml).unwrap_err();
            assert!(err.contains("appears twice once read as JSON"), "{err}");
        }
    }

    #[test]
    fn yaml_merge_keeps_the_explicit_entry_when_keys_match() {
        let yaml = "base: &b { 200: { description: inherited }, 404: { description: nf } }\n\
                    responses:\n  <<: *b\n  200: { description: explicit }\n";
        assert_eq!(
            parse(yaml).unwrap()["responses"],
            json!({ "200": { "description": "explicit" }, "404": { "description": "nf" } }),
        );
    }

    /// Keys are read as strings, so a number-looking key keeps its spelling
    /// and two spellings of one number are two keys.
    #[test]
    fn yaml_keys_keep_their_spelling() {
        assert_eq!(
            parse("r: { 1.10: a, 1.1: b, 0x10: c, 1e3: d, True: e, 017: f }\n").unwrap(),
            json!({ "r": { "1.10": "a", "1.1": "b", "0x10": "c", "1e3": "d", "True": "e", "017": "f" } }),
        );
    }

    #[test]
    fn yaml_null_and_collection_keys_are_an_error() {
        for yaml in [
            "r: { null: a }\n",
            "r: { ~: a }\n",
            "r:\n  ? [1, 2]\n  : a\n",
        ] {
            parse(yaml).unwrap_err();
        }
    }

    /// The number map YAML output carries under `arbitrary_precision` reads
    /// back the way `serde_json` reads it, with the feature on or off.
    #[test]
    fn yaml_arbitrary_precision_number_map_matches_serde_json() {
        let json = r#"{"n":{"$serde_json::private::Number":"12.50"}}"#;
        let expected: Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            parse("n: { \"$serde_json::private::Number\": \"12.50\" }\n").unwrap(),
            expected
        );
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

    #[test]
    fn read_limited_allows_exact_limit_and_rejects_streaming_overflow() {
        let path = Path::new("/tmp/spec.json");
        let exact = read_limited(&b"1234"[..], path, 4, 0).expect("exact limit must succeed");
        assert_eq!(exact, b"1234");
        let err = read_limited(&b"12345"[..], path, 4, 0).expect_err("overflow must fail");
        match err {
            LoaderError::ReadFile { source, .. } => {
                assert_eq!(source.kind(), ErrorKind::InvalidData);
            }
            other => panic!("expected ReadFile, got {other:?}"),
        }
    }
}
