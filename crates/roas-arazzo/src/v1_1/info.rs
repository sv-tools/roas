//! Arazzo v1.1 `Info` object (unchanged from v1.0).
//!
//! Per [Info Object](https://spec.openapis.org/arazzo/v1.1.0.html#info-object):
//! metadata about the Arazzo description, with required `title` and
//! `version`.

use crate::validation::{Context, ValidateWithContext, ValidationOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Info {
    /// **Required** A human-readable title of the Arazzo description.
    pub title: String,

    /// A short summary of the Arazzo description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A description of the purpose of the workflows defined. CommonMark
    /// syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// **Required** A version identifier for the Arazzo document
    /// (distinct from the Arazzo Specification version).
    pub version: String,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Info {
    fn validate_with_context(&self, ctx: &mut Context) {
        if !ctx.is_option(ValidationOptions::IgnoreEmptyInfoTitle) {
            ctx.require_non_empty("title", &self.title);
        }
        if !ctx.is_option(ValidationOptions::IgnoreEmptyInfoVersion) {
            ctx.require_non_empty("version", &self.version);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn deserialize_minimal_info_round_trips() {
        let info: Info = serde_json::from_value(json!({ "title": "T", "version": "1.0" })).unwrap();
        assert_eq!(info.title, "T");
        assert_eq!(info.version, "1.0");
        assert!(info.extensions.is_none());
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            json!({ "title": "T", "version": "1.0" })
        );
    }

    #[test]
    fn validate_rejects_empty_title_and_version() {
        let mut c = Context::with_path(EnumSet::empty(), "#.info");
        Info::default().validate_with_context(&mut c);
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.info.title: must not be empty")
        );
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.info.version: must not be empty")
        );
    }
}
