//! Turning the places a value can come from into a value.
//!
//! A v1.1 `value` is either a literal (which may hold runtime
//! expressions) or a [`Selector`] naming structured data and an
//! expression that picks from it. Both end up here, as does the
//! `target` of a payload replacement.
//!
//! JSON Pointer and JSONPath are supported; XPath is not, and says so
//! rather than quietly selecting nothing.

use crate::expression::{self, ExpressionError, Scope};
use roas_arazzo::v1_1::{ExpressionKind, Selector, SelectorKind, SelectorType, ValueOrSelector};
use serde_json::Value;
use serde_json_path::JsonPath;

/// Why a value could not be produced.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
    /// A runtime expression in the value could not be evaluated.
    #[error(transparent)]
    Expression(#[from] ExpressionError),
    /// The selector expression itself is malformed.
    #[error("`{selector}` is not a valid {kind} expression: {message}")]
    Malformed {
        /// The expression as written.
        selector: String,
        /// The language it was read as.
        kind: &'static str,
        /// What the parser said.
        message: String,
    },
    /// The selector is valid but picked nothing.
    #[error("`{selector}` selects nothing from `{context}`")]
    Empty {
        /// The expression as written.
        selector: String,
        /// The runtime expression naming what it applied to.
        context: String,
    },
    /// XPath: no engine here, and pretending otherwise would be worse.
    #[error("{0} expressions are not supported by this executor")]
    Unsupported(&'static str),
}

/// The value a `value | selector` position holds.
pub(crate) fn value_of(value: &ValueOrSelector, scope: &Scope<'_>) -> Result<Value, SelectError> {
    match value {
        ValueOrSelector::Literal(literal) => resolve(literal, scope),
        ValueOrSelector::Selector(selector) => select(selector, scope),
    }
}

/// A literal, with every runtime expression inside it replaced.
///
/// A string that *is* an expression becomes whatever the expression
/// produced, keeping its type — `$statusCode` is a number, not `"200"`.
/// A string that merely *contains* one is interpolated. Objects and
/// arrays are walked, so a request payload written with expressions
/// inside it comes out filled in.
pub(crate) fn resolve(value: &Value, scope: &Scope<'_>) -> Result<Value, SelectError> {
    Ok(match value {
        Value::String(text) if expression::is_expression(text) => {
            expression::evaluate(text, scope)?
        }
        Value::String(text) if text.contains("{$") => {
            Value::String(expression::interpolate(text, scope)?)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve(item, scope))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| Ok((name.clone(), resolve(member, scope)?)))
                .collect::<Result<_, SelectError>>()?,
        ),
        other => other.clone(),
    })
}

/// What a selector picks out of the data its context names.
pub(crate) fn select(selector: &Selector, scope: &Scope<'_>) -> Result<Value, SelectError> {
    let context = expression::evaluate(&selector.context, scope)?;
    let picked = apply(kind_of(&selector.type_)?, &selector.selector, &context)?;
    picked.ok_or_else(|| SelectError::Empty {
        selector: selector.selector.clone(),
        context: selector.context.clone(),
    })
}

/// The language a selector or replacement target is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    Pointer,
    Path,
}

/// Read a `SelectorType` as a language this crate can apply.
pub(crate) fn kind_of(type_: &SelectorType) -> Result<Language, SelectError> {
    match type_ {
        SelectorType::Simple(SelectorKind::Jsonpointer) => Ok(Language::Pointer),
        SelectorType::Simple(SelectorKind::Jsonpath) => Ok(Language::Path),
        SelectorType::Simple(SelectorKind::Xpath) => Err(SelectError::Unsupported("XPath")),
        SelectorType::Expression(expression) => match expression.type_ {
            ExpressionKind::Jsonpointer => Ok(Language::Pointer),
            ExpressionKind::Jsonpath => Ok(Language::Path),
            ExpressionKind::Xpath => Err(SelectError::Unsupported("XPath")),
        },
    }
}

/// Apply `selector` to `data`. `None` when it picked nothing.
pub(crate) fn apply(
    language: Language,
    selector: &str,
    data: &Value,
) -> Result<Option<Value>, SelectError> {
    match language {
        Language::Pointer => {
            // A pointer may be written with the `#` that would precede
            // it in a URI fragment.
            let pointer = selector.strip_prefix('#').unwrap_or(selector);
            Ok(data.pointer(pointer).cloned())
        }
        Language::Path => {
            let path = JsonPath::parse(selector).map_err(|error| SelectError::Malformed {
                selector: selector.to_owned(),
                kind: "JSONPath",
                message: error.to_string(),
            })?;
            let nodes = path.query(data);
            Ok(match nodes.len() {
                0 => None,
                // One node is the value; several are the list of them,
                // which is what a JSONPath that matches many means.
                1 => nodes.first().cloned(),
                _ => Some(Value::Array(
                    nodes.iter().map(|&node| node.clone()).collect(),
                )),
            })
        }
    }
}

