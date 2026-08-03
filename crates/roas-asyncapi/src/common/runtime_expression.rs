//! The AsyncAPI runtime expression grammar.
//!
//! Used by `correlationId.location` and `operationReply.address.location`
//! ([Runtime Expression](https://www.asyncapi.com/docs/reference/specification/v3.0.0#runtimeExpression)):
//!
//! ```text
//! expression       = "$message" "." source
//! source           = header-reference / payload-reference
//! header-reference = "header" ["#" fragment]
//! payload-reference= "payload" ["#" fragment]
//! fragment         = a JSON Pointer (RFC 6901)
//! ```
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
    /// The RFC 6901 JSON Pointer after `#`, if the expression has one.
    /// `$message.payload` alone selects the whole payload.
    pub pointer: Option<&'a str>,
}

/// Why a string is not a valid runtime expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeExpressionError {
    /// Does not start with `$message.`.
    MissingMessagePrefix,
    /// The source is neither `header` nor `payload`.
    UnknownSource,
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
            Self::InvalidPointer => "fragment must be a JSON Pointer starting with `/`",
        }
    }
}

/// Parse a runtime expression, e.g. `$message.header#/correlationId`.
pub fn parse(expression: &str) -> Result<RuntimeExpression<'_>, RuntimeExpressionError> {
    let rest = expression
        .strip_prefix("$message.")
        .ok_or(RuntimeExpressionError::MissingMessagePrefix)?;

    let (source, fragment) = match rest.split_once('#') {
        Some((source, fragment)) => (source, Some(fragment)),
        None => (rest, None),
    };

    let source = match source {
        "header" => RuntimeExpressionSource::Header,
        "payload" => RuntimeExpressionSource::Payload,
        _ => return Err(RuntimeExpressionError::UnknownSource),
    };

    // RFC 6901: a pointer is either empty or a sequence of `/`-prefixed
    // reference tokens.
    if let Some(fragment) = fragment
        && !fragment.is_empty()
        && !fragment.starts_with('/')
    {
        return Err(RuntimeExpressionError::InvalidPointer);
    }

    Ok(RuntimeExpression {
        source,
        pointer: fragment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_payload_with_pointer() {
        let e = parse("$message.header#/correlationId").unwrap();
        assert_eq!(e.source, RuntimeExpressionSource::Header);
        assert_eq!(e.pointer, Some("/correlationId"));

        let e = parse("$message.payload#/user/id").unwrap();
        assert_eq!(e.source, RuntimeExpressionSource::Payload);
        assert_eq!(e.pointer, Some("/user/id"));
    }

    #[test]
    fn parses_bare_source_without_fragment() {
        let e = parse("$message.payload").unwrap();
        assert_eq!(e.source, RuntimeExpressionSource::Payload);
        assert_eq!(e.pointer, None);
    }

    #[test]
    fn empty_fragment_is_the_whole_document_pointer() {
        let e = parse("$message.header#").unwrap();
        assert_eq!(e.pointer, Some(""));
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
        assert_eq!(
            parse("$message.body#/x").unwrap_err(),
            RuntimeExpressionError::UnknownSource
        );
        assert_eq!(
            parse("$message.").unwrap_err(),
            RuntimeExpressionError::UnknownSource
        );
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
    }
}
