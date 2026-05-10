//! The root document object of the OpenAPI v3.0.X specification.

use crate::common::helpers::{
    Context, InvalidComponentName, PushError, ValidateWithContext, check_component_name,
    validate_not_visited, validate_required_string,
};
use crate::common::reference::ResolveReference;
use crate::v3_0::callback::Callback;
use crate::v3_0::components::Components;
use crate::v3_0::example::Example;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::header::Header;
use crate::v3_0::info::Info;
use crate::v3_0::link::Link;
use crate::v3_0::parameter::Parameter;
use crate::v3_0::path_item::Paths;
use crate::v3_0::reference::{RefOr, resolve_in_map};
use crate::v3_0::request_body::RequestBody;
use crate::v3_0::response::Response;
use crate::v3_0::schema::Schema;
use crate::v3_0::security_scheme::SecurityScheme;
use crate::v3_0::server::Server;
use crate::v3_0::tag::Tag;
use crate::v3_0::validation::{
    validate_path_item, validate_path_template_uniqueness, validate_security_requirements,
    validate_tag_uniqueness,
};
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
    /// The value MUST match the OAS 3.0 schema pattern
    /// `^3\.0\.\d+(-.+)?$`, i.e. any `3.0.x` patch release with an
    /// optional prerelease suffix.
    /// The bare `3.0` short alias is also accepted on the wire and
    /// normalised to `3.0.4`. See [`Version`] for constructors and the
    /// `FromStr` / `TryFrom<&str>` parsers used to build arbitrary
    /// schema-conformant values programmatically.
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
    pub paths: Paths,

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

    /// ReDoc/Redocly extension that groups tags in the side menu.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-tagGroups")]
    pub x_tag_groups: Option<Vec<TagGroup>>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// ReDoc/Redocly `x-tagGroups` extension entry.
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

/// The OpenAPI Specification version. Per the OAS 3.0 JSON Schema, the
/// `openapi` field matches `^3\.0\.\d+(-.+)?$` — any 3.0.x patch version,
/// optionally with a prerelease suffix. The bare `3.0` short alias is
/// accepted and normalised to `3.0.4`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self("3.0.4".to_owned())
    }
}

impl Version {
    /// Convenience constructor for the `3.0.0` value.
    #[allow(non_snake_case)]
    pub fn V3_0_0() -> Self {
        Self("3.0.0".to_owned())
    }
    /// Convenience constructor for the `3.0.1` value.
    #[allow(non_snake_case)]
    pub fn V3_0_1() -> Self {
        Self("3.0.1".to_owned())
    }
    /// Convenience constructor for the `3.0.2` value.
    #[allow(non_snake_case)]
    pub fn V3_0_2() -> Self {
        Self("3.0.2".to_owned())
    }
    /// Convenience constructor for the `3.0.3` value.
    #[allow(non_snake_case)]
    pub fn V3_0_3() -> Self {
        Self("3.0.3".to_owned())
    }
    /// Convenience constructor for the canonical `3.0.4` value.
    #[allow(non_snake_case)]
    pub fn V3_0_4() -> Self {
        Self("3.0.4".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        // Delegate to `TryFrom<String>` so the owned `s` from serde
        // moves straight into `Version(s)` on success (no `to_owned`
        // round-trip). On failure, the offending string travels back
        // through `InvalidVersion` and is borrowed once for the serde
        // error message.
        Version::try_from(String::deserialize(de)?).map_err(|InvalidVersion(s)| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&s),
                &"a version matching the OAS 3.0 schema pattern `^3\\.0\\.\\d+(-.+)?$`",
            )
        })
    }
}

/// Returned by `Version::from_str` / `TryFrom<&str>` when the input
/// does not match the OAS 3.0 schema pattern (see [`Version`]).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("version {0:?} must match the OAS 3.0 schema pattern `^3\\.0\\.\\d+(-.+)?$`")]
pub struct InvalidVersion(pub String);

impl Version {
    fn from_str_inner(s: &str) -> Result<Self, InvalidVersion> {
        if s == "3.0" {
            return Ok(Version("3.0.4".to_owned()));
        }
        let re = lazy_regex::regex!(r"^3\.0\.\d+(-.+)?$");
        if re.is_match(s) {
            Ok(Version(s.to_owned()))
        } else {
            Err(InvalidVersion(s.to_owned()))
        }
    }
}