/// Put `value` where `target` points inside `data`.
///
/// A payload replacement writes into the body the step is about to
/// send, so the target has to be somewhere that body has — or somewhere
/// its parent object can take a new member.
pub(crate) fn place(
    language: Language,
    target: &str,
    data: &mut Value,
    value: Value,
) -> Result<(), String> {
    let pointer = match language {
        Language::Pointer => target.strip_prefix('#').unwrap_or(target).to_owned(),
        Language::Path => {
            let path = JsonPath::parse(target)
                .map_err(|error| format!("`{target}` is not a valid JSONPath: {error}"))?;
            path.query_located(data)
                .locations()
                .next()
                .map(|location| location.to_json_pointer())
                .ok_or_else(|| format!("`{target}` matches nothing in the payload"))?
        }
    };
    if let Some(slot) = data.pointer_mut(&pointer) {
        *slot = value;
        return Ok(());
    }
    // A pointer may name a member that is not there yet, which is how a
    // replacement adds one.
    let (parent, member) = pointer
        .rsplit_once('/')
        .ok_or_else(|| format!("`{target}` does not point anywhere in the payload"))?;
    match data.pointer_mut(parent) {
        Some(Value::Object(members)) => {
            members.insert(member.replace("~1", "/").replace("~0", "~"), value);
            Ok(())
        }
        _ => Err(format!("`{target}` points into nothing the payload has")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::tests::{Fixture, exchange};
    use serde_json::json;

    fn selector(context: &str, selector: &str, kind: SelectorKind) -> Selector {
        Selector {
            context: context.to_owned(),
            selector: selector.to_owned(),
            type_: SelectorType::Simple(kind),
            extensions: None,
        }
    }

    #[test]
    fn a_literal_keeps_the_type_the_expression_produced() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let scope = fixture.scope();
        assert_eq!(
            value_of(&ValueOrSelector::literal("$statusCode"), &scope),
            Ok(json!(200)),
            "a whole expression keeps its own type"
        );
        assert_eq!(
            value_of(&ValueOrSelector::literal("id-{$inputs.petId}"), &scope),
            Ok(json!("id-7")),
            "an expression inside text makes text"
        );
        assert_eq!(
            value_of(&ValueOrSelector::literal("plain"), &scope),
            Ok(json!("plain"))
        );
        assert_eq!(
            value_of(&ValueOrSelector::literal(42), &scope),
            Ok(json!(42))
        );
    }

    #[test]
    fn a_payload_is_filled_in_wherever_an_expression_sits() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let payload = json!({
            "pet": { "id": "$response.body#/id" },
            "tags": ["$response.body#/tags/0", "plain"],
            "count": 1,
        });
        assert_eq!(
            resolve(&payload, &fixture.scope()),
            Ok(json!({
                "pet": { "id": 7 },
                "tags": ["cat", "plain"],
                "count": 1,
            }))
        );
    }

    #[test]
    fn a_pointer_selector_picks_from_its_context() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let scope = fixture.scope();
        assert_eq!(
            select(
                &selector("$response.body", "/tags/0", SelectorKind::Jsonpointer),
                &scope
            ),
            Ok(json!("cat"))
        );
        // Written the way a URI fragment spells it.
        assert_eq!(
            select(
                &selector("$response.body", "#/id", SelectorKind::Jsonpointer),
                &scope
            ),
            Ok(json!(7))
        );
        assert!(matches!(
            select(
                &selector("$response.body", "/nope", SelectorKind::Jsonpointer),
                &scope
            ),
            Err(SelectError::Empty { .. })
        ));
    }

    #[test]
    fn a_path_selector_picks_one_node_or_the_list_of_them() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let scope = fixture.scope();
        assert_eq!(
            select(
                &selector("$response.body", "$.id", SelectorKind::Jsonpath),
                &scope
            ),
            Ok(json!(7)),
            "one node is the value itself"
        );
        assert_eq!(
            select(
                &selector("$response.body", "$.tags[*]", SelectorKind::Jsonpath),
                &scope
            ),
            Ok(json!("cat")),
            "a single match stays a single value"
        );
        assert!(matches!(
            select(
                &selector("$response.body", "$.nope", SelectorKind::Jsonpath),
                &scope
            ),
            Err(SelectError::Empty { .. })
        ));
        assert!(matches!(
            select(
                &selector("$response.body", "$[", SelectorKind::Jsonpath),
                &scope
            ),
            Err(SelectError::Malformed { .. })
        ));
    }

    #[test]
    fn several_nodes_come_back_as_the_list_of_them() {
        let data = json!({ "tags": ["cat", "small"] });
        assert_eq!(
            apply(Language::Path, "$.tags[*]", &data),
            Ok(Some(json!(["cat", "small"])))
        );
    }

    #[test]
    fn xpath_says_it_is_not_supported() {
        assert_eq!(
            kind_of(&SelectorType::Simple(SelectorKind::Xpath)),
            Err(SelectError::Unsupported("XPath"))
        );
        let fixture = Fixture::default();
        assert_eq!(
            select(
                &selector("$inputs", "/x", SelectorKind::Xpath),
                &fixture.scope()
            ),
            Err(SelectError::Unsupported("XPath"))
        );
    }

    #[test]
    fn an_expression_type_names_the_same_languages() {
        use roas_arazzo::v1_1::ExpressionType;
        let typed = |kind| {
            SelectorType::Expression(ExpressionType {
                type_: kind,
                version: String::new(),
                extensions: None,
            })
        };
        assert_eq!(
            kind_of(&typed(ExpressionKind::Jsonpointer)),
            Ok(Language::Pointer)
        );
        assert_eq!(
            kind_of(&typed(ExpressionKind::Jsonpath)),
            Ok(Language::Path)
        );
        assert_eq!(
            kind_of(&typed(ExpressionKind::Xpath)),
            Err(SelectError::Unsupported("XPath"))
        );
    }

    #[test]
    fn a_selector_whose_context_is_not_there_says_which_part_failed() {
        let fixture = Fixture::default();
        assert!(matches!(
            select(
                &selector("$response.body", "/id", SelectorKind::Jsonpointer),
                &fixture.scope()
            ),
            Err(SelectError::Expression(_))
        ));
    }
}
