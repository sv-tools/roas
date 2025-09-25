//! The root document object of the OpenAPI v3.0.X specification.

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_not_visited};
use crate::common::reference::{ResolveReference, resolve_in_map};
use crate::v3_0::callback::Callback;
use crate::v3_0::components::Components;
use crate::v3_0::example::Example;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::header::Header;
use crate::v3_0::info::Info;
use crate::v3_0::link::Link;
use crate::v3_0::parameter::Parameter;
use crate::v3_0::path_item::PathItem;
use crate::v3_0::request_body::RequestBody;
use crate::v3_0::response::Response;
use crate::v3_0::schema::Schema;
use crate::v3_0::security_scheme::SecurityScheme;
use crate::v3_0::server::Server;
use crate::v3_0::tag::Tag;
use crate::validation::{Error, Options, Validate};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};

/// This is the root document object of the OpenAPI document.
///
/// Specification example:
///
/// ```yaml
/// openapi: "3.0.4"
/// info:
///   version: 1.0.0
///   title: Swagger Petstore
///   license:
///     name: MIT
/// servers:
///   - url: https://petstore.swagger.io/v1
/// paths:
///   /pets:
///     get:
///       summary: List all pets
///       operationId: listPets
///       tags:
///         - pets
///       parameters:
///         - name: limit
///           in: query
///           description: How many items to return at one time (max 100)
///           required: false
///           schema:
///             type: integer
///             maximum: 100
///             format: int32
///       responses:
///         '200':
///           description: A paged array of pets
///           headers:
///             x-next:
///               description: A link to the next page of responses
///               schema:
///                 type: string
///           content:
///             application/json:    
///               schema:
///                 $ref: "#/components/schemas/Pets"
///         default:
///           description: unexpected error
///           content:
///             application/json:
///               schema:
///                 $ref: "#/components/schemas/Error"
///     post:
///       summary: Create a pet
///       operationId: createPets
///       tags:
///         - pets
///       responses:
///         '201':
///           description: Null response
///         default:
///           description: unexpected error
///           content:
///             application/json:
///               schema:
///                 $ref: "#/components/schemas/Error"
///   /pets/{petId}:
///     get:
///       summary: Info for a specific pet
///       operationId: showPetById
///       tags:
///         - pets
///       parameters:
///         - name: petId
///           in: path
///           required: true
///           description: The id of the pet to retrieve
///           schema:
///             type: string
///       responses:
///         '200':
///           description: Expected response to a valid request
///           content:
///             application/json:
///               schema:
///                 $ref: "#/components/schemas/Pet"
///         default:
///           description: unexpected error
///           content:
///             application/json:
///               schema:
///                 $ref: "#/components/schemas/Error"
/// components:
///   schemas:
///     Pet:
///       type: object
///       required:
///         - id
///         - name
///       properties:
///         id:
///           type: integer
///           format: int64
///         name:
///           type: string
///         tag:
///           type: string
///     Pets:
///       type: array
///       maxItems: 100
///       items:
///         $ref: "#/components/schemas/Pet"
///     Error:
///       type: object
///       required:
///         - code
///         - message
///       properties:
///         code:
///           type: integer
///           format: int32
///         message:
///           type: string
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    /// **Required** This string MUST be the semantic version number of the OpenAPI Specification
    /// version that the OpenAPI document uses.
    /// The openapi field SHOULD be used by tooling specifications and clients to interpret
    /// the OpenAPI document.
    /// This is not related to the API info.version string.
    ///
    /// The value MUST be one of ["3.0.0", "3.0.1", "3.0.2", "3.0.3", "3.0.4"].
    pub openapi: Version,

    /// **Required** Provides metadata about the API.
    /// The metadata MAY be used by tooling as required.
    pub info: Info,

    /// An array of Server Objects, which provide connectivity information to a target server.
    /// If the servers property is not provided, or is an empty array,
    /// the default value would be a Server Object with a url value of /.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<Server>>,

    /// **Required** The available paths and operations for the API.
    ///
    /// A relative path to an individual endpoint.
    /// The field name MUST begin with a forward slash (`/`).
    /// The path is appended (no relative URL resolution) to the expanded URL
    /// from the `Server Object`’s `url` field in order to construct the full URL.
    /// Path templating is allowed.
    /// When matching URLs, concrete (non-templated) paths would be matched before
    /// their templated counterparts.
    /// Templated paths with the same hierarchy but different templated names MUST NOT exist
    /// as they are identical.
    /// In case of ambiguous matching, it’s up to the tooling to decide which one to use.
    ///
    /// Support of extensions is dropped for simplicity.
    ///
    /// Specification example:
    ///
    /// ```yaml
    /// /pets:
    ///   get:
    ///     description: Returns all pets from the system that the user has access to
    ///     responses:
    ///       '200':
    ///         description: A list of pets.
    ///         content:
    ///           application/json:
    ///             schema:
    ///               type: array
    ///               items:
    ///                 $ref: '#/components/schemas/pet'
    /// ```
    pub paths: BTreeMap<String, PathItem>,

    /// An element to hold various schemas for the specification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,

    /// A declaration of which security mechanisms can be used across the API.
    /// The list of values includes alternative security requirement objects that can be used.
    /// Only one of the security requirement objects need to be satisfied to authorize a request.
    /// Individual operations can override this definition.
    /// To make security optional, an empty security requirement (`{}`) can be included in the array.
    ///
    /// Support of extensions is dropped for simplicity.
    ///
    /// Specification example:
    ///
    /// ```yaml
    /// - {}
    /// - petstore_auth:
    ///   - write:pets
    ///   - read:pets
    /// ```
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<BTreeMap<String, Vec<String>>>>,

    /// A list of tags used by the specification with additional metadata.
    /// The order of the tags can be used to reflect on their order by the parsing tools.
    /// Not all tags that are used by the Operation Object must be declared.
    /// The tags that are not declared MAY be organized randomly or based on the tools’ logic.
    /// Each tag name in the list MUST be unique.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Tag>>,

    /// Additional external documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// The Swagger Specification version.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub enum Version {
    /// `3.0.0` version
    #[serde(rename = "3.0.0")]
    V3_0_0,

    /// `3.0.1` version
    #[serde(rename = "3.0.1")]
    V3_0_1,

    /// `3.0.2` version
    #[serde(rename = "3.0.2")]
    V3_0_2,

    /// `3.0.3` version
    #[serde(rename = "3.0.3")]
    V3_0_3,

    /// `3.0.4` version
    #[default]
    #[serde(rename = "3.0.4", alias = "3.0")]
    V3_0_4,
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::V3_0_0 => write!(f, "3.0.0"),
            Self::V3_0_1 => write!(f, "3.0.1"),
            Self::V3_0_2 => write!(f, "3.0.2"),
            Self::V3_0_3 => write!(f, "3.0.3"),
            Self::V3_0_4 => write!(f, "3.0.4"),
        }
    }
}

