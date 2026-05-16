//! The root document object of the OpenAPI v3.1.X specification.
//!
//! https://spec.openapis.org/oas/v3.1.2

use crate::common::helpers::{validate_not_visited, validate_required_string};
use crate::common::reference::{RefOr, ResolveReference, resolve_in_map};
use crate::loader::Loader;
use crate::v3_1::callback::Callback;
use crate::v3_1::components::Components;
use crate::v3_1::example::Example;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::header::Header;
use crate::v3_1::info::Info;
use crate::v3_1::link::Link;
use crate::v3_1::operation::Operation;
use crate::v3_1::parameter::Parameter;
use crate::v3_1::path_item::{PathItem, Paths};
use crate::v3_1::request_body::RequestBody;
use crate::v3_1::response::Response;
use crate::v3_1::schema::Schema;
use crate::v3_1::security_scheme::SecurityScheme;
use crate::v3_1::server::Server;
use crate::v3_1::tag::Tag;
use crate::v3_1::validation::{
    validate_path_item, validate_path_template_uniqueness, validate_security_requirements,
    validate_tag_uniqueness,
};
use crate::validation::{
    Context, InvalidComponentName, PushError, ValidateWithContext, check_component_name,
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
    /// The value MUST be one of ["3.1.0", "3.1.1", "3.1.2"].
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

    /// The available paths and operations for the API.
    /// Optional in OAS 3.1 (was required in 3.0).
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Paths>,

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
    pub webhooks: Option<Paths>,

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

/// The OpenAPI Specification version. Per the OAS 3.1 JSON Schema, the
/// `openapi` field matches `^3\.1\.\d+(-.+)?$` — any 3.1.x patch version,
/// optionally with a prerelease suffix. The bare `3.1` short alias is
/// accepted and normalised to `3.1.2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self("3.1.2".to_owned())
    }
}

impl Version {
    /// Convenience constructor for the `3.1.0` value.
    #[allow(non_snake_case)]
    pub fn V3_1_0() -> Self {
        Self("3.1.0".to_owned())
    }
    /// Convenience constructor for the `3.1.1` value.
    #[allow(non_snake_case)]
    pub fn V3_1_1() -> Self {
        Self("3.1.1".to_owned())
    }
    /// Convenience constructor for the canonical `3.1.2` value.
    #[allow(non_snake_case)]
    pub fn V3_1_2() -> Self {
        Self("3.1.2".to_owned())
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

/// Human-readable description of the OAS 3.1 version pattern, shared
/// by serde's `expected` payload and `InvalidVersion`'s `Display`.
/// The literal regex itself lives in [`matches_oas_3_1_version`] —
/// keep both in sync if either ever changes.
const VERSION_SCHEMA_DESCRIPTION: &str =
    "a version matching the OAS 3.1 schema pattern `^3\\.1\\.\\d+(-.+)?$`";

/// Single source of truth for the regex check. `lazy_regex::regex!`
/// requires a string literal so we can't host the pattern in a `const`,
/// but every parsing path goes through this one function.
fn matches_oas_3_1_version(s: &str) -> bool {
    lazy_regex::regex!(r"^3\.1\.\d+(-.+)?$").is_match(s)
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
                &VERSION_SCHEMA_DESCRIPTION,
            )
        })
    }
}

/// Returned by `Version::from_str` / `TryFrom<&str>` when the input
/// does not match the OAS 3.1 schema pattern (see [`Version`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidVersion(pub String);

impl fmt::Display for InvalidVersion {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "version {:?} must be {VERSION_SCHEMA_DESCRIPTION}",
            self.0
        )
    }
}

impl std::error::Error for InvalidVersion {}

impl std::str::FromStr for Version {
    type Err = InvalidVersion;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "3.1" {
            return Ok(Version("3.1.2".to_owned()));
        }
        if matches_oas_3_1_version(s) {
            Ok(Version(s.to_owned()))
        } else {
            Err(InvalidVersion(s.to_owned()))
        }
    }
}

impl TryFrom<&str> for Version {
    type Error = InvalidVersion;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl TryFrom<String> for Version {
    type Error = InvalidVersion;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s == "3.1" {
            return Ok(Version("3.1.2".to_owned()));
        }
        if matches_oas_3_1_version(&s) {
            Ok(Version(s))
        } else {
            Err(InvalidVersion(s))
        }
    }
}

impl ValidateWithContext<Spec> for Version {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if !matches_oas_3_1_version(&self.0) {
            ctx.error(path, format_args!("must be {VERSION_SCHEMA_DESCRIPTION}"));
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

    /// Insert a path item under `#/components/pathItems/{name}` and return a `$ref` pointing to it.
    ///
    /// Returns an error if `name` does not match `^[a-zA-Z0-9.\-_]+$`.
    pub fn define_path_item(
        &mut self,
        name: impl Into<String>,
        path_item: PathItem,
    ) -> Result<PathItem, InvalidComponentName> {
        let name = name.into();
        check_component_name(&name)?;
        let reference = format!("#/components/pathItems/{name}");
        self.components
            .get_or_insert_with(Default::default)
            .path_items
            .get_or_insert_with(Default::default)
            .insert(name, path_item);
        // v3.1 containers (Paths / Webhooks / Callback / Components.pathItems)
        // hold bare `PathItem` values; the Reference form is a `PathItem`
        // whose `reference` field is set. Return that shape so callers can
        // drop the result directly into any of those maps without an extra
        // wrapping step.
        Ok(PathItem {
            reference: Some(reference),
            ..Default::default()
        })
    }

