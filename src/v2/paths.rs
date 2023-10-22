//! Path Items

use std::collections::BTreeMap;
use std::ops::Add;

use serde::{Deserialize, Serialize};

use crate::common::reference::RefOr;
use crate::v2::operation::Operation;
use crate::v2::parameter::Parameter;
use crate::v2::spec::Spec;
use crate::validation::{Context, ValidateWithContext};

/// Describes the operations available on a single path.
/// A Path Item may be empty, due to [ACL constraints](https://swagger.io/specification/v2/#security-filtering).
/// The path itself is still exposed to the documentation viewer
/// but they will not know which operations and parameters are available.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct PathItem {
    /// A definition of a GET operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "GET")] // in most cases the methods are in uppercase
    pub get: Option<Operation>,

    /// A definition of a HEAD operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "HEAD")] // in most cases the methods are in uppercase
    pub head: Option<Operation>,

    /// A definition of a POST operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "POST")] // in most cases the methods are in uppercase
    pub post: Option<Operation>,

    /// A definition of a PUT operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "PUT")] // in most cases the methods are in uppercase
    pub put: Option<Operation>,

    /// A definition of a DELETE operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "DELETE")] // in most cases the methods are in uppercase
    pub delete: Option<Operation>,

    /// A definition of a PATCH operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "PATCH")] // in most cases the methods are in uppercase
    pub patch: Option<Operation>,

    /// A definition of a OPTIONS operation on this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "OPTIONS")] // in most cases the methods are in uppercase
    pub options: Option<Operation>,

    /// A list of parameters that are applicable for all the operations described under this path.
    /// These parameters can be overridden at the operation level, but cannot be removed there.
    /// The list MUST NOT include duplicated parameters.
    /// A unique parameter is defined by a combination of a name and location.
    /// The list can use the [Reference Object](crate::common::reference::Ref) to link to parameters
    /// that are defined at the [Swagger Object's](crate::v2::spec::Spec::parameters) parameters.
    /// There can be one "body" parameter at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RefOr<Parameter>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for PathItem {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(o) = &self.get {
            o.validate_with_context(ctx, path.clone().add(".get"));
        }
        if let Some(o) = &self.head {
            o.validate_with_context(ctx, path.clone().add(".head"));
        }
        if let Some(o) = &self.post {
            o.validate_with_context(ctx, path.clone().add(".post"));
        }
        if let Some(o) = &self.put {
            o.validate_with_context(ctx, path.clone().add(".put"));
        }
        if let Some(o) = &self.delete {
            o.validate_with_context(ctx, path.clone().add(".delete"));
        }
        if let Some(o) = &self.patch {
            o.validate_with_context(ctx, path.clone().add(".patch"));
        }
        if let Some(o) = &self.options {
            o.validate_with_context(ctx, path.clone().add(".options"));
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
                get: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                post: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                put: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                head: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                delete: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                patch: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                options: Some(Operation {
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
                        }),),
                        ..Default::default()
                    },
                    ..Default::default()
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
