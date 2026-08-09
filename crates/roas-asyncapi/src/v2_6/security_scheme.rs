//! AsyncAPI v2.6 `Security Scheme`, `Security Requirement`, and OAuth 2
//! flow objects.
//!
//! Per [Security Scheme Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#securitySchemeObject).
//!
//! Two shapes differ from v3. A *requirement* here is the OpenAPI-style
//! map of scheme name → scopes ([`SecurityRequirement`]), where v3
//! carries a list of schemes inline. And an OAuth flow spells its scope
//! map `scopes`, which v3 renamed to `availableScopes`.

use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A map of security scheme name → the scopes it requires.
///
/// The scope list must be empty for every scheme type except `oauth2`
/// and `openIdConnect`; the document validator checks that, since only
/// it can resolve a name to its scheme.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(transparent)]
pub struct SecurityRequirement(pub BTreeMap<String, Vec<String>>);

impl SecurityRequirement {
    /// The scopes required for `name`, if the requirement names it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Vec<String>> {
        self.0.get(name)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl ValidateWithContext for SecurityRequirement {
    fn validate_with_context(&self, ctx: &mut Context) {
        for (name, scopes) in &self.0 {
            // `uniqueItems` on the scope array.
            for (i, scope) in scopes.iter().enumerate() {
                if scopes[..i].contains(scope) {
                    ctx.error_field(name, format!("duplicate scope `{scope}`"));
                }
            }
        }
    }
}

/// The `type` discriminator of a security scheme.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
pub enum SecuritySchemeType {
    #[default]
    #[serde(rename = "userPassword")]
    UserPassword,
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "X509")]
    X509,
    #[serde(rename = "symmetricEncryption")]
    SymmetricEncryption,
    #[serde(rename = "asymmetricEncryption")]
    AsymmetricEncryption,
    #[serde(rename = "httpApiKey")]
    HttpApiKey,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "oauth2")]
    OAuth2,
    #[serde(rename = "openIdConnect")]
    OpenIdConnect,
    #[serde(rename = "plain")]
    Plain,
    #[serde(rename = "scramSha256")]
    ScramSha256,
    #[serde(rename = "scramSha512")]
    ScramSha512,
    #[serde(rename = "gssapi")]
    Gssapi,
}

impl SecuritySchemeType {
    /// The `type` value as it appears in a document.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserPassword => "userPassword",
            Self::ApiKey => "apiKey",
            Self::X509 => "X509",
            Self::SymmetricEncryption => "symmetricEncryption",
            Self::AsymmetricEncryption => "asymmetricEncryption",
            Self::HttpApiKey => "httpApiKey",
            Self::Http => "http",
            Self::OAuth2 => "oauth2",
            Self::OpenIdConnect => "openIdConnect",
            Self::Plain => "plain",
            Self::ScramSha256 => "scramSha256",
            Self::ScramSha512 => "scramSha512",
            Self::Gssapi => "gssapi",
        }
    }

    /// Whether a security requirement naming this scheme may list
    /// scopes.
    #[must_use]
    pub fn takes_scopes(self) -> bool {
        matches!(self, Self::OAuth2 | Self::OpenIdConnect)
    }
}

/// Where an API key lives.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    User,
    Password,
    Query,
    Header,
    Cookie,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct SecurityScheme {
    /// **Required** The type of the security scheme.
    #[serde(rename = "type")]
    pub scheme_type: SecuritySchemeType,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// **Required for `apiKey` / `httpApiKey`** — where the key is
    /// carried.
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_: Option<ApiKeyLocation>,

    /// **Required for `httpApiKey`** — the header / query / cookie name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// **Required for `http`** — the HTTP Authorization scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,

    #[serde(rename = "bearerFormat", skip_serializing_if = "Option::is_none")]
    pub bearer_format: Option<String>,

    /// **Required for `oauth2`** — the supported flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flows: Option<OAuthFlows>,

    /// **Required for `openIdConnect`** — the discovery URL.
    #[serde(rename = "openIdConnectUrl", skip_serializing_if = "Option::is_none")]
    pub open_id_connect_url: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl SecurityScheme {
    /// The fields this scheme's `type` may carry, beyond `type`,
    /// `description`, and `x-` extensions.
    fn allowed_fields(&self) -> &'static [&'static str] {
        match self.scheme_type {
            SecuritySchemeType::ApiKey => &["in"],
            SecuritySchemeType::HttpApiKey => &["in", "name"],
            SecuritySchemeType::Http => &["scheme", "bearerFormat"],
            SecuritySchemeType::OAuth2 => &["flows"],
            SecuritySchemeType::OpenIdConnect => &["openIdConnectUrl"],
            _ => &[],
        }
    }

    /// Report any field belonging to a different variant — each branch
    /// of the schema's `oneOf` is `additionalProperties: false`.
    fn check_forbidden_fields(&self, ctx: &mut Context) {
        let allowed = self.allowed_fields();
        for (field, present) in [
            ("in", self.in_.is_some()),
            ("name", self.name.is_some()),
            ("scheme", self.scheme.is_some()),
            ("bearerFormat", self.bearer_format.is_some()),
            ("flows", self.flows.is_some()),
            ("openIdConnectUrl", self.open_id_connect_url.is_some()),
        ] {
            if present && !allowed.contains(&field) {
                ctx.error_field(
                    field,
                    format!(
                        "is not allowed on a `{}` security scheme",
                        self.scheme_type.as_str()
                    ),
                );
            }
        }
    }
}

