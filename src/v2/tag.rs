//! Tag Object

use std::collections::BTreeMap;
use std::ops::Add;

use crate::common::helpers::{validate_required_string, Context, ValidateWithContext};
use serde::{Deserialize, Serialize};

use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::spec::Spec;

/// Allows adding meta data to a single tag that is used by the Operation Object.
/// It is not mandatory to have a Tag Object per tag used there.
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
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown)
    /// syntax can be used for rich text representation.
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
        validate_required_string(&self.name, ctx, path.clone().add(".name"));
        if let Some(doc) = &self.external_docs {
            doc.validate_with_context(ctx, path.add(".externalDocs"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_deserialize() {
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
            "deserialize",
        );
    }

    #[test]
    fn test_tag_serialize() {
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
            "serialize",
        );
    }
}
