//! `roas preview <FILE>` — start a local HTTP server that renders the spec in
//! a browser via [Redoc](https://redocly.com/redoc).
//!
//! Two routes only:
//!   * `/` (and `/index.html`) — a small HTML shell that mounts
//!     `<redoc spec-url='/spec'></redoc>` and pulls the Redoc bundle from the
//!     official CDN.
//!   * `/spec` (and `/spec.json`) — the input spec, parsed via the existing
//!     `read_and_parse` / `detect_or_use` pipeline and re-serialised as JSON.
//!
//! The default browser is launched at the server URL via the `webbrowser`
//! crate; `--no-open` suppresses the launch (the URL is still printed to
//! stderr). Ctrl+C tears the server down. The spec is served as-is — no
//! auto-downconversion; Redoc handles OAS 3.0 / 3.1 today and silently skips
//! 3.2-only fields.

use anyhow::{Context, Result, anyhow, bail};
use std::path::PathBuf;

use crate::read_and_parse;
use crate::versioned::{self, SpecVersion};

#[derive(clap::Args)]
pub struct PreviewArgs {
    /// Path to the spec file (JSON or YAML).
    pub(crate) file: PathBuf,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    pub(crate) from: Option<SpecVersion>,

    /// Don't open the browser; just print the server URL and serve.
    #[arg(long)]
    pub(crate) no_open: bool,
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

pub fn run_preview(args: PreviewArgs) -> Result<()> {
    let value = read_and_parse(&args.file)?;
    let detected = versioned::detect_or_use(args.from, value)?;
    // `convert_to(<same version>)` is a "serialise back as Value" pass —
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
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(suffix: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "roas-cli-preview-test-{}-{}-{suffix}",
            std::process::id(),
            n,
        ))
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

    fn parse_status_and_body(stream: &mut TcpStream) -> (u16, String, String) {
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read response");
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .map(|(h, b)| (h.to_string(), b.to_string()))
            .unwrap_or_else(|| (text.clone(), String::new()));
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

    fn send_request(addr: SocketAddr, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect(addr).expect("connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).expect("write request");
        stream
    }

    /// Stand up a real `tiny_http::Server`, drive `serve_preview_requests` on
    /// a background thread, hit all four routes, and verify each.
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

        let mut s = send_request(addr, "/");
        let (code, ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(ctype.contains("text/html"));
        assert!(body.contains("HELLO"));

        let mut s = send_request(addr, "/index.html");
        let (code, _ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(body.contains("HELLO"));

        let mut s = send_request(addr, "/spec");
        let (code, ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(ctype.contains("application/json"));
        assert_eq!(body, spec);

        let mut s = send_request(addr, "/spec.json");
        let (code, _ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert_eq!(body, spec);

        let mut s = send_request(addr, "/nope");
        let (code, _ctype, _body) = parse_status_and_body(&mut s);
        assert_eq!(code, 404);

        // Background-thread `tiny_http::Server` is dropped when the test
        // exits, which closes the listener and unblocks the loop. Detach.
        drop(thread);
    }

    #[test]
    fn run_preview_missing_file_errors_with_reading_context() {
        let args = PreviewArgs {
            file: temp_path("missing.json"),
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
