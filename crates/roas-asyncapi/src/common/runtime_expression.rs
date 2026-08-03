//! The AsyncAPI runtime expression grammar.
//!
//! Used by `correlationId.location` and `operationReply.address.location`
//! ([Runtime Expression](https://www.asyncapi.com/docs/reference/specification/v3.0.0#runtimeExpression)):
//!
//! ```text
//! expression       = "$message" "." source
//! source           = header-reference / payload-reference
//! header-reference = "header" "#" fragment
//! payload-reference= "payload" "#" fragment
//! fragment         = a JSON Pointer (RFC 6901)
//! ```
//!
//! The `#` is mandatory: the schema pins these fields to
//! `^\$message\.(header|payload)#(\/(([^\/~])|(~[01]))*)*`, so a bare
//! `$message.payload` is not a valid location. Selecting the whole
//! headers or payload is spelled `$message.payload#`.
//!
//! Identical in every AsyncAPI version that has runtime expressions, so
//! it lives in `common`. Hand-rolled rather than regex-driven, matching
//! how the sibling crates avoid a regex dependency for a single check.

/// Which part of the message an expression selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeExpressionSource {
    Header,
    Payload,
}

/// A parsed runtime expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeExpression<'a> {
    pub source: RuntimeExpressionSource,
    /// The RFC 6901 JSON Pointer after `#`. Empty selects the whole
    /// headers / payload.
    pub pointer: &'a str,
}

/// Why a string is not a valid runtime expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeExpressionError {
    /// Does not start with `$message.`.
    MissingMessagePrefix,
    /// The source is neither `header` nor `payload`.
    UnknownSource,
    /// No `#` separates the source from the fragment.
    MissingFragment,
    /// The fragment after `#` is not a valid JSON Pointer (RFC 6901
    /// requires it to be empty or start with `/`).
    InvalidPointer,
}

impl RuntimeExpressionError {
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingMessagePrefix => "must start with `$message.`",
            Self::UnknownSource => "source must be `header` or `payload`",
            Self::MissingFragment => "must have a `#` fragment (use `#` alone for the whole value)",
            Self::InvalidPointer => "fragment must be a JSON Pointer starting with `/`",
        }
    }
}

/// Parse a runtime expression, e.g. `$message.header#/correlationId`.
pub fn parse(expression: &str) -> Result<RuntimeExpression<'_>, RuntimeExpressionError> {
    let rest = expression
        .strip_prefix("$message.")
        .ok_or(RuntimeExpressionError::MissingMessagePrefix)?;

    let Some((source, pointer)) = rest.split_once('#') else {
        // Report the more specific problem when the source itself is
        // also wrong, e.g. `$message.body`.
        return Err(match rest {
            "header" | "payload" => RuntimeExpressionError::MissingFragment,
            _ => RuntimeExpressionError::UnknownSource,
        });
    };

    let source = match source {
        "header" => RuntimeExpressionSource::Header,
        "payload" => RuntimeExpressionSource::Payload,
        _ => return Err(RuntimeExpressionError::UnknownSource),
    };

    // RFC 6901: a pointer is either empty or a sequence of `/`-prefixed
    // reference tokens.
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(RuntimeExpressionError::InvalidPointer);
    }

    Ok(RuntimeExpression { source, pointer })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_payload_with_pointer() {
        let e = parse("$message.header#/correlationId").unwrap();
        assert_eq!(e.source, RuntimeExpressionSource::Header);
        assert_eq!(e.pointer, "/correlationId");

        let e = parse("$message.payload#/user/id").unwrap();
        assert_eq!(e.source, RuntimeExpressionSource::Payload);
        assert_eq!(e.pointer, "/user/id");
    }

    #[test]
    fn empty_fragment_selects_the_whole_value() {
        let e = parse("$message.header#").unwrap();
        assert_eq!(e.pointer, "");
    }

    #[test]
    fn rejects_a_bare_source_without_a_fragment() {
        // The schema pattern requires the `#`.
        for bad in ["$message.payload", "$message.header"] {
            assert_eq!(
                parse(bad).unwrap_err(),
                RuntimeExpressionError::MissingFragment,
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn rejects_missing_prefix() {
        for bad in ["header#/x", "$request.header", "", "$message"] {
            assert_eq!(
                parse(bad).unwrap_err(),
                RuntimeExpressionError::MissingMessagePrefix,
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn rejects_unknown_source() {
        for bad in ["$message.body#/x", "$message.", "$message.body"] {
            assert_eq!(
                parse(bad).unwrap_err(),
                RuntimeExpressionError::UnknownSource,
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn rejects_fragment_that_is_not_a_json_pointer() {
        assert_eq!(
            parse("$message.payload#user/id").unwrap_err(),
            RuntimeExpressionError::InvalidPointer
        );
    }

    #[test]
    fn error_messages_are_human_readable() {
        assert!(
            RuntimeExpressionError::MissingMessagePrefix
                .message()
                .contains("$message.")
        );
        assert!(
            RuntimeExpressionError::UnknownSource
                .message()
                .contains("header")
        );
        assert!(
            RuntimeExpressionError::InvalidPointer
                .message()
                .contains("JSON Pointer")
        );
        assert!(
            RuntimeExpressionError::MissingFragment
                .message()
                .contains('#')
        );
    }
}
