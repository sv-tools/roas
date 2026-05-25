//! Arazzo v1.1 `Source Description` object.
//!
//! Per [Source Description Object](https://spec.openapis.org/arazzo/v1.1.0.html#source-description-object).
//! New in v1.1: the `asyncapi` source type.

use crate::validation::{Context, ValidateWithContext, is_valid_name};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The kind of document a [`SourceDescription`] points at.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// An Arazzo description.
    Arazzo,
    /// An OpenAPI description.
    #[default]
    Openapi,
    /// An AsyncAPI description. Added in v1.1.
    Asyncapi,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct SourceDescription {
    /// **Required** A unique name for the source description
    /// (pattern `^[A-Za-z0-9_\-]+$`).
    pub name: String,

    /// **Required** A URL to the source description (URI reference).
    pub url: String,

    /// The type of source description.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<SourceType>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for SourceDescription {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("name", &self.name);
        if !self.name.is_empty() && !is_valid_name(&self.name) {
            ctx.error_field("name", r"must match `^[A-Za-z0-9_\-]+$`");
        }
        ctx.require_non_empty("url", &self.url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn asyncapi_type_round_trips() {
        let sd: SourceDescription = serde_json::from_value(
            json!({ "name": "events", "url": "asyncapi.yaml", "type": "asyncapi" }),
        )
        .unwrap();
        assert_eq!(sd.type_, Some(SourceType::Asyncapi));
        assert_eq!(
            serde_json::to_value(&sd).unwrap()["type"],
            json!("asyncapi")
        );
    }

    #[test]
    fn validate_rejects_bad_name() {
        let mut c = Context::with_path(EnumSet::empty(), "#.s");
        let sd = SourceDescription {
            name: "bad name".into(),
            url: "u".into(),
            ..Default::default()
        };
        sd.validate_with_context(&mut c);
        assert!(c.errors.iter().any(|e| e.contains("must match")));
    }
}
