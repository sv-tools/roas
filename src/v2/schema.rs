//! Schema Object

use crate::common::bool_or::BoolOr;
use crate::common::formats::{IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_pattern};
use crate::common::reference::RefOr;
use crate::v2::external_documentation::ExternalDocumentation;
use crate::v2::spec::Spec;
use crate::v2::xml::XML;
use monostate::MustBe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Schema {
    #[serde(rename = "string")]
    String(Box<StringSchema>),

    #[serde(rename = "integer")]
    Integer(Box<IntegerSchema>),

    #[serde(rename = "number")]
    Number(Box<NumberSchema>),

    #[serde(rename = "boolean")]
    Boolean(Box<BooleanSchema>),

    #[serde(rename = "array")]
    Array(Box<ArraySchema>),

    /// `null` type — extra (not in OAS 2.0 / draft-04). Intentionally retained
    /// as a permissive deviation from the v2 spec.
    #[serde(rename = "null")]
    Null(Box<NullSchema>),

    #[serde(rename = "object")]
    Object(Box<ObjectSchema>),

    /// A schema composed of `allOf` other schemas (no parent `type` field).
    /// Per JSON Schema draft-04, `allOf` may appear on any schema; this variant
    /// is the top-level form (parallel to v3.0/v3.1's design). The inner
    /// `ObjectSchema` also keeps its own `all_of` for backward compatibility,
    /// so an `{"allOf": [...], "type": "object"}` input still deserializes as
    /// `Object`. // must be last — typed variants take precedence.
    AllOf(Box<AllOfSchema>),
}

impl Default for Schema {
    fn default() -> Self {
        Schema::Object(Box::default())
    }
}

impl From<StringSchema> for Schema {
    fn from(s: StringSchema) -> Self {
        Schema::String(Box::new(s))
    }
}

impl From<IntegerSchema> for Schema {
    fn from(s: IntegerSchema) -> Self {
        Schema::Integer(Box::new(s))
    }
}

impl From<NumberSchema> for Schema {
    fn from(s: NumberSchema) -> Self {
        Schema::Number(Box::new(s))
    }
}

impl From<BooleanSchema> for Schema {
    fn from(s: BooleanSchema) -> Self {
        Schema::Boolean(Box::new(s))
    }
}

impl From<ArraySchema> for Schema {
    fn from(s: ArraySchema) -> Self {
        Schema::Array(Box::new(s))
    }
}

impl From<NullSchema> for Schema {
    fn from(s: NullSchema) -> Self {
        Schema::Null(Box::new(s))
    }
}

impl From<ObjectSchema> for Schema {
    fn from(s: ObjectSchema) -> Self {
        Schema::Object(Box::new(s))
    }
}

impl From<AllOfSchema> for Schema {
    fn from(s: AllOfSchema) -> Self {
        Schema::AllOf(Box::new(s))
    }
}

