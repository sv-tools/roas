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

/// Renderer choice for the preview page. Both load their bundle from a public
/// CDN; the spec itself is always served from the local server.
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub(crate) enum Renderer {
    /// [Redoc](https://redocly.com/redoc) — single-page reference renderer.
    Redoc,
    /// [Swagger UI](https://swagger.io/tools/swagger-ui/) — the canonical
    /// interactive UI from the Swagger project.
    #[value(name = "swagger-ui")]
    SwaggerUi,
}

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

    /// Renderer for the preview page. Defaults to Redoc.
    #[arg(long, value_enum, default_value = "redoc")]
    pub(crate) renderer: Renderer,
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

/// Embedded Swagger UI shell. Loads the `swagger-ui-dist` bundle + CSS from
/// the public `unpkg.com` CDN and initialises against our `/spec` route. The
/// `window.onload` hook is the pattern Swagger UI's own examples use to avoid
/// racing the script load.
const SWAGGER_UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <title>roas — OpenAPI viewer</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@latest/swagger-ui.css" />
    <style>body { margin: 0; padding: 0; }</style>
  </head>
  <body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@latest/swagger-ui-bundle.js"></script>
    <script>
      window.onload = () => {
        window.ui = SwaggerUIBundle({ url: '/spec', dom_id: '#swagger-ui' });
      };
    </script>
  </body>
</html>
"#;

/// Resolve the chosen renderer to its embedded HTML shell.
fn renderer_html(renderer: Renderer) -> &'static str {
    match renderer {
        Renderer::Redoc => REDOC_HTML,
        Renderer::SwaggerUi => SWAGGER_UI_HTML,
    }
}

/// Everything `run_preview` collects before it commits to the blocking
/// `incoming_requests()` loop. Pulled out so unit tests can exercise the
/// happy path of `run_preview` without owning a server that blocks forever.
struct PreparedPreview {
    server: tiny_http::Server,
    url: String,
    renderer_label: &'static str,
    html: &'static str,
    spec_json: String,
}

fn prepare_preview(args: &PreviewArgs) -> Result<PreparedPreview> {
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
    let renderer_label = match args.renderer {
        Renderer::Redoc => "Redoc",
        Renderer::SwaggerUi => "Swagger UI",
    };
    let html = renderer_html(args.renderer);

    Ok(PreparedPreview {
        server,
        url,
        renderer_label,
        html,
        spec_json,
    })
}

pub fn run_preview(args: PreviewArgs) -> Result<()> {
    let prepared = prepare_preview(&args)?;
    drive_prepared_preview(&args, prepared);
    Ok(())
}

