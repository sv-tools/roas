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
///
/// Downstream matches must include a fallback for future diagnostics:
///
/// ```
/// use roas_arazzo_executor::CriterionError;
/// fn condition(error: &CriterionError) -> Option<&str> {
///     match error {
///         CriterionError::Syntax { condition, .. }
///         | CriterionError::Regex { condition, .. } => Some(condition),
///         _ => None,
///     }
/// }
/// ```
///
/// Matching all currently known variants without a fallback is not supported:
///
/// ```compile_fail,E0004
/// use roas_arazzo_executor::CriterionError;
/// fn exhaustive(error: CriterionError) {
///     match error {
///         CriterionError::Expression(_)
///         | CriterionError::Select(_)
///         | CriterionError::Syntax { .. }
///         | CriterionError::Regex { .. }
///         | CriterionError::MissingContext(_)
///         | CriterionError::Unsupported(_) => {}
///     }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
        Some(CriterionType::Simple(CriterionKind::Regex)) => {
            regex(&written()?, &context("regex")?, scope)
        }
        Some(CriterionType::Simple(CriterionKind::Jsonpath)) => {
            selects(Language::Path, &written()?, &context("jsonpath")?, scope)
        }
        Some(CriterionType::Simple(CriterionKind::Xpath)) => {
            Err(CriterionError::Unsupported("XPath"))
        }
        Some(CriterionType::Expression(expression)) => match expression.type_ {
            ExpressionKind::Jsonpath => {
                selects(Language::Path, &written()?, &context("jsonpath")?, scope)
            }
            ExpressionKind::Jsonpointer => selects(
                Language::Pointer,
                &written()?,
                &context("jsonpointer")?,
                scope,
            ),
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
fn selects(
    language: Language,
    condition: &str,
    context: &Value,
    scope: &Scope<'_>,
) -> Result<bool, CriterionError> {
    let selected = select::apply(language, condition, context, scope.compiled)?;
    // A null JSONPath context fails, but a selected node containing null in
    // a non-null document still counts. JSON Pointer is a separate extension.
    Ok(selected.is_some() && (language != Language::Path || !context.is_null()))
}

pub(crate) fn compile_regex(condition: &str) -> Result<regex::Regex, CriterionError> {
    #[cfg(test)]
    crate::prepare::instrumentation::compiled(2);
    regex::Regex::new(condition).map_err(|error| CriterionError::Regex {
        condition: condition.to_owned(),
        message: error.to_string(),
    })
}

fn regex(condition: &str, context: &Value, scope: &Scope<'_>) -> Result<bool, CriterionError> {
    let owned;
    let regex = match scope
        .compiled
        .and_then(|compiled| compiled.regexes.get(condition))
    {
        Some(regex) => regex,
        None => {
            owned = compile_regex(condition)?;
            &owned
        }
    };
    Ok(!context.is_null() && regex.is_match(&text(context)))
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

// ---- the simple condition language --------------------------------

// Parsing borrows syntax, never runtime values.
#[derive(Clone, Debug)]
pub(crate) enum Condition<'a> {
    Operand(Operand<'a>),
    Not(Box<Condition<'a>>),
    Compare(Comparison, Box<Condition<'a>>, Box<Condition<'a>>),
    And(Vec<Condition<'a>>),
    Or(Vec<Condition<'a>>),
}

#[derive(Clone, Debug)]
pub(crate) enum Operand<'a> {
    Runtime {
        base: crate::runtime_syntax::Expression<'a>,
        navigation: Vec<Access<'a>>,
        text: &'a str,
        /// Byte offset in the original condition, retained when the AST is reused.
        offset: usize,
    },
    Literal(Value),
}

#[derive(Clone, Debug)]
pub(crate) struct Access<'a> {
    offset: usize,
    kind: AccessKind<'a>,
}

#[derive(Clone, Debug)]
enum AccessKind<'a> {
    Member(&'a str),
    Index(usize),
}

#[derive(Clone, Debug)]
struct Token<'a> {
    kind: TokenKind<'a>,
    offset: usize,
}

#[derive(Clone, Debug)]
enum TokenKind<'a> {
    Open,
    Close,
    Not,
    And,
    Or,
    Compare(Comparison),
    Value(Operand<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

/// Visit dependencies even in a branch whose value will be short-circuited.
/// A syntax error is an error here too, never an empty list of dependencies.
pub(crate) fn expressions_in(
    condition: &str,
) -> Result<Vec<crate::runtime_syntax::Expression<'_>>, CriterionError> {
    let tree = parse(condition)?;
    let mut found = Vec::new();
    tree.expressions(&mut found);
    Ok(found)
}

/// Parse the expression-bearing parts of a criterion without evaluating them.
/// Callers decide whether these reads describe a step dependency or only need
/// syntax validation (for example, a workflow-wide action).
pub(crate) fn references(
    criterion: &Criterion,
) -> Result<Vec<crate::runtime_syntax::Expression<'_>>, CriterionError> {
    let mut found = Vec::new();
    if let Some(context) = &criterion.context {
        found.push(crate::runtime_syntax::parse(context)?);
    }
    if matches!(
        criterion.type_,
        None | Some(CriterionType::Simple(CriterionKind::Simple))
    ) {
        found.extend(expressions_in(&criterion.condition)?);
    } else {
        // Regex anchors and JSONPath roots are not runtime expressions.
        for reference in expression::interpolations(&criterion.condition) {
            found.push(crate::runtime_syntax::parse(reference)?);
        }
    }
    Ok(found)
}

impl<'a> Condition<'a> {
    pub(crate) fn expressions(&self, found: &mut Vec<crate::runtime_syntax::Expression<'a>>) {
        self.visit_expressions(&mut |base, _| found.push(base.clone()));
    }

    pub(crate) fn visit_expressions(
        &self,
        visit: &mut impl FnMut(&crate::runtime_syntax::Expression<'a>, usize),
    ) {
        match self {
            Self::Operand(Operand::Runtime { base, offset, .. }) => visit(base, *offset),
            Self::Operand(Operand::Literal(_)) => {}
            Self::Not(inner) => inner.visit_expressions(visit),
            Self::Compare(_, left, right) => {
                left.visit_expressions(visit);
                right.visit_expressions(visit);
            }
            Self::And(items) | Self::Or(items) => {
                for item in items {
                    item.visit_expressions(visit);
                }
            }
        }
    }

    fn evaluate(&self, scope: &Scope<'_>) -> Result<Value, CriterionError> {
        Ok(match self {
            Self::Operand(operand) => operand.evaluate(scope)?,
            Self::Not(inner) => Value::Bool(!truthy(&inner.evaluate(scope)?)),
            Self::Compare(comparison, left, right) => Value::Bool(holds(
                *comparison,
                &left.evaluate(scope)?,
                &right.evaluate(scope)?,
            )),
            Self::And(items) => {
                for item in items {
                    if !truthy(&item.evaluate(scope)?) {
                        return Ok(Value::Bool(false));
                    }
                }
                Value::Bool(true)
            }
            Self::Or(items) => {
                for item in items {
                    if truthy(&item.evaluate(scope)?) {
                        return Ok(Value::Bool(true));
                    }
                }
                Value::Bool(false)
            }
        })
    }
}

impl Operand<'_> {
    fn evaluate(&self, scope: &Scope<'_>) -> Result<Value, CriterionError> {
        let (base, navigation, text) = match self {
            Self::Literal(value) => return Ok(value.clone()),
            Self::Runtime {
                base,
                navigation,
                text,
                ..
            } => (base, navigation, text),
        };
        let value = expression::evaluate_parsed(base, scope)?;
        let mut current = &value;
        for access in navigation {
            let invalid = |message: &str| ExpressionError::Navigation {
                expression: (*text).to_owned(),
                offset: access.offset,
                message: message.to_owned(),
            };
            let (next, name) = match access.kind {
                AccessKind::Member(name) => {
                    let object = current
                        .as_object()
                        .ok_or_else(|| invalid("property access requires an object"))?;
                    (object.get(name), format!("property `{name}`"))
                }
                AccessKind::Index(index) => {
                    let array = current
                        .as_array()
                        .ok_or_else(|| invalid("index access requires an array"))?;
                    (array.get(index), format!("array index `{index}`"))
                }
            };
            current = next.ok_or_else(|| ExpressionError::Missing {
                expression: (*text).to_owned(),
                what: format!("{name} at byte {}, which is absent", access.offset),
            })?;
        }
        Ok(current.clone())
    }
}

fn syntax(condition: &str, offset: usize, message: &str) -> CriterionError {
    CriterionError::Syntax {
        condition: condition.to_owned(),
        message: format!("at byte {offset}: {message}"),
    }
}

fn simple(condition: &str, scope: &Scope<'_>) -> Result<bool, CriterionError> {
    if let Some(tree) = scope
        .compiled
        .and_then(|compiled| compiled.conditions.get(condition))
    {
        return Ok(truthy(&tree.evaluate(scope)?));
    }
    let tree = parse(condition)?;
    let mut expressions = Vec::new();
    tree.expressions(&mut expressions);
    for expression in expressions {
        expression::check_reference(&expression, scope)?;
    }
    Ok(truthy(&tree.evaluate(scope)?))
}

pub(crate) fn parse(condition: &str) -> Result<Condition<'_>, CriterionError> {
    #[cfg(test)]
    crate::prepare::instrumentation::compiled(1);
    let tokens = tokenize(condition)?;
    let mut parser = Parser {
        tokens: tokens.into_iter().peekable(),
        condition,
        depth: 0,
    };
    let tree = parser.disjunction()?;
    if parser.tokens.peek().is_some() {
        return Err(
            parser.error("unexpected trailing input (chained comparisons are not supported)")
        );
    }
    Ok(tree)
}

