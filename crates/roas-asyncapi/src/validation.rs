//! Validation framework for AsyncAPI documents.
//!
//! Modeled after [`roas::validation`] / `roas-arazzo`: a public
//! [`Validate`] trait drives a recursive descent through a
//! crate-internal trait. The current location is held as a single
//! mutable path buffer on the `Context`; nodes `enter` a child
//! segment, recurse, and the segment is truncated on the way out. The
//! path string is cloned only when an error is actually recorded, so a
//! valid document allocates no per-node path strings. Errors collect
//! rather than fail fast.
//!
//! [`roas::validation`]: https://docs.rs/roas/latest/roas/validation/index.html

use enumset::{EnumSet, EnumSetType};
use std::collections::BTreeMap;
use std::fmt::{self, Display, Write};

/// A single validation finding.
///
/// `path` is a human-readable locator (e.g. `#.channels.user.address`),
/// not an RFC 6901 JSON Pointer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
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
#[non_exhaustive]
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
/// Each option relaxes or tightens one check so callers can tune the
/// diagnostics without disabling the whole validator. Marked
/// `#[non_exhaustive]` so future toggles are non-breaking additions.
#[derive(EnumSetType, Debug)]
#[non_exhaustive]
pub enum ValidationOptions {
    /// Allow `info.title` to be empty (still required to be present).
    IgnoreEmptyInfoTitle,
    /// Allow `info.version` to be empty (still required to be present).
    IgnoreEmptyInfoVersion,
    /// Report a `$ref` that points outside the current document.
    ///
    /// Cross-reference checks resolve only local (`#/...`) pointers —
    /// following an external reference needs a loader, which is outside
    /// this crate's scope. By default an external `$ref` is accepted
    /// without further checking; enable this to require a document that
    /// is self-contained.
    ErrorOnExternalReference,
    /// Allow a channel parameter that is declared but never appears as
    /// a `{placeholder}` in the channel's `address`.
    IgnoreUnusedChannelParameter,
}

#[cfg(feature = "clap")]
impl clap::ValueEnum for ValidationOptions {
    fn value_variants<'a>() -> &'a [Self] {
        &[
            ValidationOptions::IgnoreEmptyInfoTitle,
            ValidationOptions::IgnoreEmptyInfoVersion,
            ValidationOptions::ErrorOnExternalReference,
            ValidationOptions::IgnoreUnusedChannelParameter,
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
            ValidationOptions::ErrorOnExternalReference => (
                "external-reference",
                "Report `$ref`s pointing outside the document",
            ),
            ValidationOptions::IgnoreUnusedChannelParameter => (
                "unused-channel-parameter",
                "Allow channel parameters missing from the address",
            ),
        };
        Some(clap::builder::PossibleValue::new(name).help(help))
    }
}

/// Validate an AsyncAPI document, collecting every diagnostic.
pub trait Validate {
    fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error>;
}

/// Crate-internal: implemented by every component type. The location is
/// carried by [`Context`]'s path buffer rather than a per-call string.
pub(crate) trait ValidateWithContext {
    fn validate_with_context(&self, ctx: &mut Context);
}

pub(crate) struct Context {
    options: EnumSet<ValidationOptions>,
    pub errors: Vec<ValidationError>,
    /// The current location, e.g. `#.channels.user.messages.signup`.
    /// Mutated in place via `in_*`; only cloned when an error is
    /// recorded.
    path: String,
    /// The document being validated, as plain JSON.
    ///
    /// Every reference in the model is followed against this, at
    /// whatever depth it sits, so resolution does not depend on some
    /// other check happening to walk that way.
    document: Option<serde_json::Value>,
}

impl Context {
    pub fn new(options: EnumSet<ValidationOptions>) -> Self {
        Self {
            options,
            errors: Vec::new(),
            path: "#".to_owned(),
            document: None,
        }
    }

    /// A context that can follow the document's own references.
    pub fn for_document<D>(options: EnumSet<ValidationOptions>, document: &D) -> Self
    where
        D: serde::Serialize,
    {
        Self {
            // Serializing a document model cannot fail; `null` simply
            // leaves every pointer naming nothing.
            document: Some(serde_json::to_value(document).unwrap_or_default()),
            ..Self::new(options)
        }
    }

    /// The document as plain JSON, when this context has one.
    pub fn document(&self) -> Option<&serde_json::Value> {
        self.document.as_ref()
    }

    /// Whether an error was already recorded at `<current>.<field>`.
    ///
    /// Lets the general check stay quiet where a check that knows what
    /// the reference is *for* has already spoken.
    pub fn has_error_at_field(&mut self, field: &str) -> bool {
        let mark = self.path.len();
        self.push_field(field);
        let found = self.errors.iter().any(|error| error.path == self.path);
        self.path.truncate(mark);
        found
    }

