//! The root document object of the OpenAPI v2.0 specification.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};

use enumset::EnumSet;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::common::helpers::{validate_optional_string_matches, Context, ValidateWithContext};
use crate::common::reference::ResolveReference;
use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::info::Info;
use crate::v2::parameter::Parameter;
use crate::v2::path_item::PathItem;
use crate::v2::response::Response;
use crate::v2::schema::{ObjectSchema, Schema};
use crate::v2::security_scheme::SecurityScheme;
use crate::v2::tag::Tag;
use crate::validation::{Error, Options, Validate};

/// This is the root document object for the API specification.
/// It combines what previously was the Resource Listing and API Declaration (version 1.2 and earlier) together into one document.
///
/// Specification example:
///
/// ```yaml
/// swagger: "2.0"
/// info:
///   version: "1.0.0"
///   title: "Swagger Petstore"
///   description: "A sample API that uses a petstore as an example to demonstrate features in the swagger-2.0 specification"
///   termsOfService: "https://swagger.io/terms/"
///   contact:
///     name: "Swagger API Team"
///   license:
///     name: "MIT"
/// host: "petstore.swagger.io"
/// basePath: "/api"
/// schemes:
///   - "https"
/// consumes:
///   - "application/json"
/// produces:
///   - "application/json"
/// paths:
///   /pets:
///     get:
///       description: "Returns all pets from the system that the user has access to"
///       produces:
///         - "application/json"
///       responses:
///         "200":
///           description: "A list of pets."
///           schema:
///             type: "array"
///             items:
///               $ref: "#/definitions/Pet"
/// definitions:
///   Pet:
///     type: "object"
///     required:
///       - "id"
///       - "name"
///     properties:
///       id:
///         type: "integer"
///         format: "int64"
///       name:
///         type: "string"
///       tag:
///         type: "string"
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    /// **Required** Specifies the Swagger Specification version being used.
    /// It can be used by the Swagger UI and other clients to interpret the API listing.
    /// The value MUST be "2.0".
    pub swagger: Version,

    /// **Required** Provides metadata about the API.
    /// The metadata can be used by the clients if needed.
    pub info: Info,

    /// The host (name or ip) serving the API.
    /// This MUST be the host only and does not include the scheme nor sub-paths.
    /// It MAY include a port.
    /// If the host is not included, the host serving the documentation is to be used (including the port).
    /// The `host` does not support path templating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// The base path on which the API is served, which is relative to the `host`.
    /// If it is not included, the API is served directly under the host.
    /// The value MUST start with a leading slash (`/`).
    /// The `basePath` does not support path templating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,

    /// The transfer protocol of the API.
    /// Values MUST be from the list: "http", "https", "ws", "wss".
    /// If the schemes is not included, the default scheme to be used is the one used to access the Swagger definition itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemes: Option<Vec<Scheme>>,

    /// A list of MIME types the APIs can consume.
    /// This is global to all APIs but can be overridden on specific API calls.
    /// Value MUST be valid [Mime Type](https://developer.mozilla.org/en-US/docs/Web/HTTP/Basics_of_HTTP/MIME_types).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumes: Option<Vec<String>>,

    /// A list of MIME types the APIs can produce.
    /// This is global to all APIs but can be overridden on specific API calls.
    /// Value MUST be [Mime Type](https://developer.mozilla.org/en-US/docs/Web/HTTP/Basics_of_HTTP/MIME_types).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub produces: Option<Vec<String>>,

    /// **Required** The available paths and operations for the API.
    /// Holds the relative paths to the individual endpoints.
    /// The field name MUST begin with a slash.
    /// The path is appended to the basePath in order to construct the full URL.
    /// [Path templating](https://swagger.io/specification/v2/#path-templating) is allowed.
    ///
    /// The extensions support is dropped for simplicity.
    pub paths: BTreeMap<String, PathItem>,

    /// An object to hold data types produced and consumed by operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definitions: Option<BTreeMap<String, Schema>>,

    /// An object to hold parameters that can be used across operations.
    /// This property does not define global parameters for all operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, Parameter>>,

    /// An object to hold responses that can be used across operations.
    /// This property does not define global responses for all operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responses: Option<BTreeMap<String, Response>>,

    /// Security scheme definitions that can be used across the specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "securityDefinitions")]
    pub security_definitions: Option<BTreeMap<String, SecurityScheme>>,

    /// A declaration of which security schemes are applied for the API as a whole.
    /// The list of values describes alternative security schemes that can be used
    /// (that is, there is a logical OR between the security requirements).
    /// Individual operations can override this definition.
    ///
    /// Example:
    ///
    /// ```yaml
    /// - oAuthSample:
    ///   - write_pets
    ///   - read_pets
    /// ```
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<BTreeMap<String, Vec<String>>>>,

    /// A list of tags used by the specification with additional metadata.
    /// The order of the tags can be used to reflect on their order by the parsing tools.
    /// Not all tags that are used by the Operation Object must be declared.
    /// The tags that are not declared may be organized randomly or based on the tools' logic.
    /// Each tag name in the list MUST be unique.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Tag>>,

    /// Additional external documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// The Swagger Specification version.
