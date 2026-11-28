//! Arazzo v1.0 `Criterion` object.
//!
//! Per [Criterion Object](https://spec.openapis.org/arazzo/v1.0.1.html#criterion-object):
//! an assertion used in step `successCriteria` and action `criteria`.
//!
//! Both the prose specification's nested *Criterion Expression Type Object*
//! (`type: { type: "jsonpath" | "xpath", version: "..." }`) and the v1.0 JSON
//! Schema's flattened `type` + `version` form are accepted and preserved across
//! round trips.

use crate::validation::{Context, ValidateWithContext};
use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// The expression language of a v1.0 [`CriterionExpressionType`].
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CriterionExpressionKind {
    #[default]
    Jsonpath,
    Xpath,
}

/// Nested *Criterion Expression Type Object* per Arazzo v1.0.1 prose.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct CriterionExpressionType {
    /// **Required** The expression language (`jsonpath` or `xpath`).
    #[serde(rename = "type")]
    pub type_: CriterionExpressionKind,

    /// **Required** A shorthand string for the expression-type version.
    pub version: String,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for CriterionExpressionType {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("version", &self.version);
        if !self.version.is_empty() {
            match self.type_ {
                CriterionExpressionKind::Jsonpath if self.version != JSONPATH_VERSION => {
                    ctx.error_field(
                        "version",
                        format!("must be `{JSONPATH_VERSION}` for type `jsonpath`"),
                    );
                }
                CriterionExpressionKind::Xpath
                    if !XPATH_VERSIONS.contains(&self.version.as_str()) =>
                {
                    ctx.error_field(
                        "version",
                        "must be one of `xpath-10`, `xpath-20`, `xpath-30` for type `xpath`",
                    );
                }
                _ => {}
            }
        }
        ctx.validate_unknown_fields(&self.extensions);
    }
}

/// The type of condition expressed by a [`Criterion`]: a plain string enum or
/// the nested [`CriterionExpressionType`] object defined in the v1.0.1 prose.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum CriterionType {
    #[default]
    Simple,
    Regex,
    Jsonpath,
    Xpath,
    Expression(CriterionExpressionType),
}

