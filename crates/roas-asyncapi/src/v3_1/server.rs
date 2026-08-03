//! AsyncAPI v3.1 `Server` and `Server Variable` objects.
//!
//! Per [Server Object](https://www.asyncapi.com/docs/reference/specification/v3.1.0#serverObject).
//!
//! v3 splits the 2.x `url` into `host` + `pathname`, so a server is
//! addressed as `<protocol>://<host><pathname>`.

use crate::common::bindings::Bindings;
use crate::common::reference::RefOr;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::security_scheme::SecurityScheme;
use crate::v3_1::tag::Tag;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Server {
    /// **Required** The server host name, optionally with a port. It
    /// MAY contain `{variable}` template placeholders.
    pub host: String,

    /// **Required** The protocol this server supports for connectivity,
    /// e.g. `kafka`, `amqp`, `ws`.
    pub protocol: String,

    /// The path to a resource in the host, e.g. `/production`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pathname: Option<String>,

    /// The version of the protocol used for connection.
    #[serde(rename = "protocolVersion", skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,

    /// A human-friendly title for the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short summary of the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// An optional description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A map between a variable name and its value, for substitution
    /// into `host` / `pathname`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, RefOr<ServerVariable>>,

    /// The security requirements for this server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<RefOr<SecurityScheme>>,

    /// Tags for logical grouping and categorization of servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<RefOr<Tag>>,

    /// Additional external documentation for this server.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    /// Protocol-specific definitions for the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bindings: Option<RefOr<Bindings>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Server {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("host", &self.host);
        ctx.require_non_empty("protocol", &self.protocol);

        // The host carries the scheme in `protocol`, so repeating it
        // here yields an unusable address like `kafka://kafka://…`.
        if self.host.contains("://") {
            ctx.error_field(
                "host",
                "must not include a scheme — use `protocol` for that",
            );
        }
        if let Some(pathname) = &self.pathname
            && !pathname.is_empty()
            && !pathname.starts_with('/')
        {
            ctx.error_field("pathname", "must start with `/`");
        }

        // Every `{placeholder}` in the address needs a variable.
        let mut declared: Vec<&str> = Vec::new();
        for name in self.variables.keys() {
            declared.push(name.as_str());
        }
        let address = match &self.pathname {
            Some(pathname) => format!("{}{pathname}", self.host),
            None => self.host.clone(),
        };
        for placeholder in placeholders(&address) {
            if !declared.contains(&placeholder) {
                ctx.error_field(
                    "variables",
                    format!("`{{{placeholder}}}` is not declared in `variables`"),
                );
            }
        }

        for (name, variable) in &self.variables {
            ctx.in_key("variables", name, |ctx| variable.validate_with_context(ctx));
        }
        for (i, scheme) in self.security.iter().enumerate() {
            ctx.in_index("security", i, |ctx| scheme.validate_with_context(ctx));
        }
        for (i, tag) in self.tags.iter().enumerate() {
            ctx.in_index("tags", i, |ctx| tag.validate_with_context(ctx));
        }
        if let Some(docs) = &self.external_docs {
            ctx.in_field("externalDocs", |ctx| docs.validate_with_context(ctx));
        }
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

/// A variable substituted into a server's `host` / `pathname`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ServerVariable {
    /// An enumeration of string values to be used if the substitution
    /// options are from a limited set.
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,

    /// The default value to use for substitution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// An optional description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// An array of examples of the server variable.
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
            "host": "rabbitmq.example.com:5672",
            "protocol": "amqp",
            "pathname": "/production",
            "protocolVersion": "0.9.1",
            "title": "Production",
            "summary": "Prod broker",
            "description": "The production broker",
            "variables": { "port": { "default": "5672", "enum": ["5672", "5673"] } },
            "security": [ { "type": "userPassword" } ],
            "tags": [ { "name": "prod" } ],
            "externalDocs": { "url": "https://example.com" },
            "bindings": { "amqp": { "bindingVersion": "0.3.0" } },
            "x-team": "infra"
        });
        let server: Server = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(server.protocol, "amqp");
        assert_eq!(serde_json::to_value(&server).unwrap(), value);
        assert!(errors_for(value).is_empty());
    }

    #[test]
    fn host_and_protocol_are_required() {
        let errors = errors_for(json!({ "host": "", "protocol": "" }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.host: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.protocol: must not be empty")
        );
    }

    #[test]
    fn host_must_not_repeat_the_scheme() {
        let errors = errors_for(json!({ "host": "kafka://broker:9092", "protocol": "kafka" }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("must not include a scheme")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn pathname_must_be_rooted() {
        let errors = errors_for(json!({ "host": "h", "protocol": "ws", "pathname": "ws" }));
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.pathname: must start with `/`")
        );

        // An empty pathname is accepted (it addresses the host root).
        assert!(errors_for(json!({ "host": "h", "protocol": "ws", "pathname": "" })).is_empty());
    }

    #[test]
    fn undeclared_host_and_path_placeholders_are_reported() {
        let errors = errors_for(json!({
            "host": "{stage}.example.com:{port}",
            "protocol": "kafka",
            "pathname": "/{tenant}",
            "variables": { "port": { "default": "9092" } }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`{stage}` is not declared"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("`{tenant}` is not declared"))
        );
        assert!(!errors.iter().any(|e| e.contains("`{port}`")));
    }

    #[test]
    fn placeholders_handles_edge_cases() {
        assert_eq!(placeholders("{a}.b.{c}"), vec!["a", "c"]);
        assert_eq!(placeholders("no-placeholders"), Vec::<&str>::new());
        assert_eq!(placeholders("{}"), Vec::<&str>::new());
        // An unterminated brace stops the scan rather than looping.
        assert_eq!(placeholders("{a}.{unclosed"), vec!["a"]);
    }

    #[test]
    fn nested_objects_report_under_their_own_path() {
        let errors = errors_for(json!({
            "host": "h",
            "protocol": "p",
            "variables": { "v": { "enum": ["a"], "default": "b" } },
            "security": [ { "type": "http" } ],
            "tags": [ { "name": "" } ],
            "externalDocs": { "url": "" },
            "bindings": { "kafka": "not-an-object" }
        }));
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.servers.prod.variables.v.default"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.servers.prod.security[0].scheme"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.tags[0].name: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e == "#.servers.prod.externalDocs.url: must not be empty")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("#.servers.prod.bindings.kafka"))
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
