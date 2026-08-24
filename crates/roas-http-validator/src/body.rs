//! Judging the request body against the Request Body Object.
//!
//! The body reaches this crate as bytes, already buffered — see
//! [`crate::request`] for why that is the caller's decision and not
//! this crate's. What is left is choosing which Media Type Object
//! describes those bytes, reading them as that media type, and handing
//! the result to the Schema Object.
//!
//! A media type this crate cannot read is reported as unchecked rather
//! than passed over, so a `multipart/form-data` upload never looks
//! validated because nothing looked at it.

use std::collections::BTreeMap;

use roas::common::reference::RefOr;
use roas::v3_2::media_type::{Encoding, MediaType};
use roas::v3_2::request_body::RequestBody;
use roas::v3_2::schema::{Schema, SingleSchema};
use roas::v3_2::spec::Spec;
use serde_json::Value;

use crate::parameter::is_json;
use crate::report::{ErrorKind, Location, ValidationError};
use crate::request::RequestView;
use crate::schema;

/// Judge the body, appending whatever is wrong to `errors`.
pub(crate) fn validate(
    request_body: &RequestBody,
    request: &RequestView<'_>,
    spec: &Spec,
    errors: &mut Vec<ValidationError>,
) {
    let mut push = |pointer: String, kind: ErrorKind| {
        errors.push(ValidationError {
            location: Location::Body,
            name: String::new(),
            pointer,
            kind,
        });
    };

    let sent = request.content_type();

    // `None` is "no body was supplied"; `Some(&[])` is "a body was
    // supplied and it was empty". Those are different questions and get
    // different answers — an empty JSON body is malformed, an empty
    // `text/plain` body is the empty string, and an empty body whose
    // media type the operation does not describe is still a body of the
    // wrong media type. A caller that means "no body" passes `None`.
    let Some(bytes) = request.body.as_deref() else {
        // `required` defaults to false.
        if request_body.required == Some(true) {
            push(String::new(), ErrorKind::Missing);
        }
        return;
    };

    let Some((media_type, entry)) = select(&request_body.content, sent.as_deref()) else {
        push(
            String::new(),
            ErrorKind::UnexpectedMediaType {
                got: sent,
                expected: request_body.content.keys().cloned().collect(),
            },
        );
        return;
    };

    let entry = match entry.get_item(spec) {
        Ok(entry) => entry,
        Err(error) => {
            push(
                String::new(),
                ErrorKind::UnresolvedReference(error.to_string()),
            );
            return;
        }
    };

    let Some(declared) = &entry.schema else {
        // A Media Type Object without a schema describes nothing to
        // check against, which is not an error.
        return;
    };

    let value = match decode(bytes, &media_type, declared, entry.encoding.as_ref(), spec) {
        Ok(value) => value,
        Err(Decoded::Malformed(why)) => {
            push(String::new(), ErrorKind::Malformed(why));
            return;
        }
        Err(Decoded::Unsupported(what)) => {
            push(String::new(), ErrorKind::Unsupported(what));
            return;
        }
    };

    for failure in schema::check(&value, declared, spec) {
        push(
            failure.pointer,
            match failure.kind {
                schema::FailureKind::Unresolved => ErrorKind::UnresolvedReference(failure.message),
                schema::FailureKind::Unchecked => ErrorKind::Unchecked(failure.message),
                schema::FailureKind::Violated => ErrorKind::Schema(failure.message),
            },
        );
    }
}

/// Why a body could not be turned into a value.
pub(crate) enum Decoded {
    Malformed(String),
    Unsupported(String),
}

/// The Media Type Object that describes what was sent.
///
/// Exact match first, then a `type/*` range, then `*/*` — the order
/// [§4.8.14.1](https://spec.openapis.org/oas/v3.2.0#media-type-object)
/// gives for a more specific key winning over a less specific one.
fn select<'c>(
    content: &'c BTreeMap<String, RefOr<MediaType>>,
    sent: Option<&str>,
) -> Option<(String, &'c RefOr<MediaType>)> {
    let sent = sent?;
    let mut ranges: Vec<(String, &RefOr<MediaType>)> = Vec::new();
    for (key, entry) in content {
        let key_media_type = key
            .split(';')
            .next()
            .unwrap_or(key)
            .trim()
            .to_ascii_lowercase();
        if key_media_type == sent {
            return Some((sent.to_owned(), entry));
        }
        if key_media_type.ends_with("/*") || key_media_type == "*/*" {
            ranges.push((key_media_type, entry));
        }
    }
    // Longest range first, so `text/*` beats `*/*`.
    ranges.sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
    ranges.into_iter().find_map(|(key, entry)| {
        let matches = key == "*/*"
            || sent
                .split_once('/')
                .is_some_and(|(kind, _)| key == format!("{kind}/*"));
        matches.then_some((sent.to_owned(), entry))
    })
}

/// Read the bytes as the media type says they were written.
pub(crate) fn decode(
    bytes: &[u8],
    media_type: &str,
    declared: &RefOr<Schema>,
    encoding: Option<&BTreeMap<String, Encoding>>,
    spec: &Spec,
) -> Result<Value, Decoded> {
    if is_json(media_type) {
        return serde_json::from_slice(bytes)
            .map_err(|error| Decoded::Malformed(format!("invalid JSON: {error}")));
    }

    if media_type == "application/x-www-form-urlencoded" {
        let text = as_text(bytes)?;
        return crate::parameter::read_form_body(
            &text,
            object_properties(declared, spec),
            encoding,
            spec,
        )
        .map_err(Decoded::Malformed);
    }

    // Text is judged as the string it is — a `text/plain` body with a
    // `pattern` is a real thing to check.
    if media_type.starts_with("text/") {
        return Ok(Value::String(as_text(bytes)?));
    }

    Err(Decoded::Unsupported(format!("a {media_type} body")))
}

/// A body as UTF-8 text, or a malformed-body failure.
fn as_text(bytes: &[u8]) -> Result<String, Decoded> {
    String::from_utf8(bytes.to_vec())
        .map_err(|error| Decoded::Malformed(format!("is not UTF-8: {error}")))
}

/// The `properties` of a schema, when it is an object schema — what a
/// form body's fields are coerced through.
fn object_properties<'s>(
    schema: &'s RefOr<Schema>,
    spec: &'s Spec,
) -> Option<&'s BTreeMap<String, RefOr<Schema>>> {
    match schema.get_item(spec).ok()? {
        Schema::Single(single) => match single.as_ref() {
            SingleSchema::Object(object) => object.properties.as_ref(),
            _ => None,
        },
        _ => None,
    }
}