    pub fn is_option(&self, option: ValidationOptions) -> bool {
        self.options.contains(option)
    }

    /// Record an error at the current path.
    pub fn error(&mut self, message: impl Into<String>) {
        self.errors
            .push(ValidationError::new(self.path.clone(), message.into()));
    }

    /// Record an error at `<current>.<field>` without descending into it.
    pub fn error_field(&mut self, field: &str, message: impl Into<String>) {
        let mark = self.path.len();
        self.push_field(field);
        self.error(message);
        self.path.truncate(mark);
    }

    /// Push `.<field>` for the duration of `f`.
    pub fn in_field<R>(&mut self, field: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.path.len();
        self.push_field(field);
        let result = f(self);
        self.path.truncate(mark);
        result
    }

    /// Push `.<field>[<index>]` for the duration of `f`.
    pub fn in_index<R>(&mut self, field: &str, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.path.len();
        self.push_field(field);
        let _ = write!(self.path, "[{index}]");
        let result = f(self);
        self.path.truncate(mark);
        result
    }

    /// Push `.<field>.<key>` for the duration of `f` (for map entries).
    pub fn in_key<R>(&mut self, field: &str, key: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.path.len();
        self.push_field(field);
        self.push_field(key);
        let result = f(self);
        self.path.truncate(mark);
        result
    }

    /// Error at `<current>.<field>` if the required string is empty.
    pub fn require_non_empty(&mut self, field: &str, value: &str) {
        if value.is_empty() {
            self.error_field(field, "must not be empty");
        }
    }

    /// Validate that every key in a map matches the component key
    /// pattern `^[A-Za-z0-9\.\-_]+$`, reported under `.<field>`.
    pub fn validate_map_keys<V>(&mut self, field: &str, map: &BTreeMap<String, V>) {
        self.in_field(field, |ctx| {
            for key in map.keys() {
                if !is_valid_key(key) {
                    ctx.error_field(key, r"key must match `^[A-Za-z0-9\.\-_]+$`");
                }
            }
        });
    }

    fn push_field(&mut self, field: &str) {
        self.path.push('.');
        self.path.push_str(field);
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

    #[cfg(test)]
    pub fn with_path(options: EnumSet<ValidationOptions>, path: &str) -> Self {
        Self {
            path: path.to_owned(),
            ..Self::new(options)
        }
    }
}

/// Component key pattern `^[A-Za-z0-9\.\-_]+$` (non-empty, ASCII
/// alphanumerics plus `.`, `-`, `_`) — the schema spells it
/// `^[\w\d\.\-_]+$`. Hand-rolled to avoid pulling in a regex engine for
/// one check.
pub(crate) fn is_valid_key(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
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
        assert!(!e.contains("nowhere"));
    }

    #[test]
    fn error_records_at_current_path() {
        let mut ctx = Context::new(EnumSet::empty());
        ctx.error("kaboom");
        assert!(ctx.errors[0] == "#: kaboom");
    }

    #[test]
    fn in_scopes_compose_and_truncate() {
        let mut ctx = Context::new(EnumSet::empty());
        ctx.in_key("channels", "user", |ctx| {
            ctx.in_index("servers", 1, |ctx| {
                ctx.error_field("$ref", "bad");
            });
            ctx.error("here");
        });
        ctx.error("root");
        assert!(ctx.errors[0] == "#.channels.user.servers[1].$ref: bad");
        assert!(ctx.errors[1] == "#.channels.user: here");
        assert!(ctx.errors[2] == "#: root");
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
        assert!(!ctx.is_option(ValidationOptions::ErrorOnExternalReference));
    }

    #[test]
    fn require_non_empty_pushes_error_for_empty_only() {
        let mut ctx = Context::new(EnumSet::empty());
        ctx.in_field("info", |ctx| {
            ctx.require_non_empty("title", "");
            ctx.require_non_empty("version", "ok");
        });
        assert_eq!(ctx.errors.len(), 1);
        assert!(ctx.errors[0] == "#.info.title: must not be empty");
    }

    #[test]
    fn validate_map_keys_flags_only_invalid_keys() {
        let mut map = BTreeMap::new();
        map.insert("ok.key-1_2".to_owned(), ());
        map.insert("bad key".to_owned(), ());
        let mut ctx = Context::new(EnumSet::empty());
        ctx.validate_map_keys("channels", &map);
        assert_eq!(ctx.errors.len(), 1);
        assert!(ctx.errors[0].contains("#.channels.bad key"));
    }

    #[test]
    fn is_valid_key_allows_dots_and_rejects_others() {
        assert!(is_valid_key("my.channel_name-1"));
        assert!(!is_valid_key(""));
        assert!(!is_valid_key("has space"));
        assert!(!is_valid_key("slash/key"));
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
