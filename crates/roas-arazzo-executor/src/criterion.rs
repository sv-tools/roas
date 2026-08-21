//! Deciding whether a criterion holds.
//!
//! Per [Criterion Object](https://spec.openapis.org/arazzo/v1.1.0.html#criterion-object):
//! `simple` conditions are a small expression language of their own,
//! `regex` and `jsonpath` apply to the data the criterion's `context`
//! names, and `xpath` is not supported here.
//!
//! The `simple` language is parsed rather than pattern-matched:
//! comparisons of literals and runtime expressions, joined by `&&` and
//! `||`, grouped by parentheses. An operand standing alone is read for
//! its truth, which is what makes `$response.body#/ok` a condition.

use crate::expression::{self, ExpressionError, Scope};
use crate::select::{self, Language, SelectError};
use roas_arazzo::v1_1::{Criterion, CriterionKind, CriterionType, ExpressionKind};
use serde_json::Value;
use std::cmp::Ordering;

/// Why a criterion could not be decided.
///
/// A criterion that is simply *false* is not an error — it is the
/// answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CriterionError {
    /// A runtime expression in the criterion could not be evaluated.
    #[error(transparent)]
    Expression(#[from] ExpressionError),
    /// The criterion's selector could not be applied.
    #[error(transparent)]
    Select(#[from] SelectError),
    /// The `simple` condition does not parse.
    #[error("`{condition}` is not a valid condition: {message}")]
    Syntax {
        /// The condition as written.
        condition: String,
        /// What the parser objected to.
        message: String,
    },
    /// The `regex` condition is not a valid regular expression.
    #[error("`{condition}` is not a valid regular expression: {message}")]
    Regex {
        /// The condition as written.
        condition: String,
        /// What the regex engine said.
        message: String,
    },
    /// A typed criterion arrived without the `context` it needs.
    #[error("a `{0}` criterion needs a `context`")]
    MissingContext(&'static str),
    /// XPath, which this crate does not evaluate.
    #[error("{0} criteria are not supported by this executor")]
    Unsupported(&'static str),
}

/// Whether `criterion` holds in `scope`.
pub(crate) fn passes(criterion: &Criterion, scope: &Scope<'_>) -> Result<bool, CriterionError> {
    let context = |what: &'static str| -> Result<Value, CriterionError> {
        let context = criterion
            .context
            .as_deref()
            .ok_or(CriterionError::MissingContext(what))?;
        Ok(expression::evaluate(context, scope)?)
    };

    // A pattern or a path may be written with expressions inside it —
    // `{$inputs.pattern}` — and must be filled in before the engine
    // that reads it ever sees it. A `simple` condition is different:
    // its own parser evaluates the expressions it finds.
    let written = || -> Result<String, CriterionError> {
        Ok(expression::interpolate(&criterion.condition, scope)?)
    };

    match criterion.type_.as_ref() {
        None | Some(CriterionType::Simple(CriterionKind::Simple)) => {
            simple(&criterion.condition, scope)
        }
        Some(CriterionType::Simple(CriterionKind::Regex)) => regex(&written()?, &context("regex")?),
        Some(CriterionType::Simple(CriterionKind::Jsonpath)) => {
            selects(Language::Path, &written()?, &context("jsonpath")?)
        }
        Some(CriterionType::Simple(CriterionKind::Xpath)) => {
            Err(CriterionError::Unsupported("XPath"))
        }
        Some(CriterionType::Expression(expression)) => match expression.type_ {
            ExpressionKind::Jsonpath => selects(Language::Path, &written()?, &context("jsonpath")?),
            ExpressionKind::Jsonpointer => {
                selects(Language::Pointer, &written()?, &context("jsonpointer")?)
            }
            ExpressionKind::Xpath => Err(CriterionError::Unsupported("XPath")),
        },
    }
}

/// Whether the expression picks anything out of the context.
///
/// The specification is explicit: a condition passes when the
/// expression returns a non-empty nodelist and fails when it returns an
/// empty one. What was found does not matter — a node holding `false`
/// is still a node, and a filter is how a criterion asks about a value.
fn selects(language: Language, condition: &str, context: &Value) -> Result<bool, CriterionError> {
    Ok(select::apply(language, condition, context)?.is_some())
}

fn regex(condition: &str, context: &Value) -> Result<bool, CriterionError> {
    let regex = regex::Regex::new(condition).map_err(|error| CriterionError::Regex {
        condition: condition.to_owned(),
        message: error.to_string(),
    })?;
    Ok(regex.is_match(&text(context)))
}

/// A value as the text a regular expression is matched against: a
/// string as it stands, anything else as its JSON.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Whether a value counts as true where a condition wants a truth.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(bool) => *bool,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(members) => !members.is_empty(),
    }
}