impl ResolveReference<Response> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Response> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/responses/", &x.responses))
    }
}

impl ResolveReference<Parameter> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Parameter> {
        self.components.as_ref().and_then(|x| {
            resolve_in_map(self, reference, "#/components/parameters/", &x.parameters)
        })
    }
}

impl ResolveReference<RequestBody> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&RequestBody> {
        self.components.as_ref().and_then(|x| {
            resolve_in_map(
                self,
                reference,
                "#/components/requestBodies/",
                &x.request_bodies,
            )
        })
    }
}

impl ResolveReference<Header> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Header> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/headers/", &x.headers))
    }
}

impl ResolveReference<Schema> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Schema> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/schemas/", &x.schemas))
    }
}

impl ResolveReference<Example> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Example> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/examples/", &x.examples))
    }
}

impl ResolveReference<Callback> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Callback> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/callbacks/", &x.callbacks))
    }
}

impl ResolveReference<Link> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Link> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/links/", &x.links))
    }
}

impl ResolveReference<SecurityScheme> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&SecurityScheme> {
        self.components.as_ref().and_then(|x| {
            resolve_in_map(
                self,
                reference,
                "#/components/securitySchemes/",
                &x.security_schemes,
            )
        })
    }
}

impl ResolveReference<Tag> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&Tag> {
        self.tags.as_ref().and_then(|x| {
            x.iter()
                .find(|x| x.name == reference.trim_start_matches("#/tags/"))
        })
    }
}