impl ValidateWithContext for SecurityScheme {
    fn validate_with_context(&self, ctx: &mut Context) {
        self.check_forbidden_fields(ctx);
        match self.scheme_type {
            SecuritySchemeType::ApiKey => match self.in_ {
                None => ctx.error_field("in", "is required for the `apiKey` type"),
                Some(ApiKeyLocation::User | ApiKeyLocation::Password) => {}
                Some(_) => ctx.error_field("in", "must be `user` or `password` for `apiKey`"),
            },
            SecuritySchemeType::HttpApiKey => {
                match self.in_ {
                    None => ctx.error_field("in", "is required for the `httpApiKey` type"),
                    Some(
                        ApiKeyLocation::Query | ApiKeyLocation::Header | ApiKeyLocation::Cookie,
                    ) => {}
                    Some(_) => ctx.error_field(
                        "in",
                        "must be `query`, `header` or `cookie` for `httpApiKey`",
                    ),
                }
                match &self.name {
                    None => ctx.error_field("name", "is required for the `httpApiKey` type"),
                    Some(name) => ctx.require_non_empty("name", name),
                }
            }
            SecuritySchemeType::Http => {
                match &self.scheme {
                    None => ctx.error_field("scheme", "is required for the `http` type"),
                    Some(scheme) => ctx.require_non_empty("scheme", scheme),
                }
                if self.bearer_format.is_some()
                    && self.scheme.as_deref().is_some_and(|s| s != "bearer")
                {
                    ctx.error_field("bearerFormat", "is only allowed when `scheme` is `bearer`");
                }
            }
            SecuritySchemeType::OAuth2 => match &self.flows {
                None => ctx.error_field("flows", "is required for the `oauth2` type"),
                Some(flows) => ctx.in_field("flows", |ctx| flows.validate_with_context(ctx)),
            },
            SecuritySchemeType::OpenIdConnect => match &self.open_id_connect_url {
                None => ctx.error_field(
                    "openIdConnectUrl",
                    "is required for the `openIdConnect` type",
                ),
                Some(url) => ctx.require_non_empty("openIdConnectUrl", url),
            },
            _ => {}
        }
    }
}

/// The OAuth 2 flows a scheme supports.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OAuthFlows {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implicit: Option<OAuthFlow>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<OAuthFlow>,

    #[serde(rename = "clientCredentials", skip_serializing_if = "Option::is_none")]
    pub client_credentials: Option<OAuthFlow>,

    #[serde(rename = "authorizationCode", skip_serializing_if = "Option::is_none")]
    pub authorization_code: Option<OAuthFlow>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for OAuthFlows {
    fn validate_with_context(&self, ctx: &mut Context) {
        if self.implicit.is_none()
            && self.password.is_none()
            && self.client_credentials.is_none()
            && self.authorization_code.is_none()
        {
            ctx.error("must define at least one OAuth flow");
        }

        for (field, flow, required, forbidden) in [
            (
                "implicit",
                self.implicit.as_ref(),
                &["authorizationUrl"][..],
                &["tokenUrl"][..],
            ),
            (
                "password",
                self.password.as_ref(),
                &["tokenUrl"][..],
                &["authorizationUrl"][..],
            ),
            (
                "clientCredentials",
                self.client_credentials.as_ref(),
                &["tokenUrl"][..],
                &["authorizationUrl"][..],
            ),
            (
                "authorizationCode",
                self.authorization_code.as_ref(),
                &["authorizationUrl", "tokenUrl"][..],
                &[][..],
            ),
        ] {
            let Some(flow) = flow else { continue };
            ctx.in_field(field, |ctx| {
                flow.validate_grant(ctx, field, required, forbidden);
            });
        }
    }
}

