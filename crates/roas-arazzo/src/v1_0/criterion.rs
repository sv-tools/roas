//! Arazzo v1.0 `Criterion` object.
//!
//! Per [Criterion Object](https://spec.openapis.org/arazzo/v1.0.1.html#criterion-object):
//! an assertion used in step `successCriteria` and action `criteria`.
//!
//! The schema folds the *Criterion Expression Type Object* into the
//! criterion via `anyOf`, so `type` and `version` are flat optional
//! fields here rather than a nested object.

use crate::validation::{Context, ValidateWithContext, validate_required_string};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The type of condition expressed by a [`Criterion`].
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CriterionType {
    #[default]
    Simple,
    Regex,
    Jsonpath,
    Xpath,
}

/// Required `version` for `type: jsonpath` (per the schema `const`).
const JSONPATH_VERSION: &str = "draft-goessner-dispatch-jsonpath-00";
/// Allowed `version` values for `type: xpath`.
const XPATH_VERSIONS: [&str; 3] = ["xpath-10", "xpath-20", "xpath-30"];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Criterion {
    /// A runtime expression setting the context the condition applies
    /// to. Required when `type` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

    /// **Required** The condition to apply.
    pub condition: String,

    /// The type of condition (defaults to `simple` when omitted).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<CriterionType>,

    /// A shorthand string for the expression-type version. Only valid
    /// with `type: jsonpath` or `type: xpath`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Criterion {
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        validate_required_string(&self.condition, ctx, format!("{path}.condition"));

        // `dependentRequired`: a `type` requires a `context`.
        if self.type_.is_some() && self.context.is_none() {
            ctx.error(format!("{path}.context"), "is required when `type` is set");
        }

        // `version` belongs to the expression-type form (jsonpath/xpath)
        // and must match the value allowed for that type.
        if let Some(version) = &self.version {
            match self.type_ {
                Some(CriterionType::Jsonpath) if version != JSONPATH_VERSION => {
                    ctx.error(
                        format!("{path}.version"),
                        format!("must be `{JSONPATH_VERSION}` for type `jsonpath`"),
                    );
                }
                Some(CriterionType::Xpath) if !XPATH_VERSIONS.contains(&version.as_str()) => {
                    ctx.error(
                        format!("{path}.version"),
                        "must be one of `xpath-10`, `xpath-20`, `xpath-30` for type `xpath`",
                    );
                }
                Some(CriterionType::Jsonpath | CriterionType::Xpath) => {}
                _ => ctx.error(
                    format!("{path}.version"),
                    "is only valid with type `jsonpath` or `xpath`",
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(c: &Criterion) -> Vec<String> {
        let mut ctx = Context::new(EnumSet::empty());
        c.validate_with_context(&mut ctx, "#.c".into());
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn simple_condition_round_trips() {
        let c: Criterion =
            serde_json::from_value(json!({ "condition": "$statusCode == 200" })).unwrap();
        assert_eq!(c.condition, "$statusCode == 200");
        assert!(c.type_.is_none());
        assert!(validate(&c).is_empty());
    }

    #[test]
    fn flat_expression_type_round_trips() {
        let c: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "$[?count(@.pets) > 0]",
            "type": "jsonpath",
            "version": "draft-goessner-dispatch-jsonpath-00",
        }))
        .unwrap();
        assert_eq!(c.type_, Some(CriterionType::Jsonpath));
        assert_eq!(c.version.as_deref(), Some(JSONPATH_VERSION));
        assert!(validate(&c).is_empty());
    }

    #[test]
    fn empty_condition_is_rejected() {
        let c = Criterion::default();
        assert!(validate(&c).iter().any(|e| e.contains("condition")));
    }

    #[test]
    fn type_without_context_is_rejected() {
        let c = Criterion {
            condition: "x".into(),
            type_: Some(CriterionType::Regex),
            ..Default::default()
        };
        assert!(
            validate(&c)
                .iter()
                .any(|e| e == "#.c.context: is required when `type` is set")
        );
    }

    #[test]
    fn jsonpath_with_wrong_version_is_rejected() {
        let c = Criterion {
            context: Some("$x".into()),
            condition: "x".into(),
            type_: Some(CriterionType::Jsonpath),
            version: Some("nope".into()),
            ..Default::default()
        };
        assert!(validate(&c).iter().any(|e| e.contains("jsonpath")));
    }

    #[test]
    fn xpath_version_must_be_in_set() {
        let bad = Criterion {
            context: Some("$x".into()),
            condition: "x".into(),
            type_: Some(CriterionType::Xpath),
            version: Some("xpath-99".into()),
            ..Default::default()
        };
        assert!(validate(&bad).iter().any(|e| e.contains("xpath")));

        let ok = Criterion {
            version: Some("xpath-30".into()),
            ..bad
        };
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn version_without_expression_type_is_rejected() {
        let c = Criterion {
            context: Some("$x".into()),
            condition: "x".into(),
            type_: Some(CriterionType::Simple),
            version: Some("xpath-10".into()),
            ..Default::default()
        };
        assert!(validate(&c).iter().any(|e| e.contains("only valid with")));
    }
}
