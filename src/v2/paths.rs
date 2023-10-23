//! Path Items

use std::collections::BTreeMap;
use std::fmt;

use crate::common::helpers::{Context, ValidateWithContext};
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::common::reference::RefOr;
use crate::v2::operation::Operation;
use crate::v2::parameter::Parameter;
use crate::v2::spec::Spec;

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
    /// A definition of the operations on this path.
    ///
    /// Any map items that can be converted to an `Operation` object will be stored here.
    /// This includes `get`, `put`, `post`, `delete`, `options`, `head`, `patch`, `trace`,
    /// and any other custom operations, like SEARCH and etc...
    operations: Option<BTreeMap<String, Operation>>,

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
                    if key == "parameters" {
                        if res.parameters.is_some() {
                            return Err(Error::duplicate_field("parameters"));
                        }
                        res.parameters = Some(map.next_value()?);
                    } else if key.starts_with("x-") {
                        if extensions.contains_key(key.clone().as_str()) {
                            return Err(Error::custom(format!("duplicate field '{}'", key)));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else {
                        let key = key.to_lowercase();
                        if operations.contains_key(key.as_str()) {
                            return Err(Error::custom(format!("duplicate field '{}'", key)));
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
        if let Some(other) = &self.operations {
            for (method, operation) in other.iter() {
                operation.validate_with_context(ctx, format!("{}.{}", path, method));
            }
        }

        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{}.parameters[{}]", path, i));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::common::reference::Ref;
    use crate::v2::formats::CollectionFormat;
    use crate::v2::items::{Items, StringItem};
    use crate::v2::parameter::{ArrayParameter, InPath};
    use crate::v2::response::{Response, Responses};

    use super::*;

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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
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
                                default: Some(RefOr::Item(Response {
                                    description: String::from("error payload"),
                                    schema: Some(RefOr::Ref(Ref {
                                        reference: String::from("#/definitions/ErrorModel"),
                                        ..Default::default()
                                    })),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    );
                    operations
                }),
                parameters: Some(vec![RefOr::Item(Parameter::Path(InPath::Array(
                    ArrayParameter {
                        name: String::from("id"),
                        description: Some(String::from("ID of pet to use")),
                        required: Some(true),
                        items: Items::String(StringItem {
                            ..Default::default()
                        }),
                        collection_format: Some(CollectionFormat::CSV),
                        ..Default::default()
                    }
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
}
