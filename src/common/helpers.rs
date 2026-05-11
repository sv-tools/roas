use enumset::EnumSet;
use lazy_regex::regex;
use regex::Regex;
use std::collections::HashSet;
use std::fmt;
use thiserror::Error;

use crate::loader::Loader;
use crate::validation::{Error, Options};

/// Allowed character set for OpenAPI component / definition map keys.
/// Mirrors the pattern enforced by `v3_0::Components` / `v3_1::Components`
/// validators. Used by `Spec::define_*` helpers to surface invalid keys
/// at insertion time rather than during a later `validate()` pass.
pub const COMPONENT_NAME_PATTERN: &str = r"^[a-zA-Z0-9.\-_]+$";

/// Returned by `Spec::define_*` helpers when a component name does not
/// match [`COMPONENT_NAME_PATTERN`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Error)]
#[error("component name {name:?} must match pattern `{COMPONENT_NAME_PATTERN}`")]
pub struct InvalidComponentName {
    pub name: String,
}

/// Returns `Ok(())` if `name` matches [`COMPONENT_NAME_PATTERN`],
/// otherwise an [`InvalidComponentName`] error.
pub fn check_component_name(name: &str) -> Result<(), InvalidComponentName> {
    if regex!(r"^[a-zA-Z0-9.\-_]+$").is_match(name) {
        Ok(())
    } else {
        Err(InvalidComponentName {
            name: name.to_owned(),
        })
    }
}

/// ValidateWithContext is a trait for validating an object with a context.
/// It allows the object to be validated with additional context information,
/// such as the specification and validation options.
pub trait ValidateWithContext<T> {
    fn validate_with_context(&self, ctx: &mut Context<T>, path: String);
}

pub struct Context<'a, T> {
    pub spec: &'a T,
    /// Optional external-reference loader. When set, `RefOr::validate_with_context`
    /// resolves non-`#/` `$ref`s through the loader and validates the
    /// fetched value recursively. Defaults to `None`, in which case
    /// external refs surface as `ExternalUnsupported` errors (suppressed
    /// by [`Options::IgnoreExternalReferences`]).
    pub loader: Option<&'a mut Loader>,
    pub visited: HashSet<String>,
    pub errors: Vec<String>,
    pub options: EnumSet<Options>,
}

pub trait PushError<T> {
    fn error(&mut self, path: String, args: T);
}

impl<T> PushError<&str> for Context<'_, T> {
    fn error(&mut self, path: String, msg: &str) {
        if msg.starts_with('.') {
            self.errors.push(format!("{path}{msg}"));
        } else {
            self.errors.push(format!("{path}: {msg}"));
        }
    }
}

impl<T> PushError<String> for Context<'_, T> {
    fn error(&mut self, path: String, msg: String) {
        self.error(path, msg.as_str());
    }
}

impl<T> PushError<fmt::Arguments<'_>> for Context<'_, T> {
    fn error(&mut self, path: String, args: fmt::Arguments<'_>) {
        self.error(path, args.to_string().as_str());
    }
}

impl<T> Context<'_, T> {
    pub fn reset(&mut self) {
        self.visited.clear();
        self.errors.clear();
    }

    pub fn visit(&mut self, path: String) -> bool {
        self.visited.insert(path)
    }

    pub fn is_visited(&self, path: &str) -> bool {
        self.visited.contains(path)
    }

    pub fn is_option(&self, option: Options) -> bool {
        self.options.contains(option)
    }
}

impl Context<'_, ()> {
    pub fn new<T>(spec: &T, options: EnumSet<Options>) -> Context<'_, T> {
        Context {
            spec,
            loader: None,
            visited: HashSet::new(),
            errors: Vec::new(),
            options,
        }
    }
}

impl<'a, T> Context<'a, T> {
    /// Attach an external-reference loader to the context. The loader's
    /// lifetime must outlive the validation pass.
    pub fn with_loader(mut self, loader: &'a mut Loader) -> Self {
        self.loader = Some(loader);
        self
    }
}

impl<'a, T> From<Context<'a, T>> for Result<(), Error> {
    fn from(val: Context<'a, T>) -> Self {
        if val.errors.is_empty() {
            Ok(())
        } else {
            Err(Error { errors: val.errors })
        }
    }
}

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

const HTTP: &str = "http://";
const HTTPS: &str = "https://";

/// Validates an optional URL string.
/// If the URL is present, it checks if it is valid using `validate_required_url`.
/// Records an error in the context if the URL is invalid.
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

#[cfg(test)]
mod tests {
    use super::*;

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
                .contains(&"test_url: must be a valid URL, found `foo-bar`".to_string()),
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
                .contains(&"test_url: must be a valid URL, found `foo-bar`".to_string()),
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
    fn validate_optional_uri_rejects_control_chars_and_whitespace() {
        // Tab, newline, DEL, and other C0 / DEL controls each fail.
        for s in ["bad\turi", "with\nnewline", "with\x01ctl", "with\x7fdel"] {
            let mut ctx = Context::new(&(), Options::new());
            validate_optional_uri(&Some(s.to_owned()), &mut ctx, "u".to_owned());
            assert!(
                ctx.errors.iter().any(|e| e.contains("must be a valid URI")),
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
