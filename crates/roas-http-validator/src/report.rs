//! What the validator says happened.
//!
//! Two kinds of answer, kept apart because a caller does different
//! things with them. [`RoutingError`] means the description says nothing
//! about this request — usually a 404, or a request that should be
//! passed through untouched. A [`ValidationReport`] means the request
//! was found and judged; its `errors` are the ones a 400 would name.
//!
//! Errors are collected rather than raised one at a time, the way
//! `roas`'s own description validator collects them: a client that sent
//! three bad parameters is better served by hearing about all three.

use std::fmt::{self, Display, Formatter};

/// Where in the request an error was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Location {
    /// A parameter with `in: path`.
    Path,
    /// A parameter with `in: query`.
    Query,
    /// A parameter with `in: querystring` — the whole query string as
    /// one value, which OpenAPI 3.2 added.
    Querystring,
    /// A parameter with `in: header`.
    Header,
    /// A parameter with `in: cookie`.
    Cookie,
    /// The request body.
    Body,
    /// Not the request at all: the description itself could not be read
    /// far enough to judge the request — an unresolvable `$ref` where a
    /// Parameter Object should be, say. Reported rather than dropped,
    /// because the parameter it named went unchecked.
    Description,
}

impl Display for Location {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Location::Path => "path",
            Location::Query => "query",
            Location::Querystring => "querystring",
            Location::Header => "header",
            Location::Cookie => "cookie",
            Location::Body => "body",
            Location::Description => "description",
        })
    }
}

/// One thing wrong with the request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationError {
    /// Which part of the request this is about.
    pub location: Location,
    /// The parameter name, or empty for the body.
    pub name: String,
    /// What is wrong.
    pub kind: ErrorKind,
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.name.is_empty() {
            write!(f, "{}: {}", self.location, self.kind)
        } else {
            write!(
                f,
                "{} parameter {:?}: {}",
                self.location, self.name, self.kind
            )
        }
    }
}

impl std::error::Error for ValidationError {}

/// What was wrong with one parameter or with the body.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A `required` parameter was not sent, or a required body was
    /// absent.
    Missing,

    /// The value did not satisfy its Schema Object. `pointer` is a JSON
    /// Pointer into the value — empty for the value itself.
    Schema {
        /// Where inside the value, as a JSON Pointer.
        pointer: String,
        /// What the schema required and the value did not give.
        message: String,
    },

    /// A body arrived, but its media type is not one the Request Body
    /// Object describes.
    UnexpectedMediaType {
        /// What the request said it was sending, if it said.
        got: Option<String>,
        /// The media types the operation accepts.
        expected: Vec<String>,
    },

    /// The value could not be read as the media type or `style` said it
    /// would be — malformed JSON, a form field that is not a number.
    Malformed(String),

    /// The description uses something this crate does not implement
    /// yet, so the value was **not** checked. Reported rather than
    /// skipped: "not validated" must never read as "valid".
    Unsupported(String),

    /// The description could be read but not applied faithfully, so the
    /// value went **unchecked** — a `pattern` that will not compile, or
    /// a bound whose digits were lost to floating point before this
    /// crate ever saw it. Same guarantee as [`ErrorKind::Unsupported`]:
    /// unchecked never reads as valid.
    Unchecked(String),

    /// A `$ref` in the description could not be resolved, so there was
    /// no schema to judge the value against.
    UnresolvedReference(String),

    /// The request carried a query parameter the operation does not
    /// describe. Only reported when
    /// [`Options::reject_undescribed_query_parameters`](crate::Options::reject_undescribed_query_parameters)
    /// asks for it.
    Undescribed,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Missing => f.write_str("is required and was not sent"),
            ErrorKind::Schema { pointer, message } if pointer.is_empty() => f.write_str(message),
            ErrorKind::Schema { pointer, message } => write!(f, "at {pointer}: {message}"),
            ErrorKind::UnexpectedMediaType { got, expected } => {
                let expected = expected.join(", ");
                match got {
                    Some(got) => write!(f, "media type {got:?} is not one of: {expected}"),
                    None => write!(f, "no media type was sent; expected one of: {expected}"),
                }
            }
            ErrorKind::Malformed(why) => write!(f, "cannot be read: {why}"),
            ErrorKind::Unsupported(what) => {
                write!(f, "was NOT checked — {what} is not implemented yet")
            }
            ErrorKind::Unchecked(why) => write!(f, "was NOT checked — {why}"),
            ErrorKind::UnresolvedReference(reference) => {
                write!(f, "has an unresolvable `$ref`: {reference}")
            }
            ErrorKind::Undescribed => f.write_str("is not described by this operation"),
        }
    }
}

/// The verdict on one request that the description does describe.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationReport {
    /// The Path Item template the request matched, e.g. `/pets/{petId}`.
    pub template: String,
    /// The HTTP method token the matched operation describes — `GET`,
    /// not the `get` that OpenAPI keys it under, and exactly as written
    /// for one that came from `additionalOperations`.
    pub method: String,
    /// The matched operation's `operationId`, when it has one.
    pub operation_id: Option<String>,
    /// Path parameters as the template read them.
    pub path_parameters: Vec<(String, String)>,
    /// Everything wrong with the request. Empty means valid.
    pub errors: Vec<ValidationError>,
}

