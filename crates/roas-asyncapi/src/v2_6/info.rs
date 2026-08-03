//! AsyncAPI v2.6 `Info`, `Contact`, and `License` objects.
//!
//! Per [Info Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#infoObject).
//!
//! Unlike v3, 2.6's `info` carries no `tags` or `externalDocs` — those
//! live at the document root.

use crate::validation::{Context, ValidateWithContext, ValidationOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Info {
    /// **Required** The title of the application.
    pub title: String,

    /// **Required** The version of this application's API definition
    /// (not the AsyncAPI version).
    pub version: String,

    /// A short description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A URL to the Terms of Service for the API.
    #[serde(rename = "termsOfService", skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,

    /// Contact information for the exposed API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<Contact>,

    /// License information for the exposed API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<License>,

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
        if let Some(contact) = &self.contact {
            ctx.in_field("contact", |ctx| contact.validate_with_context(ctx));
        }
        if let Some(license) = &self.license {
            ctx.in_field("license", |ctx| license.validate_with_context(ctx));
        }
    }
}

/// Contact information for the exposed API.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Contact {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Contact {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(email) = &self.email
            && !email.contains('@')
        {
            ctx.error_field("email", "must be an email address");
        }
    }
}

/// License information for the exposed API.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct License {
    /// **Required** The license name used for the API.
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for License {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("name", &self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn deserializes_full_info_and_round_trips() {
        let value = json!({
            "title": "Streetlights",
            "version": "1.0.0",
            "description": "Smart lighting",
            "termsOfService": "https://example.com/tos",
            "contact": { "name": "Ops", "email": "ops@example.com" },
            "license": { "name": "Apache 2.0", "url": "https://example.com" },
            "x-internal": true
        });
        let info: Info = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(info.title, "Streetlights");
        assert_eq!(serde_json::to_value(&info).unwrap(), value);

        let mut ctx = Context::with_path(EnumSet::empty(), "#.info");
        info.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn tags_and_external_docs_are_not_info_fields_in_2_6() {
        // They belong to the document root here, so they are dropped
        // as unknown keys rather than parsed.
        let info: Info = serde_json::from_value(json!({
            "title": "T",
            "version": "1",
            "tags": [ { "name": "x" } ],
            "externalDocs": { "url": "https://e" }
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            json!({ "title": "T", "version": "1" })
        );
    }

    #[test]
    fn empty_title_and_version_are_reported_unless_ignored() {
        let info = Info::default();

        let mut ctx = Context::with_path(EnumSet::empty(), "#.info");
        info.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.info.title: must not be empty")
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.info.version: must not be empty")
        );

        let mut ctx = Context::with_path(
            ValidationOptions::IgnoreEmptyInfoTitle | ValidationOptions::IgnoreEmptyInfoVersion,
            "#.info",
        );
        info.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn contact_email_and_license_name_are_checked() {
        let info: Info = serde_json::from_value(json!({
            "title": "T",
            "version": "1",
            "contact": { "email": "nope" },
            "license": { "name": "" }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.info");
        info.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.info.contact.email: must be an email address")
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.info.license.name: must not be empty")
        );
    }
}
