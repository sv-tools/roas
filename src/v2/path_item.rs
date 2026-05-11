//! Path Items

use crate::common::helpers::{Context, ValidateWithContext};
use crate::v2::operation::Operation;
use crate::v2::parameter::Parameter;
use crate::v2::reference::RefOr;
use crate::v2::spec::Spec;
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// Describes the operations available on a single path.
/// A Path Item may be empty, due to [ACL constraints](https://swagger.io/specification/v2/#security-filtering).
/// The path itself is still exposed to the documentation viewer
/// but they will not know which operations and parameters are available.
///
/// Specification example:
///
/// ```yaml
/// get:
///   description: Returns pets based on ID
///   summary: Find pets by ID
///   operationId: getPetsById
///   produces:
///   - application/json
///   - text/html
///   responses:
///     '200':
///       description: pet response
///       schema:
///         type: array
///         items:
///           $ref: '#/definitions/Pet'
///     default:
///       description: error payload
///       schema:
///         $ref: '#/definitions/ErrorModel'
/// search:
///   description: Returns pets based on the search parameters
///   summary: Find pets by ID
///   operationId: getPetsById
///   produces:
///   - application/json
///   - text/html
///   responses:
///     '200':
///       description: pet response
///       schema:
///         type: array
///         items:
///           $ref: '#/definitions/Pet'
///     default:
///       description: error payload
///       schema:
///         $ref: '#/definitions/ErrorModel'
/// parameters:
/// - name: id
///   in: path
///   description: ID of pet to use
///   required: true
///   type: array
///   items:
///     type: string
///   collectionFormat: csv
/// ```
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PathItem {
    /// Allows for an external definition of this path item.
    /// The referenced structure MUST be in the format of a Path Item Object.
    /// If there are conflicts between the referenced definition and this Path Item's
    /// definition, the behavior is undefined.
    pub reference: Option<String>,

    /// A definition of the operations on this path.
    ///
    /// Any map items that can be converted to an `Operation` object will be stored here.
    /// This includes `get`, `put`, `post`, `delete`, `options`, `head`, `patch`, `trace`,
    /// and any other custom operations, like SEARCH and etc...
    ///
    /// Note: v2 spec defines a closed set (`get/put/post/delete/options/head/patch`); this
    /// implementation accepts arbitrary HTTP methods as an intentional permissive extension.
    pub operations: Option<BTreeMap<String, Operation>>,

    /// A list of parameters that are applicable for all the operations described under this path.
    /// These parameters can be overridden at the operation level, but cannot be removed there.
    /// The list MUST NOT include duplicated parameters.
    /// A unique parameter is defined by a combination of a name and location.
    /// The list can use the [Reference Object](crate::common::reference::Ref) to link to parameters
    /// that are defined at the [Swagger Object's](crate::v2::spec::Spec::parameters) parameters.
    /// There can be one "body" parameter at most.
    pub parameters: Option<Vec<RefOr<Parameter>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Serialize for PathItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        if let Some(reference) = &self.reference {
            map.serialize_entry("$ref", reference)?;
        }

        if let Some(o) = &self.operations {
            for (k, v) in o {
                map.serialize_entry(&k, &v)?;
            }
        }

        if let Some(parameters) = &self.parameters {
            map.serialize_entry("parameters", parameters)?;
        }

        if let Some(ref ext) = self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(&k, &v)?;
                }
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for PathItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "$ref",
            "parameters",
            "get",
            "head",
            "post",
            "put",
            "patch",
            "delete",
            "options",
            "trace",
            "<custom method>",
            "x-<ext name>",
        ];

        struct PathItemVisitor;

        impl<'de> Visitor<'de> for PathItemVisitor {
            type Value = PathItem;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct PathItem")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PathItem, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut res = PathItem::default();
                let mut operations: BTreeMap<String, Operation> = BTreeMap::new();
                let mut extensions: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "$ref" {
                        if res.reference.is_some() {
                            return Err(Error::duplicate_field("$ref"));
                        }
                        res.reference = Some(map.next_value()?);
                    } else if key == "parameters" {
                        if res.parameters.is_some() {
                            return Err(Error::duplicate_field("parameters"));
                        }
                        res.parameters = Some(map.next_value()?);
                    } else if key.starts_with("x-") {
                        if extensions.contains_key(key.clone().as_str()) {
                            return Err(Error::custom(format!("duplicate field '{key}'")));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else {
                        let key = key.to_lowercase();
                        if operations.contains_key(key.as_str()) {
                            return Err(Error::custom(format!("duplicate field '{key}'")));
                        }
                        operations.insert(key, map.next_value()?);
                    }
                }
                if !operations.is_empty() {
                    res.operations = Some(operations);
                }
                if !extensions.is_empty() {
                    res.extensions = Some(extensions);
                }
                Ok(res)
            }
        }

        deserializer.deserialize_struct("PathItem", FIELDS, PathItemVisitor)
    }
}

