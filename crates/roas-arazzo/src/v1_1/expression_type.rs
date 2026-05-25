//! Arazzo v1.1 `Expression Type` object.
//!
//! Per [Expression Type Object](https://spec.openapis.org/arazzo/v1.1.0.html#expression-type-object):
//! the type and version of an expression used in a `Criterion` or
//! `Selector`. New in v1.1 (replaces v1.0's flat criterion
//! `type` + `version`).

use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The expression language of an [`ExpressionType`].
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExpressionKind {
    #[default]
    Jsonpath,
    Xpath,
    Jsonpointer,
}

impl ExpressionKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ExpressionKind::Jsonpath => "jsonpath",
            ExpressionKind::Xpath => "xpath",
            ExpressionKind::Jsonpointer => "jsonpointer",
        }
    }

    /// The `version` values the schema allows for this expression type.
    pub(crate) fn allowed_versions(self) -> &'static [&'static str] {
        match self {
            ExpressionKind::Jsonpath => &["rfc9535", "draft-goessner-dispatch-jsonpath-00"],
            ExpressionKind::Xpath => &["xpath-10", "xpath-20", "xpath-30", "xpath-31"],
            ExpressionKind::Jsonpointer => &["rfc6901"],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ExpressionType {
    /// **Required** The expression language.
    #[serde(rename = "type")]
    pub type_: ExpressionKind,

    /// **Required** The version of the expression language. Must be one
    /// of the values allowed for `type`.
    pub version: String,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for ExpressionType {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("version", &self.version);
        let allowed = self.type_.allowed_versions();
        if !self.version.is_empty() && !allowed.contains(&self.version.as_str()) {
            ctx.error_field(
                "version",
                format!(
                    "must be one of {} for type `{}`",
                    allowed
                        .iter()
                        .map(|v| format!("`{v}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.type_.as_str(),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(et: &ExpressionType) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.type");
        et.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn round_trips() {
        let et: ExpressionType =
            serde_json::from_value(json!({ "type": "jsonpath", "version": "rfc9535" })).unwrap();
        assert_eq!(et.type_, ExpressionKind::Jsonpath);
        assert_eq!(et.version, "rfc9535");
        assert!(validate(&et).is_empty());
    }

    #[test]
    fn jsonpointer_requires_rfc6901() {
        let ok: ExpressionType =
            serde_json::from_value(json!({ "type": "jsonpointer", "version": "rfc6901" })).unwrap();
        assert!(validate(&ok).is_empty());

        let bad = ExpressionType {
            type_: ExpressionKind::Jsonpointer,
            version: "nope".into(),
            ..Default::default()
        };
        assert!(validate(&bad).iter().any(|e| e.contains("jsonpointer")));
    }

    #[test]
    fn xpath_version_must_be_in_set() {
        let bad = ExpressionType {
            type_: ExpressionKind::Xpath,
            version: "xpath-99".into(),
            ..Default::default()
        };
        assert!(validate(&bad).iter().any(|e| e.contains("xpath")));
    }
}
