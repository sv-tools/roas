//! Operation Object

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{validate_required_string, Context, ValidateWithContext};
use crate::common::reference::{RefOr, ResolveReference};
use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::parameter::Parameter;
use crate::v2::response::Responses;
use crate::v2::spec::{Scheme, Spec};
use crate::v2::tag::Tag;
use crate::validation::Options;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Operation {
    /// A list of tags for API documentation control.
    /// Tags can be used for logical grouping of operations by resources or any other qualifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    /// A short summary which by default SHOULD override that of the referenced component.
    /// If the referenced object-type does not allow a summary field, then this field has no effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A description which by default SHOULD override that of the referenced component.
    /// CommonMark syntax MAY be used for rich text representation.
    /// If the referenced object-type does not allow a description field, then this field has no effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional external documentation for this operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// Unique string used to identify the operation.
    /// The id MUST be unique among all operations described in the API.
    /// Tools and libraries MAY use the operationId to uniquely identify an operation, therefore,
    /// it is recommended to follow common programming naming conventions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,

    /// A list of MIME types the operation can consume.
    /// This overrides the consumes definition at the Swagger Object.
    /// An empty value MAY be used to clear the global definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumes: Option<Vec<String>>,

    /// A list of MIME types the operation can produce.
    /// This overrides the produces definition at the Swagger Object.
    /// An empty value MAY be used to clear the global definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub produces: Option<Vec<String>>,

    /// A list of parameters that are applicable for this operation.
    /// If a parameter is already defined at the Path Item, the new definition will override it,
    /// but can never remove it.
    /// The list MUST NOT include duplicated parameters.
    /// A unique parameter is defined by a combination of a name and location.
    /// The list can use the Reference Object to link to parameters that are defined at the Swagger Object's parameters.
    /// There can be one "body" parameter at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RefOr<Parameter>>>,

    /// **Required** The list of possible responses as they are returned from executing this operation.
    pub responses: Responses,

    /// The transfer protocol for the operation.
    /// Values MUST be from the list: "http", "https", "ws", "wss".
    /// The value overrides the Swagger Object schemes definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemes: Option<Vec<Scheme>>,

    /// Declares this operation to be deprecated.
    /// Usage of the declared operation should be refrained.
    /// Default value is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    /// A declaration of which security schemes are applied for this operation.
    /// The list of values describes alternative security schemes that can be used
    /// (that is, there is a logical OR between the security requirements).
    /// This definition overrides any declared top-level security.
    /// To remove a top-level security declaration, an empty array can be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<BTreeMap<String, Vec<String>>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Operation {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(operation_id) = &self.operation_id {
            if !ctx
                .visited
                .insert(format!("#/paths/operations/{}", operation_id))
            {
                ctx.errors.push(format!(
                    "{}: operationId `{}` already exists",
                    path, operation_id
                ));
            }
        }
        if let Some(tags) = &self.tags {
            for (i, tag) in tags.iter().enumerate() {
                validate_required_string(tag, ctx, format!("{}.tags[{}]", path, i));
                if tag.is_empty() {
                    continue;
                }
                let spec_tag: Option<&Tag> = ctx
                    .spec
                    .resolve_reference(format!("#/tags/{}", tag).as_str());
                match spec_tag {
                    Some(spec_tag) => {
                        let path = format!("#/tags/{}", tag);
                        if ctx.visited.insert(path.clone()) {
                            spec_tag.validate_with_context(ctx, path);
                        }
                    }
                    None => {
                        if !ctx.options.contains(Options::IgnoreMissingTags) {
                            ctx.errors.push(format!(
                                "{}.tags[{}]: `{}` not found in spec",
                                path.clone(),
                                i,
                                tag
                            ));
                        }
                    }
                }
            }
        }

        if let Some(parameters) = &self.parameters {
            let mut body_count = 0;
            for (i, parameter) in parameters.clone().iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{}.parameters[{}]", path, i));
                if let RefOr::Item(Parameter::Body(_)) = parameter {
                    body_count += 1;
                }
            }
            if body_count > 1 {
                ctx.errors.push(format!(
                    "{}.parameters: only one body parameter allowed, found {}",
                    path, body_count,
                ));
            }
        }

        self.responses
            .validate_with_context(ctx, format!("{}.responses", path));
    }
}

#[cfg(test)]
mod tests {
    use crate::v2::parameter::{InPath, StringParameter};
    use crate::v2::response::Response;

    use super::*;

