//! Which key a Path Item Object files a request method under.
//!
//! Two maps hold operations, and they are keyed differently. The eight
//! methods the specification names live in `operations` under lowercase
//! keys — `get`, `post` — while OpenAPI 3.2's `additionalOperations`
//! holds everything else under the method name as written: `COPY`,
//! `LOCK`, `REPORT`.
//!
//! Neither is a case-insensitive match.
//! [RFC 9110 §9.1](https://www.rfc-editor.org/rfc/rfc9110#section-9.1)
//! makes the method token case-sensitive, so `get` is not `GET` — it is
//! a different, unregistered method that no Path Item Object describes.
//! A request carrying it is answered with "no such method here" rather
//! than quietly validated as a `GET`.

/// The lowercase `operations` key for a request method, if that method
/// is one of the standard ones spelled the way HTTP spells it.
///
/// `None` means the request method names no entry in `operations` — it
/// may still name one in `additionalOperations`, which is looked up by
/// the method itself. The two maps are searched separately and never
/// with each other's key, or `get` would find the `get` that stands for
/// `GET`.
pub(crate) fn standard(method: &str) -> Option<String> {
    // The uppercase form is the canonical one, and `operations` is its
    // lowercase transcription. Anything not written in uppercase names
    // no standard method.
    (method == method.to_ascii_uppercase()).then(|| method.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uppercase_standard_method_maps_to_its_lowercase_key() {
        assert_eq!(standard("GET").as_deref(), Some("get"));
        assert_eq!(standard("DELETE").as_deref(), Some("delete"));
    }

    #[test]
    fn a_method_that_is_not_uppercase_names_no_standard_operation() {
        // RFC 9110 §9.1: the method token is case-sensitive, so `get`
        // is not `GET` and matches no `operations` key.
        assert_eq!(standard("get"), None);
        assert_eq!(standard("GeT"), None);
        assert_eq!(standard("Get"), None);
    }

    #[test]
    fn a_non_standard_method_is_left_for_additional_operations() {
        // Uppercase, so it produces a would-be `operations` key — which
        // that map cannot contain, since it holds only the eight the
        // specification names.
        assert_eq!(standard("COPY").as_deref(), Some("copy"));
    }
}