/// Supports only `2.0` version.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub enum Version {
    /// `2.0` version
    #[default]
    #[serde(rename = "2.0")]
    V2_0,
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::V2_0 => write!(f, "2.0"),
        }
    }
}

/// The possible values of the transfer protocol of the API
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub enum Scheme {
    /// `http` protocol
    #[serde(rename = "http")]
    HTTP,
    /// `https` protocol
    #[default]
    #[serde(rename = "https")]
    HTTPS,
    /// `ws` protocol (WebSocket)
    #[serde(rename = "ws")]
    WS,
    /// `wss` protocol (WebSocket Secure)
    #[serde(rename = "wss")]
    WSS,
}

impl Display for Scheme {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::HTTP => write!(f, "http"),
            Self::HTTPS => write!(f, "https"),
            Self::WS => write!(f, "ws"),
            Self::WSS => write!(f, "wss"),
        }
    }
}

impl ResolveReference<Schema> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Schema> {
        if let Some(definitions) = &self.definitions {
            definitions.get(reference.trim_start_matches("#/definitions/"))
        } else {
            None
        }
    }
}

impl ResolveReference<ObjectSchema> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&ObjectSchema> {
        if let Some(definitions) = &self.definitions {
            if let Some(Schema::Object(object_schema)) =
                definitions.get(reference.trim_start_matches("#/definitions/"))
            {
                Some(object_schema)
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl ResolveReference<Parameter> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Parameter> {
        if let Some(parameters) = &self.parameters {
            parameters.get(reference.trim_start_matches("#/parameters/"))
        } else {
            None
        }
    }
}

impl ResolveReference<Response> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Response> {
        if let Some(responses) = &self.responses {
            responses.get(reference.trim_start_matches("#/responses/"))
        } else {
            None
        }
    }
}

impl ResolveReference<SecurityScheme> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&SecurityScheme> {
        if let Some(definitions) = &self.security_definitions {
            definitions.get(reference.trim_start_matches("#/securityDefinitions/"))
        } else {
            None
        }
    }
}

impl ResolveReference<Tag> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Tag> {
        self.tags
            .iter()
            .flatten()
            .find(|tag| tag.name == reference.trim_start_matches("#/tags/"))
    }
}

impl Validate for Spec {
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error> {
        let mut ctx = Context::new(self, options);
        self.validate_with_context(&mut ctx, String::from("#"));
        ctx.errors
            .is_empty()
            .then_some(())
            .ok_or(Error { errors: ctx.errors })
    }
}

impl ValidateWithContext<Spec> for Spec {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        self.info
            .validate_with_context(ctx, format!("{}.info", path));

        let re = Regex::new(r"^[^{}/ :\\]+(?::\d+)?$").unwrap();
        validate_optional_string_matches(&self.host, &re, ctx, format!("{}.host", path));

        if let Some(base_path) = &self.base_path {
            if !base_path.starts_with('/') {
                ctx.errors.push(format!(
                    "{}.basePath: must start with `/`, found `{}`",
                    path, base_path
                ));
            }
        }

