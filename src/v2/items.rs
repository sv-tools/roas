//! Item Object

use std::collections::BTreeMap;
use std::ops::Add;

use serde::{Deserialize, Serialize};

use crate::common::formats::{CollectionFormat, IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{Context, ValidateWithContext};
use crate::v2::spec::Spec;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum Items {
    #[serde(rename = "string")]
    String(StringItem),

    #[serde(rename = "integer")]
    Integer(IntegerItem),

    #[serde(rename = "number")]
    Number(NumberItem),

    #[serde(rename = "boolean")]
    Boolean(BooleanItem),

    #[serde(rename = "array")]
    Array(ArrayItem),
}

impl Default for Items {
    fn default() -> Self {
        Items::String(StringItem::default())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct StringItem {
    /// The extending format for the string type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,

    /// Declares the value of the item that the server will use if none is provided.
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
pub struct IntegerItem {
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
pub struct NumberItem {
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
pub struct BooleanItem {
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
pub struct ArrayItem {
    /// **Required** Describes the type of items in the array.
    pub items: Box<Items>,

    /// Declares the values of the item that the server will use if none is provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<serde_json::Value>>,

    /// Determines the format of the array if type array is used.
    #[serde(rename = "collectionFormat")]
    #[serde(skip_serializing_if = "Option::is_none")]
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

impl ValidateWithContext<Spec> for Items {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            Items::String(item) => item.validate_with_context(ctx, path),
            Items::Integer(item) => item.validate_with_context(ctx, path),
            Items::Number(item) => item.validate_with_context(ctx, path),
            Items::Boolean(item) => item.validate_with_context(ctx, path),
            Items::Array(item) => item.validate_with_context(ctx, path),
        }
    }
}

impl ValidateWithContext<Spec> for StringItem {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for IntegerItem {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for NumberItem {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for BooleanItem {
    fn validate_with_context(&self, _ctx: &mut Context<Spec>, _path: String) {}
}

impl ValidateWithContext<Spec> for ArrayItem {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        self.items.validate_with_context(ctx, path.add(".items"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_items_deserialize() {
        assert_eq!(
            serde_json::from_value::<Items>(serde_json::json!({
                "type": "string",
                "format": "byte",
                "default": "default",
                "enum": ["enum1", "enum2"],
                "maxLength": 10,
                "minLength": 1,
                "pattern": "pattern",
                "x-internal-id": 123,
            }))
            .unwrap(),
            Items::String(StringItem {
                format: Some(StringFormat::Byte),
                default: Some(String::from("default")),
                enum_values: Some(vec![String::from("enum1"), String::from("enum2")]),
                max_length: Some(10),
                min_length: Some(1),
                pattern: Some(String::from("pattern")),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_string_items_serialize() {
        assert_eq!(
            serde_json::to_value(&Items::String(StringItem {
                format: Some(StringFormat::Byte),
                default: Some(String::from("default")),
                enum_values: Some(vec![String::from("enum1"), String::from("enum2")]),
                max_length: Some(10),
                min_length: Some(1),
                pattern: Some(String::from("pattern")),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "string",
                "format": "byte",
                "default": "default",
                "enum": ["enum1", "enum2"],
                "maxLength": 10,
                "minLength": 1,
                "pattern": "pattern",
                "x-internal-id": 123,
            }),
            "serialize",
        );
    }

    #[test]
    fn test_integer_items_deserialize() {
        assert_eq!(
            serde_json::from_value::<Items>(serde_json::json!({
                "type": "integer",
                "format": "int64",
                "default": 42,
                "enum": [42, 105],
                "minimum": 1,
                "exclusiveMinimum": true,
                "maximum": 10,
                "exclusiveMaximum": true,
                "multipleOf": 2.0,
                "x-internal-id": 123,
            }))
            .unwrap(),
            Items::Integer(IntegerItem {
                format: Some(IntegerFormat::Int64),
                default: Some(42),
                enum_values: Some(vec![42, 105]),
                minimum: Some(1),
                exclusive_minimum: Some(true),
                maximum: Some(10),
                exclusive_maximum: Some(true),
                multiple_of: Some(2.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_integer_items_serialize() {
        assert_eq!(
            serde_json::to_value(&Items::Integer(IntegerItem {
                format: Some(IntegerFormat::Int64),
                default: Some(42),
                enum_values: Some(vec![42, 105]),
                minimum: Some(1),
                exclusive_minimum: Some(true),
                maximum: Some(10),
                exclusive_maximum: Some(true),
                multiple_of: Some(2.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "integer",
                "format": "int64",
                "default": 42,
                "enum": [42, 105],
                "minimum": 1,
                "exclusiveMinimum": true,
                "maximum": 10,
                "exclusiveMaximum": true,
                "multipleOf": 2.0,
                "x-internal-id": 123,
            }),
            "serialize",
        );
    }

    #[test]
    fn test_number_items_deserialize() {
        assert_eq!(
            serde_json::from_value::<Items>(serde_json::json!({
                "type": "number",
                "format": "double",
                "default": 42.0,
                "enum": [42.0, 105.0],
                "minimum": 1.0,
                "exclusiveMinimum": true,
                "maximum": 10.0,
                "exclusiveMaximum": true,
                "multipleOf": 2.0,
                "x-internal-id": 123,
            }))
            .unwrap(),
            Items::Number(NumberItem {
                format: Some(NumberFormat::Double),
                default: Some(42.0),
                enum_values: Some(vec![42.0, 105.0]),
                minimum: Some(1.0),
                exclusive_minimum: Some(true),
                maximum: Some(10.0),
                exclusive_maximum: Some(true),
                multiple_of: Some(2.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_number_items_serialize() {
        assert_eq!(
            serde_json::to_value(&Items::Number(NumberItem {
                format: Some(NumberFormat::Double),
                default: Some(42.0),
                enum_values: Some(vec![42.0, 105.0]),
                minimum: Some(1.0),
                exclusive_minimum: Some(true),
                maximum: Some(10.0),
                exclusive_maximum: Some(true),
                multiple_of: Some(2.0),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "number",
                "format": "double",
                "default": 42.0,
                "enum": [42.0, 105.0],
                "minimum": 1.0,
                "exclusiveMinimum": true,
                "maximum": 10.0,
                "exclusiveMaximum": true,
                "multipleOf": 2.0,
                "x-internal-id": 123,
            }),
            "serialize",
        );
    }

    #[test]
    fn test_boolean_items_deserialize() {
        assert_eq!(
            serde_json::from_value::<Items>(serde_json::json!({
                "type": "boolean",
                "default": false,
                "x-internal-id": 123,
            }))
            .unwrap(),
            Items::Boolean(BooleanItem {
                default: Some(false),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_boolean_items_serialize() {
        assert_eq!(
            serde_json::to_value(&Items::Boolean(BooleanItem {
                default: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "boolean",
                "default": true,
                "x-internal-id": 123,
            }),
            "serialize",
        );
    }

    #[test]
    fn test_array_items_deserialize() {
        assert_eq!(
            serde_json::from_value::<Items>(serde_json::json!({
                "type": "array",
                "items": {
                    "type": "number",
                    "format": "double",
                },
                "default": [42.0],
                "collectionFormat": "csv",
                "maxItems": 10,
                "minItems": 1,
                "uniqueItems": true,
                "x-internal-id": 123,
            }))
            .unwrap(),
            Items::Array(ArrayItem {
                items: Box::new(Items::Number(NumberItem {
                    format: Some(NumberFormat::Double),
                    ..Default::default()
                })),
                default: Some(vec![serde_json::json!(42.0)]),
                collection_format: Some(CollectionFormat::CSV),
                max_items: Some(10),
                min_items: Some(1),
                unique_items: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }),
            "deserialize",
        );
    }

    #[test]
    fn test_array_items_serialize() {
        assert_eq!(
            serde_json::to_value(&Items::Array(ArrayItem {
                items: Box::new(Items::Number(NumberItem {
                    format: Some(NumberFormat::Double),
                    ..Default::default()
                })),
                default: Some(vec![serde_json::json!(42.0)]),
                collection_format: Some(CollectionFormat::CSV),
                max_items: Some(10),
                min_items: Some(1),
                unique_items: Some(true),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert(String::from("x-internal-id"), 123.into());
                    map
                }),
            }))
            .unwrap(),
            serde_json::json!({
                "type": "array",
                "items": {
                    "type": "number",
                    "format": "double",
                },
                "default": [42.0],
                "collectionFormat": "csv",
                "maxItems": 10,
                "minItems": 1,
                "uniqueItems": true,
                "x-internal-id": 123,
            }),
            "serialize",
        );
    }
}