/// A schema composed of `allOf` other schemas.
/// Per JSON Schema draft-04, `allOf` may appear on any schema; this is the
/// top-level form.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct AllOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "allOf")]
    pub all_of: Vec<RefOr<Schema>>,

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// Adds support for polymorphism. The discriminator is the schema property
    /// name that is used to differentiate between other schemas which may
    /// satisfy the payload description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Display for Schema {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Schema::String(_) => write!(f, "string"),
            Schema::Integer(_) => write!(f, "integer"),
            Schema::Number(_) => write!(f, "number"),
            Schema::Boolean(_) => write!(f, "boolean"),
            Schema::Array(_) => write!(f, "array"),
            Schema::Object(_) => write!(f, "object"),
            Schema::Null(_) => write!(f, "null"),
            Schema::AllOf(_) => write!(f, "allOf"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct StringSchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("string"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the schema.
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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct IntegerSchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("integer"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
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
    pub minimum: Option<serde_json::Number>,

    /// Declares that the value of the parameter is strictly greater than the value of `minimum`
    #[serde(rename = "exclusiveMinimum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<bool>,

    /// Declares the minimum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<serde_json::Number>,

    /// Declares that the value of the parameter is strictly less than the value of `maximum`
    #[serde(rename = "exclusiveMaximum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<bool>,

    /// Declares that the value of the parameter can be restricted to a multiple of a given number
    #[serde(rename = "multipleOf")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct NumberSchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("number"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct BooleanSchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("boolean"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Declares the value of the header that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ArraySchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("array"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// **Required** Describes the type of items in the array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<RefOr<Schema>>,

    /// Declares the values of the header that the server will use if none is provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<serde_json::Value>>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ObjectSchema {
    /// `type: "object"`. The field is also accepted as **absent** —
    /// per common practice, a Schema with no declared `type` is
    /// treated as an object schema. When missing, serde fills in the
    /// default value, so the parsed value round-trips with an explicit
    /// `type: "object"`.
    #[serde(rename = "type", default)]
    pub schema_type: MustBe!("object"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Describes the properties in the object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, RefOr<Schema>>>,

    /// Declares the default value of the schema. For an object schema, this is
    /// typically a JSON object that conforms to the property definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// Declares the maximum number of items that are allowed in the array.
    #[serde(rename = "maxProperties")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<u64>,

    /// Declares the minimum number of items that are allowed in the array.
    #[serde(rename = "minProperties")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<u64>,

    /// Declares the properties whose names are not listed in the `properties`
    #[serde(rename = "additionalProperties")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<BoolOr<RefOr<Schema>>>,

    /// A list of required properties.
    /// If the object is defined at the root of the document,
    /// the `required` property MUST be omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,

    /// Adds support for polymorphism.
    /// The discriminator is the schema property name that is used to differentiate between
    /// other schema that inherit this schema.
    /// The property name used MUST be defined at this schema and it MUST be in the required
    /// property list.
    /// When used, the value MUST be the name of this schema or any schema that inherits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// ReDoc extension that marks this schema as nullable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-nullable")]
    pub x_nullable: Option<bool>,

    /// Takes an array of object definitions that are validated independently
    /// but together compose a single object
    #[serde(rename = "allOf")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<RefOr<ObjectSchema>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct NullSchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("null"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "read only".
    /// This means that it MAY be sent as part of a response but MUST NOT be sent as part of
    /// the request.
    /// Properties marked as readOnly being true SHOULD NOT be in the required list of
    /// the defined schema.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "readOnly")]
    pub read_only: Option<bool>,

    /// This MAY be used only on properties schemas.
    /// It has no effect on root schemas.
    /// Adds Additional metadata to describe the XML representation format of this property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xml: Option<XML>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Schema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            Schema::String(s) => s.validate_with_context(ctx, path),
            Schema::Integer(s) => s.validate_with_context(ctx, path),
            Schema::Number(s) => s.validate_with_context(ctx, path),
            Schema::Boolean(s) => s.validate_with_context(ctx, path),
            Schema::Array(s) => s.validate_with_context(ctx, path),
            Schema::Object(s) => s.validate_with_context(ctx, path),
            Schema::Null(s) => s.validate_with_context(ctx, path),
            Schema::AllOf(s) => s.validate_with_context(ctx, path),
        }
    }
}

impl ValidateWithContext<Spec> for AllOfSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if self.all_of.is_empty() {
            ctx.error(path.clone(), ".allOf: must not be empty");
        }
        for (i, schema) in self.all_of.iter().enumerate() {
            schema.validate_with_context(ctx, format!("{path}.allOf[{i}]"));
        }
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
    }
}

impl ValidateWithContext<Spec> for StringSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }
        if let Some(pattern) = &self.pattern {
            validate_pattern(pattern, ctx, format!("{path}.pattern"));
        }
    }
}

impl ValidateWithContext<Spec> for IntegerSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }
    }
}

impl ValidateWithContext<Spec> for NumberSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }
    }
}

impl ValidateWithContext<Spec> for BooleanSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }
    }
}

impl ValidateWithContext<Spec> for ArraySchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }

        // Per OAS 2.0: when `type: array`, `items` MUST be present.
        match &self.items {
            Some(items) => items.validate_with_context(ctx, format!("{path}.items")),
            None => ctx.error(path, ".items: is required for `type: array`"),
        }
    }
}

