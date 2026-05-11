//! The root document object of the OpenAPI v3.2.X specification.
//!
//! https://spec.openapis.org/oas/v3.2.0

use crate::common::helpers::{
    Context, InvalidComponentName, PushError, ValidateWithContext, check_component_name,
    validate_not_visited,
};
use crate::common::reference::{RefOr, ResolveReference, resolve_in_map};
use crate::loader::Loader;
use crate::v3_2::callback::Callback;
use crate::v3_2::components::Components;
use crate::v3_2::example::Example;
use crate::v3_2::external_documentation::ExternalDocumentation;
use crate::v3_2::header::Header;
use crate::v3_2::info::Info;
use crate::v3_2::link::Link;
use crate::v3_2::operation::Operation;
use crate::v3_2::parameter::Parameter;
use crate::v3_2::path_item::{PathItem, Paths};
use crate::v3_2::request_body::RequestBody;
use crate::v3_2::response::Response;
use crate::v3_2::schema::Schema;
use crate::v3_2::security_scheme::SecurityScheme;
use crate::v3_2::server::Server;
use crate::v3_2::tag::Tag;
use crate::v3_2::validation::{
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
/// openapi: "3.2.0"
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
    /// The value MUST be `3.2.0`.
    pub openapi: Version,

    /// Self-assigned URI for this document, added in OAS 3.2. Used as the
    /// base URI for resolving relative `$ref`s and `operationRef`s in the
    /// document.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "$self")]
    pub self_uri: Option<String>,

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
    /// Optional in OAS 3.1+ (was required in 3.0).
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

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// The OpenAPI Specification version. Per the OAS 3.2 JSON Schema, the
/// `openapi` field matches `^3\.2\.\d+(-.+)?$` — i.e. any 3.2.x patch
/// version, optionally with a prerelease suffix. We accept `3.2` as a
/// short alias and normalise it to `3.2.0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self("3.2.0".to_owned())
    }
}