    /// Merge `other` into `self` in place. Incoming entries always win:
    ///
    /// * **Map-like sections** (`paths`, `webhooks`, every
    ///   `components.<bag>`, top-level Specification Extensions): incoming
    ///   entries replace base entries with the same key; new keys are
    ///   appended.
    /// * **`tags`** (and `x-tagGroups`): deduplicated by `name`; incoming
    ///   wins per name and new entries are appended.
    /// * **`servers` / `security`**: replaced wholesale when incoming is
    ///   non-empty; an absent or empty incoming list leaves the base
    ///   alone.
    /// * **`externalDocs`, `jsonSchemaDialect`**: replaced when incoming
    ///   is `Some`.
    /// * **`info` / `openapi`**: untouched — the base keeps its identity.
    ///
    /// `$ref`s are not rewritten. If a base component is replaced by an
    /// incoming one of the same name, every existing `$ref` to that name
    /// resolves to the incoming definition.
    pub fn merge(&mut self, other: Self) {
        use crate::common::merge::{
            merge_named_list, merge_optional, merge_optional_list, merge_optional_map,
        };

        merge_optional(&mut self.json_schema_dialect, other.json_schema_dialect);
        merge_optional_list(&mut self.servers, other.servers);

        match (&mut self.paths, other.paths) {
            (Some(base), Some(inc)) => base.merge(inc),
            (slot @ None, Some(inc)) => *slot = Some(inc),
            (_, None) => {}
        }
        match (&mut self.webhooks, other.webhooks) {
            (Some(base), Some(inc)) => base.merge(inc),
            (slot @ None, Some(inc)) => *slot = Some(inc),
            (_, None) => {}
        }
        match (&mut self.components, other.components) {
            (Some(base), Some(inc)) => base.merge(inc),
            (slot @ None, Some(inc)) => *slot = Some(inc),
            (_, None) => {}
        }

        merge_optional_list(&mut self.security, other.security);
        merge_named_list(&mut self.tags, other.tags, |t| t.name.as_str());
        merge_optional(&mut self.external_docs, other.external_docs);
        merge_named_list(&mut self.x_tag_groups, other.x_tag_groups, |g| {
            g.name.as_str()
        });
        merge_optional_map(&mut self.extensions, other.extensions);
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
        // `path_items` holds bare `PathItem` (not `RefOr<PathItem>`), so we
        // can't use `resolve_in_map` here; do a direct prefix-stripped lookup.
        let key = reference.strip_prefix("#/components/pathItems/")?;
        self.components
            .as_ref()
            .and_then(|c| c.path_items.as_ref())
            .and_then(|m| m.get(key))
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

/// Append every `&Operation` reachable from `item` (and recursively from
/// each Operation's `callbacks`) to `out`, tagging it with a display
/// `location`. `seen_cb` deduplicates `Callback` payloads so two refs to
/// the same components.callbacks entry don't double-count its operations.
fn walk_path_item_ops<'a>(
    item: &'a PathItem,
    location: String,
    spec: &'a Spec,
    out: &mut Vec<(&'a Operation, String)>,
    seen_cb: &mut std::collections::HashSet<*const Callback>,
) {
    let Some(operations) = &item.operations else {
        return;
    };
    for (method, op) in operations {
        let op_loc = format!("{location}.{method}");
        out.push((op, op_loc.clone()));
        if let Some(cbs) = &op.callbacks {
            for (cb_name, cb_ref) in cbs {
                if let Ok(cb) = cb_ref.get_item(spec)
                    && seen_cb.insert(cb as *const Callback)
                {
                    for (expr, pi) in &cb.paths {
                        walk_path_item_ops(
                            pi,
                            format!("{op_loc}.callbacks[{cb_name}][{expr}]"),
                            spec,
                            out,
                            seen_cb,
                        );
                    }
                }
            }
        }
    }
}

impl Spec {
    fn validate_inner<'a>(
        &'a self,
        options: EnumSet<Options>,
        loader: Option<&'a mut Loader>,
    ) -> Result<(), Error> {
        let mut ctx = match loader {
            Some(l) => Context::new(self, options).with_loader(l),
            None => Context::new(self, options),
        };

        self.openapi
            .validate_with_context(&mut ctx, "#.openapi".to_owned());
        self.info
            .validate_with_context(&mut ctx, "#.info".to_owned());

        // jsonSchemaDialect MUST be a URI per OAS 3.1 (default-value spec for
        // the `$schema` keyword in nested Schema Objects). Use the generic
        // URI validator (not the HTTP-only URL one) so non-HTTP dialect
        // identifiers like `urn:example:dialect` are accepted.
        crate::common::helpers::validate_optional_uri(
            &self.json_schema_dialect,
            &mut ctx,
            "#.jsonSchemaDialect".to_owned(),
        );

        if let Some(servers) = &self.servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(&mut ctx, format!("#.servers[{i}]"))
            }
        }

