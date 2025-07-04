//! Representing a Server.

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_required_string};
use crate::v3_0::spec::Spec;
use crate::validation::Options;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// An object representing a Server.
///
/// Specification example:
///
/// ```yaml
/// servers:
/// - url: https://{username}.gigantic-server.com:{port}/{basePath}
///   description: The production API server
///   variables:
///     username:
///       # note! no enum here means it is an open value
///       default: demo
///       description: this value is assigned by the service provider, in this example `gigantic-server.com`
///     port:
///       enum:
///         - '8443'
///         - '443'
///       default: '8443'
///     basePath:
///       # open meaning there is the opportunity to use special base paths as assigned by the provider, default is `v2`
///       default: v2
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Server {
    /// **Required** A URL to the target host.
    /// This URL supports Server Variables and MAY be relative,
    /// to indicate that the host location is relative to the location
    /// where the OpenAPI document is being served.
    /// Variable substitutions will be made when a variable is named in {brackets}.
    pub url: String,

    /// An optional string describing the host designated by the URL.
    /// [CommonMark](https://spec.commonmark.org)  syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A map between a variable name and its value.
    /// The value is used for substitution in the server's URL template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<BTreeMap<String, ServerVariable>>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// An object representing a Server Variable for server URL template substitution.
///
/// Specification example:
///
/// ```yaml
/// enum:
///   - '8443'
///   - '443'
/// default: '8443'
/// description: the port to serve HTTP traffic on
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ServerVariable {
    /// An enumeration of string values to be used if the substitution options are from a limited set.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "enum")]
    enum_values: Option<Vec<String>>,

    /// **Required** The default value to use for substitution,
    /// which SHALL be sent if an alternate value is not supplied.
    /// Note this behavior is different than the Schema Object’s treatment of default values,
    /// because in those cases parameter values are optional.
    /// If the enum is defined, the value SHOULD exist in the enum’s values.
    default: String,

    /// An optional description for the server variable.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    description: Option<String>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Server {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.url, ctx, format!("{path}.url"));
        let mut visited = HashSet::<String>::new();
        if let Some(variables) = &self.variables {
            for (name, variable) in variables {
                variable.validate_with_context(ctx, format!("{path}.variables[{name}]"));
                visited.insert(name.clone());
            }
        };
        let re = Regex::new(r"\{([a-zA-Z0-9.\-_]+)}").unwrap();
        for (_, [name]) in re.captures_iter(&self.url).map(|c| c.extract()) {
            if !visited.remove(name) {
                ctx.error(
                    path.clone(),
                    format_args!(".url: `{name}` is not defined in `variables`"),
                );
            }
        }
        if !ctx.is_option(Options::IgnoreUnusedServerVariables) {
            for name in visited {
                ctx.error(
                    path.clone(),
                    format_args!(".variables[{name}]: unused in `url`"),
                );
            }
        }
    }
}