impl Condition<'_> {
    /// Bare values rely on the executor's truthiness profile, not a portable
    /// Arazzo boolean-coercion table. Comparisons and boolean literals do not.
    pub(crate) fn uses_bare_values(&self) -> bool {
        match self {
            Self::Operand(Operand::Literal(Value::Bool(_))) | Self::Compare(..) => false,
            Self::Operand(_) => true,
            Self::Not(inner) => inner.uses_bare_values(),
            Self::And(items) | Self::Or(items) => items.iter().any(Self::uses_bare_values),
        }
    }
}

fn tokenize(condition: &str) -> Result<Vec<Token<'_>>, CriterionError> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(c) = condition[at..].chars().next() {
        let offset = at;
        let kind = match c {
            c if c.is_whitespace() => {
                at += c.len_utf8();
                continue;
            }
            '(' => {
                at += 1;
                TokenKind::Open
            }
            ')' => {
                at += 1;
                TokenKind::Close
            }
            '&' | '|' => {
                if condition.as_bytes().get(at + 1) != Some(&(c as u8)) {
                    return Err(syntax(
                        condition,
                        at,
                        &format!(
                            "`{c}` must be doubled; operator characters delimit runtime operands"
                        ),
                    ));
                }
                at += 2;
                if c == '&' {
                    TokenKind::And
                } else {
                    TokenKind::Or
                }
            }
            '!' if condition.as_bytes().get(at + 1) != Some(&b'=') => {
                at += 1;
                TokenKind::Not
            }
            '=' | '!' | '<' | '>' => {
                let doubled = condition.as_bytes().get(at + 1) == Some(&b'=');
                let comparison = match (c, doubled) {
                    ('=', true) => Comparison::Equal,
                    ('!', true) => Comparison::NotEqual,
                    ('<', true) => Comparison::LessOrEqual,
                    ('>', true) => Comparison::GreaterOrEqual,
                    ('<', false) => Comparison::Less,
                    ('>', false) => Comparison::Greater,
                    _ => {
                        return Err(syntax(
                            condition,
                            at,
                            "`=` must be followed by `=`; an operator inside a runtime name or pointer is not supported in a simple operand",
                        ));
                    }
                };
                at += if doubled { 2 } else { 1 };
                TokenKind::Compare(comparison)
            }
            '\'' | '"' => {
                let quote = c;
                at += 1;
                let mut value = String::new();
                loop {
                    let Some(c) = condition[at..].chars().next() else {
                        return Err(syntax(
                            condition,
                            offset,
                            "a string is missing its closing quote",
                        ));
                    };
                    at += c.len_utf8();
                    if c == quote {
                        if condition[at..].starts_with(quote) {
                            at += 1;
                        } else {
                            break;
                        }
                    }
                    value.push(c);
                }
                TokenKind::Value(Operand::Literal(Value::String(value)))
            }
            _ => {
                at += condition[at..]
                    .find(|c: char| {
                        c.is_whitespace()
                            || matches!(c, '(' | ')' | '&' | '|' | '=' | '!' | '<' | '>')
                    })
                    .unwrap_or(condition.len() - at);
                TokenKind::Value(operand(&condition[offset..at], condition, offset)?)
            }
        };
        tokens.push(Token { kind, offset });
    }
    if tokens.is_empty() {
        return Err(syntax(condition, 0, "the condition is empty"));
    }
    Ok(tokens)
}

