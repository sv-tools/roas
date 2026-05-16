//! `roas preview <FILE>` — start a local HTTP server that renders the spec in
//! a browser via [Redoc](https://redocly.com/redoc) or
//! [Swagger UI](https://swagger.io/tools/swagger-ui/).
//!
//! Routes:
//!   * `/` (and `/index.html`) — a small HTML shell that mounts the renderer
//!     and pulls its bundle from the official CDN.
//!   * `/spec` (and `/spec.json`) — the input spec, parsed via the existing
//!     `read_and_parse` / `detect_or_use` pipeline and re-serialised as JSON.
//!   * `/reload` — when `--watch` is on, a Server-Sent-Events endpoint that
//!     pushes one `data: reload` frame per file change. The injected
//!     page-side `EventSource` subscriber calls `window.location.reload()` on
//!     every event.
//!
//! Backed by [`axum`] on top of [`tokio`] / [`hyper`]; the per-process tokio
//! runtime is constructed inside [`run_preview`] so the rest of the CLI
//! stays sync.

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use notify::RecursiveMode;
use notify_debouncer_mini::{Debouncer, new_debouncer};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

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
}

impl SpecSource {
    fn from_args(args: &PreviewArgs) -> Self {
        Self {
            file: args.file.clone(),
            from: args.from,
        }
    }

    /// Read + parse the spec, returning the JSON string the preview server
    /// hands to the renderer. Uses `convert_to(<same version>)` as a
    /// "serialise back as a `Value`" pass.
    fn build_spec_json(&self) -> Result<String> {
        let value = read_and_parse(&self.file)?;
        let detected = versioned::detect_or_use(self.from, value)?;
        let version = detected.version();
        serde_json::to_string(&detected.convert_to(version)?)
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
/// cached JSON and broadcast a reload to every SSE subscriber. Parse
/// failures are logged to stderr and the previous good JSON is left in
/// place, so a half-saved file doesn't black-hole the preview.
fn spawn_file_watcher(
    source: SpecSource,
    spec_json: Arc<Mutex<String>>,
    reload_tx: broadcast::Sender<()>,
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
                    // Ignored: a `SendError` here just means no SSE
                    // subscribers are connected right now, which is fine.
                    let _ = reload_tx.send(());
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

/// Shared state injected into every axum handler via [`State`]. Cloning is
/// cheap (everything is `Arc`-backed).
#[derive(Clone)]
struct AppState {
    html: Arc<String>,
    spec_json: Arc<Mutex<String>>,
    /// Broadcast sender for `--watch`. Each `/reload` SSE handler calls
    /// `subscribe()`; the filesystem watcher calls `send(())`. `None` when
    /// `--watch` is off, in which case `/reload` returns 404.
    reload_tx: Option<broadcast::Sender<()>>,
}

/// Everything `run_preview` constructs before it hands control to
/// `axum::serve`. Pulled out so unit tests can exercise the setup path
/// (state assembly, filesystem watcher, listener) without taking over the
/// runtime indefinitely.
struct PreparedPreview {
    listener: tokio::net::TcpListener,
    url: String,
    renderer_label: &'static str,
    state: AppState,
    /// Kept alive only to hold the filesystem watch open for as long as the
    /// server is running. Dropped when `PreparedPreview` is dropped.
    _debouncer: Option<Debouncer<notify::RecommendedWatcher>>,
}

async fn prepare_preview(args: &PreviewArgs) -> Result<PreparedPreview> {
    let source = SpecSource::from_args(args);
    let initial_json = source.build_spec_json()?;
    let spec_json = Arc::new(Mutex::new(initial_json));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding preview server listener")?;
    let addr = listener
        .local_addr()
        .context("reading preview server local addr")?;
    let url = format!("http://{addr}/");
    let renderer_label = match args.renderer {
        Renderer::Redoc => "Redoc",
        Renderer::SwaggerUi => "Swagger UI",
    };
    let html = Arc::new(render_html(args.renderer, args.watch));

    // Wire up the file watcher if `--watch` was passed. The debouncer is
    // returned so the caller can keep it alive — it stops watching when
    // dropped.
    let (reload_tx, debouncer) = if args.watch {
        let (tx, _initial_rx) = broadcast::channel::<()>(16);
        let debouncer = spawn_file_watcher(source, Arc::clone(&spec_json), tx.clone())?;
        (Some(tx), Some(debouncer))
    } else {
        (None, None)
    };

    let state = AppState {
        html,
        spec_json,
        reload_tx,
    };

    Ok(PreparedPreview {
        listener,
        url,
        renderer_label,
        state,
        _debouncer: debouncer,
    })
}

pub fn run_preview(args: PreviewArgs) -> Result<()> {
    // Spin up a per-command multi-threaded runtime so the rest of the CLI
    // can stay synchronous. `enable_all` switches on the I/O + time drivers
    // axum / hyper / notify all rely on.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime for preview server")?;
    runtime.block_on(run_preview_async(args))
}

async fn run_preview_async(args: PreviewArgs) -> Result<()> {
    let prepared = prepare_preview(&args).await?;
    drive_prepared_preview(&args, prepared).await
}

/// Tail of `run_preview_async` — prints the "Serving …" banner, optionally
/// pokes `webbrowser::open`, and runs `axum::serve` until the listener
/// closes. Split out so a test can drive it against a `PreparedPreview`
/// it constructed itself.
async fn drive_prepared_preview(args: &PreviewArgs, prepared: PreparedPreview) -> Result<()> {
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

    let app = preview_router(prepared.state);
    axum::serve(prepared.listener, app)
        .await
        .context("serving preview HTTP requests")
}

/// Build the axum router for the preview server. Pulled out so tests can
/// assemble the router against a hand-rolled `AppState`.
fn preview_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handler_index))
        .route("/index.html", get(handler_index))
        .route("/spec", get(handler_spec))
        .route("/spec.json", get(handler_spec))
        .route("/reload", get(handler_reload))
        .with_state(state)
}

