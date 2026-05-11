use enumset::{EnumSet, EnumSetType, enum_set};
use lazy_regex::regex;
use std::collections::HashSet;
use std::fmt::{self, Display};
use thiserror::Error as ThisError;

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub errors: Vec<String>,
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

/// Validates the OpenAPI specification.
pub trait Validate {
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error>;
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
/// Implemented by every component type that participates in spec
/// validation. Implementors push errors into the context via
/// [`PushError::error`] and recurse into sub-objects by calling each
/// child's `validate_with_context`.
pub trait ValidateWithContext<T> {
    fn validate_with_context(&self, ctx: &mut Context<T>, path: String);
}

/// Validation context — carries the spec being validated, accumulated
/// errors, the per-call options, and a `visited` set used for unused
/// detection and cycle handling.
#[derive(Debug, Clone, PartialEq)]
pub struct Context<'a, T> {
    pub spec: &'a T,
    pub visited: HashSet<String>,
    pub errors: Vec<String>,
    pub options: EnumSet<Options>,
}

/// Generic "push an error message into a [`Context`]" trait. The blanket
/// impls accept `&str`, `String`, and `fmt::Arguments`, so callers can
/// `ctx.error(path, "literal")`, `ctx.error(path, format!(...))`, or
/// `ctx.error(path, format_args!(...))` interchangeably.
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
    pub fn new<'a, T>(spec: &'a T, options: EnumSet<Options>) -> Context<'a, T> {
        Context {
            spec,
            visited: HashSet::new(),
            errors: Vec::new(),
            options,
        }
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