impl ValidateWithContext<Spec> for ServerVariable {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.default, ctx, format!("{path}.default"));
        if let Some(enum_values) = &self.enum_values {
            if !enum_values.contains(&self.default) {
                ctx.error(
                    path,
                    format!(
                        ".default: `{}` must be in enum values: {:?}",
                        self.default, enum_values,
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Options;
    use enumset::EnumSet;

    #[test]
    fn test_server_variable_deserialize() {
        assert_eq!(
            serde_json::from_value::<ServerVariable>(serde_json::json!({
                "enum": [
                    "8443",
                    "443"
                ],
                "default": "8443",
                "description": "the port to serve HTTP traffic on"
            }))
            .unwrap(),
            ServerVariable {
                enum_values: Some(vec![String::from("8443"), String::from("443")]),
                default: String::from("8443"),
                description: Some(String::from("the port to serve HTTP traffic on")),
                ..Default::default()
            },
            "deserialize",
        );
    }

    #[test]
    fn test_server_variable_serialize() {
        assert_eq!(
            serde_json::to_value(ServerVariable {
                enum_values: Some(vec![String::from("8443"), String::from("443")]),
                default: String::from("8443"),
                description: Some(String::from("the port to serve HTTP traffic on")),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "enum": [
                    "8443",
                    "443"
                ],
                "default": "8443",
                "description": "the port to serve HTTP traffic on"
            }),
            "serialize",
        );
    }

    #[test]
    fn test_server_variable_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        ServerVariable {
            enum_values: Some(vec![String::from("8443"), String::from("443")]),
            default: String::from("8443"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("serverVariable"));
        assert_eq!(ctx.errors.len(), 0, "no errors: {:?}", ctx.errors);

        ServerVariable {
            enum_values: Some(vec![String::from("443")]),
            default: String::from("8443"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("serverVariable"));
        assert_eq!(ctx.errors.len(), 1, "one error: {:?}", ctx.errors);
        assert_eq!(
            ctx.errors[0],
            "serverVariable.default: `8443` must be in enum values: [\"443\"]",
        );
    }

    #[test]
    fn test_server_serialize() {
        assert_eq!(
            serde_json::to_value(Server {
                url: String::from("https://development.gigantic-server.com/v1"),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "url": "https://development.gigantic-server.com/v1",
            }),
            "serialize with url only",
        );

        assert_eq!(
            serde_json::to_value(Server {
                url: String::from("https://development.gigantic-server.com/v1"),
                description: Some(String::from("Development server")),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "url": "https://development.gigantic-server.com/v1",
                "description": "Development server",
            }),
            "serialize with url and description",
        );

        assert_eq!(
            serde_json::to_value(Server {
                url: String::from("https://{username}.gigantic-server.com:{port}/{basePath}"),
                description: Some(String::from("Development server")),
                variables: Some({
                    let mut vars = BTreeMap::<String, ServerVariable>::new();
                    vars.insert(
                        String::from("username"),
                        ServerVariable {
                            default: String::from("demo"),
                            description: Some(String::from(
                                "this value is assigned by the service provider, in this example `gigantic-server.com`"
                            )),
                            ..Default::default()
                        },
                    );
                    vars.insert(
                        String::from("port"),
                        ServerVariable {
                            enum_values: Some(vec![String::from("8443"), String::from("443")]),
                            default: String::from("8443"),
                            description: Some(String::from("the port to serve HTTP traffic on")),
                            ..Default::default()
                        },
                    );
                    vars.insert(
                        String::from("basePath"),
                        ServerVariable {
                            default: String::from("v2"),
                            description: Some(String::from(
                                "open meaning there is the opportunity to use special base paths as assigned by the provider, default is `v2`"
                            )),
                            ..Default::default()
                        },
                    );
                    vars
                }),
                ..Default::default()
            })
                .unwrap(),
            serde_json::json!({
                "url": "https://{username}.gigantic-server.com:{port}/{basePath}",
                "description": "Development server",
                "variables": {
                    "username": {
                        "default": "demo",
                        "description": "this value is assigned by the service provider, in this example `gigantic-server.com`"
                    },
                    "port": {
                        "enum": [
                            "8443",
                            "443"
                        ],
                        "default": "8443",
                        "description": "the port to serve HTTP traffic on"
                    },
                    "basePath": {
                        "default": "v2",
                        "description": "open meaning there is the opportunity to use special base paths as assigned by the provider, default is `v2`"
                    }
                }
            }),
            "serialize with url, description and variables",
        );
    }

    #[test]
    fn test_server_deserialize() {
        assert_eq!(
            serde_json::from_value::<Server>(serde_json::json!({
                "url": "https://development.gigantic-server.com/v1",
            }))
            .unwrap(),
            Server {
                url: String::from("https://development.gigantic-server.com/v1"),
                ..Default::default()
            },
            "deserialize with url only",
        );

        assert_eq!(
            serde_json::from_value::<Server>(serde_json::json!({
                "url": "https://development.gigantic-server.com/v1",
                "description": "Development server",
            }))
            .unwrap(),
            Server {
                url: String::from("https://development.gigantic-server.com/v1"),
                description: Some(String::from("Development server")),
                ..Default::default()
            },
            "deserialize with url and description",
        );
        assert_eq!(
            serde_json::from_value::<Server>(serde_json::json!({
                "url": "https://{username}.gigantic-server.com:{port}/{basePath}",
                "description": "Development server",
                "variables": {
                    "username": {
                        "default": "demo",
                        "description": "this value is assigned by the service provider, in this example `gigantic-server.com`"
                    },
                    "port": {
                        "enum": [
                            "8443",
                            "443"
                        ],
                        "default": "8443",
                        "description": "the port to serve HTTP traffic on"
                    },
                    "basePath": {
                        "default": "v2",
                        "description": "open meaning there is the opportunity to use special base paths as assigned by the provider, default is `v2`"
                    }
                }
            })).unwrap(),
            Server {
                url: String::from("https://{username}.gigantic-server.com:{port}/{basePath}"),
                description: Some(String::from("Development server")),
                variables: Some({
                    let mut vars = BTreeMap::<String, ServerVariable>::new();
                    vars.insert(
                        String::from("username"),
                        ServerVariable {
                            default: String::from("demo"),
                            description: Some(String::from(
                                "this value is assigned by the service provider, in this example `gigantic-server.com`"
                            )),
                            ..Default::default()
                        },
                    );
                    vars.insert(
                        String::from("port"),
                        ServerVariable {
                            enum_values: Some(vec![String::from("8443"), String::from("443")]),
                            default: String::from("8443"),
                            description: Some(String::from("the port to serve HTTP traffic on")),
                            ..Default::default()
                        },
                    );
                    vars.insert(
                        String::from("basePath"),
                        ServerVariable {
                            default: String::from("v2"),
                            description: Some(String::from(
                                "open meaning there is the opportunity to use special base paths as assigned by the provider, default is `v2`"
                            )),
                            ..Default::default()
                        },
                    );
                    vars
                }),
                ..Default::default()
            },
            "deserialize with url, description and variables",
        );
    }

    #[test]
    fn test_server_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        Server {
            url: String::from("https://development.gigantic-server.com/v1"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("server"));
        assert_eq!(ctx.errors.len(), 0, "no errors: {:?}", ctx.errors);

        Server {
            url: String::from("https://{username}.gigantic-server.com:{port}/{basePath}"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("server"));
        assert_eq!(ctx.errors.len(), 3, "3 errors: {:?}", ctx.errors);

        ctx = Context::new(&spec, Default::default());
        Server {
            url: String::from("https://{username}.gigantic-server.com:{port}/{basePath}"),
            variables: Some({
                let mut vars = BTreeMap::<String, ServerVariable>::new();
                vars.insert(
                    String::from("username"),
                    ServerVariable {
                        default: String::from("demo"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("port"),
                    ServerVariable {
                        default: String::from("8443"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("basePath"),
                    ServerVariable {
                        default: String::from("v2"),
                        ..Default::default()
                    },
                );
                vars
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("server"));
        assert_eq!(
            ctx.errors.len(),
            0,
            "all variables are defined: {:?}",
            ctx.errors
        );

        Server {
            url: String::from("https://{username}.gigantic-server.com:{port}/{basePath}"),
            variables: Some({
                let mut vars = BTreeMap::<String, ServerVariable>::new();
                vars.insert(
                    String::from("username"),
                    ServerVariable {
                        default: String::from("demo"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("port"),
                    ServerVariable {
                        default: String::from("8443"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("basePath"),
                    ServerVariable {
                        default: String::from("v2"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("foo"),
                    ServerVariable {
                        default: String::from("bar"),
                        ..Default::default()
                    },
                );
                vars
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("server"));
        assert_eq!(ctx.errors.len(), 1, "with used variable: {:?}", ctx.errors);

        ctx = Context::new(&spec, EnumSet::only(Options::IgnoreUnusedServerVariables));
        Server {
            url: String::from("https://{username}.gigantic-server.com:{port}/{basePath}"),
            variables: Some({
                let mut vars = BTreeMap::<String, ServerVariable>::new();
                vars.insert(
                    String::from("username"),
                    ServerVariable {
                        default: String::from("demo"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("port"),
                    ServerVariable {
                        default: String::from("8443"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("basePath"),
                    ServerVariable {
                        default: String::from("v2"),
                        ..Default::default()
                    },
                );
                vars.insert(
                    String::from("foo"),
                    ServerVariable {
                        default: String::from("bar"),
                        ..Default::default()
                    },
                );
                vars
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("server"));
        assert_eq!(
            ctx.errors.len(),
            0,
            "ignore used variable: {:?}",
            ctx.errors
        );
    }
}
