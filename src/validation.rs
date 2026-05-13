use enumset::{EnumSet, EnumSetType, enum_set};
use lazy_regex::regex;
use std::collections::HashSet;
use std::fmt::{self, Display};
use thiserror::Error as ThisError;

use crate::loader::Loader;

/// A single validation finding. Carries the JSON-Pointer-style path
/// of the failing element and a human-readable message.
///
/// Constructed internally by the recursive validators via
/// [`PushError::error`]; emitted to callers as the elements of
/// [`Error::errors`]. The [`Display`] impl renders as
/// `"<path>: <message>"`, except messages beginning with `.` are
/// concatenated directly onto the path (so callers can use a leading
/// dot to keep the path-extension form like `"#.info.title"` from a
/// child message of `".title: must not be empty"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl ValidationError {
    pub(crate) fn new(path: String, message: String) -> Self {
        Self { path, message }
    }

    /// Returns `true` if `needle` appears in either the path or the
    /// message. Convenience for tests that previously did
    /// `errors.contains(...)` against the old
    /// `Vec<String>` shape.
    pub fn contains(&self, needle: &str) -> bool {
        self.path.contains(needle) || self.message.contains(needle)
    }
}

// Convenience equality with the rendered string form, so existing tests
// that compare a `ValidationError` against a literal like
// `"#.info.title: must not be empty"` keep working without rewriting
// every assertion. The comparison is byte-wise across the joined form
// `<path><sep><message>` (sep = `""` when the message starts with `.`,
// otherwise `": "`), avoiding an allocation per call.
impl PartialEq<str> for ValidationError {
    fn eq(&self, other: &str) -> bool {
        let (sep, rest) = if let Some(rest) = self.message.strip_prefix('.') {
            (".", rest)
        } else {
            (": ", self.message.as_str())
        };
        let plen = self.path.len();
        let slen = sep.len();
        other.len() == plen + slen + rest.len()
            && other.starts_with(&self.path)
            && other[plen..].starts_with(sep)
            && other[plen + slen..] == *rest
    }
}

impl PartialEq<&str> for ValidationError {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<String> for ValidationError {
    fn eq(&self, other: &String) -> bool {
        self == other.as_str()
    }
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(rest) = self.message.strip_prefix('.') {
            write!(f, "{}.{}", self.path, rest)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub errors: Vec<ValidationError>,
}

/// Convenience predicates over a collection of [`ValidationError`]s.
///
/// Implemented for both [`Error`] (which owns the full report after
/// [`Validate::validate`] returns) and `Vec<ValidationError>` (which
/// the in-progress recursive validators accumulate as `ctx.errors`).
/// Lets callers and tests query a report with a single method call
/// instead of repeating `iter().any(|e| ...)`.
///
/// Method names are `mentions` / `has_exact` rather than
/// `contains` / `contains_exact` to avoid clashing with the inherent
/// `slice::contains(&T)` method on `Vec<ValidationError>`.
pub trait ValidationErrorsExt {
    /// Returns `true` if any error mentions `needle` in its `path` or
    /// `message` (via [`ValidationError::contains`]).
    fn mentions(&self, needle: &str) -> bool;

    /// Returns `true` if any error renders to exactly `expected`
    /// (matching the `Display` form, e.g.
    /// `"#.info.title: must not be empty"`).
    fn has_exact(&self, expected: &str) -> bool;
}

impl ValidationErrorsExt for [ValidationError] {
    fn mentions(&self, needle: &str) -> bool {
        self.iter().any(|e| e.contains(needle))
    }

    fn has_exact(&self, expected: &str) -> bool {
        self.iter().any(|e| *e == expected)
    }
}

impl ValidationErrorsExt for Error {
    fn mentions(&self, needle: &str) -> bool {
        self.errors.mentions(needle)
    }

    fn has_exact(&self, expected: &str) -> bool {
        self.errors.has_exact(expected)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{} errors found:", self.errors.len())?;
        for error in &self.errors {
            writeln!(f, "- {error}")?;
        }
        Ok(())
    }
}

#[derive(EnumSetType, Debug)]
pub enum Options {
    /// Ignore missing tags.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreMissingTags,