async fn handler_index(State(state): State<AppState>) -> Html<String> {
    Html((*state.html).clone())
}

async fn handler_spec(State(state): State<AppState>) -> Response {
    let body = state.spec_json.lock().unwrap().clone();
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn handler_reload(State(state): State<AppState>) -> Response {
    let Some(tx) = state.reload_tx.as_ref() else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let rx = tx.subscribe();
    // `BroadcastStream` yields `Result<(), BroadcastStreamRecvError>`; a
    // `Lagged` error just means this subscriber missed some events because
    // the channel filled up. Drop those silently — the next real event
    // still produces a reload, which is all the browser cares about.
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(()) => Some(Ok::<_, Infallible>(Event::default().data("reload"))),
        Err(_) => None,
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as IoRead, Write};
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

    fn write_minimal_v3_2_spec() -> PathBuf {
        let path = temp_path("ok.json");
        std::fs::write(
            &path,
            br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        )
        .expect("write temp spec");
        path
    }

    /// Stand the prepared preview's axum router up on its bound listener and
    /// return the bound address + a handle (kept alive until the test drops
    /// it) plus the `AppState` clone the router was constructed from. Tests
    /// hit the address with raw `TcpStream` so we don't pull in `reqwest`
    /// (and stay tolerant of axum-version churn around test clients).
    async fn spawn_axum_for_prepared(
        prepared: PreparedPreview,
    ) -> (SocketAddr, AppState, ServerHandle) {
        let addr = prepared
            .listener
            .local_addr()
            .expect("local_addr on prepared listener");
        let state = prepared.state.clone();
        let app = preview_router(prepared.state);
        let handle = tokio::spawn(async move {
            let _ = axum::serve(prepared.listener, app).await;
            // Keep the debouncer alive until the server exits.
            drop(prepared._debouncer);
        });
        // Give axum a tick to register the route table on the listener.
        tokio::time::sleep(Duration::from_millis(20)).await;
        (addr, state, ServerHandle(Some(handle)))
    }

    /// `JoinHandle` newtype that aborts the running axum task on drop.
    /// Without this, every test would leave a server task running until the
    /// process exits.
    struct ServerHandle(Option<tokio::task::JoinHandle<()>>);
    impl Drop for ServerHandle {
        fn drop(&mut self) {
            if let Some(h) = self.0.take() {
                h.abort();
            }
        }
    }

    fn send_request_sync(addr: SocketAddr, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect(addr).expect("connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).expect("write request");
        stream
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

    /// Async helper for hitting the running axum server. Wraps the sync
    /// `TcpStream` logic in `spawn_blocking` so it can be awaited from
    /// `#[tokio::test]` cases without blocking the runtime thread.
    async fn get(addr: SocketAddr, path: &str) -> (u16, String, String) {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || {
            let mut s = send_request_sync(addr, &path);
            parse_status_and_body(&mut s)
        })
        .await
        .expect("blocking task")
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
        // Without --watch, no SSE subscriber should be present.
        assert!(!off.contains("EventSource"));
        // With --watch, the EventSource subscriber lands in the page.
        assert!(on.contains("new EventSource('/reload')"));
        assert!(on.contains("window.location.reload()"));
        // Still has the renderer base content.
        assert!(on.contains("cdn.redoc.ly"));
    }

    #[test]
    fn run_preview_missing_file_errors_with_reading_context() {
        let args = PreviewArgs {
            file: temp_path("missing.json"),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
            watch: false,
        };
        let err = run_preview(args).expect_err("missing file must error before server starts");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[tokio::test]
    async fn prepare_preview_with_redoc_returns_bound_listener_and_redoc_assets() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
            watch: false,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");

        let addr = prepared.listener.local_addr().expect("local_addr");
        assert!(addr.ip().is_loopback(), "must bind to loopback");
        assert!(addr.port() > 0, "must allocate an ephemeral port");
        assert_eq!(prepared.url, format!("http://127.0.0.1:{}/", addr.port()));

        assert_eq!(prepared.renderer_label, "Redoc");
        assert!(prepared.state.html.contains("cdn.redoc.ly"));

        let parsed: serde_json::Value =
            serde_json::from_str(&prepared.state.spec_json.lock().unwrap()).unwrap();
        assert_eq!(parsed["openapi"], "3.2.0");

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn prepare_preview_with_swagger_ui_switches_renderer_fields() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::SwaggerUi,
            watch: false,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");

        assert_eq!(prepared.renderer_label, "Swagger UI");
        assert!(prepared.state.html.contains("swagger-ui-dist"));

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn prepare_preview_with_forced_version_round_trips_through_parse_as() {
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
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        let parsed: serde_json::Value =
            serde_json::from_str(&prepared.state.spec_json.lock().unwrap()).unwrap();
        let openapi = parsed["openapi"].as_str().unwrap();
        assert!(openapi.starts_with("3.1"), "got openapi = {openapi}");

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    /// End-to-end: serve a real router on the bound listener and exercise
    /// every route the public API exposes.
    #[tokio::test]
    async fn axum_router_serves_html_spec_and_404s_unknown_routes() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
            watch: false,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        let (addr, _state, _server) = spawn_axum_for_prepared(prepared).await;

        let (code, ctype, body) = get(addr, "/").await;
        assert_eq!(code, 200);
        assert!(ctype.contains("text/html"));
        assert!(body.contains("cdn.redoc.ly"));

        let (code, _ctype, body) = get(addr, "/index.html").await;
        assert_eq!(code, 200);
        assert!(body.contains("cdn.redoc.ly"));

        let (code, ctype, body) = get(addr, "/spec").await;
        assert_eq!(code, 200);
        assert!(ctype.contains("application/json"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["openapi"], "3.2.0");

        let (code, _ctype, body) = get(addr, "/spec.json").await;
        assert_eq!(code, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["openapi"], "3.2.0");

        let (code, _ctype, _body) = get(addr, "/nope").await;
        assert_eq!(code, 404);

        let _ = std::fs::remove_file(&path);
    }

    /// `/reload` returns 404 when `--watch` is off (no broadcast sender on
    /// the state).
    #[tokio::test]
    async fn reload_route_returns_404_when_watch_is_off() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
            watch: false,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        let (addr, _state, _server) = spawn_axum_for_prepared(prepared).await;

        let (code, _ctype, _body) = get(addr, "/reload").await;
        assert_eq!(code, 404, "/reload must 404 when --watch is off");

        let _ = std::fs::remove_file(&path);
    }

    /// `/reload` returns an SSE `text/event-stream` body containing a
    /// `data: reload` frame when the watcher fires a broadcast. We don't
    /// rely on a real fs event here — push directly through the broadcast
    /// channel so the test is deterministic.
    #[tokio::test]
    async fn reload_route_emits_sse_frame_when_broadcast_fires() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            renderer: Renderer::Redoc,
            watch: true,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        let (addr, state, _server) = spawn_axum_for_prepared(prepared).await;
        let tx = state
            .reload_tx
            .clone()
            .expect("--watch must allocate a broadcast sender");

        // Open the SSE stream on a blocking thread, then push a reload from
        // here. Read just enough of the response to confirm the frame.
        let client = tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect(addr).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("set read timeout");
            let req =
                "GET /reload HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n";
            stream.write_all(req.as_bytes()).expect("write");

            // Drain enough bytes to see the SSE frame land. We can't read to
            // EOF — the server keeps the stream open — so cap the read.
            let mut buf = [0u8; 1024];
            let mut acc = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if String::from_utf8_lossy(&acc).contains("data: reload") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            String::from_utf8_lossy(&acc).to_string()
        });

        // Give the handler a moment to subscribe before broadcasting.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = tx.send(());

        let body = client.await.expect("client task");
        assert!(
            body.contains("text/event-stream"),
            "expected SSE content-type, body was: {body:?}",
        );
        assert!(
            body.contains("data: reload"),
            "expected SSE reload frame, body was: {body:?}",
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Real filesystem event: write a fresh spec, start the watcher, rewrite
    /// the file, wait for the broadcast, and verify the cached JSON reflects
    /// the new contents.
    ///
    /// Timing-sensitive. If this turns out to be flaky in CI we can pull it
    /// behind a `#[ignore]` gate, but it's the only test that exercises the
    /// full notify-debouncer → re-parse → broadcast pipeline end-to-end.
    #[tokio::test]
    async fn file_watcher_broadcasts_reload_and_refreshes_spec_json_on_change() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            watch: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        let tx = prepared
            .state
            .reload_tx
            .clone()
            .expect("--watch must allocate a broadcast sender");
        let mut rx = tx.subscribe();
        let spec_json = Arc::clone(&prepared.state.spec_json);

        // Brief pause so the OS-side watcher is fully wired before we touch
        // the file. notify's debouncer is set to 150ms so we don't need much.
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(
            &path,
            br#"{"openapi":"3.2.0","info":{"title":"changed","version":"1"},"paths":{}}"#,
        )
        .expect("rewrite spec");

        let received = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
        assert!(
            matches!(received, Ok(Ok(()))),
            "watcher must broadcast reload within 3s of a real file change, got {received:?}",
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
    #[tokio::test]
    async fn file_watcher_keeps_previous_json_when_reparse_fails() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            watch: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        let spec_json = Arc::clone(&prepared.state.spec_json);
        let before = spec_json.lock().unwrap().clone();

        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(&path, b"%%% not parseable %%%").expect("write garbage");

        // Wait past the debouncer window + a bit of slack for the watcher's
        // reparse attempt and its eprintln.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let after = spec_json.lock().unwrap().clone();
        assert_eq!(
            before, after,
            "spec_json must be untouched when the new file fails to parse",
        );

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn prepare_preview_with_watch_wires_up_broadcast_sender_and_html_injection() {
        let path = write_minimal_v3_2_spec();
        let args = PreviewArgs {
            file: path.clone(),
            from: None,
            no_open: true,
            watch: true,
            renderer: Renderer::Redoc,
        };
        let prepared = prepare_preview(&args).await.expect("prepare ok");
        assert!(
            prepared.state.reload_tx.is_some(),
            "--watch must allocate a broadcast sender",
        );
        assert!(
            prepared.state.html.contains("new EventSource('/reload')"),
            "--watch must inject the SSE-subscriber script",
        );

        drop(prepared);
        let _ = std::fs::remove_file(&path);
    }
}
