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
    IgnoreMissingTags,
    IgnoreExternalReferences,
    IgnoreNonUniqOperationIDs,
    IgnoreUnusedTags,
    IgnoreUnusedSchemas,
    IgnoreUnusedParameters,
    IgnoreUnusedResponses,
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
    }
}

pub trait Validate {
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error>;
}