impl Version {
    /// Convenience constructor for the canonical `3.2.0` value.
    #[allow(non_snake_case)]
    pub fn V3_2_0() -> Self {
        Self("3.2.0".to_owned())
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

/// Human-readable description of the OAS 3.2 version pattern, shared
/// by serde's `expected` payload and `InvalidVersion`'s `Display`.
/// The literal regex itself lives in [`matches_oas_3_2_version`] —
/// keep both in sync if either ever changes.
const VERSION_SCHEMA_DESCRIPTION: &str =
    "`3.2.<patch>` semver, optionally with a `-<prerelease>` suffix";

/// Single source of truth for the regex check. `lazy_regex::regex!`
/// requires a string literal so we can't host the pattern in a `const`,
/// but every parsing path goes through this one function.
fn matches_oas_3_2_version(s: &str) -> bool {
    lazy_regex::regex!(r"^3\.2\.\d+(-.+)?$").is_match(s)
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
/// does not match the OAS 3.2 schema pattern (see [`Version`]).
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
        if s == "3.2" {
            return Ok(Version("3.2.0".to_owned()));
        }
        if matches_oas_3_2_version(s) {
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
        if s == "3.2" {
            return Ok(Version("3.2.0".to_owned()));
        }
        if matches_oas_3_2_version(&s) {
            Ok(Version(s))
        } else {
            Err(InvalidVersion(s))
        }
    }
}

impl Validate for Version {
    fn validate(&self, _options: EnumSet<Options>) -> Result<(), Error> {
        // Constructors and parsers all enforce the pattern, but
        // re-checking here keeps `Validate` a self-contained contract.
        if matches_oas_3_2_version(&self.0) {
            Ok(())
        } else {
            Err(Error {
                errors: vec![format!("#.openapi: must be {VERSION_SCHEMA_DESCRIPTION}")],
            })
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

impl ResolveReference<crate::v3_2::media_type::MediaType> for Spec {
    fn resolve_reference(&self, reference: &str) -> Option<&crate::v3_2::media_type::MediaType> {
        self.components.as_ref().and_then(|x| {
            resolve_in_map(self, reference, "#/components/mediaTypes/", &x.media_types)
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
    // Standard fixed-field operations.
    if let Some(operations) = &item.operations {
        for (method, op) in operations {
            walk_op(op, format!("{location}.{method}"), spec, out, seen_cb);
        }
    }
    // OAS 3.2 `additionalOperations` map — operationIds in here participate
    // in document-wide uniqueness too.
    if let Some(extra) = &item.additional_operations {
        for (method, op) in extra {
            walk_op(
                op,
                format!("{location}.additionalOperations[{method}]"),
                spec,
                out,
                seen_cb,
            );
        }
    }
}

fn walk_op<'a>(
    op: &'a Operation,
    op_loc: String,
    spec: &'a Spec,
    out: &mut Vec<(&'a Operation, String)>,
    seen_cb: &mut std::collections::HashSet<*const Callback>,
) {
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

impl Spec {
    /// Validate the spec, resolving external `$ref`s through the given loader.
    ///
    /// Equivalent to [`Validate::validate`] when no loader is needed, but
    /// when an external `$ref` is encountered, the loader is asked to
    /// fetch and deserialize the target so the resolved value is
    /// validated like any internal ref. External resolution failures
    /// surface as validation errors unless
    /// [`Options::IgnoreExternalReferences`] is set.
    pub fn validate_with_loader(
        &self,
        options: EnumSet<Options>,
        loader: &mut Loader,
    ) -> Result<(), Error> {
        self.validate_inner(options, Some(loader))
    }

    fn validate_inner<'a>(
        &'a self,
        options: EnumSet<Options>,
        loader: Option<&'a mut Loader>,
    ) -> Result<(), Error> {
        let mut ctx = Context::new(self, options);
        if let Some(l) = loader {
            ctx.loader = Some(l);
        }

        // Surface any `openapi` schema-pattern violations alongside the
        // rest of the spec's errors instead of bailing out early.
        if let Err(e) = self.openapi.validate(options) {
            ctx.errors.extend(e.errors);
        }

        self.info
            .validate_with_context(&mut ctx, "#.info".to_owned());

        // jsonSchemaDialect MUST be a URI per OAS 3.2 (default-value spec for
        // the `$schema` keyword in nested Schema Objects). Use the generic
        // URI validator (not the HTTP-only URL one) so non-HTTP dialect
        // identifiers like `urn:example:dialect` are accepted.
        crate::common::helpers::validate_optional_uri(
            &self.json_schema_dialect,
            &mut ctx,
            "#.jsonSchemaDialect".to_owned(),
        );

        // OAS 3.2 `$self` MUST be a URI without a fragment (the JSON
        // Schema enforces `pattern: "^[^#]*$"`).
        crate::common::helpers::validate_optional_uri(
            &self.self_uri,
            &mut ctx,
            "#.$self".to_owned(),
        );
        if let Some(self_uri) = &self.self_uri
            && self_uri.contains('#')
        {
            ctx.error("#.$self".to_owned(), "MUST NOT contain a fragment (`#`)");
        }

        if let Some(servers) = &self.servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(&mut ctx, format!("#.servers[{i}]"))
            }
        }

        // OAS 3.2.0: operationId MUST be unique across the whole document.
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
            // Webhook keys are arbitrary identifiers per OAS 3.2.0, not URL
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
    fn validate(&self, options: EnumSet<Options>) -> Result<(), Error> {
        self.validate_inner(options, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::IGNORE_UNUSED;

    #[test]
    fn validate_with_loader_resolves_external_schema_ref() {
        let spec: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.2.0",
            "info": { "title": "test", "version": "1.0" },
            "paths": {},
            "components": {
                "schemas": {
                    "PetRef": { "$ref": "external.json#/Pet" }
                }
            }
        }))
        .expect("spec must parse");

        // No loader: the external `$ref` becomes a validation error.
        let err = spec
            .validate(IGNORE_UNUSED)
            .expect_err("external ref must error when no loader is attached");
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("external.json#/Pet") && e.contains("not supported")),
            "expected `not supported` error, got: {:?}",
            err.errors,
        );

        // With a preloaded resource: the loader resolves the ref and the
        // resolved schema validates clean.
        let mut loader = Loader::new();
        loader
            .preload_resource(
                "external.json",
                serde_json::json!({
                    "Pet": { "type": "object", "properties": {} }
                }),
            )
            .expect("preload must succeed");
        spec.validate_with_loader(IGNORE_UNUSED, &mut loader)
            .expect("validation must succeed when external ref is preloaded");

        // With a loader that has no fetcher and no preload: the loader
        // failure surfaces as a `failed to resolve` error.
        let mut empty_loader = Loader::new();
        let err = spec
            .validate_with_loader(IGNORE_UNUSED, &mut empty_loader)
            .expect_err("missing fetcher must surface as a validation error");
        assert!(
            err.errors
                .iter()
                .any(|e| { e.contains("external.json#/Pet") && e.contains("failed to resolve") }),
            "expected `failed to resolve` error, got: {:?}",
            err.errors,
        );
    }

    #[test]
    fn loader_typed_cache_avoids_redundant_deserialization() {
        // Two separate `$ref`s to the same external pointer should
        // deserialize the target only once thanks to the typed cache.
        // We can't observe serde counts directly, but we can verify the
        // pre-warmed cache survives a `validate_with_loader` pass.
        let spec: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.2.0",
            "info": { "title": "test", "version": "1.0" },
            "paths": {},
            "components": {
                "schemas": {
                    "A": { "$ref": "external.json#/Pet" },
                    "B": { "$ref": "external.json#/Pet" }
                }
            }
        }))
        .expect("spec must parse");

        let mut loader = Loader::new();
        loader
            .preload_resource(
                "external.json",
                serde_json::json!({
                    "Pet": { "type": "object", "properties": {} }
                }),
            )
            .unwrap();

        spec.validate_with_loader(IGNORE_UNUSED, &mut loader)
            .expect("two refs to the same external pointer must validate");

        // Calling `resolve_reference_as` twice after validation must hit
        // the typed cache and return equal owned schemas.
        let first: Schema = loader
            .resolve_reference_as("external.json#/Pet")
            .expect("first lookup");
        let second: Schema = loader
            .resolve_reference_as("external.json#/Pet")
            .expect("cached lookup");
        assert_eq!(first, second, "cached typed value must round-trip equal");
    }

    #[test]
    fn test_version_deserialize() {
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.2.0")).unwrap(),
            Version::V3_2_0(),
            "correct openapi version",
        );
        assert_eq!(
            serde_json::from_value::<Version>(serde_json::json!("3.2")).unwrap(),
            Version::V3_2_0(),
            "`3.2` short alias is accepted",
        );
        assert!(
            serde_json::from_value::<Version>(serde_json::json!("foo"))
                .unwrap_err()
                .to_string()
                .contains("expected `3.2.<patch>` semver"),
            "foo as openapi version",
        );

        // Patch versions and prerelease suffixes are accepted per the
        // schema pattern `^3\\.2\\.\\d+(-.+)?$`.
        for ok in ["3.2.0", "3.2.1", "3.2.42", "3.2.0-rc1", "3.2.7-beta.3"] {
            let v: Version = serde_json::from_value(serde_json::json!(ok))
                .expect("must accept patch/prerelease");
            assert_eq!(v.as_str(), ok, "round-trip `{ok}`");
        }
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "3.2.0",
                "info": {
                    "title": "foo",
                    "version": "1",
                },
                "paths": {},
            }))
            .unwrap()
            .openapi,
            Version::V3_2_0(),
            "3.2.0 spec.openapi",
        );
        assert_eq!(
            serde_json::from_value::<Spec>(serde_json::json!({
                "openapi": "3.2",
                "info": {"title": "foo", "version": "1"},
                "paths": {},
            }))
            .unwrap()
            .openapi,
            Version::V3_2_0(),
            "`3.2` short alias accepted at Spec level too",
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
            .contains("expected `3.2.<patch>` semver"),
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
            serde_json::to_string(&Version::V3_2_0()).unwrap(),
            r#""3.2.0""#,
        );
        assert_eq!(
            serde_json::to_string(&Version::default()).unwrap(),
            r#""3.2.0""#,
        );
    }

    #[test]
    fn test_version_validate() {
        assert!(Version::default().validate(Options::new()).is_ok());
        assert!(Version::V3_2_0().validate(Options::new()).is_ok());
        assert!(
            "3.2.99"
                .parse::<Version>()
                .unwrap()
                .validate(Options::new())
                .is_ok()
        );
    }

    #[test]
    fn test_version_validate_rejects_invalid() {
        let invalid = Version("garbage".to_owned());
        let err = invalid.validate(Options::new()).unwrap_err();
        assert_eq!(err.errors.len(), 1);
        assert!(
            err.errors[0].contains("#.openapi") && err.errors[0].contains("3.2.<patch>"),
            "validate error names the field and the schema description: {:?}",
            err.errors
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
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("#.openapi") && e.contains("3.2.<patch>")),
            "Spec::validate surfaces the openapi error: {:?}",
            err.errors
        );
    }

    #[test]
    fn test_version_try_from_string_normalizes_short_alias() {
        let v: Version = "3.2".to_owned().try_into().unwrap();
        assert_eq!(v, Version::V3_2_0());
        let err: InvalidVersion = Version::try_from("nope".to_owned()).unwrap_err();
        assert_eq!(err.0, "nope");
    }

    #[test]
    fn test_version_parse_programmatically() {
        use std::str::FromStr;
        assert_eq!(
            Version::from_str("3.2.99").unwrap(),
            Version("3.2.99".to_owned())
        );
        assert_eq!(
            Version::from_str("3.2.0-rc1").unwrap(),
            Version("3.2.0-rc1".to_owned())
        );
        assert_eq!(Version::from_str("3.2").unwrap(), Version::V3_2_0());
        assert_eq!(
            <Version as TryFrom<&str>>::try_from("3.2.7").unwrap(),
            Version("3.2.7".to_owned())
        );
        assert_eq!(
            <Version as TryFrom<String>>::try_from("3.2.7".to_owned()).unwrap(),
            Version("3.2.7".to_owned())
        );
        let err = Version::from_str("foo").unwrap_err();
        assert_eq!(err, InvalidVersion("foo".to_owned()));
        assert!(
            err.to_string().contains("3.2.<patch>"),
            "error message describes the schema: {err}"
        );
    }

    #[test]
    fn full_spec_validate_drives_path_template_uniqueness() {
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::Paths;
        use crate::v3_2::response::Responses;

        let make_op = || Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
    fn webhooks_validation_runs() {
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::Paths;
        use crate::v3_2::response::Responses;

        let mut ops: BTreeMap<String, Operation> = BTreeMap::new();
        ops.insert(
            "post".to_owned(),
            Operation {
                responses: Some(Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            description: Some("ok".into()),
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
        let err = spec.validate(Options::new()).unwrap_err();
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
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::Paths;
        use crate::v3_2::response::Responses;

        let make_op = |id: &str| Operation {
            operation_id: Some(id.to_owned()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("`dup` already in use")),
            "expected operationId duplicate across paths/webhooks: {:?}",
            err.errors
        );
    }

    #[test]
    fn all_define_helpers_insert_and_return_ref() {
        use crate::v3_2::callback::Callback;
        use crate::v3_2::example::Example;
        use crate::v3_2::header::Header;
        use crate::v3_2::link::Link;
        use crate::v3_2::parameter::{InQuery, Parameter};
        use crate::v3_2::request_body::RequestBody;
        use crate::v3_2::response::Response;
        use crate::v3_2::schema::{SingleSchema, StringSchema};
        use crate::v3_2::security_scheme::{HttpSecurityScheme, SecurityScheme};

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
                    deprecated: None,
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
        use crate::v3_2::callback::Callback;
        use crate::v3_2::example::Example;
        use crate::v3_2::header::Header;
        use crate::v3_2::link::Link;
        use crate::v3_2::request_body::RequestBody;
        use crate::v3_2::response::Response;
        use crate::v3_2::security_scheme::{HttpSecurityScheme, SecurityScheme};

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
        use crate::v3_2::callback::Callback;
        use crate::v3_2::example::Example;
        use crate::v3_2::header::Header;
        use crate::v3_2::link::Link;
        use crate::v3_2::parameter::{InQuery, Parameter};
        use crate::v3_2::request_body::RequestBody;
        use crate::v3_2::response::Response;
        use crate::v3_2::schema::{SingleSchema, StringSchema};
        use crate::v3_2::security_scheme::{HttpSecurityScheme, SecurityScheme};

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
        assert_eq!(Version::V3_2_0().to_string(), "3.2.0");
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
        assert!(spec.validate(Options::new()).is_ok());

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
        let err = spec.validate(Options::new()).unwrap_err();
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
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("jsonSchemaDialect") && e.contains("must be a valid URI")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn self_uri_round_trip_and_validated() {
        // OAS 3.2: $self is a URI; round-trips and is URI-validated.
        let v = serde_json::json!({
            "openapi": "3.2.0",
            "$self": "https://example.com/api.openapi",
            "info": {"title": "x", "version": "1"},
            "paths": {}
        });
        let spec: Spec = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(
            spec.self_uri.as_deref(),
            Some("https://example.com/api.openapi")
        );
        assert_eq!(serde_json::to_value(&spec).unwrap(), v);

        // Empty $self is rejected as not a valid URI.
        let spec = Spec {
            info: Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            self_uri: Some("".into()),
            paths: Some(Default::default()),
            ..Default::default()
        };
        let err = spec.validate(Options::new()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("$self") && e.contains("must be a valid URI")),
            "errors: {:?}",
            err.errors
        );
    }

    #[test]
    fn op_id_unique_across_paths_webhooks_components_pathitems() {
        // Pre-collection should detect a duplicate operationId across all
        // three containers.
        use crate::v3_2::components::Components;
        use crate::v3_2::operation::Operation;
        use crate::v3_2::response::Responses;

        let make_op = |id: &str| Operation {
            operation_id: Some(id.to_owned()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
        let err = spec.validate(Options::new()).unwrap_err();
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
        use crate::v3_2::components::Components;
        use crate::v3_2::operation::Operation;
        use crate::v3_2::response::Responses;

        let mut pi_ops: BTreeMap<String, Operation> = BTreeMap::new();
        pi_ops.insert(
            "get".to_owned(),
            Operation {
                operation_id: Some("pickPet".to_owned()),
                responses: Some(Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            description: Some("ok".into()),
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
            description: Some("ok".into()),
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
        let res = spec.validate(Options::new());
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
                license: Some(crate::v3_2::info::License {
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
        let err = spec.validate(Options::new()).unwrap_err();
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
        use crate::v3_2::components::Components;
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::PathItem;
        use crate::v3_2::response::{Response, Responses};

        let op = Operation {
            operation_id: Some("reuse".into()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
        let res = spec.validate(Options::new());
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
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::{PathItem, Paths};
        use crate::v3_2::response::{Response, Responses};

        let op = Operation {
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
        let res = spec.validate(Options::new());
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
        use crate::v3_2::callback::Callback;
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::{PathItem, Paths};
        use crate::v3_2::response::{Response, Responses};

        let make_op = |id: &str| Operation {
            operation_id: Some(id.to_owned()),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
        let err = spec.validate(Options::new()).unwrap_err();
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
        use crate::v3_2::callback::Callback;
        use crate::v3_2::link::Link;
        use crate::v3_2::operation::Operation;
        use crate::v3_2::path_item::{PathItem, Paths};
        use crate::v3_2::response::{Response, Responses};

        let make_op = |id: Option<&str>| Operation {
            operation_id: id.map(str::to_owned),
            responses: Some(Responses {
                responses: Some(BTreeMap::from([(
                    "200".to_owned(),
                    RefOr::new_item(Response {
                        description: Some("ok".into()),
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
                        description: Some("ok".into()),
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
        let res = spec.validate(Options::new());
        if let Err(e) = res {
            assert!(
                e.errors.iter().all(|s| !s.contains("inCallback")),
                "Link.operationId in callback must resolve: {:?}",
                e.errors
            );
        }
    }

    #[test]
    fn x_tag_groups_round_trip_via_generic_extensions() {
        // 3.2 supersedes Redoc's `x-tagGroups` with `Tag.parent` /
        // `Tag.kind` / `Tag.summary`. The legacy key still survives
        // round-trip through the generic `extensions` map.
        let groups = serde_json::json!([
            {"name": "Public API", "tags": ["pets"]}
        ]);
        let value = serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "Pets", "version": "1"},
            "paths": {},
            "tags": [{"name": "pets"}],
            "x-tagGroups": groups.clone(),
        });
        let spec: Spec = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            spec.extensions.as_ref().and_then(|m| m.get("x-tagGroups")),
            Some(&groups)
        );
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
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
