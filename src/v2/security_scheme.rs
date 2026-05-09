//! Security Scheme Object

use crate::common::helpers::{
    Context, PushError, ValidateWithContext, validate_required_string, validate_required_url,
};
use crate::v2::spec::Spec;
use serde::de::{Error as DeError, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};

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
            SecurityScheme::ApiKey(_) => write!(f, "apiKey"),
            SecurityScheme::OAuth2(_) => write!(f, "oauth2"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct BasicSecurityScheme {
    /// A short description for security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ApiKeySecurityScheme {
    /// **Required** The name of the header or query parameter to be used.
    pub name: String,

    /// **Required** The location of the API key.
    #[serde(rename = "in")]
    pub location: SecuritySchemeApiKeyLocation,

    /// A short description for security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
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
    pub flow: SecuritySchemeOAuth2Flow,

    /// The authorization URL to be used for this flow.
    /// Required for `implicit` and `accessCode` flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "authorizationUrl")]
    pub authorization_url: Option<String>,

    /// The token URL to be used for this flow.
    /// Required for `password`, `application` and `accessCode` flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tokenUrl")]
    pub token_url: Option<String>,

    /// **Required** The available scopes for the OAuth2 security scheme.
    pub scopes: Scopes,

    /// A short description for security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// Lists the available scopes for an OAuth2 security scheme.
/// Extra keys starting with `x-` are stored as Specification Extensions, the
/// rest map a scope name to a short description.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scopes {
    pub scopes: BTreeMap<String, String>,
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Scopes {
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.scopes.len()
    }
}

impl<S, K, V> From<S> for Scopes
where
    S: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    fn from(iter: S) -> Self {
        Scopes {
            scopes: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            extensions: None,
        }
    }
}

impl Serialize for Scopes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Only `x-` keys are emitted from `extensions`, so the size hint must
        // count only those — a programmatically constructed map could carry
        // non-`x-` entries that would otherwise overstate the count.
        let ext_x_count = self
            .extensions
            .as_ref()
            .map(|e| e.keys().filter(|k| k.starts_with("x-")).count())
            .unwrap_or(0);
        let total = self.scopes.len() + ext_x_count;
        let mut map = serializer.serialize_map(Some(total))?;
        for (k, v) in &self.scopes {
            map.serialize_entry(k, v)?;
        }
        if let Some(ext) = &self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(k, v)?;
                }
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Scopes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ScopesVisitor;
        impl<'de> Visitor<'de> for ScopesVisitor {
            type Value = Scopes;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a Scopes object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Scopes, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut scopes: BTreeMap<String, String> = BTreeMap::new();
                let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key.starts_with("x-") {
                        if ext.contains_key(&key) {
                            return Err(DeError::custom(format_args!("duplicate field `{key}`")));
                        }
                        ext.insert(key, map.next_value()?);
                    } else {
                        if scopes.contains_key(&key) {
                            return Err(DeError::custom(format_args!("duplicate field `{key}`")));
                        }
                        scopes.insert(key, map.next_value()?);
                    }
                }
                Ok(Scopes {
                    scopes,
                    extensions: if ext.is_empty() { None } else { Some(ext) },
                })
            }
        }
        deserializer.deserialize_map(ScopesVisitor)
    }
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
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

impl ValidateWithContext<Spec> for OAuth2SecurityScheme {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if self.scopes.is_empty() {
            ctx.error(path.clone(), ".scopes: must not be empty");
        }

        // authorizationUrl required for `implicit` and `accessCode` flows.
        let auth_required = matches!(
            self.flow,
            SecuritySchemeOAuth2Flow::Implicit | SecuritySchemeOAuth2Flow::AccessCode
        );
        match (&self.authorization_url, auth_required) {
            (None, true) => ctx.error(
                path.clone(),
                format_args!(
                    ".authorizationUrl: must be present for flow `{}`",
                    self.flow,
                ),
            ),
            (Some(url), _) => {
                validate_required_url(url, ctx, format!("{path}.authorizationUrl"));
            }
            (None, false) => {}
        }