fn operand<'a>(
    word: &'a str,
    condition: &str,
    offset: usize,
) -> Result<Operand<'a>, CriterionError> {
    if expression::is_expression(word) {
        let (base, mut at) = crate::runtime_syntax::prefix(word).map_err(|error| {
            let relative = match &error {
                ExpressionError::Syntax { offset, .. } => *offset,
                _ => 0,
            };
            syntax(condition, offset + relative, &error.to_string())
        })?;
        let mut navigation = Vec::new();
        while at < word.len() {
            let start = at;
            let kind = match word.as_bytes()[at] {
                b'.' => {
                    at += 1;
                    let name = at;
                    at += word[at..]
                        .bytes()
                        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                        .count();
                    if name == at {
                        return Err(syntax(condition, offset + at, "expected a property name"));
                    }
                    AccessKind::Member(&word[name..at])
                }
                b'[' => {
                    at += 1;
                    let index = at;
                    at += word[at..].bytes().take_while(u8::is_ascii_digit).count();
                    let digits = &word[index..at];
                    if digits.is_empty()
                        || (digits.len() > 1 && digits.starts_with('0'))
                        || word.as_bytes().get(at) != Some(&b']')
                    {
                        return Err(syntax(
                            condition,
                            offset + start,
                            "expected a zero-based integer index `[0]`",
                        ));
                    }
                    let index = digits.parse().map_err(|_| {
                        syntax(condition, offset + start, "array index is too large")
                    })?;
                    at += 1;
                    AccessKind::Index(index)
                }
                _ => {
                    return Err(syntax(
                        condition,
                        offset + at,
                        "unexpected runtime operand suffix",
                    ));
                }
            };
            navigation.push(Access {
                offset: start,
                kind,
            });
        }
        return Ok(Operand::Runtime {
            base,
            navigation,
            text: word,
            offset,
        });
    }
    let literal = match word {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ if word.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
            || matches!(word, "NaN" | "inf" | "Infinity") =>
        {
            let number = serde_json::from_str::<serde_json::Number>(word).map_err(|_| {
                syntax(
                    condition,
                    offset,
                    "expected a finite JSON number; quote numeric-looking strings",
                )
            })?;
            // Cargo feature unification can enable serde_json's arbitrary
            // precision, which also accepts numbers outside finite f64 range.
            if !number.as_f64().is_some_and(f64::is_finite) {
                return Err(syntax(condition, offset, "expected a finite JSON number"));
            }
            Value::Number(number)
        }
        // Compatibility extension: non-numeric unquoted words are strings.
        _ => Value::String(word.to_owned()),
    };
    Ok(Operand::Literal(literal))
}

