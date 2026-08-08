//! AsyncAPI v2.6 `Server` and `Server Variable` objects.
//!
//! Per [Server Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#serverObject).
//!
//! 2.6 addresses a server with a single `url` (which v3 split into
//! `host` + `pathname`), and its `security` is a list of
//! [`SecurityRequirement`] maps rather than inline schemes.

use crate::common::bindings::Bindings;
use crate::common::reference::RefOr;
use crate::v2_6::security_scheme::SecurityRequirement;
use crate::v2_6::tag::Tag;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Server {
    /// **Required** The server URL. It MAY contain `{variable}`
    /// template placeholders, and MAY be relative.
    pub url: String,

    /// **Required** The protocol this server supports for connectivity.
    pub protocol: String,

    /// The version of the protocol used for connection.
    #[serde(rename = "protocolVersion", skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,

    /// An optional description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A map between a variable name and its value, for substitution
    /// into `url`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, RefOr<ServerVariable>>,

    /// The security requirements for this server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,

    /// Tags for logical grouping and categorization of servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,

    /// Protocol-specific definitions for the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<Bindings>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Server {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("url", &self.url);
        ctx.require_non_empty("protocol", &self.protocol);

        // Every `{placeholder}` in the URL needs a variable.
        for placeholder in placeholders(&self.url) {
            if !self.variables.contains_key(placeholder) {
                ctx.error_field(
                    "variables",
                    format!("`{{{placeholder}}}` is not declared in `variables`"),
                );
            }
        }

        for (name, variable) in &self.variables {
            ctx.in_key("variables", name, |ctx| variable.validate_with_context(ctx));
        }
        for (i, requirement) in self.security.iter().enumerate() {
            ctx.in_index("security", i, |ctx| requirement.validate_with_context(ctx));
        }
        crate::v2_6::message::validate_tags(ctx, &self.tags);
        if let Some(bindings) = &self.bindings {
            ctx.in_field("bindings", |ctx| bindings.validate_with_context(ctx));
        }
    }
}

/// The `{placeholder}` names appearing in `input`, in order.
pub(crate) fn placeholders(input: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if !name.is_empty() {
            found.push(name);
        }
        rest = &after[end + 1..];
    }
    found
}

/// A variable substituted into a server's `url`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ServerVariable {
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for ServerVariable {
    fn validate_with_context(&self, ctx: &mut Context) {
        // `uniqueItems: true` on the enumeration.
        for (i, value) in self.enum_values.iter().enumerate() {
            if self.enum_values[..i].contains(value) {
                ctx.in_index("enum", i, |ctx| {
                    ctx.error(format!("duplicate value `{value}`"))
                });
            }
        }
        if let Some(default) = &self.default
            && !self.enum_values.is_empty()
            && !self.enum_values.contains(default)
        {
            ctx.error_field(
                "default",
                format!("`{default}` is not one of the values listed in `enum`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn errors_for(value: serde_json::Value) -> Vec<String> {
        let server: Server = serde_json::from_value(value).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.servers.prod");
        server.validate_with_context(&mut ctx);
        ctx.errors.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn round_trips_a_full_server() {
        let value = json!({
            "url": "amqp://{stage}.example.com:{port}",
            "protocol": "amqp",
            "protocolVersion": "0.9.1",
            "description": "The production broker",
            "variables": {
                "stage": { "enum": ["prod", "staging"], "default": "prod" },
                "port": { "default": "5672", "examples": ["5672"] }
            },
            "security": [ { "user_pass": [] } ],
            "tags": [ { "name": "prod" } ],
            "bindings": { "amqp": { "bindingVersion": "0.3.0" } },
            "x-team": "infra"
        });
        let server: Server = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(server.protocol, "amqp");
        assert_eq!(serde_json::to_value(&server).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn url_and_protocol_are_required() {
        let errors = errors_for(json!({ "url": "", "protocol": "" }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.url: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.protocol: must not be empty")
        );
    }

    #[test]
    fn undeclared_url_placeholders_are_reported() {
        let errors = errors_for(json!({
            "url": "amqp://{stage}.example.com:{port}",
            "protocol": "amqp",
            "variables": { "port": { "default": "5672" } }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`{stage}` is not declared"))
        );
        assert!(!errors.iter().any(|e| e.contains("`{port}`")));
    }

    #[test]
    fn placeholders_handles_edge_cases() {
        assert_eq!(placeholders("{a}.b.{c}"), vec!["a", "c"]);
        assert_eq!(placeholders("no-placeholders"), Vec::<&str>::new());
        assert_eq!(placeholders("{}"), Vec::<&str>::new());
        assert_eq!(placeholders("{a}.{unclosed"), vec!["a"]);
    }

    #[test]
    fn nested_objects_report_under_their_own_path() {
        let errors = errors_for(json!({
            "url": "amqp://e",
            "protocol": "amqp",
            "variables": { "v": { "enum": ["a"], "default": "b" } },
            "security": [ { "auth": ["read", "read"] } ],
            "tags": [ { "name": "" } ],
            "bindings": { "amqp": 1 }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.servers.prod.variables.v.default"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.security[0].auth: duplicate scope `read`")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.servers.prod.bindings.amqp"))
        );
    }

    #[test]
    fn tags_and_variable_enums_must_be_unique() {
        let errors = errors_for(json!({
            "url": "amqp://e",
            "protocol": "amqp",
            "tags": [ { "name": "a" }, { "name": "a" } ],
            "variables": { "v": { "enum": ["a", "b", "a"] } }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.tags[1]: duplicate tag")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.variables.v.enum[2]: duplicate value `a`"),
            "got: {errors:?}"
        );
    }

    #[test]
    fn a_server_has_no_external_docs_field_in_2_6() {
        // The 2.6 Server Object has no `externalDocs`; it is dropped as
        // an unknown key rather than parsed.
        let server: Server = serde_json::from_value(json!({
            "url": "amqp://e",
            "protocol": "amqp",
            "externalDocs": { "url": "https://e" }
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&server).unwrap(),
            json!({ "url": "amqp://e", "protocol": "amqp" })
        );
    }

    #[test]
    fn server_variable_default_must_be_enumerated() {
        let variable: ServerVariable =
            serde_json::from_value(json!({ "enum": ["a"], "default": "z" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.variables.v");
        variable.validate_with_context(&mut ctx);
        assert!(ctx.errors[0].contains("is not one of the values listed in `enum`"));

        let free: ServerVariable = serde_json::from_value(json!({ "default": "z" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.variables.v");
        free.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }
}