impl std::str::FromStr for Version {
    type Err = InvalidVersion;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_inner(s)
    }
}

impl TryFrom<&str> for Version {
    type Error = InvalidVersion;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::from_str_inner(s)
    }
}

impl TryFrom<String> for Version {
    type Error = InvalidVersion;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        // Move the input directly rather than borrowing into
        // `from_str_inner` and reallocating. The `3.0` short alias
        // still needs a fresh `3.0.4` string; every other path
        // consumes `s` in place.
        if s == "3.0" {
            return Ok(Version("3.0.4".to_owned()));
        }
        let re = lazy_regex::regex!(r"^3\.0\.\d+(-.+)?$");
        if re.is_match(&s) {
            Ok(Version(s))
        } else {
            Err(InvalidVersion(s))
        }
    }
}

impl Spec {
    /// Insert a schema under `#/components/schemas/{name}` and return a `$ref` pointing to it.
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
        let reference = format!("#/components/schemas/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .schemas
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(schema.into()));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a response under `#/components/responses/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_response(
        &mut self,
        name: impl Into<String>,
        response: Response,
    ) -> Result<RefOr<Response>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/responses/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .responses
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(response));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a parameter under `#/components/parameters/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_parameter(
        &mut self,
        name: impl Into<String>,
        parameter: Parameter,
    ) -> Result<RefOr<Parameter>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/parameters/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .parameters
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(parameter));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert an example under `#/components/examples/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_example(
        &mut self,
        name: impl Into<String>,
        example: Example,
    ) -> Result<RefOr<Example>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/examples/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .examples
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(example));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a request body under `#/components/requestBodies/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_request_body(
        &mut self,
        name: impl Into<String>,
        request_body: RequestBody,
    ) -> Result<RefOr<RequestBody>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/requestBodies/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .request_bodies
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(request_body));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a header under `#/components/headers/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_header(
        &mut self,
        name: impl Into<String>,
        header: Header,
    ) -> Result<RefOr<Header>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/headers/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .headers
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(header));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a security scheme under `#/components/securitySchemes/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_security_scheme(
        &mut self,
        name: impl Into<String>,
        scheme: SecurityScheme,
    ) -> Result<RefOr<SecurityScheme>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/securitySchemes/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .security_schemes
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(scheme));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a link under `#/components/links/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_link(
        &mut self,
        name: impl Into<String>,
        link: Link,
    ) -> Result<RefOr<Link>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/links/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .links
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(link));
        Ok(RefOr::new_ref(reference))
    }

    /// Insert a callback under `#/components/callbacks/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_callback(
        &mut self,
        name: impl Into<String>,
        callback: Callback,
    ) -> Result<RefOr<Callback>, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/callbacks/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .callbacks
            .get_or_insert_with(Default::default)
            .insert(name, RefOr::new_item(callback));
        Ok(RefOr::new_ref(reference))
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

        // Top-level security: visit referenced schemes (so unused-detection
        // doesn't fault on schemes the API actually requires) and run the
        // scope-by-scheme-type checks.
        if let Some(sec) = &self.security {
            validate_security_requirements(&mut ctx, "#.security", sec);
        }

        // Equivalent-template detection: `/pets/{id}` and `/pets/{name}`
        // collapse to the same canonical shape.
        validate_path_template_uniqueness(&mut ctx, &self.paths.paths);

        for (name, item) in self.paths.iter() {
            let path = format!("#.paths[{name}]");
            if !name.starts_with('/') {
                ctx.error(path.clone(), "must start with `/`");
            }
            item.validate_with_context(&mut ctx, path.clone());
            validate_path_item(&mut ctx, name, &path, item);
        }

        if let Some(components) = &self.components {
            components.validate_with_context(&mut ctx, "#.components".to_owned());
        }

        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(&mut ctx, "#.externalDocs".to_owned())
        }

        if let Some(tag_groups) = &self.x_tag_groups {
            for (i, tag_group) in tag_groups.iter().enumerate() {
                tag_group.validate_with_context(&mut ctx, format!("#.x-tagGroups[{i}]"));
            }
        }

        if let Some(tags) = &self.tags {
            validate_tag_uniqueness(&mut ctx, tags);
            for tag in tags.iter() {
                let path = format!("#/tags/{}", tag.name);
                validate_not_visited(tag, &mut ctx, Options::IgnoreUnusedTags, path);
            }
        }

        ctx.into()
    }
}

