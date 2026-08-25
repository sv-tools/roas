//! Internal validation utility functions shared across the per-version
//! validators. The user-facing types (`Context`, `ValidateWithContext`,
//! `PushError`, `InvalidComponentName`, `check_component_name`) live in
//! [`crate::validation`]; this module is `pub(crate)` and holds only
//! crate-internal helpers.

use regex::Regex;
#[cfg(feature = "v2")]
use std::collections::HashSet;

use crate::validation::{Context, Options, PushError, ValidateWithContext};

/// Validates that the given optional email string contains an '@' character.
/// If the email is present and invalid, records an error in the context.
pub fn validate_email<T>(email: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(email) = email
        && !email.contains('@')
    {
        ctx.error(
            path,
            format_args!("must be a valid email address, found `{email}`"),
        );
    }
}

#[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1"))]
const HTTP: &str = "http://";
#[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1"))]
const HTTPS: &str = "https://";

/// Validates an optional URL string.
/// If the URL is present, it checks if it is valid using `validate_required_url`.
/// Records an error in the context if the URL is invalid.
#[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1"))]
pub fn validate_optional_url<T>(url: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(url) = url {
        validate_required_url(url, ctx, path);
    }
}

/// Validates an optional URI reference (RFC 3986). More permissive than
/// `validate_optional_url`: accepts any scheme (`scheme:rest...`), absolute
/// or relative URI references including same-document fragments
/// (`#/foo`) and rootless paths. Used for fields like `jsonSchemaDialect`
/// where JSON Schema dialect URIs may be opaque (e.g.
/// `urn:example:dialect`) and not necessarily HTTP(S).
#[cfg(any(feature = "v3_1", feature = "v3_2"))]
pub fn validate_optional_uri<T>(uri: &Option<String>, ctx: &mut Context<T>, path: String) {
    let Some(uri) = uri else { return };
    if ctx.is_option(Options::IgnoreInvalidUrls) {
        return;
    }
    // Present-but-empty is invalid: the field was set, so it must hold
    // a real URI. (Absent is fine — the caller used `Option`.)
    if uri.is_empty() {
        ctx.error(path, "must be a valid URI, found ``");
        return;
    }
    if has_uri_unsafe_bytes(uri) {
        ctx.error(path, format_args!("must be a valid URI, found `{uri}`"));
    }
}

