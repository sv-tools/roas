//! What a step sends and what comes back, and the two traits a caller
//! implements to carry them.
//!
//! This crate performs no IO of its own. The engine decides *what* to
//! send; a client decides *how* — which is what lets the same engine run
//! under a blocking client, an async one, or a fake that never touches a
//! network. The traits mirror
//! [`roas::loader::ResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.ResourceFetcher.html)
//! and its async sibling, so a caller that already has fetchers for the
//! loader will recognize the shape.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// A request a step wants performed.
///
/// Header names are kept as written — a runtime expression may name one
/// in any case, and matching is case-insensitive where it is read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpRequest {
    /// Upper-case HTTP method, e.g. `GET`.
    pub method: String,
    /// The fully resolved URL, query string included.
    pub url: String,
    /// Headers in the order the step named them.
    pub headers: Vec<(String, String)>,
    /// The encoded body, if the step has one.
    pub body: Option<Vec<u8>>,
    /// How long the step is willing to wait, from `Step.timeout`.
    pub timeout: Option<Duration>,
}

impl HttpRequest {
    /// The first value of `name`, matched case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The body as text, lossily decoded. Empty when there is no body.
    #[must_use]
    pub fn text(&self) -> Cow<'_, str> {
        match &self.body {
            Some(body) => String::from_utf8_lossy(body),
            None => Cow::Borrowed(""),
        }
    }
}

/// What a client got back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// Response headers, in the order received.
    pub headers: Vec<(String, String)>,
    /// The raw body. Empty rather than absent, as HTTP has it.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response that carries a JSON body, for tests and fakes.
    #[must_use]
    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.to_string().into_bytes(),
        }
    }

    /// The first value of `name`, matched case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The body as text, lossily decoded.
    #[must_use]
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// The body parsed as JSON, or `None` when it is not JSON.
    ///
    /// A runtime expression may point into a response body, and only a
    /// parsed body can be pointed into.
    #[must_use]
    pub fn body_as_json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Whatever went wrong carrying a request out.
///
/// The engine does not interpret it: a client failure ends the run and
/// is reported as it was reported here.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ClientError(pub String);

impl ClientError {
    /// A failure described by any error the client already has.
    pub fn new(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

/// Performs a request and waits for the answer.
pub trait HttpClient {
    /// Send `request` and return what came back.
    ///
    /// # Errors
    ///
    /// Whatever prevented the exchange from completing — a connection
    /// failure, a timeout, an unreadable body.
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, ClientError>;
}

/// The future an [`AsyncHttpClient`] returns from `send`.
pub type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HttpResponse, ClientError>> + Send + 'a>>;

/// The future an [`AsyncHttpClient`] returns from `sleep`.
pub type SleepFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Performs a request without blocking the thread.
pub trait AsyncHttpClient {
    /// Send `request` and return what came back.
    ///
    /// # Errors
    ///
    /// As [`HttpClient::send`].
    fn send<'a>(&'a mut self, request: &'a HttpRequest) -> SendFuture<'a>;

    /// Wait for `duration`.
    ///
    /// A retry may ask for a delay, and an executor that performs no IO
    /// has no runtime to wait on either — the client, which has one,
    /// says how. A test client can return immediately.
    fn sleep(&self, duration: Duration) -> SleepFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_header_is_found_whatever_case_it_was_written_in() {
        let request = HttpRequest {
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            ..HttpRequest::default()
        };
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.header("CONTENT-TYPE"), Some("application/json"));
        assert_eq!(request.header("accept"), None);
    }

    #[test]
    fn a_json_response_carries_its_body_both_ways() {
        let response = HttpResponse::json(201, &json!({ "id": 7 }));
        assert_eq!(response.status, 201);
        assert_eq!(response.header("Content-Type"), Some("application/json"));
        assert_eq!(response.body_as_json(), Some(json!({ "id": 7 })));
        assert_eq!(response.text(), r#"{"id":7}"#);
    }

    #[test]
    fn a_body_that_is_not_json_is_still_text() {
        let response = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"not json".to_vec(),
        };
        assert_eq!(response.body_as_json(), None);
        assert_eq!(response.text(), "not json");
    }

    #[test]
    fn a_request_without_a_body_reads_as_empty() {
        assert_eq!(HttpRequest::default().text(), "");
    }

    #[test]
    fn a_client_error_says_what_it_was_told() {
        let error = ClientError::new(std::io::Error::other("connection reset"));
        assert_eq!(error.to_string(), "connection reset");
    }
}
