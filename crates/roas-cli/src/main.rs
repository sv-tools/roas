//! `roas` command-line front-end.
//!
//! Three subcommands today:
//!
//! - `roas validate <FILE>` — parse and validate an OpenAPI spec. Version is
//!   auto-detected from the document; pass `--from` to force. External
//!   `$ref`s are skipped by default; use `--load file` / `--load http`
//!   (or both) to enable the loader.
//!
//! - `roas convert --to <VERSION> <FILE>` — chain the existing
//!   `From<v_X::Spec> for v_Y::Spec` migrations to upconvert a spec.
//!   Pass `--from` to force the input version.
//!
//! - `roas preview <FILE>` — start a local HTTP server on
//!   `127.0.0.1:<random>` that serves the spec rendered with
//!   [Redoc](https://redocly.com/redoc), and open the default browser at it.
//!   `--no-open` skips the launch. Ctrl+C tears the server down.
//!
//! Input may be JSON or YAML; the parser is selected by file extension
//! (`.yaml` / `.yml` → YAML, otherwise JSON).

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use roas::loader::Loader;
use roas::validation::Options;
use roas_file_fetcher::FileFetcher;
use roas_http_fetcher::HttpFetcher;
use std::fs;
use std::path::PathBuf;

// `roas::validation::Options` implements `clap::ValueEnum` under the `clap`
// feature (enabled on the `roas` dep in this crate's Cargo.toml), so we can
// hand it straight to `#[arg(value_enum)]` without a CLI-local mirror enum.
// Variants render as kebab-case with the `Ignore` prefix dropped: e.g.
// `Options::IgnoreMissingTags` ↔ `--ignore missing-tags`.

mod versioned;

use versioned::{SpecVersion, parse_value, path_looks_like_yaml};

#[derive(Parser)]
#[command(name = "roas", about, version, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse and validate an OpenAPI spec.
    Validate(ValidateArgs),
    /// Convert an OpenAPI spec to a different version.
    Convert(ConvertArgs),
    /// Preview the spec in a browser, rendered with Redoc.
    Preview(PreviewArgs),
}

#[derive(clap::Args)]
struct ValidateArgs {
    /// Path to the spec file (JSON or YAML).
    file: PathBuf,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,

    /// Enable external-reference loading. Pass `--load file` to allow
    /// `file://` refs, `--load http` to allow `http://` and `https://`.
    /// Repeat the flag to combine (e.g. `--load file --load http`).
    #[arg(long, value_enum)]
    load: Vec<LoaderKind>,

    /// Skip a specific validation check. Repeat the flag to skip several
    /// (e.g. `--ignore missing-tags --ignore external-references`). Run
    /// `roas validate --help` to see the full list.
    #[arg(long, value_enum)]
    ignore: Vec<Options>,
}

#[derive(clap::Args)]
struct ConvertArgs {
    /// Path to the spec file (JSON or YAML).
    file: PathBuf,

    /// Target spec version.
    #[arg(long, value_enum)]
    to: SpecVersion,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,
}

#[derive(clap::Args)]
struct PreviewArgs {
    /// Path to the spec file (JSON or YAML).
    file: PathBuf,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,

    /// Don't open the browser; just print the server URL and serve.
    #[arg(long)]
    no_open: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LoaderKind {
    File,
    Http,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate(args) => run_validate(args),
        Command::Convert(args) => run_convert(args),
        Command::Preview(args) => run_preview(args),
    }
}

fn read_and_parse(path: &std::path::Path) -> Result<serde_json::Value> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_value(&raw, path_looks_like_yaml(path))
}

fn build_loader(kinds: &[LoaderKind]) -> Option<Loader> {
    if kinds.is_empty() {
        return None;
    }
    let mut loader = Loader::new();
    for kind in kinds {
        match kind {
            LoaderKind::File => {
                loader.register_fetcher("file://", FileFetcher::new());
            }
            LoaderKind::Http => {
                // Build one `HttpFetcher` and clone it across both prefixes so
                // a single connection pool serves `http://` and `https://`.
                let fetcher = HttpFetcher::new();
                loader.register_fetcher("http://", fetcher.clone());
                loader.register_fetcher("https://", fetcher);
            }
        }
    }
    Some(loader)
}

