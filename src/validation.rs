use std::fmt::Display;

use enumset::{EnumSet, EnumSetType};

use crate::validation::Options::{
    IgnoreUnusedDefinitions, IgnoreUnusedParameters, IgnoreUnusedResponses, IgnoreUnusedTags,
};

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
    IgnoreUnusedDefinitions,
    IgnoreUnusedParameters,
    IgnoreUnusedResponses,
}

impl Options {
    pub fn ignore_unused() -> EnumSet<Options> {
        IgnoreUnusedTags | IgnoreUnusedDefinitions | IgnoreUnusedParameters | IgnoreUnusedResponses
    }
}

pub trait Validate {
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error>;
}
