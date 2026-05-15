use roas::loader::{AsyncResourceFetcher, LoaderError, ResourceFetcher};
use roas_http_fetcher::{AsyncHttpFetcher, HttpFetchError, HttpFetcher};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};
use std::thread::{self, JoinHandle};
use url::Url;

struct TestServer {
    base: Url,
    shutdown: Sender<()>,
    join: Option<JoinHandle<()>>,
}

struct TestResponse {
    status: u16,
    reason: &'static str,
    content_type: Option<&'static str>,
    body: Vec<u8>,
}

impl TestResponse {
    fn ok_json(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: None,
            body,
        }
    }

    #[cfg(feature = "yaml")]
    fn with_content_type(mut self, ct: &'static str) -> Self {
        self.content_type = Some(ct);
        self
    }
}

impl TestServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&str) -> TestResponse + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        listener
            .set_nonblocking(true)
            .expect("set listener non-blocking");
        let base = Url::parse(&format!("http://{addr}/")).expect("parse base url");
        let (shutdown_tx, shutdown_rx) = channel::<()>();

        let join = thread::spawn(move || {
            loop {
                if shutdown_rx.try_recv().is_ok() {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let request_line = read_request_line(&stream);
                        let resp = handler(&request_line);
                        write_response(stream, resp);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept failed: {e}"),
                }
            }
        });

        Self {
            base,
            shutdown: shutdown_tx,
            join: Some(join),
        }
    }

    fn url(&self, path: &str) -> Url {
        self.base.join(path).expect("join path")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn read_request_line(mut stream: &TcpStream) -> String {
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).expect("read request");
    let raw = String::from_utf8_lossy(&buf[..n]).to_string();
    raw.lines().next().unwrap_or("").to_string()
}

fn write_response(mut stream: TcpStream, resp: TestResponse) {
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {len}\r\nConnection: close\r\n",
        status = resp.status,
        reason = resp.reason,
        len = resp.body.len(),
    );
    if let Some(ct) = resp.content_type {
        header.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes()).expect("write header");
    stream.write_all(&resp.body).expect("write body");
}

#[test]
fn http_fetcher_returns_parsed_json_on_success() {
    let server = TestServer::start(|_req| TestResponse::ok_json(br#"{"hello":"world"}"#.to_vec()));
    let mut fetcher = HttpFetcher::new();
    let value = fetcher.fetch(&server.url("doc.json")).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "hello": "world" }));
}

#[test]
fn http_fetcher_surfaces_non_2xx_as_loader_error_fetch_with_status() {
    let server = TestServer::start(|_req| TestResponse {
        status: 404,
        reason: "Not Found",
        content_type: None,
        body: b"missing".to_vec(),
    });
    let mut fetcher = HttpFetcher::new();
    let err = fetcher
        .fetch(&server.url("nope.json"))
        .expect_err("non-2xx must error");
    match err {
        LoaderError::Fetch { uri, source } => {
            assert!(uri.ends_with("/nope.json"));
            let http = source
                .downcast_ref::<HttpFetchError>()
                .expect("source must be HttpFetchError");
            assert!(matches!(
                http,
                HttpFetchError::Status { status } if status.as_u16() == 404
            ));
        }
        other => panic!("expected LoaderError::Fetch, got {other:?}"),
    }
}

#[test]
fn http_fetcher_surfaces_invalid_json_body_as_parse_error() {
    let server = TestServer::start(|_req| TestResponse::ok_json(b"not json".to_vec()));
    let mut fetcher = HttpFetcher::new();
    let err = fetcher
        .fetch(&server.url("bad.json"))
        .expect_err("invalid JSON must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[cfg(feature = "yaml")]
#[test]
fn http_fetcher_parses_yaml_when_content_type_signals_yaml() {
    let body = b"name: pet\ncount: 3\n".to_vec();
    let server = TestServer::start(move |_req| {
        TestResponse::ok_json(body.clone()).with_content_type("application/yaml")
    });
    let mut fetcher = HttpFetcher::new();
    let value = fetcher.fetch(&server.url("doc")).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "name": "pet", "count": 3 }));
}

#[cfg(feature = "yaml")]
#[test]
fn http_fetcher_parses_yaml_from_url_extension_when_content_type_missing() {
    let body = b"items:\n  - a\n  - b\n".to_vec();
    let server = TestServer::start(move |_req| TestResponse::ok_json(body.clone()));
    let mut fetcher = HttpFetcher::new();
    let value = fetcher.fetch(&server.url("doc.yaml")).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "items": ["a", "b"] }));
}

