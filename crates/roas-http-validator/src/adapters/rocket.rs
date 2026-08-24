//! `rocket::Request`.
//!
//! Rocket is the outlier: it shares nothing with the `http` crate — its
//! `Method`, `Origin` and `HeaderMap` are its own — and its header map
//! yields headers by value rather than by reference, so the names and
//! values here are owned rather than borrowed. That is what
//! [`RequestView`]'s `Cow` fields are for.

use std::borrow::Cow;

use crate::request::{RequestView, ToRequestView};

impl ToRequestView for rocket::Request<'_> {
    fn request_view(&self) -> RequestView<'_> {
        let view = RequestView::new(self.method().as_str(), self.uri().path().as_str())
            .with_headers(self.headers().iter().map(|header| {
                (
                    Cow::Owned(header.name().as_str().to_owned()),
                    Cow::Owned(header.value().to_owned()),
                )
            }));
        match self.uri().query() {
            Some(query) => view.with_query(query.as_str().to_owned()),
            None => view,
        }
    }
}
