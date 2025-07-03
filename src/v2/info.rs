//! Metadata about the API.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{
    Context, ValidateWithContext, validate_email, validate_optional_url, validate_required_string,
};
use crate::v2::spec::Spec;
use crate::validation::Options;

/// The object provides metadata about the API.
/// The metadata can be used by the clients if needed, and can be presented in the Swagger-UI for convenience.
///
/// ### Specification example:
/// ```yaml
/// title: Swagger Sample App
/// description: This is a sample server Petstore server.
/// termsOfService: https://swagger.io/terms/
/// contact:
///   name: API Support
///   url: https://www.swagger.io/support
///   email: support@swagger.io
/// license:
///   name: Apache 2.0
///   url: https://www.apache.org/licenses/LICENSE-2.0.html
/// version: 1.0.1
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Info {
    /// **Required** The title of the application.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,

    /// A short description of the application.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The Terms of Service for the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "termsOfService")]
    pub terms_of_service: Option<String>,

    /// The contact information for the exposed API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<Contact>,

    /// The license information for the exposed API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<License>,

    /// **Required** Provides the version of the application API (not to be confused with the specification version).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// Contact information for the exposed API.
///
/// ### Specification example:
///
/// ```yaml
/// name: API Support
/// url: https://www.swagger.io/support
/// email: support@swagger.io
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    /// The identifying name of the contact person/organization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The URL pointing to the contact information. MUST be in the format of a URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// The email address of the contact person/organization.
    /// MUST be in the format of an email address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// License information for the exposed API.
///
/// ### Specification example:
///
/// ```yaml
/// name: Apache 2.0
/// url: https://www.apache.org/licenses/LICENSE-2.0.html
/// ```
///
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct License {
    /// **Required** The license name used for the API.
    pub name: String,

    /// A URL to the license used for the API.
    /// MUST be in the format of a URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Info {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if !ctx.is_option(Options::IgnoreEmptyInfoTitle) {
            validate_required_string(&self.title, ctx, format!("{}.title", path));
        }
        if !ctx.is_option(Options::IgnoreEmptyInfoVersion) {
            validate_required_string(&self.version, ctx, format!("{}.version", path));
        }

        if let Some(contact) = &self.contact {
            contact.validate_with_context(ctx, format!("{}.contact", path));
        }

        if let Some(license) = &self.license {
            license.validate_with_context(ctx, format!("{}.license", path));
        }
    }
}

impl ValidateWithContext<Spec> for Contact {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_optional_url(&self.url, ctx, format!("{}.url", path));
        validate_email(&self.email, ctx, format!("{}.email", path));
    }
}

impl ValidateWithContext<Spec> for License {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{}.name", path));
        validate_optional_url(&self.url, ctx, format!("{}.url", path));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_info_deserialize() {
        assert_eq!(
            serde_json::from_value::<Info>(json!({
              "title": "Swagger Sample App",
              "description": "This is a sample server Petstore server.",
              "termsOfService": "https://swagger.io/terms/",
              "contact": {
                "name": "API Support",
                "url": "https://www.swagger.io/support",
                "email": "support@swagger.io"
              },
              "license": {
                "name": "Apache 2.0",
                "url": "https://www.apache.org/licenses/LICENSE-2.0.html"
              },
              "version": "1.0.1"
            }))
            .unwrap(),
            Info {
                title: String::from("Swagger Sample App"),
                description: Some(String::from("This is a sample server Petstore server.")),
                terms_of_service: Some(String::from("https://swagger.io/terms/")),
                contact: Some(Contact {
                    name: Some(String::from("API Support")),
                    url: Some(String::from("https://www.swagger.io/support")),
                    email: Some(String::from("support@swagger.io")),
                    ..Default::default()
                }),
                license: Some(License {
                    name: String::from("Apache 2.0"),
                    url: Some(String::from(
                        "https://www.apache.org/licenses/LICENSE-2.0.html"
                    )),
                    ..Default::default()
                }),
                version: "1.0.1".to_owned(),
                ..Default::default()
            },
            "deserialize",
        );
    }

    #[test]
    fn test_info_serialize() {
        assert_eq!(
            serde_json::to_value(Info {
                title: String::from("Swagger Sample App"),
                description: Some(String::from("This is a sample server Petstore server.")),
                terms_of_service: Some(String::from("https://swagger.io/terms/")),
                contact: Some(Contact {
                    name: Some(String::from("API Support")),
                    url: Some(String::from("https://www.swagger.io/support")),
                    email: Some(String::from("support@swagger.io")),
                    ..Default::default()
                }),
                license: Some(License {
                    name: String::from("Apache 2.0"),
                    url: Some(String::from(
                        "https://www.apache.org/licenses/LICENSE-2.0.html"
                    )),
                    ..Default::default()
                }),
                version: "1.0.1".to_owned(),
                ..Default::default()
            })
            .unwrap(),
            json!({
              "title": "Swagger Sample App",
              "description": "This is a sample server Petstore server.",
              "termsOfService": "https://swagger.io/terms/",
              "contact": {
                "name": "API Support",
                "url": "https://www.swagger.io/support",
                "email": "support@swagger.io"
              },
              "license": {
                "name": "Apache 2.0",
                "url": "https://www.apache.org/licenses/LICENSE-2.0.html"
              },
              "version": "1.0.1"
            }),
            "serialize",
        );
    }
}