        // OAS 3.1.2: operationId MUST be unique across the whole document.
        // Gather upfront so Link.operationId/operationRef can resolve
        // targets in containers Components hasn't been recursed into yet.
        let mut found: Vec<(&Operation, String)> = Vec::new();
        let mut seen_cb: std::collections::HashSet<*const Callback> =
            std::collections::HashSet::new();
        if let Some(paths) = &self.paths {
            for (name, item) in paths.iter() {
                walk_path_item_ops(
                    item,
                    format!("paths[{name}]"),
                    self,
                    &mut found,
                    &mut seen_cb,
                );
            }
        }
        if let Some(webhooks) = &self.webhooks {
            for (name, item) in webhooks.iter() {
                walk_path_item_ops(
                    item,
                    format!("webhooks[{name}]"),
                    self,
                    &mut found,
                    &mut seen_cb,
                );
            }
        }
        if let Some(components) = &self.components {
            if let Some(map) = &components.path_items {
                for (name, item) in map.iter() {
                    walk_path_item_ops(
                        item,
                        format!("components.pathItems[{name}]"),
                        self,
                        &mut found,
                        &mut seen_cb,
                    );
                }
            }
            if let Some(cbs) = &components.callbacks {
                for (cb_name, cb_ref) in cbs {
                    if let Ok(cb) = cb_ref.get_item(self)
                        && seen_cb.insert(cb as *const Callback)
                    {
                        for (expr, pi) in &cb.paths {
                            walk_path_item_ops(
                                pi,
                                format!("components.callbacks[{cb_name}][{expr}]"),
                                self,
                                &mut found,
                                &mut seen_cb,
                            );
                        }
                    }
                }
            }
        }
        for (op, location) in found {
            if let Some(operation_id) = &op.operation_id
                && !ctx
                    .visited
                    .insert(format!("#/paths/operations/{operation_id}"))
                && !ctx.is_option(Options::IgnoreNonUniqOperationIDs)
            {
                ctx.error(
                    "#".to_owned(),
                    format_args!(".{location}.operationId: `{operation_id}` already in use"),
                );
            }
        }

        // Top-level Spec.security: visit referenced schemes (so unused-detection
        // doesn't flag legitimately-required schemes) and run scope-by-scheme-type
        // checks. Per OAS 3.1, only `oauth2` scopes are resolved against the
        // scheme's flows; the other types accept free-form role-name arrays.
        if let Some(sec) = &self.security {
            validate_security_requirements(&mut ctx, "#.security", sec);
        }

        if let Some(paths) = &self.paths {
            // Equivalent-template detection per OAS spec: `/pets/{id}` and
            // `/pets/{name}` collapse to the same canonical shape.
            validate_path_template_uniqueness(&mut ctx, "#.paths", &paths.paths);

            for (name, item) in paths.iter() {
                let path = format!("#.paths[{name}]");
                if !name.starts_with('/') {
                    ctx.error(path.clone(), "must start with `/`");
                }
                item.validate_with_context(&mut ctx, path.clone());
                validate_path_item(&mut ctx, name, &path, item);
            }
        }

        if let Some(webhooks) = &self.webhooks {
            // Webhook keys are arbitrary identifiers per OAS 3.1.2, not URL
            // templates — path-template equivalence does not apply.
            for (name, item) in webhooks.iter() {
                let path = format!("#.webhooks[{name}]");
                item.validate_with_context(&mut ctx, path);
            }
        }