// ---- the `simple` condition language --------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Compare(Comparison),
    Value(Operand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Clone, Debug, PartialEq)]
enum Operand {
    /// A runtime expression, evaluated where the condition is decided.
    Expression(String),
    /// A literal written into the condition.
    Literal(Value),
}

/// The runtime expressions a `simple` condition reads, found the way
/// the condition itself is read.
///
/// Working this out by splitting on spaces misses
/// `$statusCode==200&&$steps.b.outputs.ready`, where nothing separates
/// the operands but the operators — and would find one inside a quoted
/// literal, where there is none. The tokenizer already knows the
/// difference, so it answers. A condition that does not tokenize names
/// nothing; it will say so when it is evaluated.
pub(crate) fn expressions_in(condition: &str) -> Vec<String> {
    tokenize(condition).map_or_else(
        |_| Vec::new(),
        |tokens| {
            tokens
                .into_iter()
                .filter_map(|token| match token {
                    Token::Value(Operand::Expression(expression)) => Some(expression),
                    _ => None,
                })
                .collect()
        },
    )
}

fn simple(condition: &str, scope: &Scope<'_>) -> Result<bool, CriterionError> {
    let tokens = tokenize(condition)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        condition,
        scope,
    };
    let holds = parser.disjunction()?;
    if parser.at < parser.tokens.len() {
        return Err(parser.error("unexpected trailing input"));
    }
    Ok(holds)
}

fn tokenize(condition: &str) -> Result<Vec<Token>, CriterionError> {
    let syntax = |message: &str| CriterionError::Syntax {
        condition: condition.to_owned(),
        message: message.to_owned(),
    };
    let bytes: Vec<char> = condition.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let char = bytes[at];
        match char {
            char if char.is_whitespace() => at += 1,
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            '&' | '|' => {
                let next = bytes.get(at + 1).copied();
                if next != Some(char) {
                    return Err(syntax(&format!("`{char}` must be doubled")));
                }
                tokens.push(if char == '&' { Token::And } else { Token::Or });
                at += 2;
            }
            '=' | '!' | '<' | '>' => {
                let doubled = bytes.get(at + 1) == Some(&'=');
                let comparison = match (char, doubled) {
                    ('=', true) => Comparison::Equal,
                    ('!', true) => Comparison::NotEqual,
                    ('<', true) => Comparison::LessOrEqual,
                    ('>', true) => Comparison::GreaterOrEqual,
                    ('<', false) => Comparison::Less,
                    ('>', false) => Comparison::Greater,
                    (char, _) => return Err(syntax(&format!("`{char}` must be followed by `=`"))),
                };
                at += if doubled { 2 } else { 1 };
                tokens.push(Token::Compare(comparison));
            }
            '\'' | '"' => {
                let quote = char;
                let start = at + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != quote {
                    end += 1;
                }
                if end >= bytes.len() {
                    return Err(syntax("a string is missing its closing quote"));
                }
                let text: String = bytes[start..end].iter().collect();
                tokens.push(Token::Value(Operand::Literal(Value::String(text))));
                at = end + 1;
            }
            _ => {
                // A word: a runtime expression, a number, a keyword, or
                // an unquoted string. It runs until whitespace, a
                // bracket, or the start of an operator.
                let start = at;
                while at < bytes.len()
                    && !bytes[at].is_whitespace()
                    && !matches!(bytes[at], '(' | ')' | '&' | '|' | '=' | '!' | '<' | '>')
                {
                    at += 1;
                }
                let word: String = bytes[start..at].iter().collect();
                if word.is_empty() {
                    return Err(syntax("expected a value"));
                }
                tokens.push(Token::Value(operand(&word)));
            }
        }
    }
    if tokens.is_empty() {
        return Err(syntax("the condition is empty"));
    }
    Ok(tokens)
}