fn run_validate(args: ValidateArgs) -> Result<()> {
    let value = read_and_parse(&args.file)?;
    let detected = versioned::detect_or_use(args.from, value)?;

    let mut loader = build_loader(&args.load);

    let mut options = enumset::EnumSet::<Options>::new();
    for ignore in &args.ignore {
        options |= *ignore;
    }
    match detected.validate(options, loader.as_mut()) {
        Ok(()) => {
            // Diagnostics go to stderr so stdout stays clean for shell pipelines.
            eprintln!("{}: valid {}", args.file.display(), detected.label());
            Ok(())
        }
        Err(err) => {
            for e in &err.errors {
                eprintln!("- {e}");
            }
            Err(anyhow!(
                "{}: validation failed ({} error{})",
                args.file.display(),
                err.errors.len(),
                if err.errors.len() == 1 { "" } else { "s" }
            ))
        }
    }
}

fn run_convert(args: ConvertArgs) -> Result<()> {
    let value = read_and_parse(&args.file)?;
    let detected = versioned::detect_or_use(args.from, value)?;

    let target = args.to;
    if (detected.version() as u8) > (target as u8) {
        bail!(
            "downconversion is not supported: input is {}, target is {}",
            detected.label(),
            target.label(),
        );
    }

    let converted = detected.convert_to(target)?;
    let json = serde_json::to_string_pretty(&converted).context("serializing converted spec")?;
    println!("{json}");
    Ok(())
}

/// Embedded Redoc shell. The page mounts a `<redoc spec-url='/spec'>` element
/// and pulls the Redoc bundle from the official CDN; the spec is fetched
/// asynchronously from our own server at `/spec`. No build-time templating
/// because there's nothing to interpolate — the URL is fixed.
const REDOC_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <title>roas — OpenAPI viewer</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <style>body { margin: 0; padding: 0; }</style>
  </head>
  <body>
    <redoc spec-url='/spec'></redoc>
    <script src="https://cdn.redoc.ly/redoc/latest/bundles/redoc.standalone.js"></script>
  </body>
</html>
"#;

fn run_preview(args: PreviewArgs) -> Result<()> {
    let value = read_and_parse(&args.file)?;
    let detected = versioned::detect_or_use(args.from, value)?;
    // Reuse `convert_to(<same version>)` as a "serialise back as Value" pass —
    // pluck the version first so we don't move `detected` while borrowing it.
    let version = detected.version();
    let spec_json = serde_json::to_string(&detected.convert_to(version)?)
        .context("serializing spec for the viewer")?;

    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow!("starting Redoc viewer server: {e}"))?;
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr.port(),
        other => bail!("unexpected listen address: {other:?}"),
    };
    let url = format!("http://127.0.0.1:{port}/");

    eprintln!("Serving {} via Redoc at {url}", args.file.display());
    eprintln!("Press Ctrl+C to stop.");

    if !args.no_open {
        // Browser open is best-effort; if it fails the user can still hit
        // the URL manually from stderr.
        let _ = webbrowser::open(&url);
    }

    serve_preview_requests(server, REDOC_HTML, &spec_json);
    Ok(())
}

fn serve_preview_requests(server: tiny_http::Server, html: &str, spec_json: &str) {
    for request in server.incoming_requests() {
        let response = match request.url() {
            "/" | "/index.html" => http_response(html, "text/html; charset=utf-8"),
            "/spec" | "/spec.json" => http_response(spec_json, "application/json"),
            _ => not_found_response(),
        };
        // If the client disconnected before we finished writing, there's
        // nothing useful to log at CLI level — drop the error.
        let _ = request.respond(response);
    }
}

fn http_response(body: &str, content_type: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut response = tiny_http::Response::from_string(body.to_string());
    if let Ok(header) =
        tiny_http::Header::from_bytes(b"Content-Type".as_ref(), content_type.as_bytes())
    {
        response = response.with_header(header);
    }
    response
}

