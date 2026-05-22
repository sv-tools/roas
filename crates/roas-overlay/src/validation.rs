//! Validation framework for Overlay documents.
//!
//! Modeled after [`roas::validation`]: a public [`Validate`] trait
//! drives a recursive descent through a per-component crate-internal
//! trait; each component pushes diagnostics into a context keyed by
//! a JSONPath-flavor path string. Errors collect rather than fail
//! fast.
//!
//! [`roas::validation`]: https://docs.rs/roas/latest/roas/validation/index.html

use enumset::{EnumSet, EnumSetType};
use std::fmt::{self, Display};

/// A single validation finding.
///
/// `path` is a human-readable locator (e.g. `#.actions[3].target`),
/// not an RFC 6901 JSON Pointer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl ValidationError {
    pub(crate) fn new(path: String, message: String) -> Self {
        Self { path, message }
    }

    /// Substring search across path and message (and the rendered
    /// boundary). Mirrors the helper of the same name in `roas` for
    /// consistency.
    pub fn contains(&self, needle: &str) -> bool {
        if self.path.contains(needle) || self.message.contains(needle) {
            return true;
        }
        self.to_string().contains(needle)
    }
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl PartialEq<str> for ValidationError {
    fn eq(&self, other: &str) -> bool {
        let plen = self.path.len();
        let sep = ": ";
        other.len() == plen + sep.len() + self.message.len()
            && other.starts_with(&self.path)
            && other[plen..].starts_with(sep)
            && other[plen + sep.len()..] == self.message
    }
}

impl PartialEq<&str> for ValidationError {
    fn eq(&self, other: &&str) -> bool {
        <ValidationError as PartialEq<str>>::eq(self, other)
    }
}

/// The accumulated outcome of a validation pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub errors: Vec<ValidationError>,
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} errors found:", self.errors.len())?;
        for error in &self.errors {
            writeln!(f, "- {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// Per-call validation toggles.
///
/// Each option suppresses one shallow check so callers can opt out of
/// individual diagnostics without disabling the whole validator.
#[derive(EnumSetType, Debug)]
pub enum ValidationOptions {
    /// Allow `info.title` to be empty (still required to be present).
    IgnoreEmptyInfoTitle,
    /// Allow `info.version` to be empty (still required to be present).
    IgnoreEmptyInfoVersion,
}

#[cfg(feature = "clap")]
impl clap::ValueEnum for ValidationOptions {
    fn value_variants<'a>() -> &'a [Self] {
        &[
            ValidationOptions::IgnoreEmptyInfoTitle,
            ValidationOptions::IgnoreEmptyInfoVersion,
        ]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        let (name, help) = match self {
            ValidationOptions::IgnoreEmptyInfoTitle => {
                ("empty-info-title", "Allow empty `info.title`")
            }
            ValidationOptions::IgnoreEmptyInfoVersion => {
                ("empty-info-version", "Allow empty `info.version`")
            }
        };
        Some(clap::builder::PossibleValue::new(name).help(help))
    }
}

/// Validate an Overlay document, collecting every diagnostic.
pub trait Validate {
    fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error>;
}

/// Crate-internal: implemented by every component type.
pub(crate) trait ValidateWithContext {
    fn validate_with_context(&self, ctx: &mut Context, path: String);
}

pub(crate) struct Context {
    pub options: EnumSet<ValidationOptions>,
    pub errors: Vec<ValidationError>,
}

impl Context {
    pub fn new(options: EnumSet<ValidationOptions>) -> Self {
        Self {
            options,
            errors: Vec::new(),
        }
    }

    pub fn is_option(&self, option: ValidationOptions) -> bool {
        self.options.contains(option)
    }

    pub fn error(&mut self, path: String, message: impl Into<String>) {
        self.errors.push(ValidationError::new(path, message.into()));
    }

    pub fn into_result(self) -> Result<(), Error> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(Error {
                errors: self.errors,
            })
        }
    }
}

/// Validate that a required string is not empty. Mirrors
/// `roas::common::helpers::validate_required_string`.
pub(crate) fn validate_required_string(s: &str, ctx: &mut Context, path: String) {
    if s.is_empty() {
        ctx.error(path, "must not be empty");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_renders_with_count_and_bullets() {
        let err = Error {
            errors: vec![
                ValidationError::new("#.a".into(), "first".into()),
                ValidationError::new("#.b".into(), "second".into()),
            ],
        };
        assert_eq!(
            format!("{err}"),
            "2 errors found:\n- #.a: first\n- #.b: second\n",
        );
    }

    #[test]
    fn error_zero_count_still_renders_header() {
        let err = Error { errors: vec![] };
        assert_eq!(format!("{err}"), "0 errors found:\n");
    }

    #[test]
    fn validation_error_partial_eq_against_str_matches_display_form() {
        let e = ValidationError::new("#.info.title".into(), "must not be empty".into());
        // Exercises both PartialEq<&str> (via the literal) and
        // PartialEq<str> (via deref of an owned String).
        assert!(e == "#.info.title: must not be empty");
        let owned = String::from("#.info.title: must not be empty");
        assert!(e == *owned.as_str());
        assert!(e != "different");
    }

    #[test]
    fn validation_error_contains_matches_across_boundary() {
        let e = ValidationError::new("#.info.title".into(), "must not be empty".into());
        assert!(e.contains("title: must"));
        assert!(e.contains("#.info"));
        assert!(e.contains("must not"));
        assert!(!e.contains("nowhere"));
    }

    #[test]
    fn context_collects_errors_and_converts_to_result() {
        let mut ctx = Context::new(EnumSet::empty());
        ctx.error("#.x".into(), "kaboom");
        let r = ctx.into_result();
        let err = r.unwrap_err();
        assert_eq!(err.errors.len(), 1);
    }

    #[test]
    fn context_with_no_errors_returns_ok() {
        let ctx = Context::new(EnumSet::empty());
        assert!(ctx.into_result().is_ok());
    }

    #[test]
    fn context_is_option_reflects_set_membership() {
        let opts = EnumSet::only(ValidationOptions::IgnoreEmptyInfoTitle);
        let ctx = Context::new(opts);
        assert!(ctx.is_option(ValidationOptions::IgnoreEmptyInfoTitle));
        assert!(!ctx.is_option(ValidationOptions::IgnoreEmptyInfoVersion));
    }

    #[test]
    fn validate_required_string_pushes_error_for_empty() {
        let mut ctx = Context::new(EnumSet::empty());
        validate_required_string("", &mut ctx, "#.info.title".into());
        validate_required_string("ok", &mut ctx, "#.info.version".into());
        assert_eq!(ctx.errors.len(), 1);
        assert!(ctx.errors[0] == "#.info.title: must not be empty");
    }
}

#[cfg(all(test, feature = "clap"))]
mod clap_tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn value_variants_round_trip_through_kebab_case_names() {
        for v in <ValidationOptions as ValueEnum>::value_variants() {
            let pv = v.to_possible_value().expect("possible value");
            let name = pv.get_name();
            let parsed = <ValidationOptions as ValueEnum>::from_str(name, false).expect("parses");
            assert_eq!(parsed, *v);
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "name `{name}` must be kebab-case",
            );
        }
    }
}
