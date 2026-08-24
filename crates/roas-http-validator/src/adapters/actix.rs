//! `actix_web::HttpRequest`.
//!
//! actix-web is the reason this crate does not simply take
//! `http::Request`: actix-http still declares `http = "0.2"`, so its
//! `Method`, `Uri` and `HeaderMap` are different types from the ones
//! hyper 1 and axum 0.8 use, and no single signature accepts both.
//!
//! Nothing from either `http` version appears below — `path()` and
//! `query_string()` hand back plain `&str` — so this adapter is
//! indifferent to which one actix-web is built against.

use std::borrow::Cow;

use crate::request::{RequestView, ToRequestView};

impl ToRequestView for actix_web::HttpRequest {
    fn request_view(&self) -> RequestView<'_> {
        let view = RequestView::new(self.method().as_str(), self.path()).with_headers(
            self.headers().iter().map(|(name, value)| {
                let value = match value.to_str() {
                    Ok(text) => Cow::Borrowed(text),
                    Err(_) => Cow::Owned(String::from_utf8_lossy(value.as_bytes()).into_owned()),
                };
                (Cow::Borrowed(name.as_str()), value)
            }),
        );
        let query = self.query_string();
        if query.is_empty() {
            view
        } else {
            view.with_query(query)
        }
    }
}