/// Tail of `run_preview` — prints the "Serving …" banner, optionally pokes
/// `webbrowser::open`, and runs the blocking request loop. Split out so a
/// test thread can drive it against a `PreparedPreview` it constructed itself
/// and observe behaviour without having to discover the auto-bound port.
fn drive_prepared_preview(args: &PreviewArgs, prepared: PreparedPreview) {
    eprintln!(
        "Serving {} via {} at {}",
        args.file.display(),
        prepared.renderer_label,
        prepared.url,
    );
    eprintln!("Press Ctrl+C to stop.");

    if !args.no_open {
        // Browser open is best-effort; if it fails the user can still hit
        // the URL manually from stderr.
        let _ = webbrowser::open(&prepared.url);
    }

    serve_preview_requests(prepared.server, prepared.html, &prepared.spec_json);
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

    #[test]
    fn swagger_ui_html_constant_references_spec_url_and_swagger_ui_bundle() {
        assert!(
            SWAGGER_UI_HTML.contains("url: '/spec'"),
            "SWAGGER_UI_HTML must point SwaggerUIBundle at /spec",
        );
        assert!(
            SWAGGER_UI_HTML.contains("swagger-ui-dist"),
            "SWAGGER_UI_HTML must load swagger-ui-dist from the CDN",
        );
        assert!(
            SWAGGER_UI_HTML.contains("swagger-ui.css"),
            "SWAGGER_UI_HTML must include the swagger-ui stylesheet",
        );
    }

    #[test]
    fn renderer_html_selects_the_right_shell_per_variant() {
        assert!(renderer_html(Renderer::Redoc).contains("cdn.redoc.ly"));
        assert!(renderer_html(Renderer::SwaggerUi).contains("swagger-ui-dist"));
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
            renderer: Renderer::Redoc,
        };
        let err = run_preview(args).expect_err("missing file must error before server starts");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    /// `prepare_preview` is the happy path of `run_preview` minus the blocking
    /// `serve_preview_requests` call — driving it directly is the only way to
    /// observe the bound server, the assembled URL, and the renderer-specific
    /// fields without spawning a background thread that we can't reliably
    /// shut down.
    fn write_minimal_v3_2_spec() -> PathBuf {
        let path = temp_path("ok.json");
        std::fs::write(
            &path,
            br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .expect("write temp spec");
        path
    }

    #[test]
    fn prepare_preview_with_redoc_returns_bound_server_and_redoc_assets() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");

        // Server is bound on loopback with a non-zero port we can format into a URL.
        let port = match prepared.server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => {
                assert!(addr.ip().is_loopback(), "must bind to loopback");
                assert!(addr.port() > 0, "must allocate an ephemeral port");
                addr.port()
            }
            other => panic!("unexpected listen addr: {other:?}"),
        };
        assert_eq!(prepared.url, format!("http://127.0.0.1:{port}/"));

        // Renderer-specific fields wire through correctly.
        assert_eq!(prepared.renderer_label, "Redoc");
        assert!(prepared.html.contains("cdn.redoc.ly"));

        // Spec was parsed + re-serialised back to JSON — round-trip preserves
        // the `openapi` field we put in.
        let parsed: serde_json::Value = serde_json::from_str(&prepared.spec_json).unwrap();
        assert_eq!(parsed["openapi"], "3.2.0");

        // Drop the prepared server explicitly to release the port before the
        // tempfile teardown runs.
        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prepare_preview_with_swagger_ui_switches_renderer_fields() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::SwaggerUi,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");

        assert_eq!(prepared.renderer_label, "Swagger UI");
        assert!(prepared.html.contains("swagger-ui-dist"));

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    /// Drive `drive_prepared_preview` on a background thread and hit `/spec`
    /// through the URL it printed. Covers the eprintln + serve loop body that
    /// `run_preview`'s in-process tests can't reach directly.
    #[test]
    fn drive_prepared_preview_serves_spec_through_the_published_url() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        let addr = match prepared.server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr,
            other => panic!("unexpected listen addr: {other:?}"),
        };

        // Move `prepared` into the thread; rebuild args so the thread can own
        // its own copy without us forcing `PreviewArgs: Clone`.
        let path_for_thread = path.clone();
        let thread = std::thread::spawn(move || {
            let thread_args = PreviewArgs {
                file: path_for_thread,
                from: None,
                no_open: true,
                renderer: Renderer::Redoc,
            };
            drive_prepared_preview(&thread_args, prepared);
        });

        let mut s = send_request(addr, "/spec");
        let (code, ctype, body) = parse_status_and_body(&mut s);
        assert_eq!(code, 200);
        assert!(ctype.contains("application/json"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["openapi"], "3.2.0");

        // Background thread keeps blocking on `incoming_requests()` until
        // process exit; the server's listener is dropped when this test
        // function returns and `prepared` is dropped on the thread's stack.
        // Detach intentionally — we don't need to wait.
        drop(thread);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prepare_preview_with_forced_version_round_trips_through_parse_as() {
        // `--from v3_1` on a v3.2-shaped doc force-parses as v3.1; serialised
        // output should advertise `openapi: 3.1.*`.
        let path = temp_path("forced.json");
        std::fs::write(
            &path,
            br#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .expect("write temp spec");

        let args = PreviewArgs {
            file: path.clone(),
            from: Some(SpecVersion::V3_1),
            no_open: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        let parsed: serde_json::Value = serde_json::from_str(&prepared.spec_json).unwrap();
        let openapi = parsed["openapi"].as_str().unwrap();
        assert!(openapi.starts_with("3.1"), "got openapi = {openapi}");

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }
}