impl Validate for Spec {
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error> {
        let mut ctx = Context::new(self, options);

        self.info
            .validate_with_context(&mut ctx, "#.info".to_owned());

        if let Some(servers) = &self.servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(&mut ctx, format!("#.servers[{i}]"))
            }
        }

        // memorize all operation ids for all paths first, so we can check the links
        for (name, item) in self.paths.iter() {
            if let Some(operations) = &item.operations {
                for (method, operation) in operations.iter() {
                    if let Some(operation_id) = &operation.operation_id
                        && !ctx
                            .visited
                            .insert(format!("#/paths/operations/{operation_id}"))
                    {
                        ctx.error(
                            "#".to_owned(),
                            format!(
                                ".paths[{name}].{method}.operationId: `{operation_id}` already in use"
                            ),
                        );
                    }
                }
            }
        }

        for (name, item) in self.paths.iter() {
            let path = format!("#.paths[{name}]");
            if !name.starts_with('/') {
                ctx.error(path.clone(), "must start with `/`");
            }
            item.validate_with_context(&mut ctx, path);
        }

        if let Some(components) = &self.components {
            components.validate_with_context(&mut ctx, "{}.components".to_owned());
        }

        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(&mut ctx, "#.externalDocs".to_owned())
        }

        if let Some(tags) = &self.tags {
            for tag in tags.iter() {
                let path = format!("#/tags/{}", tag.name);
                validate_not_visited(tag, &mut ctx, Options::IgnoreUnusedTags, path);
            }
        }

        ctx.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_deserialize() {
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.0.0")).unwrap(),
            Version::V3_0_0,
            "correct openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.0")).unwrap(),
            Version::V3_0_4,
            "3.0 openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("foo"))
                .unwrap_err()
                .to_string(),
            "unknown variant `foo`, expected one of `3.0.0`, `3.0.1`, `3.0.2`, `3.0.3`, `3.0`, `3.0.4`",
            "foo as openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "3.0.4",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap()
            .openapi,
            Version::V3_0_4,
            "3.0.0 spec.openapi",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "3.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap()
            .openapi,
            Version::V3_0_4,
            "3.0 spec.openapi",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap_err()
            .to_string(),
            "unknown variant ``, expected one of `3.0.0`, `3.0.1`, `3.0.2`, `3.0.3`, `3.0`, `3.0.4`",
            "empty spec.openapi",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap_err()
            .to_string(),
            "missing field `openapi`",
            "missing spec.openapi",
        );
    }

    #[test]
    fn test_version_serialize() {
        assert_eq!(
            serde_json::to_string(&Version::V3_0_0).unwrap(),
            r#""3.0.0""#,
        );
        assert_eq!(
            serde_json::to_string(&Version::default()).unwrap(),
            r#""3.0.4""#,
        );
    }
}

//     #[test]
//     fn test_scheme_deserialize() {
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "2.0",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths": {},
//             }))
//             .unwrap(),
//             Spec {
//                 schemes: None,
//                 info: Info {
//                     title: String::from("foo"),
//                     version: String::from("1"),
//                     ..Default::default()
//                 },
//                 ..Default::default()
//             },
//             "no scheme",
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "2.0",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths":{},
//                 "schemes":null,
//             }))
//             .unwrap(),
//             Spec {
//                 schemes: None,
//                 info: Info {
//                     title: String::from("foo"),
//                     version: String::from("1"),
//                     ..Default::default()
//                 },
//                 ..Default::default()
//             },
//             "null scheme",
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "2.0",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths": {},
//                 "schemes": [],
//             }))
//             .unwrap(),
//             Spec {
//                 schemes: Some(vec![]),
//                 info: Info {
//                     title: String::from("foo"),
//                     version: String::from("1"),
//                     ..Default::default()
//                 },
//                 ..Default::default()
//             },
//             "empty schemes array",
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "2.0",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths": {},
//                 "schemes": ["http", "wss", "https", "ws"],
//             }))
//             .unwrap(),
//             Spec {
//                 schemes: Some(vec![Scheme::HTTP, Scheme::WSS, Scheme::HTTPS, Scheme::WS]),
//                 info: Info {
//                     title: String::from("foo"),
//                     version: String::from("1"),
//                     ..Default::default()
//                 },
//                 ..Default::default()
//             },
//             "correct schemes",
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "2.0",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths": {},
//                 "schemes": "foo",
//             }))
//             .unwrap_err()
//             .to_string(),
//             r#"invalid type: string "foo", expected a sequence"#,
//             "foo string as schemes"
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "2.0",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths": {},
//                 "schemes": ["foo"],
//             }))
//             .unwrap_err()
//             .to_string(),
//             r#"unknown variant `foo`, expected one of `http`, `https`, `ws`, `wss`"#,
//             "foo string as scheme",
//         );
//     }
// }
