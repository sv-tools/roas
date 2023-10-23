//! Security Scheme Object

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::ops::Add;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{
    validate_required_string, validate_url, Context, ValidateWithContext,
};
use crate::v2::spec::Spec;

/// Allows the definition of a security scheme that can be used by the operations.
/// Supported schemes are basic authentication, an API key (either as a header or as a query parameter)
/// and OAuth2's common flows (implicit, password, application and access code).
///
/// Specification Examples:
///
/// * Basic Authentication Sample:
/// ```yaml
/// type: basic
/// ```
///
/// * API Key Sample:
/// ```yaml
/// type: apiKey
/// name: api_key
/// in: header
/// ```
///
///  * Implicit OAuth2 Sample:
///  ```yaml
///  type: oauth2
///  flows: implicit
///  authorizationUrl: https://example.com/api/oauth/dialog
///  scopes:
///    write:pets: modify pets in your account
///    read:pets: read your pets
///  ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum SecurityScheme {
    /// Basic Authentication Type
    #[serde(rename = "basic")]
    Basic(BasicSecurityScheme),

    /// API Key Authentication Type
    #[serde(rename = "apiKey")]
    ApiKey(ApiKeySecurityScheme),

    /// OAuth2 Authentication Type
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2SecurityScheme),
}

impl Display for SecurityScheme {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityScheme::Basic(_) => write!(f, "basic"),
            SecurityScheme::ApiKey(_) => write!(f, "aoiKey"),
            SecurityScheme::OAuth2(_) => write!(f, "oauth2"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct BasicSecurityScheme {
    /// A short description for security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ApiKeySecurityScheme {
    /// **Required** A short description for security scheme.
    pub name: String,

    /// **Required** The location of the API key.
    #[serde(rename = "in")]
    pub location: SecuritySchemeApiKeyLocation,

    /// A short description for security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The location of the API key.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub enum SecuritySchemeApiKeyLocation {
    #[default]
    #[serde(rename = "query")]
    Query,
    #[serde(rename = "header")]
    Header,
}

impl Display for SecuritySchemeApiKeyLocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SecuritySchemeApiKeyLocation::Query => write!(f, "query"),
            SecuritySchemeApiKeyLocation::Header => write!(f, "header"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OAuth2SecurityScheme {
    /// **Required** The flow used by the OAuth2 security scheme.
    flow: SecuritySchemeOAuth2Flow,

    /// The authorization URL to be used for this flow.
    /// Required for `implicit` and `accessCode` flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "authorizationUrl")]
    authorization_url: Option<String>,

    /// The token URL to be used for this flow.
    /// Required for `password`, `application` and `accessCode` flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tokenUrl")]
    token_url: Option<String>,

    /// **Required** The available scopes for the OAuth2 security scheme.
    ///
    /// The extensions support is dropped for simplicity.
    scopes: BTreeMap<String, String>,

    /// A short description for security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// The flow used by the OAuth2 security scheme.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub enum SecuritySchemeOAuth2Flow {
    #[default]
    #[serde(rename = "implicit")]
    Implicit,
    #[serde(rename = "password")]
    Password,
    #[serde(rename = "application")]
    Application,
    #[serde(rename = "accessCode")]
    AccessCode,
}

impl Display for SecuritySchemeOAuth2Flow {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SecuritySchemeOAuth2Flow::Implicit => write!(f, "implicit"),
            SecuritySchemeOAuth2Flow::Password => write!(f, "password"),
            SecuritySchemeOAuth2Flow::Application => write!(f, "application"),
            SecuritySchemeOAuth2Flow::AccessCode => write!(f, "accessCode"),
        }
    }
}

impl ValidateWithContext<Spec> for SecurityScheme {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            SecurityScheme::Basic(basic) => basic.validate_with_context(ctx, path),
            SecurityScheme::ApiKey(api_key) => api_key.validate_with_context(ctx, path),
            SecurityScheme::OAuth2(oauth2) => oauth2.validate_with_context(ctx, path),
        }
    }
}

impl ValidateWithContext<Spec> for BasicSecurityScheme {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for ApiKeySecurityScheme {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, path.add(".name"));
    }
}

