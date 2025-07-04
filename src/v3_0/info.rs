//! Provides metadata about the API.

use crate::common::helpers::{
    Context, ValidateWithContext, validate_email, validate_optional_url, validate_required_string,
};
use crate::v3_0::spec::Spec;
use crate::validation::Options;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The object provides metadata about the API.
/// The metadata MAY be used by the clients if needed,
/// and MAY be presented in editing or documentation generation tools for convenience.
///
/// ### Specification example:
/// ```yaml
/// title: Sample Pet Store App
/// description: This is a sample server for a pet store.
/// termsOfService: https://example.com/terms/
/// contact:
///   name: API Support
///   url: https://www.example.com/support
///   email: support@example.com
/// license:
///   name: Apache 2.0
///   url: https://www.apache.org/licenses/LICENSE-2.0.html
/// version: 1.0.1
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Info {
    /// **Required** The title of the API.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,

    /// A short description of the API.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
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

    /// **Required** The version of the OpenAPI document
    /// (which is distinct from the OpenAPI Specification version or the API implementation version).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,

    /// This object MAY be extended with Specification Extensions.
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
/// url: https://www.example.com/support
/// email: support@example.com
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

    /// This object MAY be extended with Specification Extensions.
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

    /// This object MAY be extended with Specification Extensions.
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
            validate_required_string(&self.title, ctx, format!("{path}.title"));
        }
        if !ctx.is_option(Options::IgnoreEmptyInfoVersion) {
            validate_required_string(&self.version, ctx, format!("{path}.version"));
        }

        if let Some(contact) = &self.contact {
            contact.validate_with_context(ctx, format!("{path}.contact"));
        }

        if let Some(license) = &self.license {
            license.validate_with_context(ctx, format!("{path}.license"));
        }
    }
}

impl ValidateWithContext<Spec> for Contact {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_optional_url(&self.url, ctx, format!("{path}.url"));
        validate_email(&self.email, ctx, format!("{path}.email"));
    }
}