        // tokenUrl required for `password`, `application` and `accessCode` flows.
        let token_required = matches!(
            self.flow,
            SecuritySchemeOAuth2Flow::Password
                | SecuritySchemeOAuth2Flow::Application
                | SecuritySchemeOAuth2Flow::AccessCode
        );
        match (&self.token_url, token_required) {
            (None, true) => ctx.error(
                path,
                format_args!(".tokenUrl: must be present for flow `{}`", self.flow),
            ),
            (Some(url), _) => {
                validate_required_url(url, ctx, format!("{path}.tokenUrl"));
            }
            (None, false) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                extensions: None,
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_security_scheme_basic_serialize() {
        assert_eq!(
            serde_json::to_value(SecurityScheme::Basic(BasicSecurityScheme {
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                extensions: None,
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
                extensions: None,
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
                extensions: None,
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
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
            }),
            "deserialize flow = application",
        );
    }

    #[test]
    fn oauth2_validate_empty_scopes_and_url_branches() {
        use crate::common::helpers::Context;
        use crate::validation::Options;
        let spec = Spec::default();

        // Empty scopes + missing tokenUrl on `password` flow.
        let s = OAuth2SecurityScheme {
            flow: SecuritySchemeOAuth2Flow::Password,
            authorization_url: None,
            token_url: None,
            scopes: Scopes::default(),
            description: None,
            extensions: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".scopes: must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".tokenUrl: must be present for flow `password`")),
            "errors: {:?}",
            ctx.errors
        );

        // Missing authorizationUrl on `implicit` flow.
        let s = OAuth2SecurityScheme {
            flow: SecuritySchemeOAuth2Flow::Implicit,
            authorization_url: None,
            token_url: None,
            scopes: Scopes::from([("a".to_owned(), "b".to_owned())]),
            description: None,
            extensions: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".authorizationUrl: must be present for flow `implicit`")),
            "errors: {:?}",
            ctx.errors
        );

        // accessCode flow needs both URLs.
        let s = OAuth2SecurityScheme {
            flow: SecuritySchemeOAuth2Flow::AccessCode,
            authorization_url: None,
            token_url: None,
            scopes: Scopes::from([("a".to_owned(), "b".to_owned())]),
            description: None,
            extensions: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must be present for flow `accessCode`")),
            "errors: {:?}",
            ctx.errors
        );

        // Invalid URL format on tokenUrl/authorizationUrl.
        let s = OAuth2SecurityScheme {
            flow: SecuritySchemeOAuth2Flow::AccessCode,
            authorization_url: Some("ftp://x".into()),
            token_url: Some("ftp://y".into()),
            scopes: Scopes::from([("a".to_owned(), "b".to_owned())]),
            description: None,
            extensions: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must be a valid URL")),
            "errors: {:?}",
            ctx.errors
        );

        // application flow only needs tokenUrl
        let s = OAuth2SecurityScheme {
            flow: SecuritySchemeOAuth2Flow::Application,
            authorization_url: None,
            token_url: None,
            scopes: Scopes::from([("a".to_owned(), "b".to_owned())]),
            description: None,
            extensions: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must be present for flow `application`")),
            "errors: {:?}",
            ctx.errors
        );

        // Successful path: implicit with valid URL and one scope.
        let s = OAuth2SecurityScheme {
            flow: SecuritySchemeOAuth2Flow::Implicit,
            authorization_url: Some("https://example.com/auth".into()),
            token_url: None,
            scopes: Scopes::from([("a".to_owned(), "b".to_owned())]),
            description: None,
            extensions: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn scopes_serde_roundtrip_with_extensions() {
        let raw = json!({
            "read": "Read",
            "write": "Write",
            "x-extra": "ext-value",
        });
        let s: Scopes = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(s.scopes.len(), 2);
        assert!(s.extensions.is_some());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, raw);
    }

    #[test]
    fn scopes_only_extensions() {
        let raw = json!({"x-foo": "bar"});
        let s: Scopes = serde_json::from_value(raw.clone()).unwrap();
        assert!(s.scopes.is_empty());
        assert!(s.extensions.is_some());
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);
    }

    #[test]
    fn scopes_duplicate_key_error() {
        // duplicate scope key
        let err = serde_json::from_str::<Scopes>(r#"{"a": "x", "a": "y"}"#).unwrap_err();
        assert!(
            err.to_string().contains("duplicate field `a`"),
            "err: {err}"
        );

        // duplicate extension key
        let err = serde_json::from_str::<Scopes>(r#"{"x-a": "x", "x-a": "y"}"#).unwrap_err();
        assert!(
            err.to_string().contains("duplicate field `x-a`"),
            "err: {err}"
        );
    }

    #[test]
    fn scopes_helpers() {
        let s = Scopes::default();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);

        let s = Scopes::from([("a".to_owned(), "b".to_owned())]);
        assert!(!s.is_empty());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn security_scheme_display() {
        assert_eq!(
            format!("{}", SecurityScheme::Basic(BasicSecurityScheme::default())),
            "basic"
        );
        assert_eq!(
            format!(
                "{}",
                SecurityScheme::ApiKey(ApiKeySecurityScheme {
                    name: "x".into(),
                    location: SecuritySchemeApiKeyLocation::Header,
                    ..Default::default()
                })
            ),
            "apiKey"
        );
        assert_eq!(
            format!(
                "{}",
                SecurityScheme::OAuth2(OAuth2SecurityScheme::default())
            ),
            "oauth2"
        );
        assert_eq!(
            format!("{}", SecuritySchemeApiKeyLocation::Header),
            "header"
        );
        assert_eq!(format!("{}", SecuritySchemeApiKeyLocation::Query), "query");
        for (f, expected) in [
            (SecuritySchemeOAuth2Flow::Implicit, "implicit"),
            (SecuritySchemeOAuth2Flow::Password, "password"),
            (SecuritySchemeOAuth2Flow::Application, "application"),
            (SecuritySchemeOAuth2Flow::AccessCode, "accessCode"),
        ] {
            assert_eq!(format!("{f}"), expected);
        }
    }

    #[test]
    fn apikey_validate_required_name() {
        use crate::common::helpers::Context;
        use crate::validation::Options;
        let spec = Spec::default();
        let s = ApiKeySecurityScheme {
            name: "".into(),
            location: SecuritySchemeApiKeyLocation::Header,
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(ctx.errors.iter().any(|e| e.contains("must not be empty")));
    }

    #[test]
    fn test_security_scheme_oauth2_serialize() {
        assert_eq!(
            serde_json::to_value(SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: Some(String::from("https://example.com/api/oauth/dialog")),
                token_url: None,
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
                scopes: Scopes::from([
                    (
                        String::from("write:pets"),
                        String::from("modify pets in your account"),
                    ),
                    (String::from("read:pets"), String::from("read your pets"),),
                ]),
                description: Some(String::from("A short description for security scheme.")),
                extensions: None,
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
