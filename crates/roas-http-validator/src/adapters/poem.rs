//! `poem::Request`.
//!
//! poem keeps its own request type but builds it on the `http` crate's
//! parts, so the conversion is the same shape as the `http` adapter's.

use std::borrow::Cow;

use crate::request::{RequestView, ToRequestView};

impl ToRequestView for poem::Request {
    fn request_view(&self) -> RequestView<'_> {
        let view = RequestView::new(self.method().as_str(), self.uri().path()).with_headers(
            self.headers().iter().map(|(name, value)| {
                let value = match value.to_str() {
                    Ok(text) => Cow::Borrowed(text),
                    Err(_) => Cow::Owned(String::from_utf8_lossy(value.as_bytes()).into_owned()),
                };
                (Cow::Borrowed(name.as_str()), value)
            }),
        );
        match self.uri().query() {
            Some(query) => view.with_query(query),
            None => view,
        }
    }
}