impl Serialize for CriterionType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CriterionType::Simple => serializer.serialize_str("simple"),
            CriterionType::Regex => serializer.serialize_str("regex"),
            CriterionType::Jsonpath => serializer.serialize_str("jsonpath"),
            CriterionType::Xpath => serializer.serialize_str("xpath"),
            CriterionType::Expression(et) => et.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CriterionType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CriterionTypeVisitor;

        impl<'de> Visitor<'de> for CriterionTypeVisitor {
            type Value = CriterionType;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a criterion type string or expression type object")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "simple" => Ok(CriterionType::Simple),
                    "regex" => Ok(CriterionType::Regex),
                    "jsonpath" => Ok(CriterionType::Jsonpath),
                    "xpath" => Ok(CriterionType::Xpath),
                    other => Err(E::unknown_variant(
                        other,
                        &["simple", "regex", "jsonpath", "xpath"],
                    )),
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                CriterionExpressionType::deserialize(MapAccessDeserializer::new(map))
                    .map(CriterionType::Expression)
            }
        }

        deserializer.deserialize_any(CriterionTypeVisitor)
    }
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

    /// A shorthand string for the expression-type version when using the
    /// flattened schema form. Only valid with `type: jsonpath` or `type: xpath`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Criterion {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("condition", &self.condition);
        if let Some(context) = &self.context {
            ctx.require_non_empty("context", context);
        }

        // `dependentRequired`: a `type` requires a `context`.
        if self.type_.is_some() && self.context.is_none() {
            ctx.error_field("context", "is required when `type` is set");
        }

        if let Some(CriterionType::Expression(et)) = &self.type_ {
            ctx.in_field("type", |ctx| et.validate_with_context(ctx));
        }

        // `version` belongs to the flat expression-type form (jsonpath/xpath)
        // and must match the value allowed for that type.
        if let Some(version) = &self.version {
            match &self.type_ {
                Some(CriterionType::Expression(_)) => {
                    ctx.error_field(
                        "version",
                        "cannot be set when `type` is an expression type object",
                    );
                }
                Some(CriterionType::Jsonpath) if version != JSONPATH_VERSION => {
                    ctx.error_field(
                        "version",
                        format!("must be `{JSONPATH_VERSION}` for type `jsonpath`"),
                    );
                }
                Some(CriterionType::Xpath) if !XPATH_VERSIONS.contains(&version.as_str()) => {
                    ctx.error_field(
                        "version",
                        "must be one of `xpath-10`, `xpath-20`, `xpath-30` for type `xpath`",
                    );
                }
                Some(CriterionType::Jsonpath | CriterionType::Xpath) => {}
                _ => ctx.error_field("version", "is only valid with type `jsonpath` or `xpath`"),
            }
        }
        ctx.validate_unknown_fields(&self.extensions);
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

    #[test]
    fn nested_expression_type_object_round_trips_and_validates() {
        let raw = json!({
            "context": "$response.body",
            "condition": "$[?count(@.pets) > 0]",
            "type": {
                "type": "jsonpath",
                "version": "draft-goessner-dispatch-jsonpath-00",
                "x-dialect": "goessner"
            }
        });
        let c: Criterion = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(c.type_, Some(CriterionType::Expression(_))));
        assert!(validate(&c).is_empty());
        assert_eq!(serde_json::to_value(&c).unwrap(), raw);

        let xpath_ok: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "//pet",
            "type": { "type": "xpath", "version": "xpath-30" }
        }))
        .unwrap();
        assert!(validate(&xpath_ok).is_empty());

        let bad_jp: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "$.x",
            "type": { "type": "jsonpath", "version": "rfc9535" }
        }))
        .unwrap();
        assert!(
            validate(&bad_jp)
                .iter()
                .any(|e| e.starts_with("#.c.type.version:"))
        );

        let bad_xp: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "//x",
            "type": { "type": "xpath", "version": "xpath-99" }
        }))
        .unwrap();
        assert!(
            validate(&bad_xp)
                .iter()
                .any(|e| e.starts_with("#.c.type.version:"))
        );

        let empty_ver: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "//x",
            "type": { "type": "xpath", "version": "" }
        }))
        .unwrap();
        assert!(
            validate(&empty_ver)
                .iter()
                .any(|e| e == "#.c.type.version: must not be empty")
        );

        let both_forms: Criterion = serde_json::from_value(json!({
            "context": "$response.body",
            "condition": "$.x",
            "type": { "type": "jsonpath", "version": "draft-goessner-dispatch-jsonpath-00" },
            "version": "draft-goessner-dispatch-jsonpath-00"
        }))
        .unwrap();
        assert!(
            validate(&both_forms)
                .iter()
                .any(|e| e == "#.c.version: cannot be set when `type` is an expression type object")
        );
    }

    #[test]
    fn empty_context_and_invalid_type_variants_are_rejected() {
        let c = Criterion {
            context: Some(String::new()),
            condition: "ok".into(),
            ..Default::default()
        };
        assert!(
            validate(&c)
                .iter()
                .any(|e| e == "#.c.context: must not be empty")
        );

        assert!(serde_json::from_value::<CriterionType>(json!("bogus")).is_err());
        assert!(serde_json::from_value::<CriterionType>(json!(42)).is_err());

        for (variant, expected) in [
            (CriterionType::Simple, "simple"),
            (CriterionType::Regex, "regex"),
            (CriterionType::Jsonpath, "jsonpath"),
            (CriterionType::Xpath, "xpath"),
        ] {
            let serialized = serde_json::to_value(&variant).unwrap();
            assert_eq!(serialized, json!(expected));
            assert_eq!(
                serde_json::from_value::<CriterionType>(serialized).unwrap(),
                variant
            );
        }
    }
}