        // validate paths operations
        for (name, item) in self.paths.iter() {
            let path = format!("{}.paths[{}]", path, name);
            if !name.starts_with('/') {
                ctx.errors.push(format!("{}: must start with `/`", path));
            }
            item.validate_with_context(ctx, path);
        }

        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{}.externalDocs", path))
        }

        // validate unused components
        if !ctx.options.contains(Options::IgnoreUnusedTags) {
            if let Some(tags) = &self.tags {
                for tag in tags.iter() {
                    let path = format!("{}/tags/{}", path, tag.name);
                    if ctx.visited.insert(path.clone()) {
                        ctx.errors.push(format!("{}: unused", path));
                        tag.validate_with_context(ctx, path);
                    }
                }
            }
        }

        if !ctx.options.contains(Options::IgnoreUnusedSchemas) {
            if let Some(definitions) = &self.definitions {
                for (name, definition) in definitions.iter() {
                    let path = format!("{}/definitions/{}", path, name);
                    if ctx.visited.insert(path.clone()) {
                        ctx.errors.push(format!("{}: unused", path));
                        definition.validate_with_context(ctx, path);
                    }
                }
            }
        }

        if !ctx.options.contains(Options::IgnoreUnusedParameters) {
            if let Some(parameters) = &self.parameters {
                for (name, parameter) in parameters.iter() {
                    let path = format!("{}/parameters/{}", path, name);
                    if ctx.visited.insert(path.clone()) {
                        ctx.errors.push(format!("{}: unused", path));
                        parameter.validate_with_context(ctx, path);
                    }
                }
            }
        }

        if !ctx.options.contains(Options::IgnoreUnusedResponses) {
            if let Some(responses) = &self.responses {
                for (name, response) in responses.iter() {
                    let path = format!("{}/responses/{}", path, name);
                    if ctx.visited.insert(path.clone()) {
                        ctx.errors.push(format!("{}: unused", path));
                        response.validate_with_context(ctx, path);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swagger_version_deserialize() {
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap(),
            Spec {
                swagger: Version::V2_0,
                info: Info {
                    title: String::from("foo"),
                    version: String::from("1"),
                    ..Default::default()
                },
                ..Default::default()
            },
            "correct swagger version",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap_err()
            .to_string(),
            "unknown variant ``, expected `2.0`",
            "empty swagger version",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "foo",
                "info": {
                    "title": "foo",
                    "version":"1",
                }
            }))
            .unwrap_err()
            .to_string(),
            "unknown variant `foo`, expected `2.0`",
            "foo as swagger version",
        );
    }

    #[test]
    fn test_swagger_version_serialize() {
        #[derive(Deserialize)]
        struct TestVersion {
            pub swagger: String,
        }
        assert_eq!(
            serde_json::from_str::<TestVersion>(
                serde_json::to_string(&Spec {
                    swagger: Version::V2_0,
                    info: Info {
                        title: String::from("foo"),
                        version: String::from("1"),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap()
                .as_str(),
            )
            .unwrap()
            .swagger,
            "2.0",
        );
        assert_eq!(
            serde_json::from_str::<TestVersion>(
                serde_json::to_string(&Spec {
                    info: Info {
                        title: String::from("foo"),
                        version: String::from("1"),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap()
                .as_str(),
            )
            .unwrap()
            .swagger,
            "2.0",
        );
    }

    #[test]
    fn test_scheme_deserialize() {
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap(),
            Spec {
                schemes: None,
                info: Info {
                    title: String::from("foo"),
                    version: String::from("1"),
                    ..Default::default()
                },
                ..Default::default()
            },
            "no scheme",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths":{},
                "schemes":null,
            }))
            .unwrap(),
            Spec {
                schemes: None,
                info: Info {
                    title: String::from("foo"),
                    version: String::from("1"),
                    ..Default::default()
                },
                ..Default::default()
            },
            "null scheme",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
                "schemes": [],
            }))
            .unwrap(),
            Spec {
                schemes: Some(vec![]),
                info: Info {
                    title: String::from("foo"),
                    version: String::from("1"),
                    ..Default::default()
                },
                ..Default::default()
            },
            "empty schemes array",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
                "schemes": ["http", "wss", "https", "ws"],
            }))
            .unwrap(),
            Spec {
                schemes: Some(vec![Scheme::HTTP, Scheme::WSS, Scheme::HTTPS, Scheme::WS]),
                info: Info {
                    title: String::from("foo"),
                    version: String::from("1"),
                    ..Default::default()
                },
                ..Default::default()
            },
            "correct schemes",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
                "schemes": "foo",
            }))
            .unwrap_err()
            .to_string(),
            r#"invalid type: string "foo", expected a sequence"#,
            "foo string as schemes"
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "swagger": "2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
                "schemes": ["foo"],
            }))
            .unwrap_err()
            .to_string(),
            r#"unknown variant `foo`, expected one of `http`, `https`, `ws`, `wss`"#,
            "foo string as scheme",
        );
    }
}