impl ValidateWithContext<Spec> for ObjectSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }

        if let Some(properties) = &self.properties {
            for (name, schema) in properties {
                schema.validate_with_context(ctx, format!("{path}.properties.{name}"));
            }
        }

        if let Some(additional_properties) = &self.additional_properties {
            match additional_properties {
                BoolOr::Bool(_) => {}
                BoolOr::Item(schema) => {
                    schema.validate_with_context(ctx, format!("{path}.additionalProperties"));
                }
            }
        }
        if let Some(all_of) = &self.all_of {
            for (i, schema) in all_of.iter().enumerate() {
                schema.validate_with_context(ctx, format!("{path}.allOf[{i}]"));
            }
        }

        // OAS 2.0: when `discriminator` is set, it MUST name a property
        // defined on this schema, and that property MUST appear in `required`.
        if let Some(disc) = &self.discriminator {
            let in_props = self
                .properties
                .as_ref()
                .is_some_and(|p| p.contains_key(disc));
            if !in_props {
                ctx.error(
                    format!("{path}.discriminator"),
                    format_args!("`{disc}` must be a property defined on this schema"),
                );
            }
            let in_required = self
                .required
                .as_ref()
                .is_some_and(|r| r.iter().any(|n| n == disc));
            if !in_required {
                ctx.error(
                    format!("{path}.discriminator"),
                    format_args!("`{disc}` must be listed in `required`"),
                );
            }
        }

        // OAS 2.0 prose: properties marked `readOnly: true` SHOULD NOT appear
        // in `required`. Surfaced as a validation error (this crate's framework
        // has no separate warning channel) so SHOULD-violations show up in the
        // same `errors` Vec. Follow `$ref`s so a referenced schema's
        // `readOnly` is still caught.
        if let (Some(props), Some(required)) = (&self.properties, &self.required) {
            for name in required {
                let Some(prop) = props.get(name) else {
                    continue;
                };
                let resolved: Option<&Schema> = match prop {
                    RefOr::Item(s) => Some(s),
                    RefOr::Ref(_) => prop.get_item(ctx.spec).ok(),
                };
                if let Some(schema) = resolved
                    && is_schema_read_only(schema)
                {
                    ctx.error(
                        format!("{path}.required"),
                        format_args!("`{name}` is marked `readOnly` and SHOULD NOT be required"),
                    );
                }
            }
        }
    }
}

/// Returns `true` if the schema's `readOnly` flag is set. Used by
/// `ObjectSchema::validate_with_context` to surface the SHOULD-NOT rule
/// that read-only properties not appear in `required`.
fn is_schema_read_only(schema: &Schema) -> bool {
    match schema {
        Schema::String(s) => s.read_only == Some(true),
        Schema::Integer(s) => s.read_only == Some(true),
        Schema::Number(s) => s.read_only == Some(true),
        Schema::Boolean(s) => s.read_only == Some(true),
        Schema::Array(s) => s.read_only == Some(true),
        Schema::Object(s) => s.read_only == Some(true),
        Schema::Null(s) => s.read_only == Some(true),
        Schema::AllOf(_) => false,
    }
}

