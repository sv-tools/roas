//! Runtime-expression syntax, independent of the values available to a run.
//!
//! `parse` consumes a standalone field. `prefix` consumes a runtime base from
//! an already bounded condition operand; its caller owns any navigation suffix.

use crate::expression::ExpressionError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Root {
    Inputs,
    Outputs,
    Components,
    Sources,
    Self_,
    Workflows,
    Steps,
    Message,
    Url,
    Method,
    StatusCode,
    Request,
    Response,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expression<'a> {
    pub text: &'a str,
    pub root: Root,
    /// Grammar-delimited identifiers, not a split on every dot. A component
    /// key or input name containing dots occupies one entry.
    pub parts: Vec<&'a str>,
    pub pointer: Option<&'a str>,
}

impl<'a> Expression<'a> {
    pub(crate) fn step_id(&self) -> Option<&'a str> {
        (self.root == Root::Steps).then(|| self.parts[0])
    }
}

pub(crate) fn parse(text: &str) -> Result<Expression<'_>, ExpressionError> {
    #[cfg(test)]
    crate::prepare::instrumentation::compiled(0);
    let (expression, consumed) = prefix(text)?;
    if consumed != text.len() {
        return Err(syntax(
            text,
            consumed,
            "unexpected runtime-expression suffix",
        ));
    }
    Ok(expression)
}

pub(crate) fn prefix(text: &str) -> Result<(Expression<'_>, usize), ExpressionError> {
    let end = text
        .find(|c: char| c != '$' && !c.is_ascii_alphanumeric())
        .unwrap_or(text.len());
    let root = match &text[..end] {
        "$inputs" => Root::Inputs,
        "$outputs" => Root::Outputs,
        "$components" => Root::Components,
        "$sourceDescriptions" => Root::Sources,
        "$self" => Root::Self_,
        "$workflows" => Root::Workflows,
        "$steps" => Root::Steps,
        "$message" => Root::Message,
        "$url" => Root::Url,
        "$method" => Root::Method,
        "$statusCode" => Root::StatusCode,
        "$request" => Root::Request,
        "$response" => Root::Response,
        other => return Err(ExpressionError::Unknown(other.to_owned())),
    };
    let mut parser = Parser {
        text,
        at: end,
        parts: Vec::new(),
    };
    let pointer_allowed = match root {
        Root::Inputs | Root::Outputs => {
            parser.optional_name()?;
            true
        }
        Root::Components => {
            if parser.dot() {
                let group = parser.identifier(false)?;
                if !matches!(
                    group,
                    "parameters" | "successActions" | "failureActions" | "inputs"
                ) {
                    return Err(syntax(text, end + 1, "unknown component collection"));
                }
                parser.optional_name()?;
            }
            true
        }
        Root::Sources => {
            parser.required_dot()?;
            parser.identifier(false)?;
            if parser.dot() {
                parser.unrestricted()?;
            }
            false
        }
        Root::Workflows => {
            parser.required_dot()?;
            parser.identifier(false)?;
            parser.required_dot()?;
            // Keep the historical output shorthand, without stealing dots
            // from its output name. Only these two reserved fields split it.
            if parser.rest() == "inputs"
                || parser.rest().starts_with("inputs.")
                || parser.rest().starts_with("inputs#")
                || parser.rest() == "outputs"
                || parser.rest().starts_with("outputs.")
                || parser.rest().starts_with("outputs#")
            {
                parser.identifier(false)?;
                parser.optional_name()?;
            } else {
                parser.identifier(true)?;
            }
            true
        }
        Root::Steps => {
            parser.required_dot()?;
            parser.identifier(false)?;
            parser.required_dot()?;
            let field = parser.identifier(false)?;
            if field == "outputs" {
                parser.required_dot()?;
                parser.identifier(true)?;
                true
            } else {
                // Step exchange access is a retained executor extension.
                parser.exchange(field)?
            }
        }
        Root::Request | Root::Response | Root::Message => {
            parser.required_dot()?;
            parser.source()?
        }
        Root::Url | Root::Method | Root::StatusCode | Root::Self_ => false,
    };
    let pointer = if pointer_allowed && parser.rest().starts_with('#') {
        parser.at += 1;
        let pointer = parser.rest();
        validate_pointer(text, parser.at)?;
        parser.at = text.len();
        Some(pointer)
    } else {
        None
    };
    Ok((
        Expression {
            text: &text[..parser.at],
            root,
            parts: parser.parts,
            pointer,
        },
        parser.at,
    ))
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    parts: Vec<&'a str>,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn dot(&mut self) -> bool {
        if self.rest().starts_with('.') {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn required_dot(&mut self) -> Result<(), ExpressionError> {
        if self.dot() {
            Ok(())
        } else {
            Err(syntax(self.text, self.at, "expected `.` and a name"))
        }
    }

    fn identifier(&mut self, dots: bool) -> Result<&'a str, ExpressionError> {
        let start = self.at;
        self.at += self
            .rest()
            .bytes()
            .take_while(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-') || (dots && *b == b'.')
            })
            .count();
        if start == self.at {
            return Err(syntax(self.text, start, "expected an identifier"));
        }
        let name = &self.text[start..self.at];
        self.parts.push(name);
        Ok(name)
    }

    fn optional_name(&mut self) -> Result<(), ExpressionError> {
        if self.dot() {
            self.identifier(true)?;
        }
        Ok(())
    }

    fn unrestricted(&mut self) -> Result<(), ExpressionError> {
        // Fields already contain decoded JSON/YAML strings. Braces would
        // collide with interpolation delimiters; do not invent escaping.
        if let Some((offset, _)) = self
            .rest()
            .char_indices()
            .find(|(_, c)| c.is_control() || matches!(c, '{' | '}'))
        {
            return Err(syntax(
                self.text,
                self.at + offset,
                "a name cannot contain control characters or braces",
            ));
        }
        if self.rest().is_empty() {
            return Err(syntax(self.text, self.at, "expected a name"));
        }
        self.parts.push(self.rest());
        self.at = self.text.len();
        Ok(())
    }

    fn exchange(&mut self, field: &str) -> Result<bool, ExpressionError> {
        match field {
            "url" | "method" | "statusCode" => Ok(false),
            "request" | "response" | "message" => {
                self.required_dot()?;
                self.source()
            }
            _ => Err(syntax(
                self.text,
                self.at - field.len(),
                "unknown exchange field",
            )),
        }
    }

    fn source(&mut self) -> Result<bool, ExpressionError> {
        let field = self.identifier(false)?;
        match field {
            "body" | "payload" => Ok(true),
            "header" => {
                self.required_dot()?;
                let start = self.at;
                self.at += self
                    .rest()
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
                    .count();
                if start == self.at {
                    return Err(syntax(self.text, start, "expected a header token"));
                }
                self.parts.push(&self.text[start..self.at]);
                Ok(false)
            }
            "query" | "path" => {
                self.required_dot()?;
                self.unrestricted()?;
                Ok(false)
            }
            _ => Err(syntax(
                self.text,
                self.at - field.len(),
                "unknown request/response source",
            )),
        }
    }
}

