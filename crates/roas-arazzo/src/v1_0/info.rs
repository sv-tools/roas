//! Arazzo v1.0 `Info` object.
//!
//! Per [Info Object](https://spec.openapis.org/arazzo/v1.0.1.html#info-object):
//! metadata about the Arazzo description, with required `title` and
//! `version`, extensible via `x-` fields.

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

    fn ctx() -> Context {
        Context::with_path(EnumSet::empty(), "#.info")
    }

    #[test]
    fn deserialize_minimal_info_round_trips() {
        let info: Info = serde_json::from_value(json!({ "title": "T", "version": "1.0" })).unwrap();
        assert_eq!(info.title, "T");
        assert_eq!(info.version, "1.0");
        assert!(info.summary.is_none());
        assert!(info.description.is_none());
        assert!(info.extensions.is_none());

        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v, json!({ "title": "T", "version": "1.0" }));
    }

    #[test]
    fn deserialize_full_info_keeps_optionals() {
        let info: Info = serde_json::from_value(json!({
            "title": "T",
            "summary": "S",
            "description": "D",
            "version": "1.0",
        }))
        .unwrap();
        assert_eq!(info.summary.as_deref(), Some("S"));
        assert_eq!(info.description.as_deref(), Some("D"));
    }

    #[test]
    fn deserialize_keeps_x_dash_extensions_and_drops_others() {
        let info: Info = serde_json::from_value(json!({
            "title": "T",
            "version": "1.0",
            "x-team": "platform",
            "ignored": 42,
        }))
        .unwrap();
        let ext = info.extensions.as_ref().unwrap();
        assert_eq!(ext.get("x-team").unwrap(), &json!("platform"));
        assert!(!ext.contains_key("ignored"));
    }

    #[test]
    fn validate_rejects_empty_title_and_version() {
        let mut c = ctx();
        let info = Info::default();
        info.validate_with_context(&mut c);
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

    #[test]
    fn validate_ignore_options_suppress_diagnostics() {
        let opts = EnumSet::only(ValidationOptions::IgnoreEmptyInfoTitle)
            | ValidationOptions::IgnoreEmptyInfoVersion;
        let mut c = Context::with_path(opts, "#.info");
        let info = Info::default();
        info.validate_with_context(&mut c);
        assert!(c.errors.is_empty());
    }
}
