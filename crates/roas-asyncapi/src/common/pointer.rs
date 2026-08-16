//! JSON Pointer parsing for document-local `$ref`s.
//!
//! A `$ref` is a URI reference, so a document-local one carries its
//! pointer in the URI *fragment*. Getting that back into a pointer is
//! the reverse of how it was written: a pointer is percent-encoded to
//! become a fragment ([RFC 6901 §6](https://www.rfc-editor.org/rfc/rfc6901.html#section-6)),
//! so a fragment is percent-decoded *whole* to become a pointer again,
//! and only then split on `/` and unescaped per
//! [RFC 6901 §3](https://www.rfc-editor.org/rfc/rfc6901.html#section-3).
//!
//! Order matters, and this is the order that makes `%2F` a separator
//! like any other `/`: a slash *inside* a reference token has to be
//! written `~1`, because once percent-decoding has run there is
//! nothing left to tell the two apart. Every resolver in this crate
//! goes through here rather than splitting strings itself.

/// Percent-decode a URI fragment.
///
/// Returns `None` for a truncated or non-hex escape, or for octets
/// that are not UTF-8.
#[must_use]
fn percent_decode(fragment: &str) -> Option<String> {
    let bytes = fragment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            decoded.push(u8::from_str_radix(fragment.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Unescape one RFC 6901 reference token: `~1` becomes `/`, `~0`
/// becomes `~`.
///
/// Returns `None` for a `~` followed by anything else. RFC 6901 leaves
/// that undefined rather than literal, so a pointer containing one
/// names nothing rather than naming a key that happens to look like it.
#[must_use]
fn unescape(token: &str) -> Option<String> {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c != '~' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('0') => out.push('~'),
            Some('1') => out.push('/'),
            _ => return None,
        }
    }
    Some(out)
}

/// Split a document-local fragment into decoded reference tokens.
///
/// The empty fragment is the whole document, i.e. no tokens. A fragment
/// that does not decode, does not start with `/`, or contains a
/// malformed escape is not a pointer at all.
#[must_use]
pub(crate) fn tokens(fragment: &str) -> Option<Vec<String>> {
    let pointer = percent_decode(fragment)?;
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    pointer
        .strip_prefix('/')?
        .split('/')
        .map(unescape)
        .collect()
}

/// Parse an RFC 6901 array index: ASCII digits, with no leading zero
/// unless the index *is* zero.
#[must_use]
pub(crate) fn array_index(token: &str) -> Option<usize> {
    if token.len() > 1 && token.starts_with('0') {
        return None;
    }
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// Walk a pointer over a document serialized as plain JSON.
///
/// `$ref` is an unrestricted URI reference, so a pointer may legally
/// name something a typed model does not represent as its own kind — a
/// message-shaped `x-` extension, say. The typed resolvers answer
/// "which object is this"; this answers the weaker but broader "is
/// there anything here at all", which is what separates a pointer at an
/// unmodeled location from one that dangles.
#[must_use]
pub(crate) fn walk<'v>(
    value: &'v serde_json::Value,
    path: &[String],
) -> Option<&'v serde_json::Value> {
    let mut current = value;
    for token in path {
        current = match current {
            serde_json::Value::Object(map) => map.get(token)?,
            serde_json::Value::Array(items) => items.get(array_index(token)?)?,
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fragment_is_decoded_whole_and_then_split() {
        // Percent-decoding runs first, so `%2F` is a separator like
        // any other `/` — the pointer it decodes to is `/x-a/b`.
        assert_eq!(tokens("/x-a%2Fb").unwrap(), vec!["x-a", "b"]);
        assert_eq!(
            tokens("/channels/source%2Fpath").unwrap(),
            vec!["channels", "source", "path"]
        );

        // A slash *inside* a token has to be written `~1`, which
        // survives decoding because `~` is not what encodes it.
        assert_eq!(
            tokens("/channels/source~1path").unwrap(),
            vec!["channels", "source/path"]
        );
        assert_eq!(
            tokens("/channels/source%7E1path").unwrap(),
            vec!["channels", "source/path"]
        );

        assert_eq!(tokens("").unwrap(), Vec::<String>::new());
        assert_eq!(tokens("/a%20b").unwrap(), vec!["a b"]);
        assert_eq!(tokens("/a~0b").unwrap(), vec!["a~b"]);
    }

    #[test]
    fn malformed_fragments_are_not_pointers() {
        for bad in [
            "/a~2b", // `~` followed by neither 0 nor 1
            "/a~",   // a trailing `~`
            "/a%2",  // a truncated escape
            "/a%zz", // a non-hex escape
            "/a%FF", // octets that are not UTF-8
            "no-leading-slash",
        ] {
            assert!(tokens(bad).is_none(), "{bad} must not be a pointer");
        }
    }

    #[test]
    fn array_indices_follow_rfc_6901() {
        assert_eq!(array_index("0"), Some(0));
        assert_eq!(array_index("12"), Some(12));
        for bad in ["01", "007", "-1", "1.0", "x", "", " 1"] {
            assert!(array_index(bad).is_none(), "{bad} must not be an index");
        }
    }

    #[test]
    fn walk_steps_through_objects_and_arrays() {
        let document = json!({
            "channels": { "a/b": { "items": [ { "name": "first" }, { "name": "second" } ] } }
        });
        let path = tokens("/channels/a~1b/items/1/name").unwrap();
        assert_eq!(walk(&document, &path), Some(&json!("second")));

        // Missing key, index past the end, a bad index, and stepping
        // into a scalar all decline.
        for pointer in [
            "/channels/ghost",
            "/channels/a~1b/items/9",
            "/channels/a~1b/items/01",
            "/channels/a~1b/items/0/name/deeper",
        ] {
            let path = tokens(pointer).unwrap();
            assert!(walk(&document, &path).is_none(), "{pointer} must not walk");
        }

        // The empty pointer is the document itself.
        assert_eq!(walk(&document, &[]), Some(&document));
    }
}