impl ValidateWithContext<Spec> for OAuth2SecurityScheme {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if self.scopes.is_empty() {
            ctx.errors
                .push(format!("{}.scopes: must not be empty", path));
        }
        if self.authorization_url.is_none()
            && (self.flow == SecuritySchemeOAuth2Flow::Implicit
                || self.flow == SecuritySchemeOAuth2Flow::AccessCode)
        {
            ctx.errors.push(format!(
                "{}.authorization_url: must be present for flow = {}",
                path, self.flow,
            ));
        } else {
            validate_url(&self.authorization_url, ctx, path.add(".authorizationUrl"));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_security_scheme_basic_deserialize() {
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "basic",
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::Basic(BasicSecurityScheme {
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_security_scheme_basic_serialize() {
        assert_eq!(
            serde_json::to_value(SecurityScheme::Basic(BasicSecurityScheme {
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "basic",
                "description": "A short description for security scheme.",
            }),
            "serialize",
        );
    }

    #[test]
    fn test_security_scheme_api_key_deserialize() {
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "apiKey",
                "name": "api_key",
                "in": "header",
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::ApiKey(ApiKeySecurityScheme {
                name: String::from("api_key"),
                location: SecuritySchemeApiKeyLocation::Header,
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize in = header",
        );
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "apiKey",
                "name": "api_key",
                "in": "query",
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::ApiKey(ApiKeySecurityScheme {
                name: String::from("api_key"),
                location: SecuritySchemeApiKeyLocation::Query,
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize in = query",
        );
    }

    #[test]
    fn test_security_scheme_api_key_serialize() {
        assert_eq!(
            serde_json::to_value(SecurityScheme::ApiKey(ApiKeySecurityScheme {
                name: String::from("api_key"),
                location: SecuritySchemeApiKeyLocation::Header,
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "apiKey",
                "name": "api_key",
                "in": "header",
                "description": "A short description for security scheme.",
            }),
            "serialize location = header",
        );
        assert_eq!(
            serde_json::to_value(SecurityScheme::ApiKey(ApiKeySecurityScheme {
                name: String::from("api_key"),
                location: SecuritySchemeApiKeyLocation::Query,
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "apiKey",
                "name": "api_key",
                "in": "query",
                "description": "A short description for security scheme.",
            }),
            "serialize location = query",
        );
    }

    #[test]
    fn test_security_scheme_oauth2_deserialize() {
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "oauth2",
                "flow": "implicit",
                "authorizationUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: Some(String::from("https://example.com/api/oauth/dialog")),
                token_url: None,
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize flow = implicit",
        );
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "oauth2",
                "flow": "accessCode",
                "authorizationUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::AccessCode,
                authorization_url: Some(String::from("https://example.com/api/oauth/dialog")),
                token_url: None,
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize flow = accessCode",
        );
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "oauth2",
                "flow": "password",
                "tokenUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Password,
                authorization_url: None,
                token_url: Some(String::from("https://example.com/api/oauth/dialog")),
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize flow = password",
        );
        assert_eq!(
            serde_json::from_value::<SecurityScheme>(json!({
                "type": "oauth2",
                "flow": "application",
                "tokenUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }))
            .unwrap(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Application,
                authorization_url: None,
                token_url: Some(String::from("https://example.com/api/oauth/dialog")),
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }),
            "deserialize flow = application",
        );
    }

    #[test]
    fn test_security_scheme_oauth2_serialize() {
        assert_eq!(
            serde_json::to_value(SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: Some(String::from("https://example.com/api/oauth/dialog")),
                token_url: None,
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "oauth2",
                "flow": "implicit",
                "authorizationUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }),
            "serialize flow = implicit",
        );
        assert_eq!(
            serde_json::to_value(SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::AccessCode,
                authorization_url: Some(String::from("https://example.com/api/oauth/dialog")),
                token_url: None,
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "oauth2",
                "flow": "accessCode",
                "authorizationUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }),
            "serialize flow = accessCode",
        );
        assert_eq!(
            serde_json::to_value(SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Password,
                authorization_url: None,
                token_url: Some(String::from("https://example.com/api/oauth/dialog")),
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "oauth2",
                "flow": "password",
                "tokenUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }),
            "serialize flow = password",
        );
        assert_eq!(
            serde_json::to_value(SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Application,
                authorization_url: None,
                token_url: Some(String::from("https://example.com/api/oauth/dialog")),
                scopes: BTreeMap::from_iter(vec![
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
            }))
            .unwrap(),
            json!({
                "type": "oauth2",
                "flow": "application",
                "tokenUrl": "https://example.com/api/oauth/dialog",
                "scopes": {
                    "write:pets": "modify pets in your account",
                    "read:pets": "read your pets",
                },
                "description": "A short description for security scheme.",
            }),
            "serialize flow = application",
        );
    }
}
