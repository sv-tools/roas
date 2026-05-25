//! Arazzo v1.1 `Selector` object plus its supporting types.
//!
//! Per [Selector Object](https://spec.openapis.org/arazzo/v1.1.0.html#selector-object):
//! fine-grained data selection from structured data. New in v1.1 and
//! usable anywhere a value or output is expected (see [`ValueOrSelector`]).

use crate::v1_1::expression_type::ExpressionType;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// The plain (string) selector expression languages.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SelectorKind {
    #[default]
    Jsonpointer,
    Jsonpath,
    Xpath,
}

/// A selector expression type: a plain string enum, or an
/// [`ExpressionType`] when a specific version is needed.
///
/// Serializes untagged (a bare string or an object). Deserialization is
/// hand-written: a JSON string becomes [`SelectorType::Simple`] and an
/// object becomes [`SelectorType::Expression`], so a malformed
/// expression object surfaces its real error rather than the opaque
/// untagged-enum message.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SelectorType {
    Simple(SelectorKind),
    Expression(ExpressionType),
}

impl Default for SelectorType {
    fn default() -> Self {
        SelectorType::Simple(SelectorKind::default())
    }
}

impl<'de> Deserialize<'de> for SelectorType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_string() {
            serde_json::from_value(value)
                .map(SelectorType::Simple)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(SelectorType::Expression)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl ValidateWithContext for SelectorType {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let SelectorType::Expression(et) = self {
            et.validate_with_context(ctx);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Selector {
    /// **Required** Runtime expression that evaluates to the structured
    /// data the selector applies to (e.g. `$response.body`).
    pub context: String,

    /// **Required** The selector expression (JSONPath / XPath / JSON
    /// Pointer).
    pub selector: String,

    /// **Required** The selector expression type.
    #[serde(rename = "type")]
    pub type_: SelectorType,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Selector {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("context", &self.context);
        ctx.require_non_empty("selector", &self.selector);
        ctx.in_field("type", |ctx| self.type_.validate_with_context(ctx));
    }
}

/// A literal value or a [`Selector`]. Used by `Parameter.value`,
/// `PayloadReplacement.value`, and workflow / step `outputs` values,
/// all of which became `value | selector` in v1.1.
///
/// Deserialization is hand-written: an object carrying a `selector` key
/// becomes [`ValueOrSelector::Selector`] (with precise errors), anything
/// else a literal.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ValueOrSelector {
    Selector(Selector),
    Literal(serde_json::Value),
}

impl Default for ValueOrSelector {
    fn default() -> Self {
        ValueOrSelector::Literal(serde_json::Value::Null)
    }
}

impl ValueOrSelector {
    /// Wrap a literal value (used by the v1.0 → v1.1 upconversion).
    pub fn literal(value: impl Into<serde_json::Value>) -> Self {
        ValueOrSelector::Literal(value.into())
    }
}

impl<'de> Deserialize<'de> for ValueOrSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("selector").is_some() {
            serde_json::from_value(value)
                .map(ValueOrSelector::Selector)
                .map_err(serde::de::Error::custom)
        } else {
            Ok(ValueOrSelector::Literal(value))
        }
    }
}

impl ValidateWithContext for ValueOrSelector {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let ValueOrSelector::Selector(s) = self {
            s.validate_with_context(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn selector_type_string_is_simple() {
        let st: SelectorType = serde_json::from_value(json!("jsonpath")).unwrap();
        assert_eq!(st, SelectorType::Simple(SelectorKind::Jsonpath));
    }

    #[test]
    fn selector_type_object_is_expression() {
        let st: SelectorType =
            serde_json::from_value(json!({ "type": "xpath", "version": "xpath-30" })).unwrap();
        assert!(matches!(st, SelectorType::Expression(_)));
    }

    #[test]
    fn selector_round_trips_and_validates() {
        let s: Selector = serde_json::from_value(json!({
            "context": "$response.body",
            "selector": "$.id",
            "type": "jsonpath",
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.sel");
        s.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());

        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["type"], json!("jsonpath"));
    }

    #[test]
    fn selector_requires_context_and_selector() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.sel");
        Selector::default().validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(msgs.iter().any(|e| e == "#.sel.context: must not be empty"));
        assert!(
            msgs.iter()
                .any(|e| e == "#.sel.selector: must not be empty")
        );
    }

    #[test]
    fn value_or_selector_picks_literal_for_plain_value() {
        let v: ValueOrSelector = serde_json::from_value(json!("$inputs.x")).unwrap();
        assert_eq!(v, ValueOrSelector::Literal(json!("$inputs.x")));
    }

    #[test]
    fn value_or_selector_picks_selector_for_selector_object() {
        let v: ValueOrSelector = serde_json::from_value(json!({
            "context": "$response.body",
            "selector": "$.id",
            "type": "jsonpath",
        }))
        .unwrap();
        assert!(matches!(v, ValueOrSelector::Selector(_)));
    }

    #[test]
    fn value_or_selector_malformed_selector_surfaces_inner_error() {
        // Has a `selector` key so it dispatches to Selector, then fails
        // on the missing required `context`/`type` with a real error.
        let err =
            serde_json::from_value::<ValueOrSelector>(json!({ "selector": "$.id" })).unwrap_err();
        assert!(err.to_string().contains("missing field"), "got: {err}");
    }
}
