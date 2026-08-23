//! `reqwest::Request` and `reqwest::blocking::Request`.
//!
//! The one adapter on the client's side of the exchange. Validating an
//! *outgoing* request is what a spec-first test suite wants — the
//! question "does the call I am about to make match the description?"
//! is the one the Java ecosystem's validator is mostly used to answer —
//! and it is not the same type as an incoming `http::Request`, since
//! reqwest keeps a parsed `Url` rather than a `Uri`.
//!
//! This is also the one adapter that supplies the body. Everywhere else
//! a body is a stream and buffering it is the caller's decision; a
//! reqwest body that is not a stream is already bytes in memory, so
//! there is nothing to buffer and nothing to decide. A streaming body
//! still arrives as `None`, and [`RequestView::with_body`] takes over.

use std::borrow::Cow;

use crate::request::{RequestView, ToRequestView};

impl ToRequestView for reqwest::Request {
    fn request_view(&self) -> RequestView<'_> {
        view(
            self.method().as_str(),
            self.url(),
            self.headers(),
            self.body().and_then(reqwest::Body::as_bytes),
        )
    }
}

impl ToRequestView for reqwest::blocking::Request {
    fn request_view(&self) -> RequestView<'_> {
        view(
            self.method().as_str(),
            self.url(),
            self.headers(),
            self.body().and_then(reqwest::blocking::Body::as_bytes),
        )
    }
}

fn view<'r>(
    method: &'r str,
    url: &'r reqwest::Url,
    headers: &'r reqwest::header::HeaderMap,
    body: Option<&'r [u8]>,
) -> RequestView<'r> {
    let mut view =
        RequestView::new(method, url.path()).with_headers(headers.iter().map(|(name, value)| {
            let value = match value.to_str() {
                Ok(text) => Cow::Borrowed(text),
                Err(_) => Cow::Owned(String::from_utf8_lossy(value.as_bytes()).into_owned()),
            };
            (Cow::Borrowed(name.as_str()), value)
        }));
    if let Some(query) = url.query() {
        view = view.with_query(query);
    }
    match body {
        Some(body) => view.with_body(body),
        None => view,
    }
}
