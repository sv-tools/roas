//! Header Object

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v3_1::example::Example;
use crate::v3_1::media_type::MediaType;
use crate::v3_1::parameter::InHeaderStyle;
use crate::v3_1::schema::Schema;
use crate::v3_1::spec::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Header {
    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Determines whether this parameter is mandatory.
    /// If the parameter location is "path", this property is **REQUIRED** and its value MUST be `true`.
    /// Otherwise, the property MAY be included and its default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Specifies that a parameter is deprecated and SHOULD be transitioned out of usage.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    /// Describes how the parameter value will be serialized depending on the type of
    /// the parameter value.
    /// Default values is `simple`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<InHeaderStyle>,

    /// When this is `true`, parameter values of type `array` or `object` generate separate parameters
    /// for each value of the array or key-value pair of the map.
    /// For other types of parameters this property has no effect.
    /// When `style` is `form`, the default value is `true`.
    /// For all other styles, the default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,

    /// The schema defining the type used for the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<RefOr<Schema>>,

    /// Example of the parameter’s potential value.
    /// The example SHOULD match the specified schema and encoding properties if present.
    /// The `example` field is mutually exclusive of the `examples` field.
    /// Furthermore, if referencing a `schema` that contains an example,
    /// the `example` value SHALL override the example provided by the schema.
    /// To represent examples of media types that cannot naturally be represented in JSON or YAML,
    /// a string value can contain the example with escaping where necessary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// Examples of the parameter’s potential value.
    /// Each example SHOULD contain a value in the correct format as specified in the parameter encoding.
    /// The `examples` field is mutually exclusive of the `example` field.
    /// Furthermore, if referencing a `schema` that contains an example,
    /// the `examples` value SHALL override the example provided by the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<BTreeMap<String, RefOr<Example>>>,

    /// A map containing the representations for the parameter.
    /// The key is the media type and the value describes it. The map MUST only contain one entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<BTreeMap<String, MediaType>>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Header {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if self.example.is_some() && self.examples.is_some() {
            ctx.error(path.clone(), "example and examples are mutually exclusive");
        }
        if self.schema.is_some() && self.content.is_some() {
            ctx.error(path.clone(), "schema and content are mutually exclusive");
        }
        if let Some(examples) = &self.examples {
            for (k, v) in examples {
                v.validate_with_context(ctx, format!("{path}.examples[{k}]"));
            }
        }
        if let Some(content) = &self.content {
            for (k, v) in content {
                v.validate_with_context(ctx, format!("{path}.content[{k}]"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_deserialize() {
        assert_eq!(
            serde_json::from_value::<Header>(serde_json::json!({
                "description": "A short description of the header.",
                "required": true,
                "deprecated": false,
                "style": "simple",
                "explode": false,
                "x-extra": "extension",
            }))
            .unwrap(),
            Header {
                description: Some("A short description of the header.".to_owned()),
                required: Some(true),
                deprecated: Some(false),
                style: Some(InHeaderStyle::Simple),
                explode: Some(false),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
                ..Default::default()
            },
            "deserialize",
        );
    }

    #[test]
    fn test_header_serialize() {
        assert_eq!(
            serde_json::to_value(Header {
                description: Some("A short description of the header.".to_owned()),
                required: Some(true),
                deprecated: Some(false),
                style: Some(InHeaderStyle::Simple),
                explode: Some(false),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "description": "A short description of the header.",
                "required": true,
                "deprecated": false,
                "style": "simple",
                "explode": false,
                "x-extra": "extension",
            }),
            "serialize string",
        );
    }
}