        if let Some(components) = &self.components {
            components.validate_with_context(&mut ctx, "#.components".to_owned());
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

impl Validate for Spec {
    fn validate(
        &self,
        options: EnumSet<Options>,
        loader: Option<&mut Loader>,
    ) -> Result<(), Error> {
        self.validate_inner(options, loader)
    }
}

impl ValidateWithContext<Spec> for TagGroup {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        if self.tags.is_empty() {
            ctx.error(format!("{path}.tags"), "must contain at least one tag");
        }
        for (i, tag) in self.tags.iter().enumerate() {
            validate_required_string(tag, ctx, format!("{path}.tags[{i}]"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::IGNORE_UNUSED;

    #[test]
    fn validate_with_loader_resolves_external_schema_ref() {
        let spec: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.1.0",
            "info": { "title": "test", "version": "1.0" },
            "paths": {},
            "components": {
                "schemas": {
                    "PetRef": { "$ref": "external.json#/Pet" }
                }
            }
        }))
        .expect("spec must parse");

        let err = spec
            .validate(IGNORE_UNUSED, None)
            .expect_err("external ref must error when no loader is attached");
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("external.json#/Pet") && e.contains("not supported")),
            "expected `not supported` error, got: {:?}",
            err.errors,
        );

        let mut loader = Loader::new();
        loader
            .preload_resource(
                "external.json",
                serde_json::json!({
                    "Pet": { "type": "object", "properties": {} }
                }),
            )
            .expect("preload must succeed");
        spec.validate(IGNORE_UNUSED, Some(&mut loader))
            .expect("validation must succeed when external ref is preloaded");

        let mut empty_loader = Loader::new();
        let err = spec
            .validate(IGNORE_UNUSED, Some(&mut empty_loader))
            .expect_err("missing fetcher must surface as a validation error");
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("external.json#/Pet") && e.contains("failed to resolve")),
            "expected `failed to resolve` error, got: {:?}",
            err.errors,
        );
    }

    #[test]
    fn test_version_deserialize() {
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.1.0")).unwrap(),
            Version::V3_1_0(),
            "correct openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.1")).unwrap(),
            Version::V3_1_2(),
            "alias to latest `3.1.X` version",
        );
        assert!(
            serde_json::from_value::<Version>(serde_json::json!("foo"))
                .unwrap_err()
                .to_string()
                .contains("3.1 schema pattern"),
            "foo as openapi version",
        );

        // Patch versions and prerelease suffixes per the schema pattern.
        for ok in ["3.1.0", "3.1.5", "3.1.42", "3.1.0-rc1", "3.1.7-beta.3"] {
            let v: Version = serde_json::from_value(serde_json::json!(ok)).expect("must accept");
            assert_eq!(v.as_str(), ok, "round-trip `{ok}`");
        }
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "3.1.2",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap()
            .openapi,
            Version::V3_1_2(),
            "3.1.2 spec.openapi",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "3.1",
                "info": {"title": "foo", "version": "1"},
                "paths": {},
            }))
            .unwrap()
            .openapi,
            Version::V3_1_2(),
            "`3.1` short alias is accepted at the Spec level and normalises to `3.1.2`",
        );
        assert!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap_err()
            .to_string()
            .contains("3.1 schema pattern"),
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
            serde_json::to_string(&Version::V3_1_0()).unwrap(),
            r#""3.1.0""#,
        );
        assert_eq!(
            serde_json::to_string(&Version::default()).unwrap(),
            r#""3.1.2""#,
        );
    }

    #[test]
    fn test_version_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Version::default().validate_with_context(&mut ctx, "#.openapi".to_owned());
        Version::V3_1_0().validate_with_context(&mut ctx, "#.openapi".to_owned());
        Version::V3_1_2().validate_with_context(&mut ctx, "#.openapi".to_owned());
        "3.1.99"
            .parse::<Version>()
            .unwrap()
            .validate_with_context(&mut ctx, "#.openapi".to_owned());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn test_version_validate_rejects_invalid() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Version("garbage".to_owned()).validate_with_context(&mut ctx, "#.openapi".to_owned());
        assert_eq!(ctx.errors.len(), 1);
        assert!(
            ctx.errors[0].contains("#.openapi") && ctx.errors[0].contains("3\\.1\\.\\d+(-.+)?$"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn test_spec_validate_surfaces_invalid_openapi() {
        let mut spec = Spec {
            openapi: Version("3.5.0".to_owned()),
            ..Default::default()
        };
        spec.info.title = "test".to_owned();
        spec.info.version = "1".to_owned();
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("#.openapi") && e.contains("3\\.1\\.\\d+(-.+)?$")),
            "Spec::validate surfaces the openapi error: {:?}",
            err.errors
        );
    }

    #[test]
    fn test_version_try_from_string_normalizes_short_alias() {
        let v: Version = "3.1".to_owned().try_into().unwrap();
        assert_eq!(v, Version::V3_1_2());
        let err: InvalidVersion = Version::try_from("nope".to_owned()).unwrap_err();
        assert_eq!(err.0, "nope");
    }

    #[test]
    fn test_version_parse_programmatically() {
        use std::str::FromStr;
        assert_eq!(
            Version::from_str("3.1.99").unwrap(),
            Version("3.1.99".to_owned())
        );
        assert_eq!(
            Version::from_str("3.1.0-rc1").unwrap(),
            Version("3.1.0-rc1".to_owned())
        );
        assert_eq!(Version::from_str("3.1").unwrap(), Version::V3_1_2());
        assert_eq!(
            <Version as TryFrom<&str>>::try_from("3.1.7").unwrap(),
            Version("3.1.7".to_owned())
        );
        assert_eq!(
            <Version as TryFrom<String>>::try_from("3.1.7".to_owned()).unwrap(),
            Version("3.1.7".to_owned())
        );
        let err = Version::from_str("foo").unwrap_err();
        assert_eq!(err, InvalidVersion("foo".to_owned()));
        assert!(
            err.to_string().contains("3\\.1\\.\\d+(-.+)?$"),
            "error message echoes the schema regex: {err}"
        );
    }

    #[test]
    fn full_spec_validate_drives_path_template_uniqueness() {
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::Paths;
        use crate::v3_1::response::Responses;

        let make_op = || Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut ops_a: BTreeMap<String, Operation> = BTreeMap::new();
        ops_a.insert("get".to_owned(), make_op());
        let mut ops_b: BTreeMap<String, Operation> = BTreeMap::new();
        ops_b.insert("get".to_owned(), make_op());
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets/{id}".into(),
            PathItem {
                operations: Some(ops_a),
                ..Default::default()
            },
        );
        paths.paths.insert(
            "/pets/{name}".into(),
            PathItem {
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
            paths: Some(paths),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("collapse to the same shape")),
            "expected equivalent-template error: {:?}",
            err.errors
        );
    }

    #[test]
    fn webhooks_validation_runs() {
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::Paths;
        use crate::v3_1::response::Responses;

        let mut ops: BTreeMap<String, Operation> = BTreeMap::new();
        ops.insert(
            "post".to_owned(),
            Operation {
                responses: Some(Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            description: "ok".into(),
                            ..Default::default()
                        }),
                    )])),
                    ..Default::default()
                }),
                security: Some(vec![{
                    let mut req = BTreeMap::new();
                    req.insert("missing-scheme".to_owned(), vec![]);
                    req
                }]),
                ..Default::default()
            },
        );
        let mut webhooks = Paths::default();
        webhooks.paths.insert(
            "newPet".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("missing-scheme") && e.contains("post.security")),
            "expected webhook-nested security validation: {:?}",
            err.errors
        );
    }

    #[test]
    fn operation_id_unique_across_paths_and_webhooks() {
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::Paths;
        use crate::v3_1::response::Responses;

        let make_op = |id: &str| Operation {
            operation_id: Some(id.to_owned()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut path_ops: BTreeMap<String, Operation> = BTreeMap::new();
        path_ops.insert("get".to_owned(), make_op("dup"));
        let mut webhook_ops: BTreeMap<String, Operation> = BTreeMap::new();
        webhook_ops.insert("post".to_owned(), make_op("dup"));

        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                operations: Some(path_ops),
                ..Default::default()
            },
        );
        let mut webhooks = Paths::default();
        webhooks.paths.insert(
            "petCreated".to_owned(),
            PathItem {
                operations: Some(webhook_ops),
                ..Default::default()
            },
        );

        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            paths: Some(paths),
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("`dup` already in use")),
            "expected operationId duplicate across paths/webhooks: {:?}",
            err.errors
        );

        // Same spec with `IgnoreNonUniqOperationIDs` set must not surface
        // the duplicate.
        let result = spec.validate(Options::IgnoreNonUniqOperationIDs.into(), None);
        let errors_with_ignore: Vec<String> = result
            .err()
            .map(|e| e.errors.iter().map(|e| e.to_string()).collect())
            .unwrap_or_default();
        assert!(
            errors_with_ignore
                .iter()
                .all(|s| !s.contains("already in use")),
            "IgnoreNonUniqOperationIDs must suppress the duplicate, got: {errors_with_ignore:?}",
        );
    }

    #[test]
    fn all_define_helpers_insert_and_return_ref() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::example::Example;
        use crate::v3_1::header::Header;
        use crate::v3_1::link::Link;
        use crate::v3_1::parameter::{InQuery, Parameter};
        use crate::v3_1::request_body::RequestBody;
        use crate::v3_1::response::Response;
        use crate::v3_1::schema::{SingleSchema, StringSchema};
        use crate::v3_1::security_scheme::{HttpSecurityScheme, SecurityScheme};

        let mut spec = Spec::default();

        let r = spec
            .define_schema(
                "S",
                Schema::Single(Box::new(SingleSchema::String(StringSchema::default()))),
            )
            .unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/schemas/S"));

        let r = spec.define_response("R", Response::default()).unwrap();
        assert!(matches!(r, RefOr::Ref(ref rr) if rr.reference == "#/components/responses/R"));

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
                    scheme: "Basic".into(),
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

        // define_path_item returns a `PathItem` whose `reference` is set to
        // the component-pathItems URL — not a `RefOr`, since v3.1 PathItem
        // containers hold bare `PathItem`.
        let pi = spec.define_path_item("PI", PathItem::default()).unwrap();
        assert_eq!(pi.reference.as_deref(), Some("#/components/pathItems/PI"),);

        // All inserts ended up under the same Components object.
        let comp = spec.components.as_ref().unwrap();
        assert!(comp.schemas.as_ref().unwrap().contains_key("S"));
        assert!(comp.responses.as_ref().unwrap().contains_key("R"));
        assert!(comp.parameters.as_ref().unwrap().contains_key("Q"));
        assert!(comp.examples.as_ref().unwrap().contains_key("Ex"));
        assert!(comp.request_bodies.as_ref().unwrap().contains_key("RB"));
        assert!(comp.headers.as_ref().unwrap().contains_key("H"));
        assert!(comp.security_schemes.as_ref().unwrap().contains_key("S"));
        assert!(comp.links.as_ref().unwrap().contains_key("L"));
        assert!(comp.callbacks.as_ref().unwrap().contains_key("CB"));
        assert!(comp.path_items.as_ref().unwrap().contains_key("PI"));
    }

    #[test]
    fn define_helpers_reject_invalid_names() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::example::Example;
        use crate::v3_1::header::Header;
        use crate::v3_1::link::Link;
        use crate::v3_1::request_body::RequestBody;
        use crate::v3_1::response::Response;
        use crate::v3_1::security_scheme::{HttpSecurityScheme, SecurityScheme};

        let mut spec = Spec::default();
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
                    scheme: "Basic".into(),
                    ..Default::default()
                })),
            )
            .is_err()
        );
        assert!(spec.define_link(bad, Link::default()).is_err());
        assert!(spec.define_callback(bad, Callback::default()).is_err());
        assert!(spec.define_path_item(bad, PathItem::default()).is_err());
        assert!(spec.components.is_none());
    }

    #[test]
    fn resolve_reference_paths_for_each_component_kind() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::example::Example;
        use crate::v3_1::header::Header;
        use crate::v3_1::link::Link;
        use crate::v3_1::parameter::{InQuery, Parameter};
        use crate::v3_1::request_body::RequestBody;
        use crate::v3_1::response::Response;
        use crate::v3_1::schema::{SingleSchema, StringSchema};
        use crate::v3_1::security_scheme::{HttpSecurityScheme, SecurityScheme};

        let mut spec = Spec::default();
        spec.define_schema(
            "S",
            Schema::Single(Box::new(SingleSchema::String(StringSchema::default()))),
        )
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
                scheme: "Basic".into(),
                ..Default::default()
            })),
        )
        .unwrap();
        spec.define_path_item("PI", PathItem::default()).unwrap();

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
        assert!(
            <Spec as ResolveReference<PathItem>>::resolve_reference(
                &spec,
                "#/components/pathItems/PI"
            )
            .is_some()
        );

        // Wrong-prefix returns None (strict strip_prefix behavior, not silently
        // mismatched lookup).
        assert!(
            <Spec as ResolveReference<Schema>>::resolve_reference(
                &spec,
                "#/components/parameters/S"
            )
            .is_none()
        );

        // Tags resolver finds tags by name.
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
    fn version_display_all_variants() {
        assert_eq!(Version::V3_1_0().to_string(), "3.1.0");
        assert_eq!(Version::V3_1_1().to_string(), "3.1.1");
        assert_eq!(Version::V3_1_2().to_string(), "3.1.2");
    }

    #[test]
    fn json_schema_dialect_uri_validated() {
        // Free-form URI is accepted (urn:..., relative path, etc.).
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            json_schema_dialect: Some("urn:example:dialect".into()),
            paths: Some(Default::default()),
            ..Default::default()
        };
        assert!(spec.validate(Options::new(), None).is_ok());

        // Whitespace in the value rejects.
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            json_schema_dialect: Some("not a uri".into()),
            paths: Some(Default::default()),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("jsonSchemaDialect") && e.contains("must be a valid URI")),
            "errors: {:?}",
            err.errors
        );

        // Present-but-empty (`Some("")`) is also invalid: the field was
        // set, so it must hold a URI.
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            json_schema_dialect: Some("".into()),
            paths: Some(Default::default()),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("jsonSchemaDialect") && e.contains("must be a valid URI")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn op_id_unique_across_paths_webhooks_components_pathitems() {
        // Pre-collection should detect a duplicate operationId across all
        // three containers.
        use crate::v3_1::components::Components;
        use crate::v3_1::operation::Operation;
        use crate::v3_1::response::Responses;

        let make_op = |id: &str| Operation {
            operation_id: Some(id.to_owned()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };

        // paths defines `dup`
        let mut path_ops: BTreeMap<String, Operation> = BTreeMap::new();
        path_ops.insert("get".to_owned(), make_op("dup"));
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                operations: Some(path_ops),
                ..Default::default()
            },
        );

        // components.pathItems defines another `dup`
        let mut pi_ops: BTreeMap<String, Operation> = BTreeMap::new();
        pi_ops.insert("get".to_owned(), make_op("dup"));
        let comp = Components {
            path_items: Some(BTreeMap::from([(
                "Reusable".to_owned(),
                PathItem {
                    operations: Some(pi_ops),
                    ..Default::default()
                },
            )])),
            ..Default::default()
        };

        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("`dup` already in use")),
            "expected duplicate-operationId across paths + components.pathItems: {:?}",
            err.errors
        );
    }

    #[test]
    fn link_resolves_op_id_defined_in_components_path_items() {
        // The forward-pass collection in Spec::validate must visit
        // operationIds from components.pathItems before path/webhook
        // validation runs, so a Link.operationId in Spec.paths can
        // reference an op defined only in components.pathItems.
        use crate::v3_1::components::Components;
        use crate::v3_1::operation::Operation;
        use crate::v3_1::response::Responses;

        let mut pi_ops: BTreeMap<String, Operation> = BTreeMap::new();
        pi_ops.insert(
            "get".to_owned(),
            Operation {
                operation_id: Some("pickPet".to_owned()),
                responses: Some(Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            description: "ok".into(),
                            ..Default::default()
                        }),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let comp = Components {
            path_items: Some(BTreeMap::from([(
                "Reusable".to_owned(),
                PathItem {
                    operations: Some(pi_ops),
                    ..Default::default()
                },
            )])),
            ..Default::default()
        };

        // Spec.paths /pets has an operation whose response includes a Link
        // referencing `pickPet`.
        let mut links_map = BTreeMap::new();
        links_map.insert(
            "next".to_owned(),
            RefOr::new_item(Link {
                operation_id: Some("pickPet".to_owned()),
                ..Default::default()
            }),
        );
        let response = Response {
            description: "ok".into(),
            links: Some(links_map),
            ..Default::default()
        };
        let mut responses_map = BTreeMap::new();
        responses_map.insert("200".to_owned(), RefOr::new_item(response));
        let responses = Responses {
            responses: Some(responses_map),
            ..Default::default()
        };
        let mut path_ops: BTreeMap<String, Operation> = BTreeMap::new();
        path_ops.insert(
            "get".to_owned(),
            Operation {
                responses: Some(responses),
                ..Default::default()
            },
        );
        let mut paths = Paths::default();
        paths.paths.insert(
            "/pets".to_owned(),
            PathItem {
                operations: Some(path_ops),
                ..Default::default()
            },
        );

        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        // Allow IgnoreUnusedSchemas etc; we only care that the link doesn't
        // report missing.
        let res = spec.validate(Options::new(), None);
        if let Err(err) = &res {
            assert!(
                err.errors
                    .iter()
                    .all(|e| !e.contains("missing operation with id `pickPet`")),
                "Link.operationId should resolve via components.pathItems: {:?}",
                err.errors
            );
        }
    }

    #[test]
    fn license_identifier_url_mutex() {
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                license: Some(crate::v3_1::info::License {
                    name: "MIT".into(),
                    identifier: Some("MIT".into()),
                    url: Some("https://example.com/license".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            paths: Some(Default::default()),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("`identifier` and `url` are mutually exclusive")),
            "expected license mutex error: {:?}",
            err.errors
        );
    }

    #[test]
    fn components_path_items_op_id_not_double_counted() {
        use crate::v3_1::components::Components;
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::PathItem;
        use crate::v3_1::response::{Response, Responses};

        let op = Operation {
            operation_id: Some("reuse".into()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("get".to_owned(), op);
        let pi = PathItem {
            operations: Some(ops),
            ..Default::default()
        };
        let comp = Components {
            path_items: Some(BTreeMap::from([("Reusable".to_owned(), pi)])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            paths: Some(Default::default()),
            ..Default::default()
        };
        let res = spec.validate(Options::new(), None);
        match res {
            Ok(_) => {}
            Err(e) => {
                assert!(
                    e.errors.iter().all(|s| !s.contains("already in use")),
                    "spurious duplicate-id error: {:?}",
                    e.errors
                );
            }
        }
    }

    #[test]
    fn webhook_keys_no_path_template_uniqueness() {
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::{PathItem, Paths};
        use crate::v3_1::response::{Response, Responses};

        let op = Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), op);
        let pi = PathItem {
            operations: Some(ops),
            ..Default::default()
        };
        let mut webhooks = Paths::default();
        webhooks.paths.insert("pet-{kind}".to_owned(), pi.clone());
        webhooks.paths.insert("user-{kind}".to_owned(), pi);
        let spec = Spec {
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let res = spec.validate(Options::new(), None);
        if let Err(e) = res {
            assert!(
                e.errors.iter().all(|s| !s.contains("collapse to the same")),
                "webhook templates wrongly flagged: {:?}",
                e.errors
            );
        }
    }

    #[test]
    fn operation_id_uniqueness_descends_into_callbacks() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::{PathItem, Paths};
        use crate::v3_1::response::{Response, Responses};

        let make_op = |id: &str| Operation {
            operation_id: Some(id.to_owned()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut cb_paths = BTreeMap::new();
        cb_paths.insert(
            "expr".to_owned(),
            PathItem {
                operations: Some(BTreeMap::from([("post".to_owned(), make_op("dup"))])),
                ..Default::default()
            },
        );
        let mut callbacks = BTreeMap::new();
        callbacks.insert(
            "ping".to_owned(),
            RefOr::new_item(Callback {
                paths: cb_paths,
                ..Default::default()
            }),
        );
        let outer = Operation {
            operation_id: Some("dup".to_owned()),
            responses: make_op("ignored").responses,
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), outer);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/a".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("operationId") && e.contains("`dup`")),
            "expected duplicate-id across callback boundary: {:?}",
            err.errors
        );
    }

    #[test]
    fn link_operation_id_resolves_in_inline_callback() {
        use crate::v3_1::callback::Callback;
        use crate::v3_1::link::Link;
        use crate::v3_1::operation::Operation;
        use crate::v3_1::path_item::{PathItem, Paths};
        use crate::v3_1::response::{Response, Responses};

        let make_op = |id: Option<&str>| Operation {
            operation_id: id.map(str::to_owned),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };

        let mut cb_paths = BTreeMap::new();
        cb_paths.insert(
            "expr".to_owned(),
            PathItem {
                operations: Some(BTreeMap::from([(
                    "post".to_owned(),
                    make_op(Some("inCallback")),
                )])),
                ..Default::default()
            },
        );
        let mut callbacks = BTreeMap::new();
        callbacks.insert(
            "ping".to_owned(),
            RefOr::new_item(Callback {
                paths: cb_paths,
                ..Default::default()
            }),
        );
        let mut links = BTreeMap::new();
        links.insert(
            "next".to_owned(),
            RefOr::new_item(Link {
                operation_id: Some("inCallback".to_owned()),
                ..Default::default()
            }),
        );
        let outer = Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: "ok".into(),
                        links: Some(links),
                        ..Default::default()
                    }),
                )])),
                ..Default::default()
            }),
            callbacks: Some(callbacks),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert("post".to_owned(), outer);
        let mut paths = Paths::default();
        paths.paths.insert(
            "/a".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );
        let spec = Spec {
            paths: Some(paths),
            ..Default::default()
        };
        let res = spec.validate(Options::new(), None);
        if let Err(e) = res {
            assert!(
                e.errors.iter().all(|s| !s.contains("inCallback")),
                "Link.operationId in callback must resolve: {:?}",
                e.errors
            );
        }
    }

    #[test]
    fn x_tag_groups_round_trip_and_validate() {
        let value = serde_json::json!({
            "openapi": "3.1.2",
            "info": {
                "title": "Pets",
                "version": "1"
            },
            "paths": {},
            "tags": [
                {
                    "name": "pets"
                }
            ],
            "x-tagGroups": [
                {
                    "name": "Public API",
                    "tags": ["pets"]
                }
            ]
        });

        let spec: Spec = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);

        let mut ctx = Context::new(&spec, Options::new());
        spec.x_tag_groups.as_ref().unwrap()[0]
            .validate_with_context(&mut ctx, "#.x-tagGroups[0]".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&spec, Options::new());
        TagGroup::default().validate_with_context(&mut ctx, "#.x-tagGroups[0]".to_owned());
        assert_eq!(ctx.errors.len(), 2, "tag group errors: {:?}", ctx.errors);
    }

    // ────────────────────────────────────────────────────────────────────
    // `Spec::merge` coverage.
    // ────────────────────────────────────────────────────────────────────

    fn base_spec(value: serde_json::Value) -> Spec {
        serde_json::from_value(value).expect("base spec must parse")
    }

    #[test]
    fn merge_paths_deep_merges_path_items_and_appends_new() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Base", "version": "1"},
            "paths": {
                "/a": {"summary": "base-a"},
                "/b": {"summary": "base-b"}
            }
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Incoming", "version": "2"},
            "paths": {
                "/b": {"summary": "incoming-b"},
                "/c": {"summary": "incoming-c"}
            }
        })));
        let paths = base.paths.expect("paths must be present");
        assert_eq!(paths.paths["/a"].summary.as_deref(), Some("base-a"));
        assert_eq!(paths.paths["/b"].summary.as_deref(), Some("incoming-b"));
        assert_eq!(paths.paths["/c"].summary.as_deref(), Some("incoming-c"));
        assert_eq!(base.info.title, "Base");
    }

    #[test]
    fn merge_path_items_preserves_methods_only_present_on_one_side() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "get": {"summary": "base-get", "responses": {"200": {"description": "ok"}}}
                }
            }
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {
                "/pets": {
                    "post": {"summary": "incoming-post", "responses": {"201": {"description": "created"}}}
                }
            }
        })));
        let json = serde_json::to_value(&base).unwrap();
        assert_eq!(json["paths"]["/pets"]["get"]["summary"], "base-get");
        assert_eq!(json["paths"]["/pets"]["post"]["summary"], "incoming-post");
    }

    #[test]
    fn merge_components_path_items_bag_deep_merges_on_collision() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "pathItems": {
                    "Echo": {
                        "get": {"summary": "base-get", "responses": {"200": {"description": "ok"}}}
                    }
                }
            }
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "pathItems": {
                    "Echo": {
                        "post": {"summary": "incoming-post", "responses": {"201": {"description": "created"}}}
                    }
                }
            }
        })));
        let json = serde_json::to_value(&base).unwrap();
        assert_eq!(
            json["components"]["pathItems"]["Echo"]["get"]["summary"],
            "base-get"
        );
        assert_eq!(
            json["components"]["pathItems"]["Echo"]["post"]["summary"],
            "incoming-post"
        );
    }

    #[test]
    fn merge_components_callbacks_bag_deep_merges_inline_callbacks() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "callbacks": {
                    "OnEvent": {
                        "{$request.body#/url}": {
                            "get": {"summary": "base-get", "responses": {"200": {"description": "ok"}}}
                        },
                        "{$request.body#/other}": {
                            "post": {"summary": "base-only-cb", "responses": {"200": {"description": "ok"}}}
                        }
                    }
                }
            }
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "callbacks": {
                    "OnEvent": {
                        "{$request.body#/url}": {
                            "post": {"summary": "incoming-post", "responses": {"201": {"description": "created"}}}
                        },
                        "{$request.body#/new}": {
                            "get": {"summary": "incoming-only-cb", "responses": {"200": {"description": "ok"}}}
                        }
                    }
                }
            }
        })));
        let json = serde_json::to_value(&base).unwrap();
        let cb = &json["components"]["callbacks"]["OnEvent"];
        assert_eq!(cb["{$request.body#/url}"]["get"]["summary"], "base-get");
        assert_eq!(
            cb["{$request.body#/url}"]["post"]["summary"],
            "incoming-post"
        );
        assert_eq!(
            cb["{$request.body#/other}"]["post"]["summary"],
            "base-only-cb"
        );
        assert_eq!(
            cb["{$request.body#/new}"]["get"]["summary"],
            "incoming-only-cb"
        );
    }

    #[test]
    fn merge_components_callbacks_bag_ref_replaces_wholesale() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "callbacks": {
                    "OnEvent": {
                        "{$request.body#/url}": {
                            "get": {"summary": "base-get", "responses": {"200": {"description": "ok"}}}
                        }
                    }
                }
            }
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "callbacks": {
                    "OnEvent": {"$ref": "#/components/callbacks/Other"}
                }
            }
        })));
        let json = serde_json::to_value(&base).unwrap();
        assert_eq!(
            json["components"]["callbacks"]["OnEvent"]["$ref"],
            "#/components/callbacks/Other"
        );
    }

    #[test]
    fn merge_components_each_bag_incoming_wins_per_name() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "components": {
                "schemas": {"Pet": {"type": "string"}, "Owner": {"type": "string"}},
                "responses": {"NotFound": {"description": "base"}}
            }
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "y", "version": "2"},
            "paths": {},
            "components": {
                "schemas": {"Pet": {"type": "object"}, "Tag": {"type": "string"}}
            }
        })));
        let comp = base.components.expect("components present");
        let schemas = comp.schemas.expect("schemas present");
        let pet = serde_json::to_value(&schemas["Pet"]).unwrap();
        assert_eq!(pet["type"], "object");
        assert!(schemas.contains_key("Owner"));
        assert!(schemas.contains_key("Tag"));
        assert!(
            comp.responses
                .as_ref()
                .is_some_and(|m| m.contains_key("NotFound"))
        );
    }

    #[test]
    fn merge_tags_dedupe_by_name_and_append_new() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "tags": [
                {"name": "pets", "description": "base-pets"},
                {"name": "users", "description": "base-users"}
            ]
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "tags": [
                {"name": "pets", "description": "incoming-pets"},
                {"name": "orders", "description": "new-orders"}
            ]
        })));
        let tags = base.tags.unwrap();
        assert_eq!(tags.len(), 3);
        let pets = tags.iter().find(|t| t.name == "pets").unwrap();
        assert_eq!(pets.description.as_deref(), Some("incoming-pets"));
    }

    #[test]
    fn merge_servers_replaces_only_when_incoming_is_non_empty() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "servers": [{"url": "https://base.example/"}]
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {}
        })));
        assert_eq!(
            base.servers.as_ref().unwrap()[0].url,
            "https://base.example/"
        );
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "servers": [{"url": "https://incoming.example/"}]
        })));
        assert_eq!(
            base.servers.as_ref().unwrap()[0].url,
            "https://incoming.example/"
        );
    }

    #[test]
    fn merge_keeps_base_info_and_openapi_version() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Base", "version": "1"},
            "paths": {}
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.2",
            "info": {"title": "Incoming", "version": "9"},
            "paths": {}
        })));
        assert_eq!(base.info.title, "Base");
        assert_eq!(base.info.version, "1");
    }

    #[test]
    fn merge_top_level_extensions_per_key_incoming_wins() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "x-shared": "base",
            "x-base-only": "kept"
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "x-shared": "incoming",
            "x-incoming-only": "added"
        })));
        let ext = base.extensions.unwrap();
        assert_eq!(ext["x-shared"], serde_json::json!("incoming"));
        assert_eq!(ext["x-base-only"], serde_json::json!("kept"));
        assert_eq!(ext["x-incoming-only"], serde_json::json!("added"));
    }

    #[test]
    fn merge_webhooks_per_key_incoming_wins() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "webhooks": {"petCreated": {"summary": "base-hook"}}
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {},
            "webhooks": {
                "petCreated": {"summary": "incoming-hook"},
                "petDeleted": {"summary": "new-hook"}
            }
        })));
        let webhooks = base.webhooks.unwrap();
        assert_eq!(
            webhooks.paths["petCreated"].summary.as_deref(),
            Some("incoming-hook")
        );
        assert_eq!(
            webhooks.paths["petDeleted"].summary.as_deref(),
            Some("new-hook")
        );
    }

    #[test]
    fn merge_round_trips_through_json() {
        let mut base = base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "x", "version": "1"},
            "paths": {"/a": {"get": {"responses": {"200": {"description": "ok"}}}}},
            "components": {"schemas": {"Pet": {"type": "string"}}}
        }));
        base.merge(base_spec(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "y", "version": "2"},
            "paths": {"/b": {"get": {"responses": {"200": {"description": "ok"}}}}},
            "components": {"schemas": {"Owner": {"type": "string"}}}
        })));
        let json = serde_json::to_value(&base).unwrap();
        let _: Spec = serde_json::from_value(json).expect("merged spec must re-parse");
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