    /// Ignore external references.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreExternalReferences,

    /// Ignore invalid URLs.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreInvalidUrls,

    /// Ignore non-unique operation IDs.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreNonUniqOperationIDs,

    /// Ignore unused path items.
    /// Applies for v3.1
    IgnoreUnusedPathItems,

    /// Ignore unused tags.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreUnusedTags,

    /// Ignore unused schemas (definitions for v2.0).
    /// Applies for v2.0, v3.0, v3.1
    IgnoreUnusedSchemas,

    /// Ignore unused parameters.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreUnusedParameters,

    /// Ignore unused responses.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreUnusedResponses,

    /// Ignore unused server variables.
    /// Applies for v3.0, v3.1
    IgnoreUnusedServerVariables,

    /// Ignore unused examples.
    /// Applies for v3.0, v3.1
    IgnoreUnusedExamples,

    /// Ignore unused request bodies.
    /// Applies for v3.0, v3.1
    IgnoreUnusedRequestBodies,

    /// Ignore unused headers.
    /// Applies for v3.0, v3.1
    IgnoreUnusedHeaders,

    /// Ignore unused security schemes.
    /// Applies for v3.0, v3.1
    IgnoreUnusedSecuritySchemes,

    /// Ignore unused links.
    /// Applies for v3.0, v3.1
    IgnoreUnusedLinks,

    /// Ignore unused callbacks.
    /// Applies for v3.0, v3.1
    IgnoreUnusedCallbacks,

    /// Ignore unused media types (added in OAS 3.2).
    /// Applies for v3.2
    IgnoreUnusedMediaTypes,

    /// Ignore empty Info.Title field.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreEmptyInfoTitle,

    /// Ignore empty Info.Version field.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreEmptyInfoVersion,

    /// Ignore empty Response.Description field.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreEmptyResponseDescription,

    /// Ignore empty ExternalDocumentation.URL field.
    /// Applies for v2.0, v3.0, v3.1
    IgnoreEmptyExternalDocumentationUrl,
}

/// A set of options to ignore unused objects.
pub const IGNORE_UNUSED: EnumSet<Options> = enum_set!(
    Options::IgnoreUnusedTags
        | Options::IgnoreUnusedSchemas
        | Options::IgnoreUnusedParameters
        | Options::IgnoreUnusedResponses
        | Options::IgnoreUnusedServerVariables
        | Options::IgnoreUnusedExamples
        | Options::IgnoreUnusedRequestBodies
        | Options::IgnoreUnusedHeaders
        | Options::IgnoreUnusedSecuritySchemes
        | Options::IgnoreUnusedLinks
        | Options::IgnoreUnusedCallbacks
        | Options::IgnoreUnusedMediaTypes
);

/// A predefined set of options to ignore required fields that are empty.
pub const IGNORE_EMPTY_REQUIRED_FIELDS: EnumSet<Options> = enum_set!(
    Options::IgnoreEmptyInfoTitle
        | Options::IgnoreEmptyInfoVersion
        | Options::IgnoreEmptyResponseDescription
        | Options::IgnoreEmptyExternalDocumentationUrl
);

impl Options {
    /// Creates a new set of options.
    /// By default, it includes `IgnoreUnusedPathItems` to allow for more lenient validation.
    pub fn new() -> EnumSet<Options> {
        EnumSet::empty() | Options::IgnoreUnusedPathItems
    }

    /// Creates an empty set of options, representing the strictest validation.
    pub fn empty() -> EnumSet<Options> {
        EnumSet::empty()
    }