impl ValidationReport {
    /// Whether the request satisfied the description.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// The errors, as one `Err` when there are any.
    ///
    /// # Errors
    ///
    /// The report's own errors, for callers that would rather branch on
    /// a `Result` than on [`is_valid`](Self::is_valid).
    pub fn into_result(self) -> Result<Self, Vec<ValidationError>> {
        if self.is_valid() {
            Ok(self)
        } else {
            Err(self.errors)
        }
    }
}

impl Display for ValidationReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let operation = match &self.operation_id {
            Some(id) => format!(" ({id})"),
            None => String::new(),
        };
        write!(f, "{} {}{operation}: ", self.method, self.template)?;
        if self.errors.is_empty() {
            return f.write_str("valid");
        }
        writeln!(f, "{} error(s)", self.errors.len())?;
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "  - {error}")?;
        }
        Ok(())
    }
}

/// The description does not describe this request at all.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RoutingError {
    /// No Path Item template matched the request path.
    #[error("no path in the description matches {path:?}")]
    PathNotFound {
        /// The path that matched nothing.
        path: String,
    },

    /// A template matched, but it describes no such method. The methods
    /// it does describe are named, which is what an `Allow` response
    /// header needs.
    #[error("{template} describes no {method} operation (it has: {})", allowed.join(", "))]
    MethodNotAllowed {
        /// The template that matched.
        template: String,
        /// The method that was asked for, uppercased.
        method: String,
        /// The methods the Path Item Object does describe, lowercased.
        allowed: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(location: Location, name: &str, kind: ErrorKind) -> ValidationError {
        ValidationError {
            location,
            name: name.to_owned(),
            kind,
        }
    }

    fn report(errors: Vec<ValidationError>) -> ValidationReport {
        ValidationReport {
            template: "/pets/{petId}".to_owned(),
            method: "GET".to_owned(),
            operation_id: Some("getPet".to_owned()),
            path_parameters: vec![("petId".to_owned(), "7".to_owned())],
            errors,
        }
    }

    #[test]
    fn a_report_without_errors_is_valid() {
        let report = report(Vec::new());
        assert!(report.is_valid());
        assert_eq!(report.to_string(), "GET /pets/{petId} (getPet): valid");
        assert!(report.into_result().is_ok());
    }

    #[test]
    fn a_report_lists_every_error_it_found() {
        let report = report(vec![
            error(Location::Query, "limit", ErrorKind::Missing),
            error(
                Location::Body,
                "",
                ErrorKind::Schema {
                    pointer: "/name".to_owned(),
                    message: "expected string, got integer".to_owned(),
                },
            ),
        ]);
        assert!(!report.is_valid());
        assert_eq!(
            report.to_string(),
            "GET /pets/{petId} (getPet): 2 error(s)\n  \
             - query parameter \"limit\": is required and was not sent\n  \
             - body: at /name: expected string, got integer",
        );
        assert_eq!(report.into_result().unwrap_err().len(), 2);
    }

    #[test]
    fn an_operation_without_an_id_is_named_by_its_template_alone() {
        let mut report = report(Vec::new());
        report.operation_id = None;
        assert_eq!(report.to_string(), "GET /pets/{petId}: valid");
    }

    #[test]
    fn every_error_kind_says_what_it_means() {
        let kinds = [
            (ErrorKind::Missing, "is required and was not sent"),
            (
                ErrorKind::Schema {
                    pointer: String::new(),
                    message: "expected integer".to_owned(),
                },
                "expected integer",
            ),
            (
                ErrorKind::UnexpectedMediaType {
                    got: Some("text/plain".to_owned()),
                    expected: vec!["application/json".to_owned()],
                },
                "media type \"text/plain\" is not one of: application/json",
            ),
            (
                ErrorKind::UnexpectedMediaType {
                    got: None,
                    expected: vec!["application/json".to_owned()],
                },
                "no media type was sent; expected one of: application/json",
            ),
            (
                ErrorKind::Malformed("trailing comma".to_owned()),
                "cannot be read: trailing comma",
            ),
            (
                ErrorKind::Unsupported("multipart bodies".to_owned()),
                "was NOT checked — multipart bodies is not implemented yet",
            ),
            (
                ErrorKind::UnresolvedReference("#/components/schemas/Gone".to_owned()),
                "has an unresolvable `$ref`: #/components/schemas/Gone",
            ),
            (
                ErrorKind::Unchecked("the bound lost its digits".to_owned()),
                "was NOT checked — the bound lost its digits",
            ),
            (ErrorKind::Undescribed, "is not described by this operation"),
        ];
        for (kind, expected) in kinds {
            assert_eq!(kind.to_string(), expected);
        }
    }

    #[test]
    fn a_location_names_itself() {
        for (location, expected) in [
            (Location::Path, "path"),
            (Location::Query, "query"),
            (Location::Querystring, "querystring"),
            (Location::Header, "header"),
            (Location::Cookie, "cookie"),
            (Location::Body, "body"),
            (Location::Description, "description"),
        ] {
            assert_eq!(location.to_string(), expected);
        }
    }

    #[test]
    fn a_routing_error_says_which_path_or_which_methods() {
        assert_eq!(
            RoutingError::PathNotFound {
                path: "/nope".to_owned(),
            }
            .to_string(),
            "no path in the description matches \"/nope\"",
        );
        assert_eq!(
            RoutingError::MethodNotAllowed {
                template: "/pets".to_owned(),
                method: "DELETE".to_owned(),
                allowed: vec!["get".to_owned(), "post".to_owned()],
            }
            .to_string(),
            "/pets describes no DELETE operation (it has: get, post)",
        );
    }
}
