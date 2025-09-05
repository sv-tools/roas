use enumset::{EnumSet, EnumSetType, enum_set};
use std::fmt::Display;

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