/// Returns `true` if `s` contains any byte that an RFC 3986 URI cannot
/// hold — ASCII whitespace or a C0/DEL control char. This is the shared
/// "obviously broken" predicate used by `validate_optional_uri` /
/// `validate_required_uri` and by callers (e.g. XML namespace
/// validators) that want to skip a follow-up scheme check when the URI
/// validator has already flagged the value.
#[cfg(any(feature = "v3_1", feature = "v3_2"))]
pub fn has_uri_unsafe_bytes(s: &str) -> bool {
    s.bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

/// Required-URI validator: errors if the value is empty and otherwise
/// enforces the same `validate_optional_uri` rules. Used for fields the
/// OAS 3.2 JSON Schema marks as `format: uri-reference`, where any
/// RFC 3986 URI reference is allowed (including relative refs and
/// non-HTTP schemes). Callers that want to silence empty values for a
/// specific field should gate the call on the relevant `Options::*`
/// toggle themselves — this helper does not bake in any field-specific
/// option.
#[cfg(any(feature = "v3_1", feature = "v3_2"))]
pub fn validate_required_uri<T>(uri: &String, ctx: &mut Context<T>, path: String) {
    validate_required_string(uri, ctx, path.clone());
    if uri.is_empty() || ctx.is_option(Options::IgnoreInvalidUrls) {
        return;
    }
    if has_uri_unsafe_bytes(uri) {
        ctx.error(path, format_args!("must be a valid URI, found `{uri}`"));
    }
}

/// Validates that the given URL string starts with "http://" or "https://".
/// If the URL is invalid, records an error in the context.
#[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1"))]
pub fn validate_required_url<T>(url: &String, ctx: &mut Context<T>, path: String) {
    if !ctx.is_option(Options::IgnoreEmptyExternalDocumentationUrl) {
        validate_required_string(url, ctx, path.clone());
    }
    // If the URL is empty or the ignore option is set, skip validation.
    if url.is_empty() || ctx.is_option(Options::IgnoreInvalidUrls) {
        return;
    }

    // TODO: Consider using a more robust URL validation library.
    if !url.starts_with(HTTP) && !url.starts_with(HTTPS) {
        ctx.error(path, format_args!("must be a valid URL, found `{url}`"));
    }
}

/// Validates that the given string is not empty.
/// If the string is empty, records an error in the context.
pub fn validate_required_string<T>(s: &str, ctx: &mut Context<T>, path: String) {
    if s.is_empty() {
        ctx.error(path, "must not be empty");
    }
}

/// Validates that the given string matches the provided regex pattern.
/// If the string does not match, records an error in the context with details.
pub fn validate_string_matches<T>(s: &str, pattern: &Regex, ctx: &mut Context<T>, path: String) {
    if !pattern.is_match(s) {
        ctx.error(
            path,
            format_args!("must match pattern `{pattern}`, found `{s}`"),
        );
    }
}

// Validates an optional string against a regex pattern if present.
#[cfg(feature = "v2")]
pub fn validate_optional_string_matches<T>(
    s: &Option<String>,
    pattern: &Regex,
    ctx: &mut Context<T>,
    path: String,
) {
    if let Some(s) = s {
        validate_string_matches(s, pattern, ctx, path);
    }
}

/// Validates that the given regex pattern is valid.
/// If the pattern is invalid, records an error in the context with details.
pub fn validate_pattern<T>(pattern: &str, ctx: &mut Context<T>, path: String) {
    match Regex::new(pattern) {
        Ok(_) => {}
        Err(e) => ctx.error(path, format_args!("pattern `{pattern}` is invalid: {e}")),
    }
}

/// Validates that the items in `items` are unique by the key produced by `key`.
/// Records an error for every duplicate (the *later* occurrence) with the index
/// in the source slice. The first occurrence is not flagged.
///
/// Used to enforce `uniqueItems: true` on lists where the schema requires it
/// (schemes, MIME type lists, tags by name, scope arrays, etc.).
#[cfg(feature = "v2")]
pub fn validate_unique_by<T, K, S, F>(items: &[T], ctx: &mut Context<S>, path: String, key: F)
where
    K: Eq + std::hash::Hash,
    F: Fn(&T) -> K,
{
    let mut seen: HashSet<K> = HashSet::new();
    for (i, item) in items.iter().enumerate() {
        if !seen.insert(key(item)) {
            ctx.error(format!("{path}[{i}]"), "duplicate value");
        }
    }
}

/// Validates that the given object has not been visited before,
/// optionally ignoring the check based on the provided option.
/// If the object has already been visited and the ignore option is not set, an error is recorded.
/// Then, the object's own validation logic is invoked.
pub fn validate_not_visited<T, D>(
    obj: &D,
    ctx: &mut Context<T>,
    ignore_option: Options,
    path: String,
) where
    D: ValidateWithContext<T>,
{
    if ctx.visit(path.clone()) {
        if !ctx.is_option(ignore_option) {
            ctx.error(path.clone(), "unused");
        }
        obj.validate_with_context(ctx, path);
    }
}

// Every helper here is gated on the versions that use it, so with no
// version feature there is nothing left to test.
#[cfg(all(
    test,
    any(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2")
))]
mod tests {
    use super::*;
    use crate::validation::ValidationErrorsExt;

