use std::fmt::Display;

use enumset::{EnumSet, EnumSetType};

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub errors: Vec<String>,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{} errors found:", self.errors.len())?;
        for error in &self.errors {
            writeln!(f, "- {}", error)?;
        }
        Ok(())
    }
}

#[derive(EnumSetType, Debug)]
pub enum Options {
    /// Ignore missing tags.
    /// Applies for v2.0, v3.0
    IgnoreMissingTags,

    /// Ignore external references.
    /// Applies for v2.0, v3.0
    IgnoreExternalReferences,

    /// Ignore non-unique operation IDs.
    /// Applies for v2.0, v3.0
    IgnoreNonUniqOperationIDs,

    /// Ignore unused tags.
    /// Applies for v2.0, v3.0
    IgnoreUnusedTags,

    /// Ignore unused schemas (definitions for v2.0).
    /// Applies for v2.0, v3.0
    IgnoreUnusedSchemas,

    /// Ignore unused parameters.
    /// Applies for v2.0, v3.0
    IgnoreUnusedParameters,

    /// Ignore unused responses.
    /// Applies for v2.0, v3.0
    IgnoreUnusedResponses,

    /// Ignore unused security definitions.
    /// Applies for v3.0
    IgnoreUnusedServerVariables,

    /// Ignore unused examples.
    /// Applies for v3.0
    IgnoreUnusedExamples,

    /// Ignore unused request bodies.
    /// Applies for v3.0
    IgnoreUnusedRequestBodies,

    /// Ignore unused headers.
    /// Applies for v3.0
    IgnoreUnusedHeaders,

    /// Ignore unused security schemes.
    /// Applies for v3.0
    IgnoreUnusedSecuritySchemes,

    /// Ignore unused links.
    /// Applies for v3.0
    IgnoreUnusedLinks,

    /// Ignore unused callbacks.
    /// Applies for v3.0
    IgnoreUnusedCallbacks,
}

impl Options {
    /// Create an empty (strict) set of options.
    pub fn new() -> EnumSet<Options> {
        EnumSet::empty()
    }

    /// Create options to ignore unused elements.
    pub fn ignore_unused() -> EnumSet<Options> {
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
    }

    pub fn only(&self) -> EnumSet<Options> {
        EnumSet::only(*self)
    }
}

/// Validate a OpenAPI specification.
pub trait Validate {
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error>;
}
