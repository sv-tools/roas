use roas::loader::{LoaderError, ResourceFetcher};
use roas_http_fetcher::{HttpFetchError, HttpFetcher};
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

impl TestServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&str) -> (u16, &'static str, Vec<u8>) + Send + 'static,
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
                        let (status, reason, body) = handler(&request_line);
                        write_response(stream, status, reason, &body);
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

fn write_response(mut stream: TcpStream, status: u16, reason: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body.len(),
    );
    stream.write_all(header.as_bytes()).expect("write header");
    stream.write_all(body).expect("write body");
}

#[test]
fn http_fetcher_returns_parsed_json_on_success() {
    let server = TestServer::start(|_req| (200, "OK", br#"{"hello":"world"}"#.to_vec()));
    let mut fetcher = HttpFetcher::new();
    let value = fetcher.fetch(&server.url("doc.json")).expect("fetch ok");
    assert_eq!(value, serde_json::json!({ "hello": "world" }));
}

#[test]
fn http_fetcher_surfaces_non_2xx_as_loader_error_fetch_with_status() {
    let server = TestServer::start(|_req| (404, "Not Found", b"missing".to_vec()));
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
    let server = TestServer::start(|_req| (200, "OK", b"not json".to_vec()));
    let mut fetcher = HttpFetcher::new();
    let err = fetcher
        .fetch(&server.url("bad.json"))
        .expect_err("invalid JSON must error");
    assert!(matches!(err, LoaderError::Parse { .. }));
}

#[test]
fn http_fetcher_surfaces_connection_refused_as_loader_error_fetch_request() {
    // Bind, capture the port, then drop the listener so nothing is listening.
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
