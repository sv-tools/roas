//! Header Object

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::formats::{CollectionFormat, IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{Context, ValidateWithContext};
use crate::v2::items::Items;
use crate::v2::spec::Spec;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum Header {
    #[serde(rename = "string")]
    String(StringHeader),

    #[serde(rename = "integer")]
    Integer(IntegerHeader),

    #[serde(rename = "number")]
    Number(NumberHeader),

    #[serde(rename = "boolean")]
    Boolean(BooleanHeader),

    #[serde(rename = "array")]
    Array(ArrayHeader),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct StringHeader {
    /// A short description of the header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The extending format for the string type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,

    /// Declares the value of the header that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// The list of strings that defines the possible values of this parameter.
    #[serde(rename = "enum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,

    /// Declares the maximum length of the parameter value.
    #[serde(rename = "maxLength")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,

    /// Declares the minimal length of the parameter value.
    #[serde(rename = "minLength")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,

    /// A regular expression that the parameter value MUST match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct IntegerHeader {
    /// A short description of the header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The extending format for the integer type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<IntegerFormat>,

    /// Declares the value of the header that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<i64>,

    /// The list of strings that defines the possible values of this parameter.
    #[serde(rename = "enum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<i64>>,

    /// Declares the minimum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,

    /// Declares that the value of the parameter is strictly greater than the value of `minimum`
    #[serde(rename = "exclusiveMinimum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<bool>,

    /// Declares the minimum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,

    /// Declares that the value of the parameter is strictly less than the value of `maximum`
    #[serde(rename = "exclusiveMaximum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<bool>,

    /// Declares that the value of the parameter can be restricted to a multiple of a given number
    #[serde(rename = "multipleOf")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct NumberHeader {
    /// A short description of the header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The extending format for the number type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<NumberFormat>,

    /// Declares the value of the header that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,

    /// The list of strings that defines the possible values of this parameter.
    #[serde(rename = "enum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<f64>>,

    /// Declares the minimum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,

    /// Declares that the value of the parameter is strictly greater than the value of `minimum`
    #[serde(rename = "exclusiveMinimum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<bool>,

    /// Declares the minimum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,

    /// Declares that the value of the parameter is strictly less than the value of `maximum`
    #[serde(rename = "exclusiveMaximum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<bool>,

    /// Declares that the value of the parameter can be restricted to a multiple of a given number
    #[serde(rename = "multipleOf")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct BooleanHeader {
    /// A short description of the header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Declares the value of the header that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ArrayHeader {
    /// A short description of the header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// **Required** Describes the type of items in the array.
    pub items: Items,

    /// Declares the values of the header that the server will use if none is provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<serde_json::Value>>,

    /// Determines the format of the array if type array is used.
    #[serde(rename = "collectionFormat")]
    pub collection_format: Option<CollectionFormat>,

    // Declares the maximum number of items that are allowed in the array.
    #[serde(rename = "maxItems")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,

    // Declares the minimum number of items that are allowed in the array.
    #[serde(rename = "minItems")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,

    // Declares the items in the array must be unique.
    #[serde(rename = "uniqueItems")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique_items: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Header {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            Header::String(header) => header.validate_with_context(ctx, path),
            Header::Integer(header) => header.validate_with_context(ctx, path),
            Header::Number(header) => header.validate_with_context(ctx, path),
            Header::Boolean(header) => header.validate_with_context(ctx, path),
            Header::Array(header) => header.validate_with_context(ctx, path),
        }
    }
}

impl ValidateWithContext<Spec> for StringHeader {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for IntegerHeader {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for NumberHeader {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for BooleanHeader {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for ArrayHeader {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        self.items
            .validate_with_context(ctx, format!("{}.items", path));
    }
}

#[cfg(test)]
mod tests {
    use crate::v2::items::StringItem;

    use super::*;

