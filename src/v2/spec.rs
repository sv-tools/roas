//! The root document object of the OpenAPI v2.0 specification.

use crate::common::helpers::{
    Context, InvalidComponentName, PushError, ValidateWithContext, check_component_name,
    validate_not_visited, validate_optional_string_matches,
};
use crate::common::reference::ResolveReference;
use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::info::Info;
use crate::v2::parameter::Parameter;
use crate::v2::path_item::Paths;
use crate::v2::reference::RefOr;
use crate::v2::response::Response;
use crate::v2::schema::{ObjectSchema, Schema};
use crate::v2::security_scheme::SecurityScheme;
use crate::v2::tag::Tag;
use crate::validation::{Error, Options, Validate};
use enumset::EnumSet;
use lazy_regex::regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};

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
    /// Supports `^x-` Specification Extensions on the Paths Object itself.
    pub paths: Paths,

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

    /// ReDoc extension that backports OpenAPI 3 servers to Swagger 2.0.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-servers")]
    pub x_servers: Option<Vec<Server>>,

    /// ReDoc extension that groups tags in the side menu.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-tagGroups")]
    pub x_tag_groups: Option<Vec<TagGroup>>,

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

/// ReDoc `x-servers` extension entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Server {
    /// **Required** The server URL.
    pub url: String,

    /// A short description of the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Allows extensions on the server extension object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// ReDoc `x-tagGroups` extension entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct TagGroup {
    /// **Required** The display name for the tag group.
    pub name: String,

    /// **Required** The tags included in the group.
    pub tags: Vec<String>,

    /// Allows extensions on the tag group extension object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
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
        self.definitions
            .as_ref()
            .and_then(|x| x.get(reference.trim_start_matches("#/definitions/")))
    }
}

impl ResolveReference<ObjectSchema> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&ObjectSchema> {
        if let Schema::Object(schema) = self
            .definitions
            .as_ref()
            .and_then(|x| x.get(reference.trim_start_matches("#/definitions/")))?
        {
            Some(schema)
        } else {
            None
        }
    }
}

impl ResolveReference<Parameter> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Parameter> {
        self.parameters
            .as_ref()
            .and_then(|x| x.get(reference.trim_start_matches("#/parameters/")))
    }
}

impl Spec {
    /// Insert a schema under `#/definitions/{name}` and return a `$ref` pointing to it.
    /// Replaces any existing entry with the same name.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_schema(
        &mut self,
        name: impl Into<String>,
        schema: impl Into<Schema>,
    ) -> Result<RefOr<Schema>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/definitions/{name}");
        self.definitions
            .get_or_insert_with(Default::default)
            .insert(name, schema.into());
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a parameter under `#/parameters/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_parameter(
        &mut self,
        name: impl Into<String>,
        parameter: Parameter,
    ) -> Result<RefOr<Parameter>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/parameters/{name}");
        self.parameters
            .get_or_insert_with(Default::default)
            .insert(name, parameter);
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a response under `#/responses/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_response(
        &mut self,
        name: impl Into<String>,
        response: Response,
    ) -> Result<RefOr<Response>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/responses/{name}");
        self.responses
            .get_or_insert_with(Default::default)
            .insert(name, response);
        Ok(RefOr::new_ref(reference))
    }
}

impl ResolveReference<Response> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Response> {
        self.responses
            .as_ref()
            .and_then(|x| x.get(reference.trim_start_matches("#/responses/")))
    }
}

impl ResolveReference<SecurityScheme> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&SecurityScheme> {
        self.security_definitions
            .as_ref()
            .and_then(|x| x.get(reference.trim_start_matches("#/securityDefinitions/")))
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

        self.info
            .validate_with_context(&mut ctx, "#.info".to_owned());

        validate_optional_string_matches(
            &self.host,
            regex!(r"^[^{}/ :\\]+(?::\d+)?$"),
            &mut ctx,
            "#.host".to_owned(),
        );

