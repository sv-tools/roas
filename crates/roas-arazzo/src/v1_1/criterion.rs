//! Arazzo v1.1 `Criterion` object.
//!
//! Per [Criterion Object](https://spec.openapis.org/arazzo/v1.1.0.html#criterion-object).
//! Changed in v1.1: `type` is now either a plain string enum or an
//! [`ExpressionType`] (v1.0 used a flat `type` + `version`).

use crate::v1_1::expression_type::ExpressionType;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// The plain (string) condition types.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CriterionKind {
    #[default]
    Simple,
    Regex,
    Jsonpath,
    Xpath,
}

/// The condition type: a plain string enum, or an [`ExpressionType`]
/// when a specific expression version is needed.
///
/// Serializes untagged (a bare string or an object); deserialization is
/// hand-written so a malformed expression object surfaces its real error.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum CriterionType {
    Simple(CriterionKind),
    Expression(ExpressionType),
}

impl Default for CriterionType {
    fn default() -> Self {
        CriterionType::Simple(CriterionKind::default())
    }
}

impl<'de> Deserialize<'de> for CriterionType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_string() {
            serde_json::from_value(value)
                .map(CriterionType::Simple)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(CriterionType::Expression)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl ValidateWithContext for CriterionType {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let CriterionType::Expression(et) = self {
            et.validate_with_context(ctx);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Criterion {
    /// A runtime expression setting the context the condition applies
    /// to. Required when `type` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

    /// **Required** The condition to apply.
    pub condition: String,

    /// The condition type (defaults to `simple` when omitted).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<CriterionType>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Criterion {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("condition", &self.condition);

        // `dependentRequired`: a `type` requires a `context`.
        if self.type_.is_some() && self.context.is_none() {
            ctx.error_field("context", "is required when `type` is set");
        }

        if let Some(type_) = &self.type_ {
            ctx.in_field("type", |ctx| type_.validate_with_context(ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(c: &Criterion) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.c");
        c.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn simple_string_type_round_trips() {
        let c: Criterion =
            serde_json::from_value(json!({ "condition": "$x", "context": "$y", "type": "regex" }))
                .unwrap();
        assert_eq!(c.type_, Some(CriterionType::Simple(CriterionKind::Regex)));
        assert!(validate(&c).is_empty());
    }

    #[test]
    fn expression_type_round_trips_and_validates() {
        let c: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "$.ok",
            "type": { "type": "jsonpath", "version": "rfc9535" }
        }))
        .unwrap();
        assert!(matches!(c.type_, Some(CriterionType::Expression(_))));
        assert!(validate(&c).is_empty());
    }

    #[test]
    fn expression_type_with_bad_version_is_rejected() {
        let c: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "$.ok",
            "type": { "type": "jsonpath", "version": "nope" }
        }))
        .unwrap();
        assert!(validate(&c).iter().any(|e| e.contains(".type.version")));
    }

    #[test]
    fn criterion_type_default_is_simple() {
        assert_eq!(
            CriterionType::default(),
            CriterionType::Simple(CriterionKind::Simple)
        );
    }

    #[test]
    fn type_without_context_is_rejected() {
        let c = Criterion {
            condition: "x".into(),
            type_: Some(CriterionType::Simple(CriterionKind::Regex)),
            ..Default::default()
        };
        assert!(
            validate(&c)
                .iter()
                .any(|e| e == "#.c.context: is required when `type` is set")
        );
    }
}