#[cfg(feature = "yaml")]
#[test]
fn http_fetcher_yaml_parse_error_surfaces_as_loader_error_parse() {
    // ` - foo: bar\nfoo: baz` would still parse; instead, use an
    // unambiguously-broken document (tab-indented, which YAML 1.2 forbids).
    let body = b"key:\n\tvalue: oops\n".to_vec();
    let server = TestServer::start(move |_req| {
        TestResponse::ok_json(body.clone()).with_content_type("application/yaml")
    });
    let mut fetcher = HttpFetcher::new();
    let err = fetcher
        .fetch(&server.url("bad.yaml"))
        .expect_err("malformed YAML must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[cfg(feature = "yaml")]
#[test]
fn http_fetcher_still_parses_json_when_content_type_is_json_despite_yaml_extension() {
    let body = br#"{"explicit":"json"}"#.to_vec();
    let server = TestServer::start(move |_req| {
        TestResponse::ok_json(body.clone()).with_content_type("application/json")
    });
    let mut fetcher = HttpFetcher::new();
    let value = fetcher
        .fetch(&server.url("misleading.yaml"))
        .expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "explicit": "json" }));
}

#[test]
fn http_fetcher_surfaces_connection_refused_as_loader_error_fetch_request() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let url = Url::parse(&format!("http://{addr}/missing.json")).expect("url");

    let mut fetcher = HttpFetcher::new();
    let err = fetcher.fetch(&url).expect_err("no server must error");
    match err {
        LoaderError::Fetch { source, .. } => {
            let http = source
                .downcast_ref::<HttpFetchError>()
                .expect("source must be HttpFetchError");
            assert!(matches!(http, HttpFetchError::Request { .. }));
        }
        other => panic!("expected LoaderError::Fetch, got {other:?}"),
    }
}

#[tokio::test]
async fn async_http_fetcher_returns_parsed_json_on_success() {
    let server = TestServer::start(|_req| TestResponse::ok_json(br#"{"hello":"world"}"#.to_vec()));
    let mut fetcher = AsyncHttpFetcher::new();
    let value = fetcher
        .fetch(&server.url("doc.json"))
        .await
        .expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "hello": "world" }));
}

#[tokio::test]
async fn async_http_fetcher_surfaces_non_2xx_as_loader_error_fetch_with_status() {
    let server = TestServer::start(|_req| TestResponse {
        status: 404,
        reason: "Not Found",
        content_type: None,
        body: b"missing".to_vec(),
    });
    let mut fetcher = AsyncHttpFetcher::new();
    let err = fetcher
        .fetch(&server.url("nope.json"))
        .await
        .expect_err("non-2xx must error");
    match err {
        LoaderError::Fetch { uri, source } => {
            assert!(uri.ends_with("/nope.json"));
            let http = source
                .downcast_ref::<HttpFetchError>()
                .expect("source must be HttpFetchError");
            assert!(matches!(
                http,
                HttpFetchError::Status { status } if status.as_u16() == 404
            ));
        }
        other => panic!("expected LoaderError::Fetch, got {other:?}"),
    }
}

#[tokio::test]
async fn async_http_fetcher_surfaces_connection_refused_as_loader_error_fetch_request() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let url = Url::parse(&format!("http://{addr}/missing.json")).expect("url");

    let mut fetcher = AsyncHttpFetcher::new();
    let err = fetcher.fetch(&url).await.expect_err("no server must error");
    match err {
        LoaderError::Fetch { source, .. } => {
            let http = source
                .downcast_ref::<HttpFetchError>()
                .expect("source must be HttpFetchError");
            assert!(matches!(http, HttpFetchError::Request { .. }));
        }
        other => panic!("expected LoaderError::Fetch, got {other:?}"),
    }
}

#[cfg(feature = "yaml")]
#[tokio::test]
async fn async_http_fetcher_parses_yaml_when_content_type_signals_yaml() {
    let body = b"name: pet\ncount: 3\n".to_vec();
    let server = TestServer::start(move |_req| {
        TestResponse::ok_json(body.clone()).with_content_type("application/yaml")
    });
    let mut fetcher = AsyncHttpFetcher::new();
    let value = fetcher.fetch(&server.url("doc")).await.expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "name": "pet", "count": 3 }));
}
