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
use notify::RecursiveMode;
use notify_debouncer_mini::{Debouncer, new_debouncer};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

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

    /// Convert the spec to this OpenAPI version before serving it. Uses the
    /// same upconvert chain as `roas convert`; downconversion is rejected.
    #[arg(long, value_enum)]
    pub(crate) convert_to: Option<SpecVersion>,

    /// Don't open the browser; just print the server URL and serve.
    #[arg(long)]
    pub(crate) no_open: bool,

    /// Watch the spec file and live-reload the browser on every change.
    /// Off by default; enable to spawn a filesystem watcher and an SSE
    /// `/reload` route the rendered HTML subscribes to.
    #[arg(long)]
    pub(crate) watch: bool,

    /// Renderer for the preview page. Defaults to Redoc.
    #[arg(long, value_enum, default_value = "redoc")]
    pub(crate) renderer: Renderer,
}

/// The minimal slice of `PreviewArgs` the spec-reading pipeline needs.
/// Extracted so the file-watcher thread can re-run the same pipeline on
/// every disk change without holding a reference to the full args struct.
#[derive(Clone, Debug)]
struct SpecSource {
    file: PathBuf,
    from: Option<SpecVersion>,
    convert_to: Option<SpecVersion>,
}

impl SpecSource {
    fn from_args(args: &PreviewArgs) -> Self {
        Self {
            file: args.file.clone(),
            from: args.from,
            convert_to: args.convert_to,
        }
    }

    /// Read + parse + (optionally up)convert the spec, returning the JSON
    /// string the preview server hands to the renderer.
    fn build_spec_json(&self) -> Result<String> {
        let value = read_and_parse(&self.file)?;
        let detected = versioned::detect_or_use(self.from, value)?;
        // `convert_to(<same version>)` is a "serialise back as Value" pass —
        // pluck the version first so we don't move `detected` while borrowing it.
        let version = detected.version();
        // Resolve the target version: `--convert-to` if set, else same version
        // (no-op transform). Downconversion is rejected explicitly, matching
        // `roas convert`'s guard so users get the same error shape.
        let target = self.convert_to.unwrap_or(version);
        if (version as u8) > (target as u8) {
            bail!(
                "downconversion is not supported: input is {}, target is {}",
                version.label(),
                target.label(),
            );
        }
        serde_json::to_string(&detected.convert_to(target)?)
            .context("serializing spec for the viewer")
    }
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

/// SSE-subscriber `<script>` block. Injected into the served HTML shell when
/// `--watch` is on so the browser reloads itself on every file change.
const RELOAD_SCRIPT: &str = r#"
    <script>
      (function () {
        const es = new EventSource('/reload');
        es.onmessage = function () { window.location.reload(); };
      })();
    </script>"#;

/// Resolve the chosen renderer to its embedded HTML shell, splicing in the
/// SSE-subscriber `<script>` when `watch` is on.
fn render_html(renderer: Renderer, watch: bool) -> String {
    let base = match renderer {
        Renderer::Redoc => REDOC_HTML,
        Renderer::SwaggerUi => SWAGGER_UI_HTML,
    };
    if watch {
        // Cheap one-shot substitution; both base shells have exactly one
        // closing `</body>` tag.
        base.replace("</body>", &format!("{RELOAD_SCRIPT}\n  </body>"))
    } else {
        base.to_string()
    }
}

/// Spawn a debounced filesystem watcher on `source.file`. On each detected
/// change we re-run the spec-building pipeline and, on success, swap the
/// cached JSON and broadcast a reload to every subscriber in `bus`. Parse
/// failures are logged to stderr and the previous good JSON is left in
/// place, so a half-saved file doesn't black-hole the preview.
fn spawn_file_watcher(
    source: SpecSource,
    spec_json: Arc<Mutex<String>>,
    bus: ReloadBus,
) -> Result<Debouncer<notify::RecommendedWatcher>> {
    let (event_tx, event_rx) = mpsc::channel::<()>();
    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        move |res: notify_debouncer_mini::DebounceEventResult| {
            // Squash every batch into a single tick — we don't care which
            // events were in it, only that something happened. `Err` here
            // means the watcher itself failed; skip and try again on the
            // next event.
            if res.is_ok() {
                let _ = event_tx.send(());
            }
        },
    )
    .context("starting filesystem watcher")?;
    debouncer
        .watcher()
        .watch(&source.file, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", source.file.display()))?;

    let display_path = source.file.display().to_string();
    thread::spawn(move || {
        while event_rx.recv().is_ok() {
            match source.build_spec_json() {
                Ok(new_json) => {
                    *spec_json.lock().unwrap() = new_json;
                    broadcast_reload(&bus);
                    eprintln!("preview: reloaded {display_path}");
                }
                Err(e) => {
                    eprintln!(
                        "preview: failed to reload {display_path}, keeping previous version: {e}",
                    );
                }
            }
        }
    });
    Ok(debouncer)
}

/// Push a reload event to every connected SSE subscriber, dropping any
/// senders whose receiver has been disconnected (i.e. browser tab closed).
fn broadcast_reload(bus: &ReloadBus) {
    let mut senders = bus.lock().unwrap();
    senders.retain(|tx| tx.send(()).is_ok());
}

fn subscribe_reload(bus: &ReloadBus) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    bus.lock().unwrap().push(tx);
    rx
}