fn not_found_response() -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string("not found").with_status_code(404)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Write;

    #[test]
    fn build_loader_returns_none_for_empty_kinds() {
        assert!(build_loader(&[]).is_none());
    }

    #[test]
    fn build_loader_returns_some_for_file_kind() {
        assert!(build_loader(&[LoaderKind::File]).is_some());
    }

    #[test]
    fn build_loader_returns_some_for_http_kind() {
        assert!(build_loader(&[LoaderKind::Http]).is_some());
    }

    #[test]
    fn build_loader_returns_some_for_combined_kinds() {
        assert!(build_loader(&[LoaderKind::File, LoaderKind::Http]).is_some());
    }

    /// `run_convert`'s downconvert guard uses `(SpecVersion as u8)` ordering;
    /// the variant declaration order must match the chronological version
    /// order or `roas convert --to v2 spec_3_2.json` would silently succeed.
    #[test]
    fn spec_version_discriminants_order_by_version() {
        assert!((SpecVersion::V2 as u8) < (SpecVersion::V3_0 as u8));
        assert!((SpecVersion::V3_0 as u8) < (SpecVersion::V3_1 as u8));
        assert!((SpecVersion::V3_1 as u8) < (SpecVersion::V3_2 as u8));
    }

    #[test]
    fn cli_parses_minimal_validate_invocation() {
        let cli = Cli::try_parse_from(["roas", "validate", "spec.json"]).expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.file.to_string_lossy(), "spec.json");
                assert!(args.from.is_none());
                assert!(args.load.is_empty());
                assert!(args.ignore.is_empty());
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_ignore_flag_into_options_variants() {
        let cli = Cli::try_parse_from([
            "roas",
            "validate",
            "--ignore",
            "missing-tags",
            "--ignore",
            "unused-server-variables",
            "--ignore",
            "empty-info-title",
            "spec.json",
        ])
        .expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(
                    args.ignore,
                    vec![
                        Options::IgnoreMissingTags,
                        Options::IgnoreUnusedServerVariables,
                        Options::IgnoreEmptyInfoTitle,
                    ]
                );
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_rejects_unknown_ignore_value() {
        let res =
            Cli::try_parse_from(["roas", "validate", "--ignore", "no-such-check", "spec.json"]);
        assert!(res.is_err(), "unknown --ignore value must error");
    }

    #[test]
    fn cli_parses_repeated_load_flag_into_vec() {
        let cli = Cli::try_parse_from([
            "roas",
            "validate",
            "--load",
            "file",
            "--load",
            "http",
            "spec.json",
        ])
        .expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.load.len(), 2);
                assert!(matches!(args.load[0], LoaderKind::File));
                assert!(matches!(args.load[1], LoaderKind::Http));
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_convert_with_explicit_from() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--from",
            "v2",
            "--to",
            "v3_2",
            "spec.json",
        ])
        .expect("convert parse");
        match cli.command {
            Command::Convert(args) => {
                assert_eq!(args.from, Some(SpecVersion::V2));
                assert_eq!(args.to, SpecVersion::V3_2);
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_rejects_convert_without_to() {
        let res = Cli::try_parse_from(["roas", "convert", "spec.json"]);
        assert!(res.is_err(), "convert without --to must error");
    }

    /// Process-scoped unique temp path so parallel tests don't collide.
    fn temp_path(suffix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "roas-cli-test-{}-{}-{suffix}",
            std::process::id(),
            n,
        ))
    }

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn write(suffix: &str, body: &[u8]) -> Self {
            let path = temp_path(suffix);
            let mut f = std::fs::File::create(&path).expect("create temp file");
            f.write_all(body).expect("write temp file");
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn read_and_parse_json_file_returns_parsed_value() {
        let f = TempFile::write("ok.json", br#"{"hello":"world"}"#);
        let v = read_and_parse(&f.0).expect("parse ok");
        assert_eq!(v, serde_json::json!({"hello": "world"}));
    }

    #[test]
    fn read_and_parse_yaml_file_routes_through_yaml_parser() {
        let f = TempFile::write("ok.yaml", b"name: pet\ncount: 3\n");
        let v = read_and_parse(&f.0).expect("parse ok");
        assert_eq!(v, serde_json::json!({"name": "pet", "count": 3}));
    }

    #[test]
    fn read_and_parse_missing_file_errors_with_reading_context() {
        let p = temp_path("missing.json");
        let err = read_and_parse(&p).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[test]
    fn read_and_parse_invalid_json_surfaces_parser_error() {
        let f = TempFile::write("bad.json", b"@@@ not json");
        let err = read_and_parse(&f.0).expect_err("invalid JSON must error");
        assert!(
            err.to_string().contains("parsing JSON"),
            "expected `parsing JSON` context, got: {err}",
        );
    }

    #[test]
    fn read_and_parse_invalid_yaml_surfaces_parser_error() {
        let f = TempFile::write("bad.yaml", b"key:\n\tvalue: oops\n");
        let err = read_and_parse(&f.0).expect_err("invalid YAML must error");
        assert!(
            err.to_string().contains("parsing YAML"),
            "expected `parsing YAML` context, got: {err}",
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // run_validate / run_convert end-to-end-ish tests (drive each function
    // with a constructed args struct + a temp-file spec; assert return
    // shape, not stdout/stderr text).
    // ────────────────────────────────────────────────────────────────────

    /// A minimal valid v3.2 spec body — used by several validate / convert
    /// tests that just need *some* spec on disk.
    const MINIMAL_V3_2: &[u8] =
        br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#;
    const MINIMAL_V2: &[u8] = br#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#;

    #[test]
    fn run_validate_returns_ok_for_clean_spec() {
        let f = TempFile::write("clean.json", MINIMAL_V3_2);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: Vec::new(),
            ignore: Vec::new(),
        };
        run_validate(args).expect("clean spec must validate");
    }

    #[test]
    fn run_validate_returns_err_for_spec_with_unused_tag() {
        // Default ignore set fires on unused tags.
        let body = br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"unused"}]}"#;
        let f = TempFile::write("unused-tag.json", body);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: Vec::new(),
            ignore: Vec::new(),
        };
        let err = run_validate(args).expect_err("unused tag must fail");
        assert!(err.to_string().contains("validation failed"), "got: {err}",);
    }

    #[test]
    fn run_validate_with_ignore_suppresses_validation_failure() {
        let body = br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"unused"}]}"#;
        let f = TempFile::write("ignored.json", body);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: Vec::new(),
            ignore: vec![Options::IgnoreUnusedTags],
        };
        run_validate(args).expect("--ignore unused-tags must suppress");
    }

    #[test]
    fn run_validate_with_load_file_builds_loader() {
        let f = TempFile::write("with-load.json", MINIMAL_V3_2);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: vec![LoaderKind::File],
            ignore: Vec::new(),
        };
        run_validate(args).expect("clean spec with file loader must validate");
    }

    #[test]
    fn run_validate_missing_file_errors_with_reading_context() {
        let args = ValidateArgs {
            file: temp_path("missing.json"),
            from: None,
            load: Vec::new(),
            ignore: Vec::new(),
        };
        let err = run_validate(args).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[test]
    fn run_convert_v2_to_v3_2_succeeds() {
        let f = TempFile::write("v2.json", MINIMAL_V2);
        let args = ConvertArgs {
            file: f.0.clone(),
            to: SpecVersion::V3_2,
            from: None,
        };
        run_convert(args).expect("v2 → v3.2 must succeed");
    }

    #[test]
    fn run_convert_rejects_downconversion() {
        let f = TempFile::write("v3.json", MINIMAL_V3_2);
        let args = ConvertArgs {
            file: f.0.clone(),
            to: SpecVersion::V2,
            from: None,
        };
        let err = run_convert(args).expect_err("downconversion must error");
        assert!(
            err.to_string().contains("downconversion is not supported"),
            "got: {err}",
        );
    }

    #[test]
    fn run_convert_missing_file_errors_with_reading_context() {
        let args = ConvertArgs {
            file: temp_path("missing.json"),
            to: SpecVersion::V3_2,
            from: None,
        };
        let err = run_convert(args).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // `roas ui` tests. The Cli wiring is unit-tested directly; the server's
    // request handling is exercised end-to-end by binding our own listener
    // on an ephemeral port and hitting it with a stdlib TcpStream.
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn cli_parses_preview_subcommand() {
        let cli =
            Cli::try_parse_from(["roas", "preview", "spec.json", "--no-open"]).expect("ui parse");
        match cli.command {
            Command::Preview(args) => {
                assert_eq!(args.file.to_string_lossy(), "spec.json");
                assert!(args.no_open);
                assert!(args.from.is_none());
            }
            _ => panic!("expected Preview"),
        }
    }

    #[test]
    fn redoc_html_constant_references_spec_url_and_redoc_cdn() {
        assert!(
            REDOC_HTML.contains("spec-url='/spec'"),
            "REDOC_HTML must point at /spec",
        );
        assert!(
            REDOC_HTML.contains("cdn.redoc.ly"),
            "REDOC_HTML must load Redoc from its CDN",
        );
    }

    fn parse_status_and_body(stream: &mut std::net::TcpStream) -> (u16, String, String) {
        use std::io::Read;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read response");
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .map(|(h, b)| (h.to_string(), b.to_string()))
            .unwrap_or_else(|| (text.clone(), String::new()));
        // Parse "HTTP/1.1 <code> ..."
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let content_type = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
            .map(|l| {
                l.split_once(':')
                    .map(|x| x.1)
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        (status, content_type, body)
    }

    fn send_request(addr: std::net::SocketAddr, path: &str) -> std::net::TcpStream {
        use std::io::Write;
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).expect("write request");
        stream
    }

    /// Stand up a real `tiny_http::Server`, drive `serve_preview_requests` on a
    /// background thread, hit all four routes, and verify each. Tears the
    /// server down by dropping our reference to it via the join handle.
    #[test]
    fn serve_preview_requests_routes_html_spec_and_404() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
        let addr = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr,
            other => panic!("unexpected listen addr: {other:?}"),
        };

        let html = "<!doctype html><body>HELLO</body>";
        let spec = r#"{"openapi":"3.2.0"}"#;
        let html_owned = html.to_string();
        let spec_owned = spec.to_string();

        let thread = std::thread::spawn(move || {
            serve_preview_requests(server, &html_owned, &spec_owned);
        });

        // `/` → html
        let mut s = send_request(addr, "/");
        let (code, ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(ctype.contains("text/html"));
        assert!(body.contains("HELLO"));

        // `/index.html` → html (alias)
        let mut s = send_request(addr, "/index.html");
        let (code, _ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(body.contains("HELLO"));

        // `/spec` → json
        let mut s = send_request(addr, "/spec");
        let (code, ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(ctype.contains("application/json"));
        assert_eq!(body, spec);

        // `/spec.json` → json (alias)
        let mut s = send_request(addr, "/spec.json");
        let (code, _ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert_eq!(body, spec);

        // unknown path → 404
        let mut s = send_request(addr, "/nope");
        let (code, _ctype, _body) = parse_status_and_body(&mut s);
        assert_eq!(code, 404);

        // The server thread blocks in `incoming_requests()`; the
        // background-thread `tiny_http::Server` it owns is dropped when the
        // main thread exits the test scope, which closes the listener and
        // unblocks the loop. Detach the join handle — we don't strictly need
        // to wait for it.
        drop(thread);
    }

    #[test]
    fn run_preview_missing_file_errors_with_reading_context() {
        let args = PreviewArgs {
            file: temp_path("missing-ui.json"),
            from: None,
            no_open: true,
        };
        let err = run_preview(args).expect_err("missing file must error before server starts");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }
}
