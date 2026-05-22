//! Overlay v1.1 `Info` object.
//!
//! Per [§3.2 Info Object](https://spec.openapis.org/overlay/v1.1.0.html#info-object):
//! the v1.0 fields plus an optional `description`.

use crate::validation::{
    Context, ValidateWithContext, ValidationOptions, validate_required_string,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Info {
    /// **Required** A human-readable description of the purpose of the overlay.
    pub title: String,

    /// **Required** A version identifier for changes to the Overlay document.
    pub version: String,

    /// Optional CommonMark-flavored prose description. Added in v1.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Info {
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        if !ctx.is_option(ValidationOptions::IgnoreEmptyInfoTitle) {
            validate_required_string(&self.title, ctx, format!("{path}.title"));
        }
        if !ctx.is_option(ValidationOptions::IgnoreEmptyInfoVersion) {
            validate_required_string(&self.version, ctx, format!("{path}.version"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn ctx() -> Context {
        Context::new(EnumSet::empty())
    }

    #[test]
    fn deserialize_minimal_info_round_trips() {
        let info: Info = serde_json::from_value(json!({ "title": "T", "version": "1.0" })).unwrap();
        assert_eq!(info.title, "T");
        assert_eq!(info.version, "1.0");
        assert!(info.description.is_none());
        assert!(info.extensions.is_none());

        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v, json!({ "title": "T", "version": "1.0" }));
    }

    #[test]
    fn deserialize_with_description_round_trips() {
        let info: Info = serde_json::from_value(json!({
            "title": "T",
            "version": "1.0",
            "description": "Adds error responses."
        }))
        .unwrap();
        assert_eq!(info.description.as_deref(), Some("Adds error responses."));

        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(
            v,
            json!({
                "title": "T",
                "version": "1.0",
                "description": "Adds error responses."
            }),
        );
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
        info.validate_with_context(&mut c, "#.info".into());
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
        let mut c = Context::new(opts);
        let info = Info::default();
        info.validate_with_context(&mut c, "#.info".into());
        assert!(c.errors.is_empty());
    }
}