struct Parser<'a> {
    tokens: std::iter::Peekable<std::vec::IntoIter<Token<'a>>>,
    condition: &'a str,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn error(&mut self, message: &str) -> CriterionError {
        syntax(
            self.condition,
            self.tokens
                .peek()
                .map_or(self.condition.len(), |token| token.offset),
            message,
        )
    }

    fn disjunction(&mut self) -> Result<Condition<'a>, CriterionError> {
        let first = self.conjunction()?;
        if !matches!(self.tokens.peek().map(|t| &t.kind), Some(TokenKind::Or)) {
            return Ok(first);
        }
        let mut items = vec![first];
        while matches!(self.tokens.peek().map(|t| &t.kind), Some(TokenKind::Or)) {
            self.tokens.next();
            items.push(self.conjunction()?);
        }
        Ok(Condition::Or(items))
    }

    fn conjunction(&mut self) -> Result<Condition<'a>, CriterionError> {
        let first = self.comparison()?;
        if !matches!(self.tokens.peek().map(|t| &t.kind), Some(TokenKind::And)) {
            return Ok(first);
        }
        let mut items = vec![first];
        while matches!(self.tokens.peek().map(|t| &t.kind), Some(TokenKind::And)) {
            self.tokens.next();
            items.push(self.comparison()?);
        }
        Ok(Condition::And(items))
    }

    fn comparison(&mut self) -> Result<Condition<'a>, CriterionError> {
        let left = self.unary()?;
        let Some(Token {
            kind: TokenKind::Compare(comparison),
            ..
        }) = self.tokens.peek()
        else {
            return Ok(left);
        };
        let comparison = *comparison;
        self.tokens.next();
        Ok(Condition::Compare(
            comparison,
            Box::new(left),
            Box::new(self.unary()?),
        ))
    }

    fn unary(&mut self) -> Result<Condition<'a>, CriterionError> {
        let mut negate = false;
        let mut has_negation = false;
        while matches!(self.tokens.peek().map(|t| &t.kind), Some(TokenKind::Not)) {
            self.tokens.next();
            negate = !negate;
            has_negation = true;
        }
        let value = match self.tokens.next() {
            Some(Token {
                kind: TokenKind::Value(value),
                ..
            }) => Condition::Operand(value),
            Some(Token {
                kind: TokenKind::Open,
                ..
            }) => {
                // Bound recursion for untrusted documents, including evaluation
                // and destruction of the tree. Logical chains remain flat.
                if self.depth >= 64 {
                    return Err(self.error("condition nesting exceeds 64 groups"));
                }
                self.depth += 1;
                let value = self.disjunction()?;
                self.depth -= 1;
                if !matches!(self.tokens.peek().map(|t| &t.kind), Some(TokenKind::Close)) {
                    return Err(self.error("a `(` is missing its `)`"));
                }
                self.tokens.next();
                value
            }
            Some(token) => return Err(syntax(self.condition, token.offset, "expected a value")),
            None => return Err(self.error("expected a value")),
        };
        Ok(if negate {
            Condition::Not(Box::new(value))
        } else if has_negation {
            Condition::Not(Box::new(Condition::Not(Box::new(value))))
        } else {
            value
        })
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
        (Value::Number(left), Value::Number(right)) => compare_numbers(left, right),
        // "String comparisons MUST be case insensitive" — so `PLACED`
        // and `placed` are the same word to a condition.
        (Value::String(left), Value::String(right)) => {
            Some(left.to_lowercase().cmp(&right.to_lowercase()))
        }
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        (Value::Number(number), Value::String(text))
        | (Value::String(text), Value::Number(number)) => {
            let text = text
                .parse::<serde_json::Number>()
                .ok()
                .or_else(|| serde_json::Number::from_f64(text.parse().ok()?))?;
            if matches!(left, Value::Number(_)) {
                compare_numbers(number, &text)
            } else {
                compare_numbers(&text, number)
            }
        }
        _ => None,
    }
}

fn compare_numbers(left: &serde_json::Number, right: &serde_json::Number) -> Option<Ordering> {
    if let (Some(left), Some(right)) = (left.as_i64(), right.as_i64()) {
        return Some(left.cmp(&right));
    }
    if let (Some(left), Some(right)) = (left.as_u64(), right.as_u64()) {
        return Some(left.cmp(&right));
    }
    if left.is_i64() && right.is_u64() {
        return Some(Ordering::Less);
    }
    if left.is_u64() && right.is_i64() {
        return Some(Ordering::Greater);
    }
    left.as_f64()?.partial_cmp(&right.as_f64()?)
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
        // Parsing is unconditional, but a guarded missing runtime value
        // need not be resolved.
        assert_eq!(decide("$statusCode == 200 || $inputs.nope == 1"), Ok(true));
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