fn operand(word: &str) -> Operand {
    if expression::is_expression(word) {
        return Operand::Expression(word.to_owned());
    }
    match word {
        "true" => Operand::Literal(Value::Bool(true)),
        "false" => Operand::Literal(Value::Bool(false)),
        "null" => Operand::Literal(Value::Null),
        _ => match word.parse::<f64>() {
            Ok(number) => Operand::Literal(
                serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number),
            ),
            // The spec quotes its strings, but an unquoted word can only
            // be one, so read it as written rather than refusing.
            Err(_) => Operand::Literal(Value::String(word.to_owned())),
        },
    }
}

struct Parser<'p> {
    tokens: &'p [Token],
    at: usize,
    condition: &'p str,
    scope: &'p Scope<'p>,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> CriterionError {
        CriterionError::Syntax {
            condition: self.condition.to_owned(),
            message: message.to_owned(),
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// `a || b` — the loosest binding, so the outermost.
    fn disjunction(&mut self) -> Result<bool, CriterionError> {
        let mut holds = self.conjunction()?;
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            // Both sides are evaluated: a condition naming something
            // absent should say so rather than depend on the order.
            holds = self.conjunction()? || holds;
        }
        Ok(holds)
    }

    /// `a && b`.
    fn conjunction(&mut self) -> Result<bool, CriterionError> {
        let mut holds = self.comparison()?;
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            holds = self.comparison()? && holds;
        }
        Ok(holds)
    }

    /// `a == b`, or a lone operand read for its truth.
    fn comparison(&mut self) -> Result<bool, CriterionError> {
        if self.peek() == Some(&Token::Open) {
            self.at += 1;
            let holds = self.disjunction()?;
            if self.peek() != Some(&Token::Close) {
                return Err(self.error("a `(` is missing its `)`"));
            }
            self.at += 1;
            return Ok(holds);
        }
        let left = self.operand()?;
        let Some(&Token::Compare(comparison)) = self.peek() else {
            return Ok(truthy(&left));
        };
        self.at += 1;
        let right = self.operand()?;
        Ok(holds(comparison, &left, &right))
    }

    fn operand(&mut self) -> Result<Value, CriterionError> {
        match self.tokens.get(self.at) {
            Some(Token::Value(operand)) => {
                self.at += 1;
                match operand {
                    Operand::Literal(literal) => Ok(literal.clone()),
                    Operand::Expression(expression) => {
                        Ok(expression::evaluate(expression, self.scope)?)
                    }
                }
            }
            _ => Err(self.error("expected a value")),
        }
    }
}

fn holds(comparison: Comparison, left: &Value, right: &Value) -> bool {
    let ordering = compare(left, right);
    let equal = ordering == Some(Ordering::Equal) || left == right;
    match comparison {
        Comparison::Equal => equal,
        Comparison::NotEqual => !equal,
        Comparison::Less => ordering == Some(Ordering::Less),
        Comparison::LessOrEqual => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        Comparison::Greater => ordering == Some(Ordering::Greater),
        Comparison::GreaterOrEqual => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
    }
}