    /// Creates a set containing only given option.
    pub fn only(&self) -> EnumSet<Options> {
        EnumSet::only(*self)
    }
}

/// Validates an OpenAPI specification.
///
/// # Parameters
///
/// - `options`: per-call validation toggles (see [`Options`] and the
///   [`IGNORE_UNUSED`] / [`IGNORE_EMPTY_REQUIRED_FIELDS`] presets).
/// - `loader`: optional external-reference loader. Controls how
///   non-`#/` `$ref`s are handled:
///   - `None` — external refs surface as a "not supported"
///     validation error unless [`Options::IgnoreExternalReferences`]
///     is set, in which case they're skipped silently.
///   - `Some(&mut Loader)` — each external `$ref` is fetched via the
///     loader (with whichever fetchers the caller registered, e.g.
///     [`JsonFileFetcher`](crate::loader::JsonFileFetcher) for the
///     `file://` scheme), deserialized into the appropriate component
///     type, and walked recursively as if it were inline. Fetch /
///     parse / pointer failures become validation errors with the
///     underlying `LoaderError` as the source. The loader caches
///     resources by URI, so the same external document is fetched
///     once per validation pass even when many `$ref`s target it.
///   - [`Options::IgnoreExternalReferences`] short-circuits before
///     the loader is consulted, so attaching a loader to a spec with
///     broken externals never surfaces those breaks when the option
///     is set.
///
/// Returns `Ok(())` if no errors were collected, or `Err(Error)`
/// with the accumulated messages otherwise. The pass batches errors
/// rather than failing fast.
pub trait Validate {
    fn validate(&self, options: EnumSet<Options>, loader: Option<&mut Loader>)
    -> Result<(), Error>;
}

/// Returned by `Spec::define_*` helpers when a component name does not
/// match `^[a-zA-Z0-9.\-_]+$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, ThisError)]
#[error("component name {name:?} must match pattern `^[a-zA-Z0-9.\\-_]+$`")]
pub struct InvalidComponentName {
    pub name: String,
}

/// Returns `Ok(())` if `name` matches `^[a-zA-Z0-9.\-_]+$`, otherwise
/// an [`InvalidComponentName`] error.
pub fn check_component_name(name: &str) -> Result<(), InvalidComponentName> {
    let r = regex!(r"^[a-zA-Z0-9.\-_]+$");
    if r.is_match(name) {
        Ok(())
    } else {
        Err(InvalidComponentName {
            name: name.to_owned(),
        })
    }
}

/// Trait for validating an object with a [`Context`].
///
/// Crate-internal: implemented by every component type that participates
/// in spec validation. Implementors push errors into the context via
/// [`PushError::error`] and recurse into sub-objects by calling each
/// child's `validate_with_context`. External users drive validation
/// through [`Validate::validate`] rather than touching this trait
/// directly.
pub(crate) trait ValidateWithContext<T> {
    fn validate_with_context(&self, ctx: &mut Context<T>, path: String);
}

/// Validation context — carries the spec being validated, accumulated
/// errors, the per-call options, and a `visited` set used for unused
/// detection and cycle handling.
///
/// Crate-internal: constructed and consumed by [`Validate::validate`].
/// Not part of the public API.
pub(crate) struct Context<'a, T> {
    pub spec: &'a T,
    /// Optional external-reference loader. When set,
    /// [`RefOr::validate_with_context`](crate::common::reference::RefOr::validate_with_context)
    /// resolves non-`#/` `$ref`s through the loader and validates the
    /// fetched value recursively. Defaults to `None`, in which case
    /// external refs surface as `ExternalUnsupported` errors (suppressed
    /// by [`Options::IgnoreExternalReferences`]).
    pub loader: Option<&'a mut Loader>,
    pub visited: HashSet<String>,
    pub errors: Vec<ValidationError>,
    pub options: EnumSet<Options>,
}

/// Generic "push an error message into a [`Context`]" trait. The blanket
/// impls accept `&str`, `String`, and `fmt::Arguments`, so callers can
/// `ctx.error(path, "literal")`, `ctx.error(path, format!(...))`, or
/// `ctx.error(path, format_args!(...))` interchangeably.
///
/// Crate-internal — paired with [`Context`].
pub(crate) trait PushError<T> {
    fn error(&mut self, path: String, args: T);
}

impl<T> PushError<&str> for Context<'_, T> {
    fn error(&mut self, path: String, msg: &str) {
        self.errors.push(ValidationError::new(path, msg.to_owned()));
    }
}

impl<T> PushError<String> for Context<'_, T> {
    fn error(&mut self, path: String, msg: String) {
        self.errors.push(ValidationError::new(path, msg));
    }
}

impl<T> PushError<fmt::Arguments<'_>> for Context<'_, T> {
    fn error(&mut self, path: String, args: fmt::Arguments<'_>) {
        self.errors
            .push(ValidationError::new(path, args.to_string()));
    }
}