/// Everything `run_preview` collects before it commits to the blocking
/// `incoming_requests()` loop. Pulled out so unit tests can exercise the
/// happy path of `run_preview` without owning a server that blocks forever.
struct PreparedPreview {
    server: tiny_http::Server,
    url: String,
    renderer_label: &'static str,
    html: String,
    spec_json: Arc<Mutex<String>>,
    /// Multi-consumer list of senders for the SSE `/reload` route. Each
    /// connected browser tab registers its own receiver; when the watcher
    /// updates the cached spec, we broadcast to all of them. `None` when
    /// `--watch` is off, in which case `/reload` returns 404.
    reload_bus: Option<ReloadBus>,
    /// Kept alive only to hold the filesystem watch open for as long as the
    /// server is running. Dropped when `PreparedPreview` is dropped.
    _debouncer: Option<Debouncer<notify::RecommendedWatcher>>,
}

type ReloadBus = Arc<Mutex<Vec<mpsc::Sender<()>>>>;

fn prepare_preview(args: &PreviewArgs) -> Result<PreparedPreview> {
    let source = SpecSource::from_args(args);
    let initial_json = source.build_spec_json()?;
    let spec_json = Arc::new(Mutex::new(initial_json));

    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow!("starting preview server: {e}"))?;
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr.port(),
        other => bail!("unexpected listen address: {other:?}"),
    };
    let url = format!("http://127.0.0.1:{port}/");
    let renderer_label = match args.renderer {
        Renderer::Redoc => "Redoc",
        Renderer::SwaggerUi => "Swagger UI",
    };
    let html = render_html(args.renderer, args.watch);

    // Wire up the file watcher if `--watch` was passed. The debouncer is
    // returned so the caller can keep it alive — it stops watching when
    // dropped.
    let (reload_bus, debouncer) = if args.watch {
        let bus: ReloadBus = Arc::new(Mutex::new(Vec::new()));
        let debouncer = spawn_file_watcher(source, Arc::clone(&spec_json), Arc::clone(&bus))?;
        (Some(bus), Some(debouncer))
    } else {
        (None, None)
    };

    Ok(PreparedPreview {
        server,
        url,
        renderer_label,
        html,
        spec_json,
        reload_bus,
        _debouncer: debouncer,
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
    if args.watch {
        eprintln!("Watching {} for changes.", args.file.display());
    }
    eprintln!("Press Ctrl+C to stop.");

    if !args.no_open {
        // Browser open is best-effort; if it fails the user can still hit
        // the URL manually from stderr.
        let _ = webbrowser::open(&prepared.url);
    }

    serve_preview_requests(
        prepared.server,
        prepared.html,
        prepared.spec_json,
        prepared.reload_bus,
    );
}

fn serve_preview_requests(
    server: tiny_http::Server,
    html: String,
    spec_json: Arc<Mutex<String>>,
    reload_bus: Option<ReloadBus>,
) {
    let html = Arc::new(html);
    for request in server.incoming_requests() {
        // Thread per request: `/reload` is a long-lived SSE connection that
        // would otherwise block the main loop. The short-lived `/` and
        // `/spec` handlers also run on their own threads — small overhead,
        // but symmetric and avoids head-of-line blocking against reload
        // streams.
        let html = Arc::clone(&html);
        let spec_json = Arc::clone(&spec_json);
        let reload_bus = reload_bus.clone();
        thread::spawn(move || {
            handle_request(request, &html, &spec_json, reload_bus.as_ref());
        });
    }
}

fn handle_request(
    request: tiny_http::Request,
    html: &str,
    spec_json: &Mutex<String>,
    reload_bus: Option<&ReloadBus>,
) {
    let url = request.url().to_string();
    match url.as_str() {
        "/" | "/index.html" => {
            let _ = request.respond(http_response(html, "text/html; charset=utf-8"));
        }
        "/spec" | "/spec.json" => {
            let body = spec_json.lock().unwrap().clone();
            let _ = request.respond(http_response(&body, "application/json"));
        }
        "/reload" => match reload_bus {
            Some(bus) => handle_reload_stream(request, bus),
            None => {
                let _ = request.respond(not_found_response());
            }
        },
        _ => {
            let _ = request.respond(not_found_response());
        }
    }
}

fn handle_reload_stream(request: tiny_http::Request, bus: &ReloadBus) {
    let rx = subscribe_reload(bus);
    let reader = ReloadReader::new(rx);
    let headers = [
        tiny_http::Header::from_bytes(b"Content-Type".as_ref(), b"text/event-stream".as_ref()),
        tiny_http::Header::from_bytes(b"Cache-Control".as_ref(), b"no-cache".as_ref()),
    ]
    .into_iter()
    .filter_map(|h| h.ok())
    .collect();
    let response =
        tiny_http::Response::new(tiny_http::StatusCode(200), headers, reader, None, None);
    let _ = request.respond(response);
}

/// A blocking `Read` adapter that turns the reload-bus receiver into an SSE
/// byte stream. The first call emits an SSE comment so the browser gets a
/// chunk and surfaces the `EventSource` as `open`; each subsequent call
/// blocks on the receiver and emits `data: reload\n\n` per event.
struct ReloadReader {
    rx: mpsc::Receiver<()>,
    buffer: Vec<u8>,
    primed: bool,
}

impl ReloadReader {
    fn new(rx: mpsc::Receiver<()>) -> Self {
        Self {
            rx,
            buffer: Vec::new(),
            primed: false,
        }
    }
}

impl std::io::Read for ReloadReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.buffer.is_empty() {
            if !self.primed {
                // First read: emit a comment frame so the browser receives
                // chunked headers + opens `EventSource.readyState=OPEN`.
                self.buffer = b": preview-stream\n\n".to_vec();
                self.primed = true;
            } else {
                // Subsequent reads: block until the next file change.
                match self.rx.recv() {
                    Ok(()) => self.buffer = b"data: reload\n\n".to_vec(),
                    Err(_) => return Ok(0),
                }
            }
        }
        let n = buf.len().min(self.buffer.len());
        buf[..n].copy_from_slice(&self.buffer[..n]);
        self.buffer.drain(..n);
        Ok(n)
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
    fn render_html_selects_the_right_shell_per_variant() {
        assert!(render_html(Renderer::Redoc, false).contains("cdn.redoc.ly"));
        assert!(render_html(Renderer::SwaggerUi, false).contains("swagger-ui-dist"));
    }

    #[test]
    fn render_html_injects_reload_script_when_watch_is_on() {
        let off = render_html(Renderer::Redoc, false);
        let on = render_html(Renderer::Redoc, true);
        assert!(!off.contains("EventSource"));
        assert!(on.contains("EventSource"));
        assert!(on.contains("'/reload'"));
        // Still has the renderer base content.
        assert!(on.contains("cdn.redoc.ly"));
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
            serve_preview_requests(server, html_owned, Arc::new(Mutex::new(spec_owned)), None);
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
            watch: false,
            convert_to: None,
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
            watch: false,
            convert_to: None,
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
        let parsed: serde_json::Value =
            serde_json::from_str(&prepared.spec_json.lock().unwrap()).unwrap();
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
            watch: false,
            convert_to: None,
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
            watch: false,
            convert_to: None,
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
                watch: false,
                convert_to: None,
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
    fn prepare_preview_with_convert_to_upconverts_before_serving() {
        // v2.0 input + `--convert-to v3_2` → output advertises 3.2.x.
        let path = temp_path("v2.json");
        std::fs::write(
            &path,
            br#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .expect("write temp spec");

        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            convert_to: Some(SpecVersion::V3_2),
            no_open: true,
            renderer: Renderer::Redoc,
            watch: false,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        let parsed: serde_json::Value =
            serde_json::from_str(&prepared.spec_json.lock().unwrap()).unwrap();
        let openapi = parsed["openapi"].as_str().unwrap_or("");
        assert!(
            openapi.starts_with("3.2"),
            "expected upconvert to 3.2.x, got openapi = {openapi}",
        );

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prepare_preview_rejects_downconvert_target() {
        // v3.2 input + `--convert-to v2` must hit the explicit guard with a
        // "downconversion is not supported" diagnostic (same shape as
        // `roas convert`'s rejection).
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            convert_to: Some(SpecVersion::V2),
            no_open: true,
            renderer: Renderer::Redoc,
            watch: false,
        };
        let err = prepare_preview(&args)
            .err()
            .expect("downconversion must error")
            .to_string();
        assert!(
            err.contains("downconversion is not supported"),
            "got: {err}",
        );

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
            watch: false,
            convert_to: None,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        let parsed: serde_json::Value =
            serde_json::from_str(&prepared.spec_json.lock().unwrap()).unwrap();
        let openapi = parsed["openapi"].as_str().unwrap();
        assert!(openapi.starts_with("3.1"), "got openapi = {openapi}");

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    // ────────────────────────────────────────────────────────────────────
    // `--watch` coverage: SSE wiring + the broadcast/subscribe primitives.
    // The full live-reload pipeline (fs notify → re-parse → SSE event) is
    // an integration concern with real fs events and isn't worth the test
    // flakiness; we cover the deterministic pieces directly.
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn broadcast_reload_delivers_to_every_subscriber_and_drops_dead_ones() {
        let bus: ReloadBus = Arc::new(Mutex::new(Vec::new()));
        let a = subscribe_reload(&bus);
        let b = subscribe_reload(&bus);
        broadcast_reload(&bus);
        assert!(a.try_recv().is_ok());
        assert!(b.try_recv().is_ok());

        // Drop one receiver; broadcast must remove its sender from the bus.
        drop(a);
        broadcast_reload(&bus);
        assert_eq!(
            bus.lock().unwrap().len(),
            1,
            "dead sender must be retained-out"
        );
        assert!(b.try_recv().is_ok());
    }

    #[test]
    fn reload_reader_emits_priming_comment_then_blocks_for_events() {
        use std::io::Read;
        let (tx, rx) = mpsc::channel();
        let mut reader = ReloadReader::new(rx);

        // First read: priming comment frame (SSE convention so the browser
        // surfaces `EventSource.readyState=OPEN`).
        let mut buf = [0u8; 64];
        let n = reader.read(&mut buf).unwrap();
        let primed = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(primed.contains(": preview-stream"), "got: {primed}");
        assert!(primed.ends_with("\n\n"));

        // Push an event; next read should yield `data: reload\n\n`.
        tx.send(()).unwrap();
        let mut buf = [0u8; 64];
        let n = reader.read(&mut buf).unwrap();
        let event = std::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(event, "data: reload\n\n");

        // Drop the sender; the reader returns EOF (Ok(0)) rather than
        // blocking forever.
        drop(tx);
        let mut buf = [0u8; 64];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 0, "reader must return EOF when the sender is dropped");
    }

    #[test]
    fn reload_route_returns_404_when_watch_is_off() {
        // serve_preview_requests with `reload_bus: None` must 404 the route.
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
        let addr = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr,
            other => panic!("unexpected listen addr: {other:?}"),
        };
        let html = "<!doctype html><body>HELLO</body>".to_string();
        let spec_json = Arc::new(Mutex::new(r#"{}"#.to_string()));
        let thread = std::thread::spawn(move || {
            serve_preview_requests(server, html, spec_json, None);
        });

        let mut s = send_request(addr, "/reload");
        let (code, _ctype, _body) = parse_status_and_body(&mut s);
        assert_eq!(code, 404, "/reload must 404 when --watch is off");

        drop(thread);
    }

    #[test]
    fn reload_route_registers_subscriber_when_watch_is_on() {
        // End-to-end verification that /reload, when `--watch` is on,
        // reaches `handle_reload_stream` and subscribes a sender to the
        // bus. The byte-level SSE protocol (priming comment + `data: reload`
        // framing on broadcast) is covered by the standalone `ReloadReader`
        // test; reading those bytes via a `TcpStream` and tiny_http's
        // chunked encoding is timing-sensitive enough that we skip it
        // here and trust the unit test.
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
        let addr = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr,
            other => panic!("unexpected listen addr: {other:?}"),
        };

        let html = "<!doctype html>".to_string();
        let spec_json = Arc::new(Mutex::new(r#"{}"#.to_string()));
        let bus: ReloadBus = Arc::new(Mutex::new(Vec::new()));
        let bus_for_server = Arc::clone(&bus);
        let thread = std::thread::spawn(move || {
            serve_preview_requests(server, html, spec_json, Some(bus_for_server));
        });

        // Keep the connection open in `_stream` so the server-side handler
        // doesn't bail on a half-written response — we don't read from it,
        // we just need it to exist long enough for the handler to subscribe.
        let _stream = send_request(addr, "/reload");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && bus.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !bus.lock().unwrap().is_empty(),
            "SSE handler must register a subscriber on /reload",
        );

        drop(thread);
    }

    /// Real filesystem event: write a fresh spec, start the watcher, rewrite
    /// the file, wait for the broadcast, and verify the cached JSON reflects
    /// the new contents.
    ///
    /// Timing-sensitive. If this turns out to be flaky in CI we can pull it
    /// behind a `#[ignore]` gate, but the deterministic-ish bits
    /// (`broadcast_reload`, `ReloadReader`, `render_html` injection,
    /// `prepare_preview` wiring) are already covered above, so a flake here
    /// only loses *integration* coverage.
    #[test]
    fn file_watcher_broadcasts_reload_and_refreshes_spec_json_on_change() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            convert_to: None,
            no_open: true,
            watch: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        let bus = prepared
            .reload_bus
            .clone()
            .expect("--watch must allocate a reload bus");
        let spec_json = Arc::clone(&prepared.spec_json);
        let rx = subscribe_reload(&bus);

        // Brief pause so the OS-side watcher is fully wired before we touch
        // the file. notify's debouncer is set to 150ms so we don't need much.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(
            &path,
            br#"{"openapi":"3.2.0","info":{"title":"changed","version":"1"},"paths":{}}"#,
        )
        .expect("rewrite spec");

        let received = rx.recv_timeout(Duration::from_secs(3));
        assert!(
            received.is_ok(),
            "watcher must broadcast reload within 3s of a real file change",
        );

        let updated = spec_json.lock().unwrap().clone();
        let parsed: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(
            parsed["info"]["title"], "changed",
            "spec_json must reflect the rewritten file",
        );

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    /// Negative path: a broken save must not corrupt the cached JSON — the
    /// previous good version is left in place and stderr gets the diagnostic.
    #[test]
    fn file_watcher_keeps_previous_json_when_reparse_fails() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            convert_to: None,
            no_open: true,
            watch: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        let spec_json = Arc::clone(&prepared.spec_json);
        let before = spec_json.lock().unwrap().clone();

        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&path, b"%%% not parseable %%%").expect("write garbage");

        // Wait past the debouncer window + a bit of slack for the watcher's
        // reparse attempt and its eprintln.
        std::thread::sleep(Duration::from_millis(500));

        let after = spec_json.lock().unwrap().clone();
        assert_eq!(
            before, after,
            "spec_json must be untouched when the new file fails to parse",
        );

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prepare_preview_with_watch_wires_up_reload_bus_and_html_injection() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            convert_to: None,
            no_open: true,
            watch: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).expect("prepare ok");
        assert!(
            prepared.reload_bus.is_some(),
            "--watch must allocate a reload bus"
        );
        assert!(
            prepared.html.contains("EventSource"),
            "--watch must inject the SSE-subscriber script",
        );

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }
}
