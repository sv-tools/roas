//! What the validator is handed: one HTTP request, borrowed.
//!
//! Every Rust web framework has its own request type, and none of them
//! is the one a validator wants. `http::Request` is generic over a body
//! that is usually a stream; `actix_web::HttpRequest` and
//! `rocket::Request` carry no body at all; and the `http` crate itself
//! is version-split — actix-web 4 is still on `http` 0.2 while hyper 1,
//! axum 0.8 and reqwest are on 1.x, so the two `HeaderMap`s are
//! different types that cannot be passed to the same function.
//!
//! So this crate takes none of them. [`RequestView`] is the small set of
//! things an OpenAPI description actually talks about — a method, a
//! path, a query string, headers and some bytes — and each framework
//! gets a [`ToRequestView`] impl behind its own feature. That keeps the
//! core compiling for every framework at once, and testable with no
//! server at all.
//!
//! The body is bytes, deliberately. A framework body is a stream, and
//! validating one means buffering it; buffering is the caller's
//! decision — how much, and whether at all — so the adapters convert
//! the head and leave [`RequestView::with_body`] to whoever is willing
//! to pay for it.

use std::borrow::Cow;

/// One HTTP request, as much of it as an OpenAPI description describes.
///
/// Built directly, or from a framework's own request type through
/// [`ToRequestView`]:
///
/// ```
/// use roas_http_validator::RequestView;
///
/// let request = RequestView::new("GET", "/pets/7")
///     .with_query("limit=10")
///     .with_header("accept", "application/json");
///
/// assert_eq!(request.header("Accept"), Some("application/json"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestView<'a> {
    /// The HTTP method, as the request carried it.
    ///
    /// Matched case-sensitively, as
    /// [RFC 9110 §9.1](https://www.rfc-editor.org/rfc/rfc9110#section-9.1)
    /// requires of a method token: `GET` finds the `get` a Path Item
    /// Object keys it under, and `get` finds nothing.
    pub method: Cow<'a, str>,

    /// The path, without the query string. Percent-encoding is kept as
    /// it arrived — a `%2F` in a path parameter is not a separator, and
    /// decoding here would make it one.
    pub path: Cow<'a, str>,

    /// The raw query string, without the leading `?`.
    pub query: Option<Cow<'a, str>>,

    /// Headers in the order they arrived, names as written. A header
    /// may repeat, so this is a list rather than a map.
    pub headers: Vec<(Cow<'a, str>, Cow<'a, str>)>,

    /// The body, already buffered.
    ///
    /// `None` means no body was supplied; `Some(&[])` means one was and
    /// it was empty. Validation keeps them apart — an empty JSON body is
    /// malformed where an absent optional one is fine — so a caller that
    /// means "no body" leaves this `None` rather than passing no bytes.
    pub body: Option<Cow<'a, [u8]>>,
}

