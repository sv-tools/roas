//! AsyncAPI v3.0 `Security Scheme` object and its OAuth 2 flows.
//!
//! Per [Security Scheme Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#securitySchemeObject).
//!
//! The schema spells this as a `oneOf` over nine shapes that share a
//! `type` discriminator, each with `additionalProperties: false`. It is
//! modeled here as one struct with a typed `type` plus the union of the
//! optional fields; the validator enforces both directions of each
//! variant's contract — the fields the `type` requires, and the fields
//! it forbids — so a wrong combination is a precise diagnostic rather
//! than an opaque "matched no variant" parse error.

use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

    /// A short description for the security scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// **Required for `apiKey` / `httpApiKey`** — where the key is
    /// carried. `apiKey` takes `user` / `password`; `httpApiKey` takes
    /// `query` / `header` / `cookie`.
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_: Option<ApiKeyLocation>,

    /// **Required for `httpApiKey`** — the header / query / cookie name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// **Required for `http`** — the HTTP Authorization scheme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,

    /// A hint to the client to identify how the bearer token is
    /// formatted (`http` with the `bearer` scheme).
    #[serde(rename = "bearerFormat", skip_serializing_if = "Option::is_none")]
    pub bearer_format: Option<String>,

    /// **Required for `oauth2`** — the supported flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flows: Option<OAuthFlows>,

    /// **Required for `openIdConnect`** — the OpenID Connect discovery
    /// URL.
    #[serde(rename = "openIdConnectUrl", skip_serializing_if = "Option::is_none")]
    pub open_id_connect_url: Option<String>,

    /// The list of scopes required by `oauth2` / `openIdConnect`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,

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
            SecuritySchemeType::OAuth2 => &["flows", "scopes"],
            SecuritySchemeType::OpenIdConnect => &["openIdConnectUrl", "scopes"],
            _ => &[],
        }
    }

    /// Report any field that belongs to a different variant. Each
    /// variant is `additionalProperties: false` in the schema, so
    /// carrying a foreign field breaks the `oneOf`.
    fn check_forbidden_fields(&self, ctx: &mut Context) {
        let allowed = self.allowed_fields();
        let present: [(&str, bool); 6] = [
            ("in", self.in_.is_some()),
            ("name", self.name.is_some()),
            ("scheme", self.scheme.is_some()),
            ("bearerFormat", self.bearer_format.is_some()),
            ("flows", self.flows.is_some()),
            ("openIdConnectUrl", self.open_id_connect_url.is_some()),
        ];
        for (field, is_present) in present {
            if is_present && !allowed.contains(&field) {
                ctx.error_field(
                    field,
                    format!(
                        "is not allowed on a `{}` security scheme",
                        self.scheme_type.as_str()
                    ),
                );
            }
        }
        if !self.scopes.is_empty() && !allowed.contains(&"scopes") {
            ctx.error_field(
                "scopes",
                format!(
                    "is not allowed on a `{}` security scheme",
                    self.scheme_type.as_str()
                ),
            );
        }
    }
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
            SecuritySchemeType::Http => match &self.scheme {
                None => ctx.error_field("scheme", "is required for the `http` type"),
                Some(scheme) => ctx.require_non_empty("scheme", scheme),
            },
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
        // Each flow requires the URLs its grant type actually uses.
        if let Some(flow) = &self.implicit {
            ctx.in_field("implicit", |ctx| {
                flow.validate_with_context(ctx);
                flow.require_url(ctx, "authorizationUrl", &flow.authorization_url);
            });
        }
        if let Some(flow) = &self.password {
            ctx.in_field("password", |ctx| {
                flow.validate_with_context(ctx);
                flow.require_url(ctx, "tokenUrl", &flow.token_url);
            });
        }
        if let Some(flow) = &self.client_credentials {
            ctx.in_field("clientCredentials", |ctx| {
                flow.validate_with_context(ctx);
                flow.require_url(ctx, "tokenUrl", &flow.token_url);
            });
        }
        if let Some(flow) = &self.authorization_code {
            ctx.in_field("authorizationCode", |ctx| {
                flow.validate_with_context(ctx);
                flow.require_url(ctx, "authorizationUrl", &flow.authorization_url);
                flow.require_url(ctx, "tokenUrl", &flow.token_url);
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

    /// **Required** The available scopes, keyed by scope name. Renamed
    /// from `scopes` in AsyncAPI 2.x.
    #[serde(rename = "availableScopes", default)]
    pub available_scopes: BTreeMap<String, String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl OAuthFlow {
    fn require_url(&self, ctx: &mut Context, field: &str, value: &Option<String>) {
        match value {
            None => ctx.error_field(field, format!("`{field}` is required for this flow")),
            Some(url) => ctx.require_non_empty(field, url),
        }
    }
}

impl ValidateWithContext for OAuthFlow {
    fn validate_with_context(&self, _ctx: &mut Context) {
        // Per-flow URL requirements are enforced by `OAuthFlows`, which
        // knows which grant type each flow is.
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
            let errors = validate(json!({ "type": scheme_type }));
            assert!(errors.is_empty(), "{scheme_type}: {errors:?}");
        }
    }

    #[test]
    fn round_trips_an_oauth2_scheme() {
        let value = json!({
            "type": "oauth2",
            "description": "OAuth",
            "scopes": ["read"],
            "flows": {
                "authorizationCode": {
                    "authorizationUrl": "https://example.com/auth",
                    "tokenUrl": "https://example.com/token",
                    "availableScopes": { "read": "Read access" }
                }
            }
        });
        let scheme: SecurityScheme = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(scheme.scheme_type, SecuritySchemeType::OAuth2);
        assert_eq!(serde_json::to_value(&scheme).unwrap(), value);
        assert!(validate(value).is_empty());
    }

    #[test]
    fn api_key_location_is_constrained_to_user_or_password() {
        assert!(validate(json!({ "type": "apiKey", "in": "user" })).is_empty());

        let errors = validate(json!({ "type": "apiKey", "in": "header" }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("must be `user` or `password`")),
            "got: {errors:?}"
        );

        let errors = validate(json!({ "type": "apiKey" }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("is required for the `apiKey`"))
        );
    }

    #[test]
    fn http_api_key_requires_name_and_a_transport_location() {
        assert!(
            validate(json!({ "type": "httpApiKey", "name": "X-Key", "in": "header" })).is_empty()
        );

        let errors = validate(json!({ "type": "httpApiKey", "in": "user", "name": "" }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("must be `query`, `header` or `cookie`"))
        );
        assert!(errors.iter().any(|e| e.contains("name: must not be empty")));

        let errors = validate(json!({ "type": "httpApiKey" }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("in` is required") || e.contains("in: is required"))
        );
        assert!(errors.iter().any(|e| e.contains("name: is required")));
    }

    #[test]
    fn http_requires_a_scheme_and_openid_a_url() {
        assert!(validate(json!({ "type": "http", "scheme": "bearer" })).is_empty());
        assert!(validate(json!({ "type": "http" }))[0].contains("scheme: is required"));
        assert!(validate(json!({ "type": "http", "scheme": "" }))[0].contains("must not be empty"));

        assert!(
            validate(json!({ "type": "openIdConnect", "openIdConnectUrl": "https://e/x" }))
                .is_empty()
        );
        assert!(validate(json!({ "type": "openIdConnect" }))[0].contains("openIdConnectUrl"));
    }

    #[test]
    fn oauth2_requires_flows_and_each_flow_its_urls() {
        assert!(validate(json!({ "type": "oauth2" }))[0].contains("flows: is required"));

        let errors = validate(json!({ "type": "oauth2", "flows": {} }));
        assert!(errors.iter().any(|e| e.contains("at least one OAuth flow")));

        let errors = validate(json!({
            "type": "oauth2",
            "flows": {
                "implicit": { "availableScopes": {} },
                "password": { "availableScopes": {} },
                "clientCredentials": { "availableScopes": {} },
                "authorizationCode": { "availableScopes": {} }
            }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("implicit.authorizationUrl"))
        );
        assert!(errors.iter().any(|e| e.contains("password.tokenUrl")));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("clientCredentials.tokenUrl"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("authorizationCode.authorizationUrl"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("authorizationCode.tokenUrl"))
        );
    }

    #[test]
    fn fields_belonging_to_another_variant_are_rejected() {
        // Each `oneOf` branch is `additionalProperties: false`, so a
        // `userPassword` scheme carrying `flows` breaks the contract.
        let errors = validate(json!({ "type": "userPassword", "flows": {} }));
        assert!(
            errors.iter().any(|e| e
                == "#.securitySchemes.s.flows: is not allowed on a `userPassword` security scheme"),
            "got: {errors:?}"
        );

        let errors = validate(json!({
            "type": "http",
            "scheme": "bearer",
            "in": "header",
            "name": "X-Key",
            "openIdConnectUrl": "https://e/x",
            "scopes": ["read"]
        }));
        for field in ["in", "name", "openIdConnectUrl", "scopes"] {
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains(&format!(".{field}: is not allowed"))),
                "expected {field} to be rejected, got: {errors:?}"
            );
        }
        // `bearerFormat` belongs to `http`, so it is not reported.
        assert!(!errors.iter().any(|e| e.contains("bearerFormat")));
    }

    #[test]
    fn each_variant_keeps_its_own_fields() {
        for value in [
            json!({ "type": "apiKey", "in": "user" }),
            json!({ "type": "httpApiKey", "in": "header", "name": "X-Key" }),
            json!({ "type": "http", "scheme": "bearer", "bearerFormat": "JWT" }),
            json!({
                "type": "oauth2",
                "scopes": ["read"],
                "flows": { "clientCredentials": { "tokenUrl": "https://e/t", "availableScopes": {} } }
            }),
            json!({ "type": "openIdConnect", "openIdConnectUrl": "https://e/x", "scopes": ["read"] }),
        ] {
            let errors = validate(value.clone());
            assert!(errors.is_empty(), "{value}: {errors:?}");
        }
    }

    #[test]
    fn type_renders_as_its_document_spelling() {
        for (scheme_type, text) in [
            (SecuritySchemeType::UserPassword, "userPassword"),
            (SecuritySchemeType::ApiKey, "apiKey"),
            (SecuritySchemeType::X509, "X509"),
            (
                SecuritySchemeType::SymmetricEncryption,
                "symmetricEncryption",
            ),
            (
                SecuritySchemeType::AsymmetricEncryption,
                "asymmetricEncryption",
            ),
            (SecuritySchemeType::HttpApiKey, "httpApiKey"),
            (SecuritySchemeType::Http, "http"),
            (SecuritySchemeType::OAuth2, "oauth2"),
            (SecuritySchemeType::OpenIdConnect, "openIdConnect"),
            (SecuritySchemeType::Plain, "plain"),
            (SecuritySchemeType::ScramSha256, "scramSha256"),
            (SecuritySchemeType::ScramSha512, "scramSha512"),
            (SecuritySchemeType::Gssapi, "gssapi"),
        ] {
            assert_eq!(scheme_type.as_str(), text);
            assert_eq!(serde_json::to_value(scheme_type).unwrap(), json!(text));
        }
    }

    #[test]
    fn empty_flow_urls_are_reported_as_empty() {
        let errors = validate(json!({
            "type": "oauth2",
            "flows": { "password": { "tokenUrl": "", "availableScopes": {} } }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.securitySchemes.s.flows.password.tokenUrl: must not be empty"),
            "got: {errors:?}"
        );
    }
}
