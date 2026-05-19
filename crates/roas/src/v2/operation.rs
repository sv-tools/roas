//! Operation Object

use crate::common::helpers::{validate_required_string, validate_unique_by};
use crate::common::reference::RefOr;
use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::parameter::Parameter;
use crate::v2::response::Responses;
use crate::v2::spec::{Scheme, Spec};
use crate::v2::tag::Tag;
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

    /// ReDoc extension with code samples associated with this operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-codeSamples")]
    pub x_code_samples: Option<Vec<CodeSample>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// ReDoc `x-codeSamples` extension entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct CodeSample {
    /// **Required** Code sample language.
    pub lang: String,

    /// Optional display label for the language tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// **Required** Code sample source code.
    pub source: String,

    /// Allows extensions on the code sample extension object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Operation {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(operation_id) = &self.operation_id
            && !ctx
                .visited
                .insert(format!("#/paths/operations/{operation_id}"))
            && !ctx.is_option(Options::IgnoreNonUniqOperationIDs)
        {
            ctx.error(
                path.clone(),
                format_args!("operationId `{operation_id}` already exists"),
            );
        }
        // OAS 2.0 schema marks `tags`, `consumes`, `produces`, `schemes`, and
        // `security` as `uniqueItems: true`.
        if let Some(tags) = &self.tags {
            validate_unique_by(tags, ctx, format!("{path}.tags"), |t| t.clone());
        }
        if let Some(consumes) = &self.consumes {
            validate_unique_by(consumes, ctx, format!("{path}.consumes"), |s| s.clone());
        }
        if let Some(produces) = &self.produces {
            validate_unique_by(produces, ctx, format!("{path}.produces"), |s| s.clone());
        }
        if let Some(schemes) = &self.schemes {
            validate_unique_by(schemes, ctx, format!("{path}.schemes"), |s| s.clone());
        }
        if let Some(security) = &self.security {
            validate_unique_by(security, ctx, format!("{path}.security"), |r| r.clone());
        }
        if let Some(tags) = &self.tags {
            for (i, tag) in tags.iter().enumerate() {
                validate_required_string(tag, ctx, format!("{path}.tags[{i}]"));
                if tag.is_empty() {
                    continue;
                }

                let reference = format!("#/tags/{tag}");
                if let Ok(spec_tag) = RefOr::<Tag>::new_ref(reference.clone()).get_item(ctx.spec) {
                    if ctx.visit(reference.clone()) {
                        spec_tag.validate_with_context(ctx, reference);
                    }
                } else if !ctx.is_option(Options::IgnoreMissingTags) {
                    ctx.error(
                        path.clone(),
                        format_args!(".tags[{i}]: `{tag}` not found in spec"),
                    );
                }
            }
        }

        // Per-parameter validation. Cross-cutting rules (body/formData
        // exclusivity, (name, in) duplicates, path-template correspondence)
        // run from `Spec::validate` via `crate::v2::validation`.
        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{path}.parameters[{i}]"));
            }
        }

        if let Some(samples) = &self.x_code_samples {
            for (i, sample) in samples.iter().enumerate() {
                sample.validate_with_context(ctx, format!("{path}.x-codeSamples[{i}]"));
            }
        }

        self.responses
            .validate_with_context(ctx, format!("{path}.responses"));
    }
}

impl ValidateWithContext<Spec> for CodeSample {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.lang, ctx, format!("{path}.lang"));
        validate_required_string(&self.source, ctx, format!("{path}.source"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::parameter::{InPath, StringParameter};
    use crate::v2::response::Response;
    use crate::validation::ValidationErrorsExt;

    #[test]
    fn deserialize() {
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
                    RefOr::new_item(Parameter::Path(Box::new(InPath::String(StringParameter {
                        name: "petId".to_owned(),
                        description: Some("ID of pet that needs to be updated".to_owned()),
                        required: Some(true),
                        ..Default::default()
                    })))),
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
                x_code_samples: None,
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
    fn serialize() {
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
                    RefOr::new_item(Parameter::Path(Box::new(InPath::String(StringParameter {
                        name: "petId".to_owned(),
                        description: Some("ID of pet that needs to be updated".to_owned()),
                        required: Some(true),
                        ..Default::default()
                    })))),
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
                x_code_samples: None,
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

    #[test]
    fn x_code_samples_round_trip_and_validate() {
        let value = serde_json::json!({
            "responses": {
                "default": {
                    "description": "ok"
                }
            },
            "x-codeSamples": [
                {
                    "lang": "JavaScript",
                    "label": "Node",
                    "source": "console.log('ok');",
                    "x-extra": "kept"
                }
            ]
        });
        let operation = serde_json::from_value::<Operation>(value.clone()).unwrap();
        assert_eq!(
            operation.x_code_samples,
            Some(vec![CodeSample {
                lang: "JavaScript".to_owned(),
                label: Some("Node".to_owned()),
                source: "console.log('ok');".to_owned(),
                extensions: Some(BTreeMap::from_iter([(
                    "x-extra".to_owned(),
                    serde_json::json!("kept")
                )])),
            }])
        );
        assert_eq!(serde_json::to_value(&operation).unwrap(), value);

        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        operation.validate_with_context(&mut ctx, "operation".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }

    #[test]
    fn empty_tag_name_triggers_continue_in_tag_loop() {
        // Exercises the `if tag.is_empty() { continue }` branch (line 159).
        // An empty tag name is flagged by validate_required_string, and then
        // the loop skips the reference-lookup step for that entry.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        let op = Operation {
            tags: Some(vec![String::new()]), // empty tag → triggers continue
            responses: Responses {
                default: Some(RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            ..Default::default()
        };
        op.validate_with_context(&mut ctx, "op".to_owned());
        // The empty-tag error from validate_required_string must appear.
        assert!(
            ctx.errors.mentions("must not be empty"),
            "expected empty-tag error, got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn unique_items_enforced_on_operation_lists() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        let mut req = BTreeMap::new();
        req.insert("none".to_owned(), vec![]);
        let op = Operation {
            tags: Some(vec!["pet".into(), "pet".into()]),
            consumes: Some(vec!["application/json".into(), "application/json".into()]),
            produces: Some(vec!["text/plain".into(), "text/plain".into()]),
            schemes: Some(vec![Scheme::HTTPS, Scheme::HTTPS]),
            responses: Responses {
                default: Some(RefOr::new_item(crate::v2::response::Response {
                    description: "ok".into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            security: Some(vec![req.clone(), req]),
            ..Default::default()
        };
        op.validate_with_context(&mut ctx, "op".to_owned());
        for field in [
            "op.tags[1]",
            "op.consumes[1]",
            "op.produces[1]",
            "op.schemes[1]",
            "op.security[1]",
        ] {
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains(field) && e.contains("duplicate value")),
                "missing dup error for {field}: {:?}",
                ctx.errors
            );
        }
    }
}