        if let Some(base_path) = &self.base_path
            && !base_path.starts_with('/')
        {
            ctx.error(
                "#.basePath".to_owned(),
                format_args!("must start with `/`, found `{base_path}`"),
            );
        }

        // Validate paths operations and the new path-template / parameter rules.
        for (name, item) in self.paths.iter() {
            let path = format!("#.paths[{name}]");
            if !name.starts_with('/') {
                ctx.error(path.clone(), "must start with `/`");
            }
            item.validate_with_context(&mut ctx, path.clone());
            crate::v2::validation::validate_path_item(&mut ctx, name, &path, item);
        }

        // Validate Spec-level security requirements against security_definitions.
        if let Some(security) = &self.security {
            crate::v2::validation::validate_security_requirements(&mut ctx, "#.security", security);
        }

        // Walk security_definitions: each scheme runs its own validator so
        // missing required URLs etc. are reported even when no requirement
        // references the scheme.
        crate::v2::validation::validate_security_definitions(&mut ctx);

        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(&mut ctx, "#.externalDocs".to_owned())
        }

        if let Some(servers) = &self.x_servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(&mut ctx, format!("#.x-servers[{i}]"));
            }
        }

        if let Some(tag_groups) = &self.x_tag_groups {
            for (i, tag_group) in tag_groups.iter().enumerate() {
                tag_group.validate_with_context(&mut ctx, format!("#.x-tagGroups[{i}]"));
            }
        }

        if let Some(tags) = &self.tags {
            for tag in tags.iter() {
                let path = format!("#/tags/{}", tag.name);
                validate_not_visited(tag, &mut ctx, Options::IgnoreUnusedTags, path);
            }
        }

        if let Some(definitions) = &self.definitions {
            for (name, definition) in definitions.iter() {
                let path = format!("#/definitions/{name}");
                validate_not_visited(definition, &mut ctx, Options::IgnoreUnusedSchemas, path);
            }
        }

        if let Some(parameters) = &self.parameters {
            for (name, parameter) in parameters.iter() {
                let path = format!("#/parameters/{name}");
                validate_not_visited(parameter, &mut ctx, Options::IgnoreUnusedParameters, path);
            }
        }

        if let Some(responses) = &self.responses {
            for (name, response) in responses.iter() {
                let path = format!("#/responses/{name}");
                validate_not_visited(response, &mut ctx, Options::IgnoreUnusedResponses, path);
            }
        }

        ctx.into()
    }
}

impl ValidateWithContext<Spec> for Server {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        crate::common::helpers::validate_required_string(&self.url, ctx, format!("{path}.url"));
    }
}