impl ValidateWithContext<Spec> for NullSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        if let Some(xml) = &self.xml {
            xml.validate_with_context(ctx, format!("{path}.xml"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_deserialize() {
        let spec = serde_json::from_value::<Schema>(serde_json::json!({
            "type": "string",
            "title": "foo",
        }))
        .unwrap();
        if let Schema::String(ref string) = spec {
            assert_eq!(string.title, Some("foo".to_owned()));
        } else {
            panic!("expected StringSchema");
        }
        assert_eq!(
            spec,
            Schema::String(Box::new(StringSchema {
                title: Some("foo".to_owned()),
                ..Default::default()
            })),
        );
    }

    #[test]
    fn test_all_of_deserialize() {
        let spec = serde_json::from_value::<Schema>(serde_json::json!({
            "allOf": [
                {
                    "$ref": "#/definitions/bar"
                },
                {
                    "type": "object",
                    "title": "foo",
                },
            ],
            "type": "object",
        }))
        .unwrap();
        if let Schema::Object(schema) = spec.clone() {
            if let Some(all_of) = schema.all_of {
                assert_eq!(all_of.len(), 2);
                match all_of[0].clone() {
                    RefOr::Ref(r) => {
                        assert_eq!(r.reference, "#/definitions/bar".to_owned());
                    }
                    _ => panic!("expected Ref"),
                }
                match all_of[1].clone() {
                    RefOr::Item(o) => {
                        assert_eq!(o.title, Some("foo".to_owned()));
                    }
                    _ => panic!("expected Schema"),
                }
            } else {
                panic!("expected all_of to be set");
            }
        } else {
            panic!("expected ObjectSchema");
        }
    }

    #[test]
    fn schema_without_type_parses_as_object() {
        // A schema with no `type` field is treated as an object schema
        // (matches Spectral / Stoplight / Redocly tooling behavior). For v2
        // this also affects the relative dispatch order between `Object` and
        // top-level `AllOf`: since `ObjectSchema` itself carries an `allOf`
        // field, a missing-type schema with `allOf` is now routed to
        // `Object` (its data — properties, required, allOf — is preserved).
        let json = serde_json::json!({
            "title": "Untyped",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let parsed: Schema = serde_json::from_value(json).expect("must parse");
        match &parsed {
            Schema::Object(o) => {
                assert_eq!(o.title.as_deref(), Some("Untyped"));
                assert_eq!(o.required.as_deref(), Some(&["name".to_owned()][..]));
                assert!(o.properties.is_some());
            }
            other => panic!("expected Object, got {other:?}"),
        }

        // Bare `{}` parses as the default ObjectSchema.
        let parsed: Schema = serde_json::from_value(serde_json::json!({})).expect("must parse");
        assert!(matches!(parsed, Schema::Object(_)));
    }

    #[test]
    fn schema_typed_string_still_dispatches_correctly_v2() {
        let parsed: Schema =
            serde_json::from_value(serde_json::json!({"type": "string"})).expect("must parse");
        assert!(matches!(parsed, Schema::String(_)));
    }

    #[test]
    fn test_single_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::String(Box::new(StringSchema {
                title: Some("foo".to_owned()),
                ..Default::default()
            })))
            .unwrap(),
            serde_json::json!({
                "type": "string",
                "title": "foo",
            }),
        );
        assert_eq!(
            serde_json::to_value(Schema::Object(Box::new(ObjectSchema {
                title: Some("foo".to_owned()),
                required: Some(vec!["bar".to_owned()]),
                properties: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "bar".to_owned(),
                        RefOr::new_item(Schema::from(StringSchema {
                            title: Some("foo bar".to_owned()),
                            ..Default::default()
                        })),
                    );
                    map
                }),
                ..Default::default()
            })))
            .unwrap(),
            serde_json::json!({
                "type": "object",
                "title": "foo",
                "required": ["bar"],
                "properties": {
                    "bar": {
                        "type": "string",
                        "title": "foo bar",
                    },
                },
            }),
        );
    }

    #[test]
    fn integer_number_boolean_array_null_serde_roundtrip() {
        // Integer
        let raw = serde_json::json!({"type": "integer", "format": "int32"});
        let s: Schema = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(s, Schema::Integer(_)));
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);

        // Number
        let raw = serde_json::json!({"type": "number", "format": "double"});
        let s: Schema = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(s, Schema::Number(_)));
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);

        // Boolean
        let raw = serde_json::json!({"type": "boolean"});
        let s: Schema = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(s, Schema::Boolean(_)));
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);

        // Array
        let raw = serde_json::json!({
            "type": "array",
            "items": {"type": "string"}
        });
        let s: Schema = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(s, Schema::Array(_)));
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);

        // Null
        let raw = serde_json::json!({"type": "null"});
        let s: Schema = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(s, Schema::Null(_)));
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);
    }

    #[test]
    fn schema_validate_each_variant() {
        let spec = Spec::default();

        // String w/ bad pattern
        let s = Schema::String(Box::new(StringSchema {
            pattern: Some("[".into()),
            ..Default::default()
        }));
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("pattern")),
            "errors: {:?}",
            ctx.errors
        );

        // External docs validation paths
        let ed = crate::v2::external_documentation::ExternalDocumentation {
            url: "not-a-url".into(),
            ..Default::default()
        };
        let s = Schema::Integer(Box::new(IntegerSchema {
            external_docs: Some(ed.clone()),
            ..Default::default()
        }));
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must be a valid URL")),
            "errors: {:?}",
            ctx.errors
        );

        // Number, Boolean, Array, Null with externalDocs.
        for s in [
            Schema::Number(Box::new(NumberSchema {
                external_docs: Some(ed.clone()),
                ..Default::default()
            })),
            Schema::Boolean(Box::new(BooleanSchema {
                external_docs: Some(ed.clone()),
                ..Default::default()
            })),
            Schema::Array(Box::new(ArraySchema {
                external_docs: Some(ed.clone()),
                items: Some(RefOr::new_item(Schema::from(StringSchema::default()))),
                ..Default::default()
            })),
            Schema::Null(Box::new(NullSchema {
                external_docs: Some(ed.clone()),
                ..Default::default()
            })),
        ] {
            let mut ctx = Context::new(&spec, crate::validation::Options::new());
            s.validate_with_context(&mut ctx, "p".into());
            assert!(
                ctx.errors.iter().any(|e| e.contains("must be a valid URL")),
                "errors: {:?}",
                ctx.errors
            );
        }

        // Object with properties + additionalProperties + allOf
        let s = Schema::Object(Box::new(ObjectSchema {
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert(
                    "k".into(),
                    RefOr::new_item(Schema::from(StringSchema {
                        pattern: Some("[".into()),
                        ..Default::default()
                    })),
                );
                m
            }),
            additional_properties: Some(crate::common::bool_or::BoolOr::Item(RefOr::new_item(
                Schema::from(StringSchema {
                    pattern: Some("[".into()),
                    ..Default::default()
                }),
            ))),
            all_of: Some(vec![RefOr::new_item(ObjectSchema {
                external_docs: Some(ed.clone()),
                ..Default::default()
            })]),
            ..Default::default()
        }));
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        // Should accumulate errors from each branch
        assert!(ctx.errors.len() >= 2, "errors: {:?}", ctx.errors);

        // additionalProperties = bool, no schema validation needed
        let s = Schema::Object(Box::new(ObjectSchema {
            additional_properties: Some(crate::common::bool_or::BoolOr::Bool(true)),
            ..Default::default()
        }));
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn allof_schema_from_and_validate() {
        let s = Schema::from(AllOfSchema {
            all_of: vec![RefOr::new_item(Schema::from(StringSchema::default()))],
            title: Some("t".into()),
            ..Default::default()
        });
        assert!(matches!(s, Schema::AllOf(_)));
        // Display
        assert_eq!(format!("{s}"), "allOf");

        // Empty allOf produces an error
        let s = Schema::AllOf(Box::default());
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".allOf: must not be empty")),
            "errors: {:?}",
            ctx.errors
        );

        // Non-empty allOf with externalDocs validation propagates
        let ed = crate::v2::external_documentation::ExternalDocumentation {
            url: "not-a-url".into(),
            ..Default::default()
        };
        let s = Schema::AllOf(Box::new(AllOfSchema {
            all_of: vec![RefOr::new_item(Schema::from(StringSchema::default()))],
            external_docs: Some(ed),
            ..Default::default()
        }));
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must be a valid URL")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn allof_schema_serde_roundtrip() {
        let raw = serde_json::json!({
            "allOf": [{"type": "string"}],
            "title": "T",
        });
        let s: AllOfSchema = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(s.title, Some("T".into()));
        assert_eq!(s.all_of.len(), 1);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, raw);
    }

    #[test]
    fn schema_display_formats() {
        assert_eq!(format!("{}", Schema::String(Box::default())), "string");
        assert_eq!(format!("{}", Schema::Integer(Box::default())), "integer");
        assert_eq!(format!("{}", Schema::Number(Box::default())), "number");
        assert_eq!(format!("{}", Schema::Boolean(Box::default())), "boolean");
        assert_eq!(format!("{}", Schema::Array(Box::default())), "array");
        assert_eq!(format!("{}", Schema::Object(Box::default())), "object");
        assert_eq!(format!("{}", Schema::Null(Box::default())), "null");
    }

    #[test]
    fn schema_from_helpers() {
        // Exercise each From impl.
        let _: Schema = StringSchema::default().into();
        let _: Schema = IntegerSchema::default().into();
        let _: Schema = NumberSchema::default().into();
        let _: Schema = BooleanSchema::default().into();
        let _: Schema = ArraySchema::default().into();
        let _: Schema = NullSchema::default().into();
        let _: Schema = ObjectSchema::default().into();
        let _: Schema = AllOfSchema::default().into();
    }

    #[test]
    fn test_all_of_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::Object(Box::new(ObjectSchema {
                all_of: Some(vec![
                    RefOr::new_ref("#/definitions/bar".to_owned()),
                    RefOr::new_item(ObjectSchema {
                        title: Some("foo".to_owned()),
                        ..Default::default()
                    }),
                ]),
                ..Default::default()
            })))
            .unwrap(),
            serde_json::json!({
                "type": "object",
                "allOf": [
                    {
                        "$ref": "#/definitions/bar"
                    },
                    {
                        "title": "foo",
                        "type": "object",
                    },
                ],
            }),
        );
    }

    #[test]
    fn array_schema_requires_items() {
        // Per OAS 2.0: `type: array` MUST be accompanied by `items`.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Array(Box::new(ArraySchema {
            items: None,
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".items: is required for `type: array`")),
            "errors: {:?}",
            ctx.errors
        );

        // With items: valid.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Array(Box::new(ArraySchema {
            items: Some(RefOr::new_item(Schema::from(StringSchema::default()))),
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn discriminator_must_be_property_and_required() {
        let spec = Spec::default();

        // discriminator names a non-existent property.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Object(Box::new(ObjectSchema {
            discriminator: Some("kind".into()),
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert(
                    "name".into(),
                    RefOr::new_item(Schema::from(StringSchema::default())),
                );
                m
            }),
            required: Some(vec!["name".into()]),
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must be a property defined on this schema")),
            "errors: {:?}",
            ctx.errors
        );

        // discriminator names a property that exists but isn't required.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Object(Box::new(ObjectSchema {
            discriminator: Some("kind".into()),
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert(
                    "kind".into(),
                    RefOr::new_item(Schema::from(StringSchema::default())),
                );
                m
            }),
            required: None,
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must be listed in `required`")),
            "errors: {:?}",
            ctx.errors
        );

        // discriminator names a property that is both defined and required: ok.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Object(Box::new(ObjectSchema {
            discriminator: Some("kind".into()),
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert(
                    "kind".into(),
                    RefOr::new_item(Schema::from(StringSchema::default())),
                );
                m
            }),
            required: Some(vec!["kind".into()]),
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("discriminator")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn read_only_property_in_required_warns() {
        // OAS 2.0 prose: properties marked `readOnly` SHOULD NOT appear in `required`.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Object(Box::new(ObjectSchema {
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert(
                    "id".into(),
                    RefOr::new_item(Schema::from(StringSchema {
                        read_only: Some(true),
                        ..Default::default()
                    })),
                );
                m
            }),
            required: Some(vec!["id".into()]),
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("`id`") && e.contains("readOnly") && e.contains("SHOULD NOT")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn read_only_property_via_ref_still_flagged() {
        // The readOnly check follows `$ref` properties through the spec's
        // `#/definitions/...` pool, so a referenced read-only schema in
        // `required` is caught the same as an inline one.
        let mut spec = Spec::default();
        spec.define_schema(
            "Id",
            Schema::from(StringSchema {
                read_only: Some(true),
                ..Default::default()
            }),
        )
        .unwrap();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Object(Box::new(ObjectSchema {
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert("id".into(), RefOr::new_ref("#/definitions/Id".to_owned()));
                m
            }),
            required: Some(vec!["id".into()]),
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("`id`") && e.contains("readOnly") && e.contains("SHOULD NOT")),
            "errors: {:?}",
            ctx.errors
        );

        // Unresolvable refs are silently skipped here — the bad `$ref` is
        // flagged by the regular reference walker, not by this rule.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Schema::Object(Box::new(ObjectSchema {
            properties: Some({
                let mut m = BTreeMap::new();
                m.insert(
                    "id".into(),
                    RefOr::new_ref("#/definitions/Missing".to_owned()),
                );
                m
            }),
            required: Some(vec!["id".into()]),
            ..Default::default()
        }))
        .validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("SHOULD NOT")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn x_nullable_round_trip() {
        let value = serde_json::json!({
            "type": "string",
            "x-nullable": true
        });
        let schema = serde_json::from_value::<Schema>(value.clone()).unwrap();
        match &schema {
            Schema::String(schema) => assert_eq!(schema.x_nullable, Some(true)),
            _ => panic!("expected string schema"),
        }
        assert_eq!(serde_json::to_value(schema).unwrap(), value);

        let value = serde_json::json!({
            "type": "object",
            "x-nullable": true
        });
        let schema = serde_json::from_value::<Schema>(value.clone()).unwrap();
        match &schema {
            Schema::Object(schema) => assert_eq!(schema.x_nullable, Some(true)),
            _ => panic!("expected object schema"),
        }
        assert_eq!(serde_json::to_value(schema).unwrap(), value);
    }
}