    #[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1"))]
    #[test]
    fn test_validate_url() {
        let mut ctx = Context::new(&(), Options::new());
        validate_required_url(
            &String::from("http://example.com"),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_required_url(
            &String::from("https://example.com"),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_required_url(&String::from("foo-bar"), &mut ctx, String::from("test_url"));
        assert!(
            ctx.errors
                .has_exact("test_url: must be a valid URL, found `foo-bar`"),
            "expected error: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&(), Options::only(&Options::IgnoreInvalidUrls));
        validate_required_url(&String::from("foo-bar"), &mut ctx, String::from("test_url"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(&None, &mut ctx, String::from("test_url"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(
            &Some(String::from("http://example.com")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(
            &Some(String::from("https://example.com")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(
            &Some(String::from("foo-bar")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(
            ctx.errors
                .has_exact("test_url: must be a valid URL, found `foo-bar`"),
            "expected error: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&(), Options::only(&Options::IgnoreInvalidUrls));
        validate_optional_url(
            &Some(String::from("foo-bar")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }

    #[test]
    #[cfg(any(feature = "v3_1", feature = "v3_2"))]
    fn validate_optional_uri_ignore_invalid_urls_suppresses_errors() {
        // Exercises the early-return path when IgnoreInvalidUrls is set.
        let mut ctx = Context::new(&(), Options::IgnoreInvalidUrls.only());
        validate_optional_uri(
            &Some("not a valid uri\t".to_owned()),
            &mut ctx,
            "u".to_owned(),
        );
        assert!(
            ctx.errors.is_empty(),
            "IgnoreInvalidUrls must suppress URI errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    #[cfg(any(feature = "v3_1", feature = "v3_2"))]
    fn validate_optional_uri_rejects_control_chars_and_whitespace() {
        // Tab, newline, DEL, and other C0 / DEL controls each fail.
        for s in ["bad\turi", "with\nnewline", "with\x01ctl", "with\x7fdel"] {
            let mut ctx = Context::new(&(), Options::new());
            validate_optional_uri(&Some(s.to_owned()), &mut ctx, "u".to_owned());
            assert!(
                ctx.errors.mentions("must be a valid URI"),
                "expected error for `{s:?}`: {:?}",
                ctx.errors
            );
        }
        // A clean URI passes.
        let mut ctx = Context::new(&(), Options::new());
        validate_optional_uri(
            &Some("urn:example:dialect".to_owned()),
            &mut ctx,
            "u".to_owned(),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }
}

/// Records an error unless `multipleOf` is the positive number every
/// version requires it to be.
///
/// [JSON Schema 2020-12 §6.2.1](https://json-schema.org/draft/2020-12/json-schema-validation#section-6.2.1)
/// for v3.1 and v3.2, and
/// [draft-04 §5.1.1](https://datatracker.ietf.org/doc/html/draft-fge-json-schema-validation-00#section-5.1.1)
/// for v2 and v3.0 — the wording differs, the rule does not.
///
/// Judged on the widest form the number offers rather than always
/// through `f64`: `multipleOf` is kept as a `serde_json::Number` so that
/// an integer step stays an integer, and asking this question should not
/// be the one place that throws that away.
pub fn validate_multiple_of<T>(
    multiple_of: &Option<serde_json::Number>,
    ctx: &mut Context<T>,
    path: String,
) {
    let Some(multiple_of) = multiple_of else {
        return;
    };
    let positive = if let Some(number) = multiple_of.as_u64() {
        number > 0
    } else if let Some(number) = multiple_of.as_i64() {
        number > 0
    } else {
        multiple_of.as_f64().is_some_and(|number| number > 0.0)
    };
    if !positive {
        ctx.error(
            path,
            format_args!("`multipleOf` ({multiple_of}) must be > 0"),
        );
    }
}

#[cfg(test)]
mod multiple_of_tests {
    use super::validate_multiple_of;
    use crate::validation::{Context, Options};

    /// The errors `multipleOf` produced, judged on its own.
    fn errors_for(json: &str) -> Vec<String> {
        let multiple_of = Some(serde_json::from_str(json).expect("the number must parse"));
        let spec = ();
        let mut ctx = Context::new(&spec, Options::new());
        validate_multiple_of(&multiple_of, &mut ctx, "s".to_owned());
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_positive_multiple_of_is_accepted_however_it_was_written() {
        for json in ["1", "0.5", "1e-3", "9007199254740993"] {
            assert!(errors_for(json).is_empty(), "{json} is positive");
        }
    }

    #[test]
    fn zero_and_negatives_are_rejected() {
        for json in ["0", "0.0", "-1", "-0.5"] {
            let found = errors_for(json);
            assert_eq!(found.len(), 1, "{json} is not positive: {found:?}");
            assert!(found[0].contains("multipleOf"), "{found:?}");
        }
    }

    #[test]
    fn an_absent_multiple_of_says_nothing() {
        let spec = ();
        let mut ctx = Context::new(&spec, Options::new());
        validate_multiple_of(&None, &mut ctx, "s".to_owned());
        assert!(ctx.errors.is_empty());
    }
}
