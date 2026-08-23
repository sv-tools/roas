//! `http::Request` — and so axum, warp, tonic, hyper and reqwest.
//!
//! This is the adapter that covers most of the ecosystem at once.
//! `axum::extract::Request` *is* `http::Request<axum::body::Body>`, a
//! `tower::Service` is `Service<http::Request<B>>`, and warp, tonic and
//! hyper all speak the same type — so one impl serves them all.
//!
//! Both halves are covered: [`http::Request`] itself, and
//! [`http::request::Parts`] for the middleware that has already split
//! the request to get at its body.

use std::borrow::Cow;

use http::header::HeaderMap;
use http::{Method, Uri};

use crate::request::{RequestView, ToRequestView};

impl ToRequestView for http::request::Parts {
    fn request_view(&self) -> RequestView<'_> {
        view(&self.method, &self.uri, &self.headers)
    }
}

impl<B> ToRequestView for http::Request<B> {
    fn request_view(&self) -> RequestView<'_> {
        view(self.method(), self.uri(), self.headers())
    }
}

fn view<'r>(method: &'r Method, uri: &'r Uri, headers: &'r HeaderMap) -> RequestView<'r> {
    let view = RequestView::new(method.as_str(), uri.path()).with_headers(headers.iter().map(
        |(name, value)| {
            let value = match value.to_str() {
                Ok(text) => Cow::Borrowed(text),
                // A header that is not UTF-8 is still a header. Dropping
                // it would make a required one look absent, so it is
                // carried through lossily and judged as what arrived.
                Err(_) => Cow::Owned(String::from_utf8_lossy(value.as_bytes()).into_owned()),
            };
            (Cow::Borrowed(name.as_str()), value)
        },
    ));
    match uri.query() {
        Some(query) => view.with_query(query),
        None => view,
    }
}