    #[test]
    fn test_header_deserialize() {
        assert_eq!(
            serde_json::from_value::<Header>(serde_json::json!({
                "type": "string",
                "description": "A short description of the header.",
                "format": "byte",
                "default": "default",
                "enum": ["enum"],
                "maxLength": 10,
                "minLength": 1,
                "pattern": "pattern",
                "x-extra": "extension",
            }))
            .unwrap(),
            Header::String(StringHeader {
                description: Some("A short description of the header.".to_owned()),
                format: Some(StringFormat::Byte),
                default: Some("default".to_owned()),
                enum_values: Some(vec!["enum".to_owned()]),
                max_length: Some(10),
                min_length: Some(1),
                pattern: Some("pattern".to_owned()),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }),
            "deserialize string",
        );
        assert_eq!(
            serde_json::from_value::<Header>(serde_json::json!({
                "type": "integer",
                "description": "A short description of the header.",
                "format": "int32",
                "default": 5,
                "enum": [5],
                "maximum": 10,
                "exclusiveMaximum": true,
                "minimum": 1,
                "exclusiveMinimum": true,
                "multipleOf": 1.0,
                "x-extra": "extension",
            }))
            .unwrap(),
            Header::Integer(IntegerHeader {
                description: Some("A short description of the header.".to_owned()),
                format: Some(IntegerFormat::Int32),
                default: Some(5.to_owned()),
                enum_values: Some(vec![5.to_owned()]),
                maximum: Some(10),
                exclusive_maximum: Some(true),
                minimum: Some(1),
                exclusive_minimum: Some(true),
                multiple_of: Some(1.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }),
            "deserialize integer",
        );
        assert_eq!(
            serde_json::from_value::<Header>(serde_json::json!({
                "type": "number",
                "description": "A short description of the header.",
                "format": "double",
                "default": 5.0,
                "enum": [5.0],
                "maximum": 10.0,
                "exclusiveMaximum": true,
                "minimum": 1.0,
                "exclusiveMinimum": true,
                "multipleOf": 1.0,
                "x-extra": "extension",
            }))
            .unwrap(),
            Header::Number(NumberHeader {
                description: Some("A short description of the header.".to_owned()),
                format: Some(NumberFormat::Double),
                default: Some(5.0.to_owned()),
                enum_values: Some(vec![5.0.to_owned()]),
                maximum: Some(10.0),
                exclusive_maximum: Some(true),
                minimum: Some(1.0),
                exclusive_minimum: Some(true),
                multiple_of: Some(1.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }),
            "deserialize number",
        );
        assert_eq!(
            serde_json::from_value::<Header>(serde_json::json!({
                "type": "boolean",
                "description": "A short description of the header.",
                "default": true,
                "x-extra": "extension",
            }))
            .unwrap(),
            Header::Boolean(BooleanHeader {
                description: Some("A short description of the header.".to_owned()),
                default: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }),
            "deserialize boolean",
        );
        assert_eq!(
            serde_json::from_value::<Header>(serde_json::json!({
                "type": "array",
                "items": {
                    "type": "string",
                },
                "description": "A short description of the header.",
                "default": ["default"],
                "collectionFormat": "tsv",
                "maxItems": 10,
                "minItems": 1,
                "uniqueItems": true,
                "x-extra": "extension",
            }))
            .unwrap(),
            Header::Array(ArrayHeader {
                description: Some("A short description of the header.".to_owned()),
                items: Items::String(StringItem::default()),
                default: Some(vec![serde_json::json!("default")]),
                collection_format: Some(CollectionFormat::TSV),
                max_items: Some(10),
                min_items: Some(1),
                unique_items: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }),
            "deserialize array",
        );
    }

    #[test]
    fn test_header_serialize() {
        assert_eq!(
            serde_json::to_value(Header::String(StringHeader {
                description: Some("A short description of the header.".to_owned()),
                format: Some(StringFormat::Byte),
                default: Some("default".to_owned()),
                enum_values: Some(vec!["enum".to_owned()]),
                max_length: Some(10),
                min_length: Some(1),
                pattern: Some("pattern".to_owned()),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "string",
                "description": "A short description of the header.",
                "format": "byte",
                "default": "default",
                "enum": ["enum"],
                "maxLength": 10,
                "minLength": 1,
                "pattern": "pattern",
                "x-extra": "extension",
            }),
            "serialize string",
        );
        assert_eq!(
            serde_json::to_value(Header::Integer(IntegerHeader {
                description: Some("A short description of the header.".to_owned()),
                format: Some(IntegerFormat::Int32),
                default: Some(5.to_owned()),
                enum_values: Some(vec![5.to_owned()]),
                maximum: Some(10),
                exclusive_maximum: Some(true),
                minimum: Some(1),
                exclusive_minimum: Some(true),
                multiple_of: Some(1.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "integer",
                "description": "A short description of the header.",
                "format": "int32",
                "default": 5,
                "enum": [5],
                "maximum": 10,
                "exclusiveMaximum": true,
                "minimum": 1,
                "exclusiveMinimum": true,
                "multipleOf": 1.0,
                "x-extra": "extension",
            }),
            "serialize integer",
        );
        assert_eq!(
            serde_json::to_value(Header::Number(NumberHeader {
                description: Some("A short description of the header.".to_owned()),
                format: Some(NumberFormat::Double),
                default: Some(5.0.to_owned()),
                enum_values: Some(vec![5.0.to_owned()]),
                maximum: Some(10.0),
                exclusive_maximum: Some(true),
                minimum: Some(1.0),
                exclusive_minimum: Some(true),
                multiple_of: Some(1.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "number",
                "description": "A short description of the header.",
                "format": "double",
                "default": 5.0,
                "enum": [5.0],
                "maximum": 10.0,
                "exclusiveMaximum": true,
                "minimum": 1.0,
                "exclusiveMinimum": true,
                "multipleOf": 1.0,
                "x-extra": "extension",
            }),
            "serialize number",
        );
        assert_eq!(
            serde_json::to_value(Header::Boolean(BooleanHeader {
                description: Some("A short description of the header.".to_owned()),
                default: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "boolean",
                "description": "A short description of the header.",
                "default": true,
                "x-extra": "extension",
            }),
            "serialize boolean",
        );
        assert_eq!(
            serde_json::to_value(Header::Array(ArrayHeader {
                description: Some("A short description of the header.".to_owned()),
                items: Items::String(StringItem::default()),
                default: Some(vec![serde_json::json!("default")]),
                collection_format: Some(CollectionFormat::TSV),
                max_items: Some(10),
                min_items: Some(1),
                unique_items: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), "extension".into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "array",
                "items": {
                    "type": "string",
                },
                "description": "A short description of the header.",
                "default": ["default"],
                "collectionFormat": "tsv",
                "maxItems": 10,
                "minItems": 1,
                "uniqueItems": true,
                "x-extra": "extension",
            }),
            "serialize array",
        );
    }
}