impl ValidateWithContext<Spec> for TagGroup {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        for (i, tag) in self.tags.iter().enumerate() {
            validate_required_string(tag, ctx, format!("{path}.tags[{i}]"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_deserialize() {
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.0.0")).unwrap(),
            Version::V3_0_0(),
            "correct openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.0")).unwrap(),
            Version::V3_0_4(),
            "3.0 openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("foo"))
                .unwrap_err()
                .to_string(),
            "invalid value: string \"foo\", expected a version matching the OAS 3.0 schema pattern `^3\\.0\\.\\d+(-.+)?$`",
            "foo as openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.0.99")).unwrap(),
            Version("3.0.99".to_owned()),
            "future patch is accepted by the schema regex",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.0.0-rc1")).unwrap(),
            Version("3.0.0-rc1".to_owned()),
            "prerelease suffix is accepted by the schema regex",
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
            Version::V3_0_4(),
            "3.0.4 spec.openapi",
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
            Version::V3_0_4(),
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
            "invalid value: string \"\", expected a version matching the OAS 3.0 schema pattern `^3\\.0\\.\\d+(-.+)?$`",
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
            serde_json::to_string(&Version::V3_0_0()).unwrap(),
            r#""3.0.0""#,
        );
        assert_eq!(
            serde_json::to_string(&Version::default()).unwrap(),
            r#""3.0.4""#,
        );
    }

    #[test]
    fn test_version_parse_programmatically() {
        use std::str::FromStr;
        // FromStr accepts schema-conformant patch/prerelease values
        // without going through Serde.
        assert_eq!(
            Version::from_str("3.0.99").unwrap(),
            Version("3.0.99".to_owned())
        );
        assert_eq!(
            Version::from_str("3.0.0-rc1").unwrap(),
            Version("3.0.0-rc1".to_owned())
        );
        // Bare `3.0` short alias normalises to `3.0.4`.
        assert_eq!(Version::from_str("3.0").unwrap(), Version::V3_0_4());
        // TryFrom<&str> and TryFrom<String> agree.
        assert_eq!(
            <Version as TryFrom<&str>>::try_from("3.0.7").unwrap(),
            Version("3.0.7".to_owned())
        );
        assert_eq!(
            <Version as TryFrom<String>>::try_from("3.0.7".to_owned()).unwrap(),
            Version("3.0.7".to_owned())
        );
        // Garbage rejects with a typed error carrying the offending input.
        let err = Version::from_str("foo").unwrap_err();
        assert_eq!(err, InvalidVersion("foo".to_owned()));
        assert!(
            err.to_string().contains("3\\.0\\.\\d+(-.+)?$"),
            "error message echoes the schema regex: {err}"
        );
    }

    #[test]
    fn test_define_schema() {
        use crate::v3_0::schema::{SingleSchema, StringSchema};
        let mut spec = Spec::default();
        let pet_ref = spec
            .define_schema("Pet", SingleSchema::from(StringSchema::default()))
            .expect("valid name");

        match pet_ref {
            RefOr::Ref(r) => assert_eq!(r.reference, "#/components/schemas/Pet"),
            _ => panic!("expected Ref"),
        }
        assert!(spec.components.is_some());
        assert!(
            spec.components
                .as_ref()
                .unwrap()
                .schemas
                .as_ref()
                .unwrap()
                .contains_key("Pet")
        );
    }

    #[test]
    fn test_define_replaces_existing() {
        use crate::v3_0::schema::{SingleSchema, StringSchema};
        let mut spec = Spec::default();
        spec.define_schema("Pet", SingleSchema::from(StringSchema::default()))
            .unwrap();
        spec.define_schema(
            "Pet",
            SingleSchema::from(StringSchema {
                title: Some("Pet".into()),
                ..Default::default()
            }),
        )
        .unwrap();

        let schemas = spec.components.unwrap().schemas.unwrap();
        assert_eq!(schemas.len(), 1);
        let pet = schemas.get("Pet").unwrap();
        match pet {
            RefOr::Item(Schema::Single(s)) => match s.as_ref() {
                SingleSchema::String(s) => assert_eq!(s.title.as_deref(), Some("Pet")),
                _ => panic!("expected String schema"),
            },
            _ => panic!("expected inline schema"),
        }
    }

    #[test]
    fn test_define_rejects_invalid_name() {
        use crate::v3_0::schema::{SingleSchema, StringSchema};
        let mut spec = Spec::default();
        // Spaces, slashes, and other characters break `$ref` URI fragments;
        // surface the failure at the `define_*` site, not in a later `validate()`.
        let err = spec
            .define_schema("My Pet", SingleSchema::from(StringSchema::default()))
            .unwrap_err();
        assert_eq!(err.name, "My Pet");
        // No partial state must leak in on failure.
        assert!(
            spec.components.is_none(),
            "name validation must run before mutation"
        );
    }

    #[test]
    fn all_define_helpers_insert_and_return_ref() {
        use crate::v3_0::callback::Callback;
        use crate::v3_0::example::Example;
        use crate::v3_0::header::Header;
        use crate::v3_0::link::Link;
        use crate::v3_0::parameter::{InQuery, Parameter};
        use crate::v3_0::request_body::RequestBody;
        use crate::v3_0::response::Response;
        use crate::v3_0::security_scheme::{HttpScheme, HttpSecurityScheme, SecurityScheme};

        let mut spec = Spec::default();

        let r = spec.define_response("Ok", Response::default()).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/responses/Ok"));

        let r = spec
            .define_parameter(
                "Q",
                Parameter::Query(InQuery {
                    name: "q".into(),
                    description: None,
                    required: None,
                    deprecated: None,
                    allow_empty_value: None,
                    style: None,
                    explode: None,
                    allow_reserved: None,
                    schema: None,
                    example: None,
                    examples: None,
                    content: None,
                    extensions: None,
                }),
            )
            .unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/parameters/Q"));

        let r = spec.define_example("Ex", Example::default()).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/examples/Ex"));

        let r = spec
            .define_request_body("RB", RequestBody::default())
            .unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/requestBodies/RB"));

        let r = spec.define_header("H", Header::default()).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/headers/H"));

        let r = spec
            .define_security_scheme(
                "S",
                SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
                    scheme: HttpScheme::Basic,
                    bearer_format: None,
                    description: None,
                    extensions: None,
                })),
            )
            .unwrap();
        assert!(
            matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/securitySchemes/S")
        );

        let r = spec.define_link("L", Link::default()).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/links/L"));

        let r = spec.define_callback("CB", Callback::default()).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/callbacks/CB"));

        // All inserts ended up under the same Components object.
        let comp = spec.components.as_ref().unwrap();
        assert!(comp.responses.as_ref().unwrap().contains_key("Ok"));
        assert!(comp.parameters.as_ref().unwrap().contains_key("Q"));
        assert!(comp.examples.as_ref().unwrap().contains_key("Ex"));
        assert!(comp.request_bodies.as_ref().unwrap().contains_key("RB"));
        assert!(comp.headers.as_ref().unwrap().contains_key("H"));
        assert!(comp.security_schemes.as_ref().unwrap().contains_key("S"));
        assert!(comp.links.as_ref().unwrap().contains_key("L"));
        assert!(comp.callbacks.as_ref().unwrap().contains_key("CB"));
    }

    #[test]
    fn define_helpers_reject_invalid_names() {
        use crate::v3_0::callback::Callback;
        use crate::v3_0::example::Example;
        use crate::v3_0::header::Header;
        use crate::v3_0::link::Link;
        use crate::v3_0::request_body::RequestBody;
        use crate::v3_0::response::Response;
        use crate::v3_0::security_scheme::{HttpScheme, HttpSecurityScheme, SecurityScheme};

        let mut spec = Spec::default();
        // Use a name with an invalid character that the validator rejects.
        let bad = "x y";
        assert!(spec.define_response(bad, Response::default()).is_err());
        assert!(spec.define_example(bad, Example::default()).is_err());
        assert!(
            spec.define_request_body(bad, RequestBody::default())
                .is_err()
        );
        assert!(spec.define_header(bad, Header::default()).is_err());
        assert!(
            spec.define_security_scheme(
                bad,
                SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
                    scheme: HttpScheme::Basic,
                    bearer_format: None,
                    description: None,
                    extensions: None,
                })),
            )
            .is_err()
        );
        assert!(spec.define_link(bad, Link::default()).is_err());
        assert!(spec.define_callback(bad, Callback::default()).is_err());
        // None of the failures left state behind.
        assert!(spec.components.is_none());
    }

    #[test]
    fn resolve_reference_paths_for_each_component_kind() {
        use crate::v3_0::callback::Callback;
        use crate::v3_0::example::Example;
        use crate::v3_0::header::Header;
        use crate::v3_0::link::Link;
        use crate::v3_0::parameter::{InQuery, Parameter};
        use crate::v3_0::request_body::RequestBody;
        use crate::v3_0::response::Response;
        use crate::v3_0::schema::{SingleSchema, StringSchema};
        use crate::v3_0::security_scheme::{HttpScheme, HttpSecurityScheme, SecurityScheme};

        let mut spec = Spec::default();
        spec.define_schema("S", SingleSchema::from(StringSchema::default()))
            .unwrap();
        spec.define_response("R", Response::default()).unwrap();
        spec.define_parameter(
            "P",
            Parameter::Query(InQuery {
                name: "q".into(),
                description: None,
                required: None,
                deprecated: None,
                allow_empty_value: None,
                style: None,
                explode: None,
                allow_reserved: None,
                schema: None,
                example: None,
                examples: None,
                content: None,
                extensions: None,
            }),
        )
        .unwrap();
        spec.define_request_body("RB", RequestBody::default())
            .unwrap();
        spec.define_header("H", Header::default()).unwrap();
        spec.define_example("E", Example::default()).unwrap();
        spec.define_callback("CB", Callback::default()).unwrap();
        spec.define_link("L", Link::default()).unwrap();
        spec.define_security_scheme(
            "SS",
            SecurityScheme::HTTP(Box::new(HttpSecurityScheme {
                scheme: HttpScheme::Basic,
                bearer_format: None,
                description: None,
                extensions: None,
            })),
        )
        .unwrap();

        assert!(
            <Spec as ResolveReference<Schema>>::resolve_reference(&spec, "#/components/schemas/S")
                .is_some()
        );
        assert!(
            <Spec as ResolveReference<Response>>::resolve_reference(
                &spec,
                "#/components/responses/R"
            )
            .is_some()
        );
        assert!(
            <Spec as ResolveReference<Parameter>>::resolve_reference(
                &spec,
                "#/components/parameters/P"
            )
            .is_some()
        );
        assert!(
            <Spec as ResolveReference<RequestBody>>::resolve_reference(
                &spec,
                "#/components/requestBodies/RB"
            )
            .is_some()
        );
        assert!(
            <Spec as ResolveReference<Header>>::resolve_reference(&spec, "#/components/headers/H")
                .is_some()
        );
        assert!(
            <Spec as ResolveReference<Example>>::resolve_reference(
                &spec,
                "#/components/examples/E"
            )
            .is_some()
        );
        assert!(
            <Spec as ResolveReference<Callback>>::resolve_reference(
                &spec,
                "#/components/callbacks/CB"
            )
            .is_some()
        );
        assert!(
            <Spec as ResolveReference<Link>>::resolve_reference(&spec, "#/components/links/L")
                .is_some()
        );
        assert!(
            <Spec as ResolveReference<SecurityScheme>>::resolve_reference(
                &spec,
                "#/components/securitySchemes/SS"
            )
            .is_some()
        );

        // tags resolver finds tags by name.
        let spec = Spec {
            tags: Some(vec![Tag {
                name: "pets".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert!(<Spec as ResolveReference<Tag>>::resolve_reference(&spec, "#/tags/pets").is_some());
        assert!(
            <Spec as ResolveReference<Tag>>::resolve_reference(&spec, "#/tags/missing").is_none()
        );
    }

    #[test]
    fn component_reference_alias_chain_resolves() {
        use crate::v3_0::components::Components;
        use crate::v3_0::schema::{SingleSchema, StringSchema};

        let mut schemas = BTreeMap::new();
        schemas.insert(
            "AliasA".to_owned(),
            RefOr::new_ref("#/components/schemas/AliasB"),
        );
        schemas.insert(
            "AliasB".to_owned(),
            RefOr::new_ref("#/components/schemas/Target"),
        );
        schemas.insert(
            "Target".to_owned(),
            RefOr::new_item(SingleSchema::from(StringSchema::default()).into()),
        );
        let spec = Spec {
            components: Some(Components {
                schemas: Some(schemas),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert!(
            <Spec as ResolveReference<Schema>>::resolve_reference(
                &spec,
                "#/components/schemas/AliasA"
            )
            .is_some()
        );
    }

    #[test]
    fn component_reference_alias_cycle_does_not_recurse_forever() {
        use crate::common::helpers::Context;
        use crate::v3_0::components::Components;

        let mut schemas = BTreeMap::new();
        schemas.insert(
            "AliasA".to_owned(),
            RefOr::new_ref("#/components/schemas/AliasB"),
        );
        schemas.insert(
            "AliasB".to_owned(),
            RefOr::new_ref("#/components/schemas/AliasA"),
        );
        let spec = Spec {
            components: Some(Components {
                schemas: Some(schemas),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert!(
            <Spec as ResolveReference<Schema>>::resolve_reference(
                &spec,
                "#/components/schemas/AliasA"
            )
            .is_none()
        );

        let mut ctx = Context::new(&spec, Options::new());
        RefOr::<Schema>::new_ref("#/components/schemas/AliasA")
            .validate_with_context(&mut ctx, "#.schema".to_owned());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not found")),
            "cycle should be reported as an unresolved ref, not recurse: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn version_display_all_variants() {
        assert_eq!(Version::V3_0_0().to_string(), "3.0.0");
        assert_eq!(Version::V3_0_1().to_string(), "3.0.1");
        assert_eq!(Version::V3_0_2().to_string(), "3.0.2");
        assert_eq!(Version::V3_0_3().to_string(), "3.0.3");
        assert_eq!(Version::V3_0_4().to_string(), "3.0.4");
    }

    #[test]
    fn full_spec_validate_drives_path_template_uniqueness() {
        use crate::v3_0::path_item::Paths;
        use crate::v3_0::response::{Response, Responses};

        let mut paths = Paths::default();
        let make_op = || crate::v3_0::operation::Operation {
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut ops_a: BTreeMap<String, crate::v3_0::operation::Operation> = BTreeMap::new();
        ops_a.insert("get".to_owned(), make_op());
        let mut ops_b: BTreeMap<String, crate::v3_0::operation::Operation> = BTreeMap::new();
        ops_b.insert("get".to_owned(), make_op());
        paths.paths.insert(
            "/pets/{id}".into(),
            crate::v3_0::path_item::PathItem {
                operations: Some(ops_a),
                ..Default::default()
            },
        );
        paths.paths.insert(
            "/pets/{name}".into(),
            crate::v3_0::path_item::PathItem {
                operations: Some(ops_b),
                ..Default::default()
            },
        );
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            paths,
            ..Default::default()
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("collapse to the same shape")),
            "expected equivalent-template error: {:?}",
            err.errors
        );
    }

    #[test]
    fn x_tag_groups_round_trip_and_validate() {
        let value = serde_json::json!({
            "openapi": "3.0.4",
            "info": {
                "title": "Pets",
                "version": "1.0.0"
            },
            "paths": {},
            "x-tagGroups": [
                {
                    "name": "Animals",
                    "tags": ["pets"]
                }
            ]
        });
        let spec: Spec = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);

        let mut ctx = Context::new(&spec, Options::new());
        spec.x_tag_groups
            .as_ref()
            .unwrap()
            .first()
            .unwrap()
            .validate_with_context(&mut ctx, "#.x-tagGroups[0]".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&spec, Options::new());
        TagGroup::default().validate_with_context(&mut ctx, "#.x-tagGroups[0]".to_owned());
        assert!(
            ctx.errors
                .contains(&"#.x-tagGroups[0].name: must not be empty".to_owned()),
            "expected name error: {:?}",
            ctx.errors
        );
    }
}
