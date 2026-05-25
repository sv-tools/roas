//! Arazzo v1.0 `Source Description` object.
//!
//! Per [Source Description Object](https://spec.openapis.org/arazzo/v1.0.1.html#source-description-object):
//! a named reference to an OpenAPI or Arazzo document used by one or
//! more workflows.

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
    fn deserialize_round_trips_with_type() {
        let sd: SourceDescription = serde_json::from_value(json!({
            "name": "petStore",
            "url": "openapi.yaml",
            "type": "openapi",
        }))
        .unwrap();
        assert_eq!(sd.name, "petStore");
        assert_eq!(sd.type_, Some(SourceType::Openapi));

        let v = serde_json::to_value(&sd).unwrap();
        assert_eq!(v["type"], json!("openapi"));
    }

    #[test]
    fn deserialize_arazzo_type() {
        let sd: SourceDescription =
            serde_json::from_value(json!({ "name": "wf", "url": "wf.yaml", "type": "arazzo" }))
                .unwrap();
        assert_eq!(sd.type_, Some(SourceType::Arazzo));
    }

    #[test]
    fn type_is_omitted_when_absent() {
        let sd: SourceDescription =
            serde_json::from_value(json!({ "name": "n", "url": "u" })).unwrap();
        assert!(sd.type_.is_none());
        let v = serde_json::to_value(&sd).unwrap();
        assert_eq!(v, json!({ "name": "n", "url": "u" }));
    }

    #[test]
    fn validate_rejects_empty_name_and_url() {
        let mut c = Context::with_path(EnumSet::empty(), "#.sourceDescriptions[0]");
        SourceDescription::default().validate_with_context(&mut c);
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.sourceDescriptions[0].name: must not be empty")
        );
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.sourceDescriptions[0].url: must not be empty")
        );
    }

    #[test]
    fn validate_rejects_bad_name_pattern() {
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
