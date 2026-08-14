//! JSON Pointer parsing for document-local `$ref`s.
//!
//! A `$ref` is a URI reference, so a document-local one carries its
//! pointer in the URI *fragment*: percent-encoded per
//! [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986.html) around the
//! pointer's own escapes from
//! [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901.html). Getting the
//! order right matters — `%2F` is a literal `/` *inside* one reference
//! token, while a raw `/` separates tokens — so every resolver in this
//! crate goes through here rather than splitting strings itself.

/// Decode one RFC 6901 reference token.
///
/// Percent escapes are applied first, then `~1` becomes `/` and `~0`
/// becomes `~`.
///
/// Returns `None` for a malformed token: a truncated or non-hex `%`
/// escape, or a `~` not followed by `0` or `1`. RFC 6901 leaves those
/// undefined rather than literal, so a pointer containing one names
/// nothing rather than naming a key that happens to look like it.
#[must_use]
pub(crate) fn decode_token(token: &str) -> Option<String> {
    let bytes = token.as_bytes();
    let mut percent_decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = token.get(i + 1..i + 3)?;
            percent_decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            percent_decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(percent_decoded).ok()?;

    let mut out = String::with_capacity(decoded.len());
    let mut chars = decoded.chars();
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
/// that does not start with `/`, or that contains a malformed token, is
/// not a pointer at all.
#[must_use]
pub(crate) fn tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    pointer
        .strip_prefix('/')?
        .split('/')
        .map(decode_token)
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
    fn tokens_are_percent_decoded_then_unescaped() {
        assert_eq!(
            tokens("/channels/source%2Fpath").unwrap(),
            vec!["channels", "source/path"],
            "`%2F` is a literal `/` inside one token",
        );
        assert_eq!(
            tokens("/channels/source~1path").unwrap(),
            vec!["channels", "source/path"],
        );
        assert_eq!(tokens("").unwrap(), Vec::<String>::new());
        assert_eq!(decode_token("a%20b").unwrap(), "a b");
        assert_eq!(decode_token("a~0b").unwrap(), "a~b");
    }

    #[test]
    fn malformed_tokens_are_not_literal() {
        for bad in ["a~2b", "a~", "a%2", "a%zz"] {
            assert!(decode_token(bad).is_none(), "{bad} must not decode");
        }
        assert!(tokens("/channels/bad~2escape").is_none());
        assert!(tokens("no-leading-slash").is_none());
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
