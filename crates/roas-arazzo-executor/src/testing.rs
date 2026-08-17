//! A client that answers from a script.
//!
//! Running a workflow is the interesting part; talking to a server is
//! not. [`Fake`] answers each request from a list prepared in advance
//! and keeps what it was asked, so a test can assert on both what the
//! engine sent and what it made of the answers — with no network, no
//! runtime and no timing.

use crate::http::{
    AsyncHttpClient, ClientError, HttpClient, HttpRequest, HttpResponse, SendFuture, SleepFuture,
};
use serde_json::Value;
use std::time::Duration;

/// An HTTP client that replies from a script.
#[derive(Clone, Debug, Default)]
pub struct Fake {
    replies: Vec<HttpResponse>,
    /// Every request it was asked to send, in order.
    sent: Vec<HttpRequest>,
    /// Every duration it was asked to wait, in order.
    waited: Vec<Duration>,
    at: usize,
}

impl Fake {
    /// A client with nothing to say yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer the next request with this status and JSON body.
    #[must_use]
    pub fn reply(mut self, status: u16, body: &Value) -> Self {
        self.replies.push(HttpResponse::json(status, body));
        self
    }

    /// Answer the next request with exactly this response.
    #[must_use]
    pub fn reply_with(mut self, response: HttpResponse) -> Self {
        self.replies.push(response);
        self
    }

    /// Every request the run made, in order.
    #[must_use]
    pub fn sent(&self) -> &[HttpRequest] {
        &self.sent
    }

    /// Every wait the run asked for, in order — the delays a `retry`
    /// wanted, which a test can assert on without spending them.
    #[must_use]
    pub fn waited(&self) -> &[Duration] {
        &self.waited
    }

    fn answer(&mut self, request: &HttpRequest) -> Result<HttpResponse, ClientError> {
        self.sent.push(request.clone());
        let reply = self.replies.get(self.at).cloned().ok_or_else(|| {
            ClientError(format!(
                "the script has {} replies, and this is request {}: {} {}",
                self.replies.len(),
                self.at + 1,
                request.method,
                request.url,
            ))
        })?;
        self.at += 1;
        Ok(reply)
    }
}

impl HttpClient for Fake {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, ClientError> {
        self.answer(request)
    }
}

impl AsyncHttpClient for Fake {
    fn send<'a>(&'a mut self, request: &'a HttpRequest) -> SendFuture<'a> {
        let answer = self.answer(request);
        Box::pin(std::future::ready(answer))
    }

    /// Returns at once: a test should not spend the delay it asserts on.
    fn sleep(&self, _duration: Duration) -> SleepFuture<'_> {
        Box::pin(std::future::ready(()))
    }
}

/// A blocking [`HttpClient`] that records the waits asked of it instead
/// of sleeping through them.
///
/// [`crate::execute`] sleeps on the calling thread, so a test that wants
/// a retry's delay counted rather than spent drives [`crate::Run`]
/// itself; this is the recorder it uses.
impl Fake {
    /// Note that the run asked to wait, without waiting.
    pub fn note_wait(&mut self, duration: Duration) {
        self.waited.push(duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(url: &str) -> HttpRequest {
        HttpRequest {
            method: "GET".to_owned(),
            url: url.to_owned(),
            ..HttpRequest::default()
        }
    }

    #[test]
    fn replies_come_back_in_the_order_they_were_scripted() {
        let mut fake = Fake::new()
            .reply(200, &json!({ "first": true }))
            .reply_with(HttpResponse {
                status: 503,
                headers: Vec::new(),
                body: Vec::new(),
            });
        let first = HttpClient::send(&mut fake, &request("https://example.com/a")).unwrap();
        assert_eq!(first.body_as_json(), Some(json!({ "first": true })));
        let second = HttpClient::send(&mut fake, &request("https://example.com/b")).unwrap();
        assert_eq!(second.status, 503);
        assert_eq!(fake.sent().len(), 2);
        assert_eq!(fake.sent()[1].url, "https://example.com/b");
    }

    #[test]
    fn running_out_of_script_says_which_request_it_was() {
        let mut fake = Fake::new();
        let error = HttpClient::send(&mut fake, &request("https://example.com/a")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the script has 0 replies, and this is request 1: GET https://example.com/a"
        );
    }

    #[test]
    fn the_async_client_answers_from_the_same_script() {
        let mut fake = Fake::new().reply(204, &json!(null));
        let response = futures_lite(AsyncHttpClient::send(
            &mut fake,
            &request("https://example.com/a"),
        ));
        assert_eq!(response.unwrap().status, 204);
        futures_lite(AsyncHttpClient::sleep(&fake, Duration::from_secs(60)));
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn a_wait_can_be_noted_rather_than_spent() {
        let mut fake = Fake::new();
        fake.note_wait(Duration::from_millis(1500));
        assert_eq!(fake.waited(), [Duration::from_millis(1500)]);
    }

    /// Poll a future that is ready on the first poll — enough for a
    /// client that never actually waits, and it keeps the tests free of
    /// a runtime.
    fn futures_lite<F: std::future::Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("the fake client is never pending"),
        }
    }
}