impl<'a> RequestView<'a> {
    /// A request with a method and a path and nothing else.
    #[must_use]
    pub fn new(method: impl Into<Cow<'a, str>>, path: impl Into<Cow<'a, str>>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            query: None,
            headers: Vec::new(),
            body: None,
        }
    }

    /// Set the query string. A leading `?` is accepted and dropped, so
    /// both halves of `uri.query()` and `"?" + query` work.
    #[must_use]
    pub fn with_query(mut self, query: impl Into<Cow<'a, str>>) -> Self {
        let query = query.into();
        self.query = Some(match query {
            Cow::Borrowed(query) => Cow::Borrowed(query.strip_prefix('?').unwrap_or(query)),
            Cow::Owned(mut query) => {
                if query.starts_with('?') {
                    query.remove(0);
                }
                Cow::Owned(query)
            }
        });
        self
    }

    /// Add one header. Repeating a name adds a second value rather than
    /// replacing the first.
    #[must_use]
    pub fn with_header(
        mut self,
        name: impl Into<Cow<'a, str>>,
        value: impl Into<Cow<'a, str>>,
    ) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Add many headers at once.
    #[must_use]
    pub fn with_headers<N, V>(mut self, headers: impl IntoIterator<Item = (N, V)>) -> Self
    where
        N: Into<Cow<'a, str>>,
        V: Into<Cow<'a, str>>,
    {
        self.headers
            .extend(headers.into_iter().map(|(n, v)| (n.into(), v.into())));
        self
    }

    /// Supply the buffered body.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<Cow<'a, [u8]>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The first value of `name`, matched case-insensitively as
    /// [RFC 9110 §5.1](https://www.rfc-editor.org/rfc/rfc9110#name-field-names)
    /// requires.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_ref())
    }

    /// Every value of `name`, in order.
    pub fn header_values<'s>(&'s self, name: &'s str) -> impl Iterator<Item = &'s str> + 's {
        self.headers
            .iter()
            .filter(move |(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_ref())
    }

    /// The media type from `Content-Type`, lowercased, with any
    /// parameters (`; charset=utf-8`) dropped.
    #[must_use]
    pub fn content_type(&self) -> Option<String> {
        self.header("content-type").map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
    }

    /// The query string decoded into name/value pairs, keeping order
    /// and repeats — `?tag=a&tag=b` is two pairs, not one.
    #[must_use]
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        self.query_pairs_raw()
            .into_iter()
            .map(|(name, value)| (name, decode_form(&value)))
            .collect()
    }

    /// The same pairs with their **values** left encoded.
    ///
    /// Which is what validation needs: a delimiter that arrived
    /// percent-encoded is data, not a separator, so `tags=a%2Cb` must
    /// still be one item when `style` splits it on commas. Names are
    /// decoded, because a name is never split.
    pub(crate) fn query_pairs_raw(&self) -> Vec<(String, String)> {
        self.query.as_deref().map(split_query).unwrap_or_default()
    }

    /// The cookies from the `Cookie` header, in the order sent.
    #[must_use]
    pub fn cookies(&self) -> Vec<(String, String)> {
        self.header_values("cookie")
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| {
                let pair = pair.trim();
                if pair.is_empty() {
                    return None;
                }
                let (name, value) = pair.split_once('=')?;
                Some((name.trim().to_owned(), value.trim().to_owned()))
            })
            .collect()
    }
}

/// A framework's own request type, seen as a [`RequestView`].
///
/// One impl per framework, each behind its own feature. The body is not
/// part of it — see the module documentation for why.
pub trait ToRequestView {
    /// Borrow this request as a [`RequestView`].
    fn request_view(&self) -> RequestView<'_>;
}

/// Split a query string into pairs, decoding the names and leaving the
/// values as they arrived.
pub(crate) fn split_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (decode_form(name), value.to_owned()),
            None => (decode_form(pair), String::new()),
        })
        .collect()
}

/// Percent-decode one form field, lossily: a byte sequence that is not
/// UTF-8 becomes replacement characters rather than an error, because a
/// malformed byte in one parameter should be reported by the schema
/// that parameter is judged against, not by refusing the whole request.
pub(crate) fn decode_form(value: &str) -> String {
    let plus_as_space = value.replace('+', " ");
    percent_encoding::percent_decode_str(&plus_as_space)
        .decode_utf8_lossy()
        .into_owned()
}