fn validate_pointer(text: &str, start: usize) -> Result<(), ExpressionError> {
    let pointer = &text[start..];
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(syntax(
            text,
            start,
            "a JSON Pointer must be empty or start with `/`",
        ));
    }
    let mut chars = pointer.char_indices();
    while let Some((at, c)) = chars.next() {
        if matches!(c, '{' | '}') {
            return Err(syntax(
                text,
                start + at,
                "braces are not allowed in a runtime JSON Pointer",
            ));
        }
        if c == '~' && !matches!(chars.next(), Some((_, '0' | '1'))) {
            return Err(syntax(
                text,
                start + at,
                "a JSON Pointer escape must be `~0` or `~1`",
            ));
        }
    }
    Ok(())
}

pub(crate) fn syntax(text: &str, offset: usize, message: &str) -> ExpressionError {
    ExpressionError::Syntax {
        expression: text.to_owned(),
        offset,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_delimited_by_grammar_not_by_runtime_data() {
        for (text, parts) in [
            ("$inputs.org.token", vec!["org.token"]),
            ("$outputs.org.token", vec!["org.token"]),
            (
                "$steps.step-id.outputs.org.token",
                vec!["step-id", "outputs", "org.token"],
            ),
            (
                "$workflows.w.outputs.org.token",
                vec!["w", "outputs", "org.token"],
            ),
            ("$workflows.w.org.token", vec!["w", "org.token"]),
            (
                "$components.parameters.org.locale",
                vec!["parameters", "org.locale"],
            ),
            (
                "$sourceDescriptions.api.op.name=a & b<c>d",
                vec!["api", "op.name=a & b<c>d"],
            ),
            ("$sourceDescriptions.api", vec!["api"]),
            ("$request.path.a.b=c & d<e>f", vec!["path", "a.b=c & d<e>f"]),
            ("$response.header.X.a!b#c&d", vec!["header", "X.a!b#c&d"]),
            ("$components.inputs", vec!["inputs"]),
        ] {
            let expression = parse(text).unwrap();
            assert_eq!(expression.parts, parts, "{text}");
            assert_eq!(expression.text, text);
            assert_eq!(expression.pointer, None);
        }
        assert!(parse("$components").unwrap().parts.is_empty());
    }

    #[test]
    fn the_condition_caller_owns_navigation_but_not_pointer_text() {
        let (base, at) = prefix("$response.body.pets[0].name").unwrap();
        assert_eq!(base.text, "$response.body");
        assert_eq!(at, "$response.body".len());
        let (base, at) = prefix("$response.body#/pets[0].name").unwrap();
        assert_eq!(base.pointer, Some("/pets[0].name"));
        assert_eq!(at, "$response.body#/pets[0].name".len());
    }

    #[test]
    fn incomplete_fields_and_invalid_names_are_syntax_errors_without_a_scope() {
        for text in [
            "$steps",
            "$steps.",
            "$steps.id",
            "$workflows.id",
            "$request",
            "$response",
            "$sourceDescriptions",
            "$sourceDescriptions.api.",
            "$request.query.",
            "$request.path.",
            "$request.path.a{b}",
            "$sourceDescriptions.api.a}b",
            "$request.query.a\nb",
            "$response.header.",
            "$request.header.😺",
            "$steps.id.bad",
            "$steps.id.request.bad",
            "$request.unknown",
            "$response.unknown",
        ] {
            let error = parse(text).unwrap_err();
            assert!(
                matches!(error, ExpressionError::Syntax { .. }),
                "{text}: {error}"
            );
        }
    }
}
