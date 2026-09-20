//! The step between parsing and generating, enforced by a type.

use crate::diagnostic::Diagnostic;
use crate::report;
use crate::source::SourceDocument;
use enumset::EnumSet;
use roas::validation::{Options, Validate, ValidationError};
use std::fmt;
use thiserror::Error;

/// Validate a document with `roas`, skipping external references.
///
/// The result is the only way to obtain a [`ValidatedSourceDocument`],
/// which is the only thing generation accepts — so the step cannot be
/// forgotten and cannot be bypassed. `options` is `roas`'s own set;
/// [`Options::IgnoreExternalReferences`] is always added, because the
/// validator would read a fetched document under the root's version.
/// External references are reported by
/// [`ValidatedSourceDocument::diagnostics`] instead.
///
/// On failure the document comes back inside the error, so nothing the
/// caller parsed is lost.
pub fn validate(
    document: SourceDocument,
    options: EnumSet<Options>,
) -> Result<ValidatedSourceDocument, SourceValidationError> {
    let options = options | Options::IgnoreExternalReferences;
    match document.spec().validate(options, None) {
        Ok(()) => Ok(ValidatedSourceDocument { document }),
        Err(error) => Err(SourceValidationError {
            document: Box::new(document),
            errors: error.errors,
        }),
    }
}

/// A [`SourceDocument`] that passed [`validate`].
#[derive(Debug, Clone)]
pub struct ValidatedSourceDocument {
    document: SourceDocument,
}

impl ValidatedSourceDocument {
    /// The document itself.
    pub fn document(&self) -> &SourceDocument {
        &self.document
    }

    /// Give the document back, dropping the proof of validation.
    pub fn into_document(self) -> SourceDocument {
        self.document
    }

    /// Everything the generator has to say before generating: what the
    /// normalization to 3.2 lost, and every reference the first release
    /// refuses to follow. Ordered by where it occurs in the source.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut out = self.document.normalization().to_vec();
        report::reference_restrictions(self.document.raw(), self.document.uri(), &mut out);
        out.sort_by(|a, b| a.pointer.cmp(&b.pointer).then(a.severity.cmp(&b.severity)));
        out
    }
}

impl std::ops::Deref for ValidatedSourceDocument {
    type Target = SourceDocument;

    fn deref(&self) -> &SourceDocument {
        &self.document
    }
}

/// The document failed `roas` validation. The document is returned, not
/// consumed, so it can be inspected or validated again with other
/// options. Boxed, since the whole 3.2 model rides inside.
#[derive(Debug, Error)]
pub struct SourceValidationError {
    pub document: Box<SourceDocument>,
    pub errors: Vec<ValidationError>,
}

impl fmt::Display for SourceValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is not a valid {} description: {} error(s)",
            self.document.uri(),
            self.document.version(),
            self.errors.len()
        )?;
        for error in &self.errors {
            write!(f, "\n- {error}")?;
        }
        Ok(())
    }
}