impl ValidateWithContext<Spec> for PathItem {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        // `$ref` (when present) MUST be a non-empty URI per the spec; the same
        // shape `v2::reference::Ref::validate_with_context` enforces.
        if let Some(reference) = &self.reference {
            crate::common::helpers::validate_required_string(
                reference,
                ctx,
                format!("{path}.$ref"),
            );
            // OAS 2.0: a Path Item with `$ref` replaces this object; mixing
            // it with inline operations or parameters has undefined behavior.
            let has_ops = self.operations.as_ref().is_some_and(|m| !m.is_empty());
            let has_params = self.parameters.as_ref().is_some_and(|p| !p.is_empty());
            if has_ops || has_params {
                crate::common::helpers::PushError::error(
                    ctx,
                    format!("{path}.$ref"),
                    "MUST NOT coexist with inline operations or parameters",
                );
            }
        }

        if let Some(other) = &self.operations {
            for (method, operation) in other.iter() {
                operation.validate_with_context(ctx, format!("{path}.{method}"));
            }
        }

        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{path}.parameters[{i}]"));
            }
        }
    }
}

/// The Paths Object: holds the relative paths to the individual endpoints.
///
/// In addition to the path-keyed entries, this object supports
/// Specification Extensions (`^x-` keys) per the v2 spec.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paths {
    /// Map from a path (which MUST begin with `/`) to its `PathItem`.
    pub paths: BTreeMap<String, PathItem>,

    /// `^x-` Specification Extensions on the Paths Object itself.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Paths {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, PathItem> {
        self.paths.iter()
    }
}

impl<S, K> From<S> for Paths
where
    S: IntoIterator<Item = (K, PathItem)>,
    K: Into<String>,
{
    fn from(iter: S) -> Self {
        Paths {
            paths: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            extensions: None,
        }
    }
}