impl ValidateWithContext<Spec> for License {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        validate_optional_url(&self.url, ctx, format!("{path}.url"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_info_deserialize() {
        assert_eq!(
            serde_json::from_value::<Info>(json!({
              "title": "Swagger Sample App",
              "description": "This is a sample server Petstore server.",
              "termsOfService": "https://example.com/terms/",
              "contact": {
                "name": "API Support",
                "url": "https://www.example.com/support",
                "email": "support@example.com"
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
                terms_of_service: Some(String::from("https://example.com/terms/")),
                contact: Some(Contact {
                    name: Some(String::from("API Support")),
                    url: Some(String::from("https://www.example.com/support")),
                    email: Some(String::from("support@example.com")),
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

        assert_eq!(
            serde_json::from_value::<Info>(json!({
              "title": "Swagger Sample App",
              "description": "This is a sample server Petstore server.",
              "termsOfService": "https://example.com/terms/",
              "version": "1.0.1"
            }))
            .unwrap(),
            Info {
                title: String::from("Swagger Sample App"),
                description: Some(String::from("This is a sample server Petstore server.")),
                terms_of_service: Some(String::from("https://example.com/terms/")),
                version: "1.0.1".to_owned(),
                ..Default::default()
            },
            "deserialize",
        );

        assert_eq!(
            serde_json::from_value::<Info>(json!({
              "title": "Swagger Sample App",
              "version": "1.0.1"
            }))
            .unwrap(),
            Info {
                title: String::from("Swagger Sample App"),
                version: "1.0.1".to_owned(),
                ..Default::default()
            },
            "deserialize",
        );

        assert_eq!(
            serde_json::from_value::<Info>(json!({
              "title": "",
              "version": ""
            }))
            .unwrap(),
            Info::default(),
            "deserialize",
        );
    }

    #[test]
    fn test_info_serialize() {
        assert_eq!(
            serde_json::to_value(Info {
                title: String::from("Swagger Sample App"),
                description: Some(String::from("This is a sample server Petstore server.")),
                terms_of_service: Some(String::from("https://example.com/terms/")),
                contact: Some(Contact {
                    name: Some(String::from("API Support")),
                    url: Some(String::from("https://www.example.com/support")),
                    email: Some(String::from("support@example.com")),
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
              "termsOfService": "https://example.com/terms/",
              "contact": {
                "name": "API Support",
                "url": "https://www.example.com/support",
                "email": "support@example.com"
              },
              "license": {
                "name": "Apache 2.0",
                "url": "https://www.apache.org/licenses/LICENSE-2.0.html"
              },
              "version": "1.0.1"
            }),
            "serialize",
        );

        assert_eq!(
            serde_json::to_value(Info {
                title: String::from("Swagger Sample App"),
                description: Some(String::from("This is a sample server Petstore server.")),
                terms_of_service: Some(String::from("https://example.com/terms/")),
                version: "1.0.1".to_owned(),
                ..Default::default()
            })
            .unwrap(),
            json!({
              "title": "Swagger Sample App",
              "description": "This is a sample server Petstore server.",
              "termsOfService": "https://example.com/terms/",
              "version": "1.0.1"
            }),
            "serialize",
        );

        assert_eq!(
            serde_json::to_value(Info {
                title: String::from("Swagger Sample App"),
                version: "1.0.1".to_owned(),
                ..Default::default()
            })
            .unwrap(),
            json!({
              "title": "Swagger Sample App",
              "version": "1.0.1"
            }),
            "serialize",
        );
        assert_eq!(
            serde_json::to_value(Info::default()).unwrap(),
            json!({}),
            "serialize",
        );
    }

    #[test]
    fn test_contact_deserialize() {
        assert_eq!(
            serde_json::from_value::<Contact>(json!({
                "name": "API Support",
                "url": "https://www.example.com/support",
                "email": "support@example.com"
            }))
            .unwrap(),
            Contact {
                name: Some(String::from("API Support")),
                url: Some(String::from("https://www.example.com/support")),
                email: Some(String::from("support@example.com")),
                ..Default::default()
            },
            "deserialize",
        );
    }

    #[test]
    fn test_contact_serialize() {
        assert_eq!(
            serde_json::to_value(Contact {
                name: Some(String::from("API Support")),
                url: Some(String::from("https://www.example.com/support")),
                email: Some(String::from("support@example.com")),
                ..Default::default()
            })
            .unwrap(),
            json!({
                "name": "API Support",
                "url": "https://www.example.com/support",
                "email": "support@example.com"
            }),
            "serialize",
        );
    }

    #[test]
    fn test_license_deserialize() {
        assert_eq!(
            serde_json::from_value::<License>(json!({
                "name": "Apache 2.0",
                "url": "https://www.apache.org/licenses/LICENSE-2.0.html",
            }))
            .unwrap(),
            License {
                name: String::from("Apache 2.0"),
                url: Some(String::from(
                    "https://www.apache.org/licenses/LICENSE-2.0.html"
                )),
                ..Default::default()
            },
            "deserialize",
        );
    }

    #[test]
    fn test_license_serialize() {
        assert_eq!(
            serde_json::to_value(License {
                name: String::from("Apache 2.0"),
                url: Some(String::from(
                    "https://www.apache.org/licenses/LICENSE-2.0.html"
                )),
                ..Default::default()
            })
            .unwrap(),
            json!({
                "name": "Apache 2.0",
                "url": "https://www.apache.org/licenses/LICENSE-2.0.html"
            }),
            "serialize",
        );
    }

    #[test]
    fn test_contact_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        Contact {
            name: Some(String::from("API Support")),
            url: Some(String::from("https://www.example.com/support")),
            email: Some(String::from("support@example.com")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("contact"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Contact {
            url: Some(String::from("https://www.example.com/support")),
            email: Some(String::from("support@example.com")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("contact"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Contact {
            url: Some(String::from("foo - bar")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("contact"));
        assert_eq!(ctx.errors.len(), 1, "incorrect url: {:?}", ctx.errors);

        ctx = Context::new(&spec, Default::default());
        Contact {
            email: Some(String::from("foo - bar")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("contact"));
        assert_eq!(ctx.errors.len(), 1, "incorrect email: {:?}", ctx.errors);
    }

    #[test]
    fn test_license_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        License {
            name: String::from("Apache 2.0"),
            url: Some(String::from(
                "https://www.apache.org/licenses/LICENSE-2.0.html",
            )),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("license"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        License {
            name: String::from("Apache 2.0"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("license"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        ctx = Context::new(&spec, Default::default());
        License {
            name: String::from(""),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("license"));
        assert_eq!(ctx.errors.len(), 1, "empty name: {:?}", ctx.errors);

        ctx = Context::new(&spec, Default::default());
        License {
            name: String::from("Apache 2.0"),
            url: Some(String::from("foo - bar")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("license"));
        assert_eq!(ctx.errors.len(), 1, "incorrect url: {:?}", ctx.errors);
    }

    #[test]
    fn test_info_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        Info {
            title: String::from("Swagger Sample App"),
            description: Some(String::from("This is a sample server Petstore server.")),
            terms_of_service: Some(String::from("https://example.com/terms/")),
            contact: Some(Contact {
                name: Some(String::from("API Support")),
                url: Some(String::from("https://www.example.com/support")),
                email: Some(String::from("support@example.com")),
                ..Default::default()
            }),
            license: Some(License {
                name: String::from("Apache 2.0"),
                url: Some(String::from(
                    "https://www.apache.org/licenses/LICENSE-2.0.html",
                )),
                ..Default::default()
            }),
            version: "1.0.1".to_owned(),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Info {
            title: String::from("Swagger Sample App"),
            description: Some(String::from("This is a sample server Petstore server.")),
            terms_of_service: Some(String::from("https://example.com/terms/")),
            version: "1.0.1".to_owned(),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Info {
            title: String::from("Swagger Sample App"),
            version: "1.0.1".to_owned(),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Info {
            title: String::from("Swagger Sample App"),
            version: String::from(""),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert_eq!(ctx.errors.len(), 1, "empty version: {:?}", ctx.errors);

        ctx = Context::new(&spec, Default::default());
        Info {
            title: String::from(""),
            version: String::from("1.0.1"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert_eq!(ctx.errors.len(), 1, "empty title: {:?}", ctx.errors);

        ctx = Context::new(&spec, Options::only(&Options::IgnoreEmptyInfoTitle));
        Info {
            title: String::from(""),
            version: String::from("1.0.1"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        ctx = Context::new(&spec, Options::only(&Options::IgnoreEmptyInfoVersion));
        Info {
            title: String::from("Swagger Sample App"),
            version: String::from(""),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("info"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }
}