/// Configuration for one OAuth 2 flow.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct OAuthFlow {
    #[serde(rename = "authorizationUrl", skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,

    #[serde(rename = "tokenUrl", skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,

    #[serde(rename = "refreshUrl", skip_serializing_if = "Option::is_none")]
    pub refresh_url: Option<String>,

    /// **Required** The available scopes, keyed by scope name. v3
    /// renamed this field to `availableScopes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<BTreeMap<String, String>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl OAuthFlow {
    fn url(&self, field: &str) -> Option<&String> {
        match field {
            "authorizationUrl" => self.authorization_url.as_ref(),
            "tokenUrl" => self.token_url.as_ref(),
            _ => None,
        }
    }

    /// Validate this flow against its grant type's contract.
    fn validate_grant(
        &self,
        ctx: &mut Context,
        grant: &str,
        required: &[&str],
        forbidden: &[&str],
    ) {
        for field in required {
            match self.url(field) {
                None => ctx.error_field(field, format!("is required for the `{grant}` flow")),
                Some(url) => ctx.require_non_empty(field, url),
            }
        }
        for field in forbidden {
            if self.url(field).is_some() {
                ctx.error_field(field, format!("is not allowed on the `{grant}` flow"));
            }
        }
        if self.scopes.is_none() {
            ctx.error_field("scopes", format!("is required for the `{grant}` flow"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(value: serde_json::Value) -> Vec<String> {
        let scheme: SecurityScheme = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.securitySchemes.s");
        scheme.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn simple_types_need_nothing_beyond_the_discriminator() {
        for scheme_type in [
            "userPassword",
            "X509",
            "symmetricEncryption",
            "asymmetricEncryption",
            "plain",
            "scramSha256",
            "scramSha512",
            "gssapi",
        ] {
            assert!(
                validate(json!({ "type": scheme_type })).is_empty(),
                "{scheme_type} should be valid"
            );
        }
    }

    #[test]
    fn oauth_flows_use_the_2_6_scopes_field() {
        let value = json!({
            "type": "oauth2",
            "flows": {
                "authorizationCode": {
                    "authorizationUrl": "https://example.com/auth",
                    "tokenUrl": "https://example.com/token",
                    "scopes": { "read": "Read access" }
                }
            }
        });
        let scheme: SecurityScheme = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&scheme).unwrap(), value);
        assert!(validate(value).is_empty());

        // `availableScopes` is the v3 spelling and is not recognized
        // here, so the flow reads as missing its scopes.
        let errors = validate(json!({
            "type": "oauth2",
            "flows": { "implicit": { "authorizationUrl": "https://e/a", "availableScopes": {} } }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("implicit.scopes: is required")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn each_grant_type_forbids_the_url_it_does_not_use() {
        let errors = validate(json!({
            "type": "oauth2",
            "flows": {
                "implicit": {
                    "authorizationUrl": "https://e/a",
                    "tokenUrl": "https://e/t",
                    "scopes": {}
                }
            }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("implicit.tokenUrl: is not allowed")),
            "got: {errors:?}"
        );

        for grant in ["password", "clientCredentials"] {
            let errors = validate(json!({
                "type": "oauth2",
                "flows": {
                    grant: { "tokenUrl": "https://e/t", "authorizationUrl": "https://e/a", "scopes": {} }
                }
            }));
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains(&format!("{grant}.authorizationUrl: is not allowed"))),
                "{grant}: {errors:?}"
            );
        }
    }

    #[test]
    fn per_type_requirements_are_enforced() {
        assert!(validate(json!({ "type": "apiKey", "in": "user" })).is_empty());
        assert!(
            validate(json!({ "type": "apiKey", "in": "header" }))[0]
                .contains("must be `user` or `password`")
        );
        assert!(validate(json!({ "type": "http" }))[0].contains("scheme: is required"));
        assert!(
            validate(json!({ "type": "http", "scheme": "basic", "bearerFormat": "JWT" }))[0]
                .contains("only allowed when `scheme` is `bearer`")
        );
        assert!(validate(json!({ "type": "oauth2" }))[0].contains("flows: is required"));
        assert!(validate(json!({ "type": "openIdConnect" }))[0].contains("openIdConnectUrl"));
        assert!(
            validate(json!({ "type": "httpApiKey", "in": "header", "name": "X-Key" })).is_empty()
        );
    }

    #[test]
    fn missing_and_empty_per_type_fields_are_each_reported() {
        for (value, needle) in [
            (
                json!({ "type": "apiKey" }),
                "in: is required for the `apiKey` type",
            ),
            (
                json!({ "type": "httpApiKey", "name": "X-Key" }),
                "in: is required for the `httpApiKey` type",
            ),
            (
                json!({ "type": "httpApiKey", "in": "user", "name": "X-Key" }),
                "must be `query`, `header` or `cookie`",
            ),
            (
                json!({ "type": "httpApiKey", "in": "header" }),
                "name: is required for the `httpApiKey` type",
            ),
            (
                json!({ "type": "httpApiKey", "in": "header", "name": "" }),
                "name: must not be empty",
            ),
            (
                json!({ "type": "openIdConnect", "openIdConnectUrl": "" }),
                "openIdConnectUrl: must not be empty",
            ),
            (
                json!({ "type": "oauth2", "flows": {} }),
                "at least one OAuth flow",
            ),
            (
                json!({
                    "type": "oauth2",
                    "flows": { "password": { "tokenUrl": "", "scopes": {} } }
                }),
                "password.tokenUrl: must not be empty",
            ),
            (
                json!({
                    "type": "oauth2",
                    "flows": { "authorizationCode": { "scopes": {} } }
                }),
                "authorizationCode.authorizationUrl: is required",
            ),
        ] {
            let errors = validate(value.clone());
            assert!(
                errors.iter().any(|e| e.contains(needle)),
                "{value}: expected {needle}, got {errors:?}"
            );
        }
    }

    #[test]
    fn fields_belonging_to_another_variant_are_rejected() {
        let errors = validate(json!({ "type": "userPassword", "flows": {} }));
        assert!(
            errors.iter().any(|e| e
                == "#.securitySchemes.s.flows: is not allowed on a `userPassword` security scheme"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn type_renders_as_its_document_spelling_and_knows_about_scopes() {
        for (scheme_type, text, scopes) in [
            (SecuritySchemeType::UserPassword, "userPassword", false),
            (SecuritySchemeType::ApiKey, "apiKey", false),
            (SecuritySchemeType::X509, "X509", false),
            (
                SecuritySchemeType::SymmetricEncryption,
                "symmetricEncryption",
                false,
            ),
            (
                SecuritySchemeType::AsymmetricEncryption,
                "asymmetricEncryption",
                false,
            ),
            (SecuritySchemeType::HttpApiKey, "httpApiKey", false),
            (SecuritySchemeType::Http, "http", false),
            (SecuritySchemeType::OAuth2, "oauth2", true),
            (SecuritySchemeType::OpenIdConnect, "openIdConnect", true),
            (SecuritySchemeType::Plain, "plain", false),
            (SecuritySchemeType::ScramSha256, "scramSha256", false),
            (SecuritySchemeType::ScramSha512, "scramSha512", false),
            (SecuritySchemeType::Gssapi, "gssapi", false),
        ] {
            assert_eq!(scheme_type.as_str(), text);
            assert_eq!(serde_json::to_value(scheme_type).unwrap(), json!(text));
            assert_eq!(scheme_type.takes_scopes(), scopes, "{text}");
        }
    }

    #[test]
    fn security_requirement_round_trips_and_rejects_duplicate_scopes() {
        let value = json!({ "petstore_auth": ["write:pets", "read:pets"] });
        let requirement: SecurityRequirement = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&requirement).unwrap(), value);
        assert_eq!(requirement.get("petstore_auth").map(Vec::len), Some(2));
        assert!(requirement.get("absent").is_none());
        assert!(!requirement.is_empty());
        assert!(SecurityRequirement::default().is_empty());

        let dup: SecurityRequirement =
            serde_json::from_value(json!({ "auth": ["read", "read"] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.security[0]");
        dup.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.security[0].auth: duplicate scope `read`"),
            "got: {:?}",
            ctx.errors
        );
    }
}
