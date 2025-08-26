//! The root document object of the OpenAPI v3.1.X specification.
//!
//! https://spec.openapis.org/oas/v3.1.0

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::common::reference::{RefOr, ResolveReference, resolve_in_map};
use crate::v3_1::callback::Callback;
use crate::v3_1::components::Components;
use crate::v3_1::example::Example;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::header::Header;
use crate::v3_1::info::Info;
use crate::v3_1::link::Link;
use crate::v3_1::parameter::Parameter;
use crate::v3_1::path_item::PathItem;
use crate::v3_1::request_body::RequestBody;
use crate::v3_1::response::Response;
use crate::v3_1::schema::Schema;
use crate::v3_1::security_scheme::SecurityScheme;
use crate::v3_1::server::Server;
use crate::v3_1::tag::Tag;
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
/// openapi: "3.1.0"
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
    /// The value MUST be one of ["3.1.0"].
    pub openapi: Version,

    /// **Required** Provides metadata about the API.
    /// The metadata MAY be used by tooling as required.
    pub info: Info,

    /// The default value for the $schema keyword within Schema Objects contained within this OAS document.
    /// This MUST be in the form of a URI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema_dialect: Option<String>,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<BTreeMap<String, RefOr<PathItem>>>,

    /// The incoming webhooks that MAY be received as part of this API and
    /// that the API consumer MAY choose to implement.
    /// Closely related to the callbacks feature,
    /// this section describes requests initiated other than by an API call,
    /// for example by an out of band registration.
    /// The key name is a unique string to refer to each webhook,
    /// while the (optionally referenced) Path Item Object describes a request
    /// that may be initiated by the API provider and the expected responses.
    ///
    /// Specification example:
    ///
    /// ```yaml
    /// webhooks:
    ///   # Each webhook needs a name
    ///   newPet:
    ///     # This is a Path Item Object, the only difference is that the request is initiated by the API provider
    ///     post:
    ///       requestBody:
    ///         description: Information about a new pet in the system
    ///         content:
    ///           application/json:
    ///             schema:
    ///               $ref: "#/components/schemas/Pet"
    ///       responses:
    ///         "200":
    ///           description: Return a 200 status to indicate that the data was received successfully
    /// ```
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhooks: Option<BTreeMap<String, RefOr<PathItem>>>,

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
    /// `3.1.0` version
    #[default]
    #[serde(rename = "3.1.0")]
    V3_1_0,
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::V3_1_0 => write!(f, "3.1.0"),
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

impl ResolveReference<PathItem> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&PathItem> {
        self.components
            .as_ref()
            .and_then(|x| resolve_in_map(self, reference, "#/components/pathItems/", &x.path_items))
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
        if let Some(paths) = &self.paths {
            for (name, r) in paths.iter() {
                let item = match r.get_item(ctx.spec) {
                    Ok(i) => i,
                    Err(e) => {
                        ctx.error("#".to_owned(), format_args!(".paths[{name}]: `{e}`"));
                        continue;
                    }
                };
                if let Some(operations) = &item.operations {
                    for (method, operation) in operations.iter() {
                        if let Some(operation_id) = &operation.operation_id
                            && !ctx
                                .visited
                                .insert(format!("#/paths/operations/{operation_id}"))
                        {
                            ctx.error(
                                    "#".to_owned(),
                                    format_args!(
                                        ".paths[{name}].{method}.operationId: `{operation_id}` already in use"
                                    ),
                                );
                        }
                    }
                }
            }

            for (name, item) in paths.iter() {
                let path = format!("#.paths[{name}]");
                if !name.starts_with('/') {
                    ctx.error(path.clone(), "must start with `/`");
                }
                item.validate_with_context(&mut ctx, path);
            }
        }

        if let Some(webhooks) = &self.webhooks {
            for (name, item) in webhooks.iter() {
                let path = format!("#.webhooks[{name}]");
                item.validate_with_context(&mut ctx, path);
            }
        }

        if let Some(components) = &self.components {
            components.validate_with_context(&mut ctx, "{}.components".to_owned());
        }

        if self.components.is_none() && self.paths.is_none() && self.webhooks.is_none() {
            ctx.error(
                "#".into(),
                "at least one of `paths`, `webhooks` or `components` must be used",
            );
        }

        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(&mut ctx, "#.externalDocs".to_owned())
        }

        if let Some(tags) = &self.tags {
            for tag in tags.iter() {
                let path = format!("#/tags/{}", tag.name);
                if ctx.visit(path.clone()) {
                    if !ctx.is_option(Options::IgnoreUnusedTags) {
                        ctx.error(path.clone(), "unused");
                    }
                    tag.validate_with_context(&mut ctx, path)
                }
            }
        }

        ctx.into()
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_swagger_version_deserialize() {
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
//                 swagger: Version::V2_0,
//                 info: Info {
//                     title: String::from("foo"),
//                     version: String::from("1"),
//                     ..Default::default()
//                 },
//                 ..Default::default()
//             },
//             "correct swagger version",
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "",
//                 "info": {
//                     "title": "foo",
//                     "version": "1",
//                 },
//                 "paths": {},
//             }))
//             .unwrap_err()
//             .to_string(),
//             "unknown variant ``, expected `2.0`",
//             "empty swagger version",
//         );
//         assert_eq!(
//             serde_json::from_value::<Spec>(serde_json::json!({
//                 "swagger": "foo",
//                 "info": {
//                     "title": "foo",
//                     "version":"1",
//                 }
//             }))
//             .unwrap_err()
//             .to_string(),
//             "unknown variant `foo`, expected `2.0`",
//             "foo as swagger version",
//         );
//     }
//
//     #[test]
//     fn test_swagger_version_serialize() {
//         #[derive(Deserialize)]
//         struct TestVersion {
//             pub swagger: String,
//         }
//         assert_eq!(
//             serde_json::from_str::<TestVersion>(
//                 serde_json::to_string(&Spec {
//                     swagger: Version::V2_0,
//                     info: Info {
//                         title: String::from("foo"),
//                         version: String::from("1"),
//                         ..Default::default()
//                     },
//                     ..Default::default()
//                 })
//                 .unwrap()
//                 .as_str(),
//             )
//             .unwrap()
//             .swagger,
//             "2.0",
//         );
//         assert_eq!(
//             serde_json::from_str::<TestVersion>(
//                 serde_json::to_string(&Spec {
//                     info: Info {
//                         title: String::from("foo"),
//                         version: String::from("1"),
//                         ..Default::default()
//                     },
//                     ..Default::default()
//                 })
//                 .unwrap()
//                 .as_str(),
//             )
//             .unwrap()
//             .swagger,
//             "2.0",
//         );
//     }
//
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
