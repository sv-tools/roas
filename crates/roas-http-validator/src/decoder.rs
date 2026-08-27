//! Reading a media type this crate does not know how to read.
//!
//! The built-in decoders cover JSON, `application/x-www-form-urlencoded`
//! and `text/*`. Everything else — `multipart/form-data`, XML, a
//! protobuf-over-HTTP body — reports [`ErrorKind::Unsupported`] rather
//! than guessing, which is honest but not much use to someone who has
//! such a body and a schema for it.
//!
//! This is the way in. A caller registers a function that turns bytes
//! into a [`Value`], and the schema takes it from there.
//!
//! It is deliberately a hook rather than more built-ins, for two
//! reasons that differ by format. **Multipart** would mean owning a
//! boundary parser and buffering file uploads, which is the one place
//! this crate's "the caller decides what to buffer" posture matters
//! most. **XML** has no specified mapping onto a JSON Schema instance
//! at all — OpenAPI's XML Object is serialization metadata for code
//! generators, so any translation is a choice, and implementations make
//! different ones. Better to take the caller's choice than to invent
//! one and report violations against it.
//!
//! ```
//! use roas_http_validator::Options;
//!
//! let options = Options::new().decoder("application/xml", |bytes, _media_type| {
//!     let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
//!     // Whatever mapping your clients actually use.
//!     Ok(serde_json::json!({ "raw": text }))
//! });
//! ```
//!
//! [`ErrorKind::Unsupported`]: crate::ErrorKind::Unsupported

use std::sync::Arc;

use serde_json::Value;

/// Turns a body of one media type into the value its schema judges.
///
/// Called with the bytes and the media type they arrived as — the
/// latter matters when one decoder is registered for a range like
/// `text/*`. An `Err` is reported as a malformed body, carrying the
/// reason given.
pub type Decoder = Arc<dyn Fn(&[u8], &str) -> Result<Value, String> + Send + Sync>;

/// The decoders one validator was given, looked up the way a Media Type
/// Object is: exact match first, then a `type/*` range, then `*/*`.
#[derive(Clone, Default)]
pub(crate) struct Decoders {
    entries: Vec<(String, Decoder)>,
}

impl Decoders {
    /// Register `decoder` for `media_type`, replacing any already there.
    pub(crate) fn insert(&mut self, media_type: &str, decoder: Decoder) {
        let key = normalize(media_type);
        match self.entries.iter_mut().find(|(known, _)| *known == key) {
            Some(entry) => entry.1 = decoder,
            None => self.entries.push((key, decoder)),
        }
    }

    /// The media types registered, for `Debug`.
    pub(crate) fn media_types(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .map(|(media_type, _)| media_type.as_str())
    }

    /// The decoder for `media_type`, most specific first.
    pub(crate) fn find(&self, media_type: &str) -> Option<&Decoder> {
        let media_type = normalize(media_type);
        let range = media_type
            .split_once('/')
            .map(|(kind, _)| format!("{kind}/*"));

        let mut best: Option<(usize, &Decoder)> = None;
        for (key, decoder) in &self.entries {
            // Lower is more specific, so the exact key always wins.
            let rank = if *key == media_type {
                0
            } else if range.as_ref() == Some(key) {
                1
            } else if key == "*/*" {
                2
            } else {
                continue;
            };
            if best.is_none_or(|(known, _)| rank < known) {
                best = Some((rank, decoder));
            }
        }
        best.map(|(_, decoder)| decoder)
    }
}

/// A media type without its parameters, lowercased — the same shape
/// [`crate::RequestView::content_type`] produces.
fn normalize(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoder(tag: &'static str) -> Decoder {
        Arc::new(move |_bytes: &[u8], _media_type: &str| Ok(Value::String(tag.to_owned())))
    }

    fn found(decoders: &Decoders, media_type: &str) -> Option<String> {
        let decoder = decoders.find(media_type)?;
        match decoder(b"", media_type) {
            Ok(Value::String(tag)) => Some(tag),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn an_exact_media_type_is_found() {
        let mut decoders = Decoders::default();
        decoders.insert("application/xml", decoder("xml"));
        assert_eq!(found(&decoders, "application/xml").as_deref(), Some("xml"));
        assert_eq!(found(&decoders, "application/json"), None);
    }

    #[test]
    fn parameters_and_case_do_not_hide_a_decoder() {
        let mut decoders = Decoders::default();
        decoders.insert("Application/XML; charset=utf-8", decoder("xml"));
        assert_eq!(found(&decoders, "application/xml").as_deref(), Some("xml"));
    }

    #[test]
    fn a_range_catches_what_no_exact_key_does() {
        let mut decoders = Decoders::default();
        decoders.insert("text/*", decoder("range"));
        decoders.insert("*/*", decoder("any"));
        assert_eq!(found(&decoders, "text/csv").as_deref(), Some("range"));
        assert_eq!(found(&decoders, "image/png").as_deref(), Some("any"));
    }

    #[test]
    fn the_most_specific_registration_wins() {
        let mut decoders = Decoders::default();
        decoders.insert("*/*", decoder("any"));
        decoders.insert("text/*", decoder("range"));
        decoders.insert("text/csv", decoder("exact"));
        assert_eq!(found(&decoders, "text/csv").as_deref(), Some("exact"));
        assert_eq!(found(&decoders, "text/plain").as_deref(), Some("range"));
    }

    #[test]
    fn registering_the_same_media_type_twice_replaces_it() {
        let mut decoders = Decoders::default();
        decoders.insert("text/csv", decoder("first"));
        decoders.insert("text/csv", decoder("second"));
        assert_eq!(found(&decoders, "text/csv").as_deref(), Some("second"));
        assert_eq!(decoders.media_types().count(), 1);
    }

    #[test]
    fn an_empty_registry_finds_nothing() {
        assert!(Decoders::default().find("text/csv").is_none());
    }
}