/// Percent-decode one path segment. `+` is a literal plus here — the
/// form encoding does not apply to paths.
pub(crate) fn decode_path_segment(segment: &str) -> String {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_is_found_whatever_case_it_was_written_in() {
        let request = RequestView::new("GET", "/").with_header("Content-Type", "application/json");
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.header("CONTENT-TYPE"), Some("application/json"));
        assert_eq!(request.header("accept"), None);
    }

    #[test]
    fn a_repeated_header_keeps_every_value() {
        let request = RequestView::new("GET", "/")
            .with_header("x-tag", "a")
            .with_header("X-Tag", "b");
        assert_eq!(request.header("x-tag"), Some("a"));
        assert_eq!(
            request.header_values("x-tag").collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn the_content_type_drops_its_parameters_and_case() {
        let request = RequestView::new("POST", "/")
            .with_header("content-type", "Application/JSON; charset=utf-8");
        assert_eq!(request.content_type().as_deref(), Some("application/json"));
        assert_eq!(RequestView::new("POST", "/").content_type(), None);
    }

    #[test]
    fn a_query_string_keeps_order_and_repeats() {
        let request = RequestView::new("GET", "/").with_query("tag=a&tag=b&limit=10");
        assert_eq!(
            request.query_pairs(),
            [
                ("tag".to_owned(), "a".to_owned()),
                ("tag".to_owned(), "b".to_owned()),
                ("limit".to_owned(), "10".to_owned()),
            ]
        );
    }

    #[test]
    fn a_leading_question_mark_is_not_part_of_the_query() {
        let borrowed = RequestView::new("GET", "/").with_query("?a=1");
        let owned = RequestView::new("GET", "/").with_query("?a=1".to_owned());
        assert_eq!(borrowed.query.as_deref(), Some("a=1"));
        assert_eq!(owned.query.as_deref(), Some("a=1"));
    }

    #[test]
    fn form_encoding_is_undone_in_the_query() {
        let request = RequestView::new("GET", "/").with_query("q=a+b%20c&flag&empty=");
        assert_eq!(
            request.query_pairs(),
            [
                ("q".to_owned(), "a b c".to_owned()),
                ("flag".to_owned(), String::new()),
                ("empty".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn a_request_with_no_query_has_no_pairs() {
        assert!(RequestView::new("GET", "/").query_pairs().is_empty());
    }

    #[test]
    fn cookies_come_from_the_cookie_header() {
        let request = RequestView::new("GET", "/")
            .with_header("cookie", "session=abc; theme=dark")
            .with_header("Cookie", "extra=1");
        assert_eq!(
            request.cookies(),
            [
                ("session".to_owned(), "abc".to_owned()),
                ("theme".to_owned(), "dark".to_owned()),
                ("extra".to_owned(), "1".to_owned()),
            ]
        );
    }

    #[test]
    fn a_malformed_cookie_pair_is_skipped_rather_than_guessed_at() {
        let request = RequestView::new("GET", "/").with_header("cookie", "novalue; ok=1; ");
        assert_eq!(request.cookies(), [("ok".to_owned(), "1".to_owned())]);
    }

    #[test]
    fn headers_can_be_added_in_bulk() {
        let request = RequestView::new("GET", "/").with_headers([("a", "1"), ("b", "2")]);
        assert_eq!(request.header("b"), Some("2"));
    }

    #[test]
    fn a_body_is_whatever_bytes_the_caller_buffered() {
        let request = RequestView::new("POST", "/").with_body(b"{}".as_slice());
        assert_eq!(request.body.as_deref(), Some(b"{}".as_slice()));
        assert_eq!(RequestView::new("POST", "/").body, None);
    }

    #[test]
    fn raw_pairs_keep_their_values_encoded_so_delimiters_stay_distinguishable() {
        let request = RequestView::new("GET", "/").with_query("tags=a%2Cb&q=x+y");
        assert_eq!(
            request.query_pairs_raw(),
            [
                ("tags".to_owned(), "a%2Cb".to_owned()),
                ("q".to_owned(), "x+y".to_owned()),
            ],
        );
        // The public accessor still hands back what a reader expects.
        assert_eq!(
            request.query_pairs(),
            [
                ("tags".to_owned(), "a,b".to_owned()),
                ("q".to_owned(), "x y".to_owned()),
            ],
        );
    }

    #[test]
    fn a_path_segment_decodes_percent_escapes_but_not_plus() {
        assert_eq!(decode_path_segment("a%20b+c"), "a b+c");
    }
}
