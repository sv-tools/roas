//! Tag Object

use crate::common::helpers::{Context, ValidateWithContext, validate_required_string};
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::spec::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Adds metadata to a single tag that is used by the Operation Object.
/// It is not mandatory to have a Tag Object per tag defined in the Operation Object instances.
///
/// Specification Example:
///
/// ```yaml
/// name: pet
/// description: Pets operations
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    /// **Required** The name of the tag.
    pub name: String,

    /// A short description for the tag.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional external documentation for this tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Tag {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        if let Some(doc) = &self.external_docs {
            doc.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize() {
        assert_eq!(
            serde_json::from_value::<Tag>(serde_json::json!({
                "name": "pet",
                "description": "Pets operations",
                "externalDocs": {
                    "description": "Find more info here",
                    "url": "https://example.com/about"
                },
            }))
            .unwrap(),
            Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                external_docs: Some(ExternalDocumentation {
                    description: Some(String::from("Find more info here")),
                    url: String::from("https://example.com/about"),
                    ..Default::default()
                }),
                ..Default::default()
            },
            "deserialize name, description and externalDocs"
        );

        assert_eq!(
            serde_json::from_value::<Tag>(serde_json::json!({
                "name": "pet",
                "description": "Pets operations",
            }))
            .unwrap(),
            Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                ..Default::default()
            },
            "deserialize name and description"
        );

        assert_eq!(
            serde_json::from_value::<Tag>(serde_json::json!({
                "name": "pet",
            }))
            .unwrap(),
            Tag {
                name: String::from("pet"),
                ..Default::default()
            },
            "deserialize name only"
        );
    }

    #[test]
    fn serialize() {
        assert_eq!(
            serde_json::to_value(Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                external_docs: Some(ExternalDocumentation {
                    description: Some(String::from("Find more info here")),
                    url: String::from("https://example.com/about"),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "name": "pet",
                "description":"Pets operations",
                "externalDocs": {
                    "description": "Find more info here",
                    "url": "https://example.com/about"
                },
            }),
            "serialize name, description and externalDocs",
        );

        assert_eq!(
            serde_json::to_value(Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "name": "pet",
                "description":"Pets operations",
            }),
            "serialize name and description",
        );

        assert_eq!(
            serde_json::to_value(Tag {
                name: String::from("pet"),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "name": "pet",
            }),
            "serialize name only",
        );
    }

    #[test]
    fn validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        Tag {
            name: String::from("pet"),
            description: Some(String::from("Pets operations")),
            external_docs: Some(ExternalDocumentation {
                description: Some(String::from("Find more info here")),
                url: String::from("https://example.com/about"),
                ..Default::default()
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Tag {
            name: String::from("pet"),
            description: Some(String::from("Pets operations")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Tag {
            name: String::from("pet"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Tag {
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert_eq!(
            ctx.errors,
            vec!["tag.name: must not be empty"],
            "name error: {:?}",
            ctx.errors
        );
    }
}