impl ValidateWithContext<Spec> for TagGroup {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        crate::common::helpers::validate_required_string(&self.name, ctx, format!("{path}.name"));
        for (i, tag) in self.tags.iter().enumerate() {
            crate::common::helpers::validate_required_string(tag, ctx, format!("{path}.tags[{i}]"));
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
    fn test_common_doc_extensions_round_trip_and_validate() {
        let value = serde_json::json!({
            "swagger": "2.0",
            "info": {
                "title": "foo",
                "version": "1",
            },
            "paths": {},
            "x-servers": [
                {
                    "url": "https://api.example.com",
                    "description": "Production",
                    "x-extra": true,
                }
            ],
            "x-tagGroups": [
                {
                    "name": "Core",
                    "tags": ["pets"],
                    "x-extra": "group",
                }
            ],
        });
        let spec = serde_json::from_value::<Spec>(value.clone()).unwrap();
        assert_eq!(
            spec.x_servers,
            Some(vec![Server {
                url: "https://api.example.com".to_owned(),
                description: Some("Production".to_owned()),
                extensions: Some(BTreeMap::from_iter([(
                    "x-extra".to_owned(),
                    serde_json::json!(true)
                )])),
            }])
        );
        assert_eq!(
            spec.x_tag_groups,
            Some(vec![TagGroup {
                name: "Core".to_owned(),
                tags: vec!["pets".to_owned()],
                extensions: Some(BTreeMap::from_iter([(
                    "x-extra".to_owned(),
                    serde_json::json!("group")
                )])),
            }])
        );
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert!(spec.validate(Default::default()).is_ok());
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

    use crate::v2::operation::Operation;
    use crate::v2::parameter::{InQuery, Parameter, StringParameter};
    use crate::v2::path_item::PathItem;
    use crate::v2::response::{Response, Responses};
    use crate::v2::schema::{ObjectSchema, Schema, StringSchema};
    use crate::v2::security_scheme::{
        BasicSecurityScheme, OAuth2SecurityScheme, Scopes, SecurityScheme, SecuritySchemeOAuth2Flow,
    };

    fn happy_op() -> Operation {
        Operation {
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn spec_with_info() -> Spec {
        Spec {
            info: crate::v2::info::Info {
                title: "T".into(),
                version: "1".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn version_and_scheme_display() {
        assert_eq!(format!("{}", Version::V2_0), "2.0");
        for (s, expected) in [
            (Scheme::HTTP, "http"),
            (Scheme::HTTPS, "https"),
            (Scheme::WS, "ws"),
            (Scheme::WSS, "wss"),
        ] {
            assert_eq!(format!("{s}"), expected);
        }
    }

    #[test]
    fn happy_path_validate_no_errors() {
        let mut spec = spec_with_info();
        let mut ops = BTreeMap::new();
        ops.insert("get".into(), happy_op());
        let mut paths = BTreeMap::new();
        paths.insert(
            "/users".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        spec.paths = crate::v2::path_item::Paths {
            paths,
            extensions: None,
        };
        let res = spec.validate(Options::new());
        assert!(res.is_ok(), "errors: {:?}", res);
    }

    #[test]
    fn validate_path_must_start_with_slash() {
        let mut spec = spec_with_info();
        let mut paths = BTreeMap::new();
        paths.insert(
            "no-slash".to_owned(),
            PathItem {
                operations: Some({
                    let mut m = BTreeMap::new();
                    m.insert("get".to_owned(), happy_op());
                    m
                }),
                ..Default::default()
            },
        );
        spec.paths = crate::v2::path_item::Paths {
            paths,
            extensions: None,
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors.iter().any(|e| e.contains("must start with `/`")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn validate_base_path_must_start_with_slash() {
        let mut spec = spec_with_info();
        spec.base_path = Some("api".into());
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("basePath") && e.contains("must start with `/`")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn validate_security_with_definitions() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "basicAuth".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        let mut req = BTreeMap::new();
        req.insert("basicAuth".to_owned(), vec![]);
        let spec = Spec {
            security_definitions: Some(defs),
            security: Some(vec![req]),
            ..spec_with_info()
        };
        let res = spec.validate(Options::new());
        assert!(res.is_ok(), "errors: {:?}", res);
    }

    #[test]
    fn validate_undefined_security_scheme() {
        let mut req = BTreeMap::new();
        req.insert("foo".to_owned(), vec![]);
        let spec = Spec {
            security: Some(vec![req]),
            ..spec_with_info()
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("no securityDefinitions on the spec")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn validate_body_and_formdata_together_via_spec() {
        let mut spec = spec_with_info();
        let mut op = happy_op();
        op.parameters = Some(vec![
            RefOr::new_item(Parameter::Body(Box::new(crate::v2::parameter::InBody {
                name: "b".into(),
                description: None,
                required: None,
                schema: RefOr::new_item(Schema::from(StringSchema::default())),
                x_examples: None,
                extensions: None,
            }))),
            RefOr::new_item(Parameter::FormData(Box::new(
                crate::v2::parameter::InFormData::String(StringParameter {
                    name: "f".into(),
                    ..Default::default()
                }),
            ))),
        ]);
        let mut ops = BTreeMap::new();
        ops.insert("post".into(), op);
        let mut paths = BTreeMap::new();
        paths.insert(
            "/p".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        spec.paths = crate::v2::path_item::Paths {
            paths,
            extensions: None,
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("`body` and `formData`")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn validate_duplicate_param_via_spec() {
        let mut spec = spec_with_info();
        let mut op = happy_op();
        op.parameters = Some(vec![
            RefOr::new_item(Parameter::Query(Box::new(InQuery::String(
                StringParameter {
                    name: "q".into(),
                    ..Default::default()
                },
            )))),
            RefOr::new_item(Parameter::Query(Box::new(InQuery::String(
                StringParameter {
                    name: "q".into(),
                    ..Default::default()
                },
            )))),
        ]);
        let mut ops = BTreeMap::new();
        ops.insert("get".into(), op);
        let mut paths = BTreeMap::new();
        paths.insert(
            "/p".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        spec.paths = crate::v2::path_item::Paths {
            paths,
            extensions: None,
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors.iter().any(|e| e.contains("duplicate parameter")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn validate_missing_path_template_param_via_spec() {
        let mut spec = spec_with_info();
        let mut ops = BTreeMap::new();
        ops.insert("get".into(), happy_op());
        let mut paths = BTreeMap::new();
        paths.insert(
            "/users/{id}".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        spec.paths = crate::v2::path_item::Paths {
            paths,
            extensions: None,
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("path template variable `{id}`")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn validate_empty_responses_via_spec() {
        let mut spec = spec_with_info();
        let mut ops = BTreeMap::new();
        ops.insert(
            "get".into(),
            Operation {
                responses: Responses::default(),
                ..Default::default()
            },
        );
        let mut paths = BTreeMap::new();
        paths.insert(
            "/p".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        spec.paths = crate::v2::path_item::Paths {
            paths,
            extensions: None,
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("must declare at least one response")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn define_schema_parameter_response_helpers() {
        let mut spec = Spec::default();

        let r = spec
            .define_schema("Foo", Schema::from(StringSchema::default()))
            .unwrap();
        match r {
            RefOr::Ref(rr) => assert_eq!(rr.reference, "#/definitions/Foo"),
            _ => panic!(),
        }
        let p = Parameter::Query(Box::new(InQuery::String(StringParameter {
            name: "p".into(),
            ..Default::default()
        })));
        let r = spec.define_parameter("Bar", p).unwrap();
        match r {
            RefOr::Ref(rr) => assert_eq!(rr.reference, "#/parameters/Bar"),
            _ => panic!(),
        }
        let r = spec
            .define_response(
                "Baz",
                Response {
                    description: "x".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        match r {
            RefOr::Ref(rr) => assert_eq!(rr.reference, "#/responses/Baz"),
            _ => panic!(),
        }
    }

    #[test]
    fn define_helpers_invalid_component_name() {
        let mut spec = Spec::default();
        let err = spec
            .define_schema("bad name", Schema::from(StringSchema::default()))
            .unwrap_err();
        assert_eq!(err.name, "bad name");
        let err = spec
            .define_parameter(
                "bad name",
                Parameter::Query(Box::new(InQuery::String(StringParameter::default()))),
            )
            .unwrap_err();
        assert_eq!(err.name, "bad name");
        let err = spec
            .define_response(
                "bad name",
                Response {
                    description: "x".into(),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(err.name, "bad name");
    }

    #[test]
    fn paths_with_x_extensions_serde() {
        let raw = serde_json::json!({
            "swagger": "2.0",
            "info": {"title": "t", "version": "1"},
            "paths": {
                "/p": {},
                "x-extra": "v",
            },
        });
        let spec: Spec = serde_json::from_value(raw.clone()).unwrap();
        assert!(spec.paths.extensions.is_some());
        let v = serde_json::to_value(&spec).unwrap();
        // The shape may differ in field ordering, but we round-trip through a
        // re-parse to confirm equivalence.
        let spec2: Spec = serde_json::from_value(v).unwrap();
        assert_eq!(spec, spec2);
    }

    #[test]
    fn resolve_reference_schema_objectschema_parameter() {
        let mut spec = Spec::default();
        let _ = spec
            .define_schema(
                "Object",
                Schema::from(ObjectSchema {
                    title: Some("t".into()),
                    ..Default::default()
                }),
            )
            .unwrap();
        let _ = spec
            .define_schema("StringSchema", Schema::from(StringSchema::default()))
            .unwrap();
        let _ = spec
            .define_parameter(
                "P",
                Parameter::Query(Box::new(InQuery::String(StringParameter {
                    name: "n".into(),
                    ..Default::default()
                }))),
            )
            .unwrap();

        // ResolveReference<Schema>
        let s: Option<&Schema> =
            <Spec as ResolveReference<Schema>>::resolve_reference(&spec, "#/definitions/Object");
        assert!(s.is_some());
        let s: Option<&Schema> =
            <Spec as ResolveReference<Schema>>::resolve_reference(&spec, "#/definitions/Missing");
        assert!(s.is_none());

        // ResolveReference<ObjectSchema>
        let s: Option<&ObjectSchema> = <Spec as ResolveReference<ObjectSchema>>::resolve_reference(
            &spec,
            "#/definitions/Object",
        );
        assert!(s.is_some());
        // Hits a non-object definition: returns None.
        let s: Option<&ObjectSchema> = <Spec as ResolveReference<ObjectSchema>>::resolve_reference(
            &spec,
            "#/definitions/StringSchema",
        );
        assert!(s.is_none());
        let s: Option<&ObjectSchema> = <Spec as ResolveReference<ObjectSchema>>::resolve_reference(
            &spec,
            "#/definitions/Missing",
        );
        assert!(s.is_none());

        // ResolveReference<Parameter>
        let p: Option<&Parameter> =
            <Spec as ResolveReference<Parameter>>::resolve_reference(&spec, "#/parameters/P");
        assert!(p.is_some());
        let p: Option<&Parameter> =
            <Spec as ResolveReference<Parameter>>::resolve_reference(&spec, "#/parameters/Missing");
        assert!(p.is_none());

        // ResolveReference<Response>
        let _ = spec
            .define_response(
                "R",
                Response {
                    description: "x".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let r: Option<&Response> =
            <Spec as ResolveReference<Response>>::resolve_reference(&spec, "#/responses/R");
        assert!(r.is_some());
        let r: Option<&Response> =
            <Spec as ResolveReference<Response>>::resolve_reference(&spec, "#/responses/Missing");
        assert!(r.is_none());

        // ResolveReference<SecurityScheme>
        let mut defs = BTreeMap::new();
        defs.insert(
            "S".to_owned(),
            SecurityScheme::Basic(BasicSecurityScheme::default()),
        );
        spec.security_definitions = Some(defs);
        let s: Option<&SecurityScheme> =
            <Spec as ResolveReference<SecurityScheme>>::resolve_reference(
                &spec,
                "#/securityDefinitions/S",
            );
        assert!(s.is_some());
        let s: Option<&SecurityScheme> =
            <Spec as ResolveReference<SecurityScheme>>::resolve_reference(
                &spec,
                "#/securityDefinitions/Missing",
            );
        assert!(s.is_none());

        // ResolveReference<Tag>
        spec.tags = Some(vec![crate::v2::tag::Tag {
            name: "t1".into(),
            ..Default::default()
        }]);
        let t =
            <Spec as ResolveReference<crate::v2::tag::Tag>>::resolve_reference(&spec, "#/tags/t1");
        assert!(t.is_some());
        let t = <Spec as ResolveReference<crate::v2::tag::Tag>>::resolve_reference(
            &spec,
            "#/tags/missing",
        );
        assert!(t.is_none());
    }

    #[test]
    fn validate_oauth2_definitions_walked() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "o".to_owned(),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: SecuritySchemeOAuth2Flow::Implicit,
                authorization_url: None,
                token_url: None,
                scopes: Scopes::default(),
                description: None,
                extensions: None,
            }),
        );
        let spec = Spec {
            security_definitions: Some(defs),
            ..spec_with_info()
        };
        let err = spec.validate(Options::new()).unwrap_err();
        // Per-scheme validate fires on missing URLs / empty scopes.
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("must not be empty") || e.contains("must be present")),
            "errors: {:?}",
            err.errors
        );
    }
}