impl<T> Context<'_, T> {
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
    pub fn new<'a, T>(spec: &'a T, options: EnumSet<Options>) -> Context<'a, T> {
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

// Manual `Debug` impl: `&mut Loader` itself isn't `Debug` (it holds
// boxed `dyn` fetcher trait objects), so the derive doesn't apply.
// Print the loader as a marker only — the field is rarely useful in
// log/test output and the visited/errors/options state is what callers
// actually want to inspect.
impl<T: fmt::Debug> fmt::Debug for Context<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("spec", &self.spec)
            .field(
                "loader",
                &if self.loader.is_some() {
                    "Some(<loader>)"
                } else {
                    "None"
                },
            )
            .field("visited", &self.visited)
            .field("errors", &self.errors)
            .field("options", &self.options)
            .finish()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_formats_with_count_and_bulleted_messages() {
        let err = Error {
            errors: vec![
                ValidationError::new("#.a".into(), "first".into()),
                ValidationError::new("#.b".into(), "second".into()),
            ],
        };
        assert_eq!(
            format!("{err}"),
            "2 errors found:\n- #.a: first\n- #.b: second\n"
        );
    }

    #[test]
    fn error_display_zero_errors_still_renders_header() {
        let err = Error { errors: vec![] };
        assert_eq!(format!("{err}"), "0 errors found:\n");
    }

    #[test]
    fn check_component_name_accepts_pattern_and_rejects_others() {
        assert!(check_component_name("Foo.Bar-1_2").is_ok());
        let err = check_component_name("has space").unwrap_err();
        assert_eq!(err.name, "has space");
        assert!(err.to_string().contains("has space"));
        // The Display includes the literal pattern so callers can fix
        // their input without consulting the source.
        assert!(err.to_string().contains("a-zA-Z0-9.\\-_"));
    }

    #[test]
    fn context_with_loader_attaches_loader() {
        let mut loader = Loader::new();
        let ctx = Context::new(&(), Options::new()).with_loader(&mut loader);
        assert!(ctx.loader.is_some());
    }

    #[test]
    fn context_debug_marks_loader_presence_without_printing_it() {
        let ctx: Context<()> = Context::new(&(), Options::new());
        let s = format!("{ctx:?}");
        assert!(s.contains("Context"), "debug includes type name: {s}");
        assert!(
            s.contains("None"),
            "no-loader Context debug must say `None`: {s}"
        );

        let mut loader = Loader::new();
        let ctx = Context::new(&(), Options::new()).with_loader(&mut loader);
        let s = format!("{ctx:?}");
        assert!(
            s.contains("Some(<loader>)"),
            "attached-loader Context debug must mark presence: {s}"
        );
    }

    #[test]
    fn context_from_returns_ok_when_empty_err_when_not() {
        let ctx: Context<()> = Context::new(&(), Options::new());
        let r: Result<(), Error> = ctx.into();
        assert!(r.is_ok());

        let mut ctx: Context<()> = Context::new(&(), Options::new());
        ctx.error("#".into(), "kaboom");
        let r: Result<(), Error> = ctx.into();
        let err = r.unwrap_err();
        assert!(err.has_exact("#: kaboom"));
    }

    #[test]
    fn push_error_routes_dot_prefixed_messages_without_separator() {
        let mut ctx: Context<()> = Context::new(&(), Options::new());
        ctx.error("#.foo".into(), ".bar: must not be empty");
        ctx.error("#.baz".into(), "must match pattern");
        assert_eq!(
            ctx.errors,
            vec![
                "#.foo.bar: must not be empty".to_string(),
                "#.baz: must match pattern".to_string(),
            ]
        );
    }

    #[test]
    fn push_error_accepts_string_and_format_args() {
        let mut ctx: Context<()> = Context::new(&(), Options::new());
        ctx.error("#.a".into(), String::from("from string"));
        ctx.error("#.b".into(), format_args!("from {} args", "format"));
        assert_eq!(
            ctx.errors,
            vec![
                "#.a: from string".to_string(),
                "#.b: from format args".to_string(),
            ]
        );
    }
}