impl Serialize for Paths {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Only `x-` keys are emitted from `extensions`, so the size hint must
        // count only those — a programmatically constructed map could carry
        // non-`x-` entries that would otherwise overstate the count.
        let ext_x_count = self
            .extensions
            .as_ref()
            .map(|e| e.keys().filter(|k| k.starts_with("x-")).count())
            .unwrap_or(0);
        let total = self.paths.len() + ext_x_count;
        let mut map = serializer.serialize_map(Some(total))?;
        for (k, v) in &self.paths {
            map.serialize_entry(k, v)?;
        }
        if let Some(ext) = &self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(k, v)?;
                }
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Paths {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PathsVisitor;
        impl<'de> Visitor<'de> for PathsVisitor {
            type Value = Paths;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a Paths object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Paths, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut paths: BTreeMap<String, PathItem> = BTreeMap::new();
                let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key.starts_with("x-") {
                        if ext.contains_key(&key) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        ext.insert(key, map.next_value()?);
                    } else {
                        if paths.contains_key(&key) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        paths.insert(key, map.next_value()?);
                    }
                }
                Ok(Paths {
                    paths,
                    extensions: if ext.is_empty() { None } else { Some(ext) },
                })
            }
        }
        deserializer.deserialize_map(PathsVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::formats::CollectionFormat;
    use crate::v2::items::{Items, StringItem};
    use crate::v2::parameter::{ArrayParameter, InPath};
    use crate::v2::response::{Response, Responses};

    #[test]
    fn test_path_item_deserialize() {
        assert_eq!(
            serde_json::from_value::<PathItem>(serde_json::json!({
                "get": {
                    "description": "Returns pets based on ID",
                    "summary": "Find pets by ID",
                    "operationId": "getPetsById",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "post": {
                    "description": "Run Pet's Action",
                    "summary": "Run pet's action by ID",
                    "operationId": "actionPet",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "consumes": [
                        "application/json",
                        "text/xml",
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "put": {
                    "description": "Update pet by ID",
                    "summary": "Update pet by ID",
                    "operationId": "updatePet",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "consumes": [
                        "application/json",
                        "text/xml",
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "head": {
                    "description": "Check pet by ID",
                    "summary": "Check if pet exists by ID",
                    "operationId": "checkPet",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "delete": {
                    "description": "Delete pet by ID",
                    "summary": "Delete pet by ID",
                    "operationId": "deletePet",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "patch": {
                    "description": "Change pet by ID",
                    "summary": "Change pet by ID",
                    "operationId": "changePet",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "consumes": [
                        "application/json",
                        "text/xml",
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "options": {
                    "description": "List of possible operations",
                    "summary": "Return the list of possible operations",
                    "operationId": "petOperations",
                    "produces": [
                        "application/json",
                        "text/html",
                        "text/plain"
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "trace": {
                    "description": "Trace pet by ID",
                    "summary": "Trace pet by ID",
                    "operationId": "tracePet",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "search": {
                    "description": "Search Pets",
                    "summary": "Search pets",
                    "operationId": "searchPets",
                    "produces": [
                        "application/json",
                        "text/html"
                    ],
                    "responses": {
                        "default": {
                            "description": "error payload",
                            "schema": {
                                "$ref": "#/definitions/ErrorModel"
                            }
                        }
                    }
                },
                "parameters": [
                    {
                        "name": "id",
                        "in": "path",
                        "description": "ID of pet to use",
                        "required": true,
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "collectionFormat": "csv"
                    }
                ],
                "x-extra": "extra",
            }))
            .unwrap(),
            PathItem {
                reference: None,
                operations: Some({
                    let mut operations = BTreeMap::new();
                    operations.insert(
                        String::from("get"),
                        Operation {
                            description: Some(String::from("Returns pets based on ID")),
                            summary: Some(String::from("Find pets by ID")),
                            operation_id: Some(String::from("getPetsById")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("post"),
                        Operation {
                            description: Some(String::from("Run Pet's Action")),
                            summary: Some(String::from("Run pet's action by ID")),
                            operation_id: Some(String::from("actionPet")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            consumes: Some(vec![
                                String::from("application/json"),
                                String::from("text/xml"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("put"),
                        Operation {
                            description: Some(String::from("Update pet by ID")),
                            summary: Some(String::from("Update pet by ID")),
                            operation_id: Some(String::from("updatePet")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            consumes: Some(vec![
                                String::from("application/json"),
                                String::from("text/xml"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("head"),
                        Operation {
                            description: Some(String::from("Check pet by ID")),
                            summary: Some(String::from("Check if pet exists by ID")),
                            operation_id: Some(String::from("checkPet")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("delete"),
                        Operation {
                            description: Some(String::from("Delete pet by ID")),
                            summary: Some(String::from("Delete pet by ID")),
                            operation_id: Some(String::from("deletePet")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("patch"),
                        Operation {
                            description: Some(String::from("Change pet by ID")),
                            summary: Some(String::from("Change pet by ID")),
                            operation_id: Some(String::from("changePet")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            consumes: Some(vec![
                                String::from("application/json"),
                                String::from("text/xml"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("options"),
                        Operation {
                            description: Some(String::from("List of possible operations")),
                            summary: Some(String::from("Return the list of possible operations")),
                            operation_id: Some(String::from("petOperations")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                                String::from("text/plain"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("trace"),
                        Operation {
                            description: Some(String::from("Trace pet by ID")),
                            summary: Some(String::from("Trace pet by ID")),
                            operation_id: Some(String::from("tracePet")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations.insert(
                        String::from("search"),
                        Operation {
                            description: Some(String::from("Search Pets")),
                            summary: Some(String::from("Search pets")),
                            operation_id: Some(String::from("searchPets")),
                            produces: Some(vec![
                                String::from("application/json"),
                                String::from("text/html"),
                            ]),
                            responses: Responses {
                                default: Some(RefOr::new_item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::new_ref(
                                        "#/definitions/ErrorModel".to_owned(),
                                    )),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations
                }),
                parameters: Some(vec![RefOr::new_item(Parameter::Path(Box::new(
                    InPath::Array(ArrayParameter {
                        name: String::from("id"),
                        description: Some(String::from("ID of pet to use")),
                        required: Some(true),
                        items: Items::String(Box::new(StringItem {
                            ..Default::default()
                        })),
                        collection_format: Some(CollectionFormat::CSV),
                        ..Default::default()
                    })
                )))]),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-extra"), serde_json::json!("extra"));
                    map
                }),
            },
            "deserialize",
        );
    }

    #[test]
    fn path_item_with_ref_roundtrip() {
        let raw = serde_json::json!({
            "$ref": "#/paths/~1foo",
            "x-extra": "y",
        });
        let item: PathItem = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(item.reference, Some("#/paths/~1foo".to_owned()));
        assert!(item.operations.is_none());
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v, raw);
    }

    #[test]
    fn path_item_ref_must_not_coexist_with_operations_or_parameters() {
        let mut ops = BTreeMap::new();
        ops.insert(
            "get".to_owned(),
            crate::v2::operation::Operation {
                responses: Responses {
                    default: Some(RefOr::new_item(Response {
                        description: "ok".into(),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let item = PathItem {
            reference: Some("#/x".to_owned()),
            operations: Some(ops),
            parameters: None,
            extensions: None,
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        item.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("$ref") && e.contains("MUST NOT coexist")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn path_item_empty_ref_is_flagged() {
        // Empty `$ref` is invalid — it must be a non-empty URI.
        let item = PathItem {
            reference: Some(String::new()),
            ..Default::default()
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        item.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn path_item_serialize_with_parameters() {
        let item = PathItem {
            reference: Some("#/x".to_owned()),
            operations: None,
            parameters: Some(vec![RefOr::new_ref("#/parameters/Foo".to_owned())]),
            extensions: Some({
                let mut m = BTreeMap::new();
                m.insert("x-bar".to_owned(), serde_json::json!("baz"));
                m
            }),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["$ref"], serde_json::json!("#/x"));
        assert_eq!(
            v["parameters"][0]["$ref"],
            serde_json::json!("#/parameters/Foo")
        );
        assert_eq!(v["x-bar"], serde_json::json!("baz"));
    }

    #[test]
    fn path_item_validate_runs_operations_and_parameters() {
        use crate::common::helpers::Context;
        use crate::v2::operation::Operation;
        use crate::v2::response::Responses;
        use crate::v2::spec::Spec;
        use crate::validation::Options;
        let spec = Spec::default();

        // operation with empty Responses (no default + no responses) triggers error
        let mut ops = BTreeMap::new();
        ops.insert(
            "get".to_owned(),
            Operation {
                responses: Responses::default(),
                ..Default::default()
            },
        );
        let item = PathItem {
            operations: Some(ops),
            parameters: Some(vec![RefOr::new_item(Parameter::Path(Box::new(
                InPath::String(crate::v2::parameter::StringParameter {
                    name: "id".into(),
                    required: None,
                    ..Default::default()
                }),
            )))]),
            ..Default::default()
        };

        let mut ctx = Context::new(&spec, Options::new());
        item.validate_with_context(&mut ctx, "p".into());
        // Responses-empty error + must-be-required error.
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must declare at least one response")),
            "errors: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors.iter().any(|e| e.contains("must be required")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn paths_serde_roundtrip_with_extensions() {
        let raw = serde_json::json!({
            "/users": {
                "get": {
                    "responses": {
                        "default": {"description": "ok"}
                    }
                }
            },
            "x-spec-ext": "yes",
        });
        let paths: Paths = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths.extensions.is_some());
        assert!(!paths.is_empty());
        let v = serde_json::to_value(&paths).unwrap();
        assert_eq!(v, raw);
    }

    #[test]
    fn paths_duplicate_path_and_extension_error() {
        let err = serde_json::from_str::<Paths>("{\"/a\": {}, \"/a\": {}}").unwrap_err();
        assert!(err.to_string().contains("duplicate field"), "err: {err}");

        let err = serde_json::from_str::<Paths>("{\"x-a\": \"1\", \"x-a\": \"2\"}").unwrap_err();
        assert!(err.to_string().contains("duplicate field"), "err: {err}");
    }

    #[test]
    fn paths_from_iter() {
        let p = Paths::from(vec![("/a".to_owned(), PathItem::default())]);
        assert_eq!(p.len(), 1);
        let mut iter = p.iter();
        assert!(iter.next().is_some());
    }

    #[test]
    fn path_item_duplicate_field_errors() {
        let err =
            serde_json::from_str::<PathItem>("{\"$ref\": \"#/x\", \"$ref\": \"#/y\"}").unwrap_err();
        assert!(err.to_string().contains("duplicate"), "err: {err}");

        let err = serde_json::from_str::<PathItem>("{\"parameters\": [], \"parameters\": []}")
            .unwrap_err();
        assert!(err.to_string().contains("duplicate"), "err: {err}");

        let err = serde_json::from_str::<PathItem>("{\"x-a\": \"1\", \"x-a\": \"2\"}").unwrap_err();
        assert!(err.to_string().contains("duplicate"), "err: {err}");

        let err = serde_json::from_str::<PathItem>(
            "{\"GET\": {\"responses\":{\"default\":{\"description\":\"ok\"}}}, \"get\": {\"responses\":{\"default\":{\"description\":\"ok\"}}}}",
        )
        .unwrap_err();
        // Duplicate detection lower-cases method names.
        assert!(err.to_string().contains("duplicate"), "err: {err}");
    }
}