/// Order two values, when they are the sort of things that can be
/// ordered. A number written as a string still compares as a number —
/// a header carries `"200"` where a status code carries `200`, and a
/// condition means the same thing by both.
fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64()?.partial_cmp(&right.as_f64()?),
        // "String comparisons MUST be case insensitive" — so `PLACED`
        // and `placed` are the same word to a condition.
        (Value::String(left), Value::String(right)) => {
            Some(left.to_lowercase().cmp(&right.to_lowercase()))
        }
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        (Value::Number(number), Value::String(text))
        | (Value::String(text), Value::Number(number)) => {
            let text: f64 = text.parse().ok()?;
            let number = number.as_f64()?;
            if matches!(left, Value::Number(_)) {
                number.partial_cmp(&text)
            } else {
                text.partial_cmp(&number)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::tests::{Fixture, exchange};
    use roas_arazzo::v1_1::ExpressionType;
    use serde_json::json;

    fn criterion(
        condition: &str,
        context: Option<&str>,
        type_: Option<CriterionType>,
    ) -> Criterion {
        Criterion {
            context: context.map(ToOwned::to_owned),
            condition: condition.to_owned(),
            type_,
            extensions: None,
        }
    }

    fn decide(condition: &str) -> Result<bool, CriterionError> {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        passes(&criterion(condition, None, None), &fixture.scope())
    }

    #[test]
    fn a_status_code_is_compared_the_way_the_examples_write_it() {
        assert_eq!(decide("$statusCode == 200"), Ok(true));
        assert_eq!(decide("$statusCode != 200"), Ok(false));
        assert_eq!(decide("$statusCode >= 200 && $statusCode < 300"), Ok(true));
        assert_eq!(decide("$statusCode > 200"), Ok(false));
        assert_eq!(decide("$statusCode <= 200"), Ok(true));
    }

    #[test]
    fn strings_compare_quoted_either_way_and_unquoted() {
        assert_eq!(decide("$response.body#/tags/0 == 'cat'"), Ok(true));
        assert_eq!(decide(r#"$response.body#/tags/0 == "cat""#), Ok(true));
        assert_eq!(decide("$response.body#/tags/0 == cat"), Ok(true));
        assert_eq!(decide("$response.body#/tags/0 == 'dog'"), Ok(false));
    }

    #[test]
    fn a_number_written_as_text_still_compares_as_a_number() {
        assert_eq!(decide("$request.path.petId == 7"), Ok(true));
        assert_eq!(decide("$request.path.petId < 8"), Ok(true));
    }

    #[test]
    fn logic_groups_the_way_parentheses_say() {
        assert_eq!(decide("$statusCode == 500 || $statusCode == 200"), Ok(true));
        assert_eq!(
            decide("($statusCode == 500 || $statusCode == 200) && $method == GET"),
            Ok(true)
        );
        assert_eq!(
            decide("$statusCode == 500 || ($statusCode == 200 && $method == POST)"),
            Ok(false)
        );
        assert_eq!(decide("true && false"), Ok(false));
        assert_eq!(decide("true || false"), Ok(true));
    }

    #[test]
    fn an_operand_alone_is_read_for_its_truth() {
        assert_eq!(
            decide("$response.body#/tags"),
            Ok(true),
            "a non-empty array"
        );
        assert_eq!(
            decide("$request.body#/name"),
            Ok(true),
            "a non-empty string"
        );
        assert_eq!(decide("false"), Ok(false));
        assert_eq!(decide("null"), Ok(false));
        assert_eq!(decide("0"), Ok(false));
    }

    #[test]
    fn a_condition_that_does_not_parse_says_where() {
        for (condition, expected) in [
            ("", "the condition is empty"),
            ("$statusCode ==", "expected a value"),
            ("($statusCode == 200", "a `(` is missing its `)`"),
            ("$statusCode & 1", "`&` must be doubled"),
            ("'unclosed", "a string is missing its closing quote"),
            ("$statusCode == 200)", "unexpected trailing input"),
        ] {
            let error = decide(condition).unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "`{condition}`: expected {expected:?}, got {error}"
            );
        }
    }

    #[test]
    fn an_expression_that_names_nothing_is_an_error_not_a_false() {
        assert!(matches!(
            decide("$inputs.nope == 1"),
            Err(CriterionError::Expression(_))
        ));
        // Both sides of `||` are evaluated, so a broken name is not
        // hidden by a true on its left.
        assert!(matches!(
            decide("$statusCode == 200 || $inputs.nope == 1"),
            Err(CriterionError::Expression(_))
        ));
    }

    #[test]
    fn a_regex_criterion_matches_the_context() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let regex = |condition| {
            passes(
                &criterion(
                    condition,
                    Some("$response.body#/tags/0"),
                    Some(CriterionType::Simple(CriterionKind::Regex)),
                ),
                &fixture.scope(),
            )
        };
        assert_eq!(regex("^c.t$"), Ok(true));
        assert_eq!(regex("^dog$"), Ok(false));
        assert!(matches!(regex("["), Err(CriterionError::Regex { .. })));
    }

    #[test]
    fn a_jsonpath_criterion_asks_whether_anything_matches() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let path = |condition| {
            passes(
                &criterion(
                    condition,
                    Some("$response.body"),
                    Some(CriterionType::Simple(CriterionKind::Jsonpath)),
                ),
                &fixture.scope(),
            )
        };
        assert_eq!(path("$.id"), Ok(true));
        assert_eq!(path("$.nope"), Ok(false));
        assert_eq!(path("$.tags[*]"), Ok(true));
        // A filter picks members, so this asks whether any member of the
        // body is the number 7 — `id` is.
        assert_eq!(path("$[?@ == 7]"), Ok(true));
        assert_eq!(path("$[?@ == 8]"), Ok(false));
    }

    #[test]
    fn a_typed_criterion_without_a_context_says_so() {
        let fixture = Fixture::default();
        assert_eq!(
            passes(
                &criterion(
                    "^x$",
                    None,
                    Some(CriterionType::Simple(CriterionKind::Regex))
                ),
                &fixture.scope()
            ),
            Err(CriterionError::MissingContext("regex"))
        );
    }

    #[test]
    fn an_expression_typed_criterion_names_its_language() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let typed = |kind, condition| {
            passes(
                &criterion(
                    condition,
                    Some("$response.body"),
                    Some(CriterionType::Expression(ExpressionType {
                        type_: kind,
                        version: String::new(),
                        extensions: None,
                    })),
                ),
                &fixture.scope(),
            )
        };
        assert_eq!(typed(ExpressionKind::Jsonpath, "$.id"), Ok(true));
        assert_eq!(typed(ExpressionKind::Jsonpointer, "/id"), Ok(true));
        assert_eq!(typed(ExpressionKind::Jsonpointer, "/nope"), Ok(false));
        assert_eq!(
            typed(ExpressionKind::Xpath, "/id"),
            Err(CriterionError::Unsupported("XPath"))
        );
    }

    #[test]
    fn xpath_says_it_is_not_supported() {
        let fixture = Fixture::default();
        assert_eq!(
            passes(
                &criterion(
                    "/x",
                    Some("$inputs"),
                    Some(CriterionType::Simple(CriterionKind::Xpath))
                ),
                &fixture.scope()
            ),
            Err(CriterionError::Unsupported("XPath"))
        );
    }

    #[test]
    fn values_that_cannot_be_ordered_are_only_ever_equal_or_not() {
        assert!(holds(Comparison::Equal, &json!({"a": 1}), &json!({"a": 1})));
        assert!(holds(
            Comparison::NotEqual,
            &json!({"a": 1}),
            &json!({"a": 2})
        ));
        assert!(!holds(Comparison::Less, &json!({"a": 1}), &json!({"a": 2})));
        assert_eq!(compare(&json!(null), &json!(1)), None);
    }
}