    #[test]
    fn test_operation_deserialize() {
        assert_eq!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "tags": [
                    "pet"
                ],
                "summary": "Updates a pet in the store with form data",
                "description": "Update Pet with Form",
                "externalDocs": {
                    "description": "find more info here",
                    "url": "https://swagger.io/about"
                },
                "operationId": "updatePetWithForm",
                "consumes": [
                    "application/x-www-form-urlencoded"
                ],
                "produces": [
                    "application/json",
                    "application/xml"
                ],
                "parameters": [
                {
                    "name": "petId",
                    "in": "path",
                    "description": "ID of pet that needs to be updated",
                    "required": true,
                    "type": "string"
                },
                {
                    "$ref": "#/definitions/Pet",
                },
                ],
                "responses": {
                    "200": {
                            "description": "Pet updated."
                    },
                    "405": {
                            "$ref": "#/responses/InvalidInput"
                    },
                    "x-extra": "extra",
                },
                "security": [
                    {
                        "petstore_auth": [
                            "write:pets",
                            "read:pets"
                        ]
                    }
                ],
                "deprecated": true,
                "schemes": [
                    "https"
                ],
                "x-extra": "extra",
            }))
            .unwrap(),
            Operation {
                tags: Some(vec!["pet".to_owned()]),
                summary: Some("Updates a pet in the store with form data".to_owned()),
                description: Some("Update Pet with Form".to_owned()),
                external_docs: Some(ExternalDocumentation {
                    description: Some("find more info here".to_owned()),
                    url: "https://swagger.io/about".to_owned(),
                    ..Default::default()
                }),
                operation_id: Some("updatePetWithForm".to_owned()),
                consumes: Some(vec!["application/x-www-form-urlencoded".to_owned()]),
                produces: Some(vec![
                    "application/json".to_owned(),
                    "application/xml".to_owned(),
                ]),
                parameters: Some(vec![
                    RefOr::new_item(Parameter::Path(InPath::String(StringParameter {
                        name: "petId".to_owned(),
                        description: Some("ID of pet that needs to be updated".to_owned()),
                        required: Some(true),
                        ..Default::default()
                    }))),
                    RefOr::new_ref("#/definitions/Pet".to_owned()),
                ]),
                responses: Responses {
                    responses: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "200".to_owned(),
                            RefOr::new_item(Response {
                                description: "Pet updated.".to_owned(),
                                ..Default::default()
                            }),
                        );
                        map.insert(
                            "405".to_owned(),
                            RefOr::new_ref("#/responses/InvalidInput".to_owned()),
                        );
                        map
                    }),
                    extensions: Some({
                        let mut map = BTreeMap::new();
                        map.insert("x-extra".to_owned(), serde_json::json!("extra"));
                        map
                    }),
                    ..Default::default()
                },
                security: Some(vec![{
                    let mut map = BTreeMap::new();
                    map.insert(
                        "petstore_auth".to_owned(),
                        vec!["write:pets".to_owned(), "read:pets".to_owned()],
                    );
                    map
                }]),
                deprecated: Some(true),
                schemes: Some(vec![Scheme::HTTPS]),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), serde_json::json!("extra"));
                    map
                }),
            },
            "deserialization"
        );
    }

    #[test]
    fn test_operation_serialize() {
        assert_eq!(
            serde_json::to_value(Operation {
                tags: Some(vec!["pet".to_owned()]),
                summary: Some("Updates a pet in the store with form data".to_owned()),
                description: Some("Update Pet with Form".to_owned()),
                external_docs: Some(ExternalDocumentation {
                    description: Some("find more info here".to_owned()),
                    url: "https://swagger.io/about".to_owned(),
                    ..Default::default()
                }),
                operation_id: Some("updatePetWithForm".to_owned()),
                consumes: Some(vec!["application/x-www-form-urlencoded".to_owned()]),
                produces: Some(vec![
                    "application/json".to_owned(),
                    "application/xml".to_owned(),
                ]),
                parameters: Some(vec![
                    RefOr::new_item(Parameter::Path(InPath::String(StringParameter {
                        name: "petId".to_owned(),
                        description: Some("ID of pet that needs to be updated".to_owned()),
                        required: Some(true),
                        ..Default::default()
                    }))),
                    RefOr::new_ref("#/definitions/Pet".to_owned()),
                ]),
                responses: Responses {
                    responses: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "200".to_owned(),
                            RefOr::new_item(Response {
                                description: "Pet updated.".to_owned(),
                                ..Default::default()
                            }),
                        );
                        map.insert(
                            "405".to_owned(),
                            RefOr::new_ref("#/responses/InvalidInput".to_owned()),
                        );
                        map
                    }),
                    extensions: Some({
                        let mut map = BTreeMap::new();
                        map.insert("x-extra".to_owned(), serde_json::json!("extra"));
                        map
                    }),
                    ..Default::default()
                },
                security: Some(vec![{
                    let mut map = BTreeMap::new();
                    map.insert(
                        "petstore_auth".to_owned(),
                        vec!["write:pets".to_owned(), "read:pets".to_owned()],
                    );
                    map
                }]),
                deprecated: Some(true),
                schemes: Some(vec![Scheme::HTTPS]),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), serde_json::json!("extra"));
                    map
                }),
            })
            .unwrap(),
            serde_json::json!({
                "tags": [
                    "pet"
                ],
                "summary": "Updates a pet in the store with form data",
                "description": "Update Pet with Form",
                "externalDocs": {
                    "description": "find more info here",
                    "url": "https://swagger.io/about"
                },
                "operationId": "updatePetWithForm",
                "consumes": [
                    "application/x-www-form-urlencoded"
                ],
                "produces": [
                    "application/json",
                    "application/xml"
                ],
                "parameters": [
                {
                    "name": "petId",
                    "in": "path",
                    "description": "ID of pet that needs to be updated",
                    "required": true,
                    "type": "string"
                },
                {
                    "$ref": "#/definitions/Pet",
                },
                ],
                "responses": {
                    "200": {
                            "description": "Pet updated."
                    },
                    "405": {
                            "$ref": "#/responses/InvalidInput"
                    },
                    "x-extra": "extra",
                },
                "security": [
                    {
                        "petstore_auth": [
                            "write:pets",
                            "read:pets"
                        ]
                    }
                ],
                "deprecated": true,
                "schemes": [
                    "https"
                ],
                "x-extra": "extra",
            }),
            "serialization"
        );
    }
}
