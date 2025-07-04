//! Schema Object

use crate::common::bool_or::BoolOr;
use crate::common::formats::{IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_pattern};
use crate::common::reference::RefOr;
use crate::v3_1::discriminator::Discriminator;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::spec::Spec;
use crate::v3_1::xml::XML;
use monostate::MustBe;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Schema {
    AllOf(Box<AllOfSchema>),
    AnyOf(Box<AnyOfSchema>),
    OneOf(Box<OneOfSchema>),
    Not(Box<NotSchema>),
    Multi(Box<MultiSchema>),
    Single(Box<SingleSchema>), // must be last
}

impl Default for Schema {
    fn default() -> Self {
        Schema::Single(Box::default())
    }
}

impl Schema {
    pub fn new_single_schema(schema: SingleSchema) -> Self {
        Schema::Single(Box::new(schema))
    }

    pub fn new_boxed_single_schema(schema: SingleSchema) -> Box<Self> {
        Box::new(Schema::Single(Box::new(schema)))
    }

    pub fn new_single_schema_ref(schema: SingleSchema) -> RefOr<Self> {
        RefOr::new_item(Schema::Single(Box::new(schema)))
    }

    pub fn new_boxed_single_schema_ref(schema: SingleSchema) -> RefOr<Box<Self>> {
        RefOr::new_item(Box::new(Schema::Single(Box::new(schema))))
    }

    pub fn new_multi_schema(schema: MultiSchema) -> Self {
        Schema::Multi(Box::new(schema))
    }

    pub fn new_boxed_multi_schema(schema: MultiSchema) -> Box<Self> {
        Box::new(Schema::Multi(Box::new(schema)))
    }

    pub fn new_multi_schema_ref(schema: MultiSchema) -> RefOr<Self> {
        RefOr::new_item(Schema::Multi(Box::new(schema)))
    }

    pub fn new_boxed_multi_schema_ref(schema: MultiSchema) -> RefOr<Box<Self>> {
        RefOr::new_item(Box::new(Schema::Multi(Box::new(schema))))
    }

    pub fn new_any_of_schema(schema: AnyOfSchema) -> Self {
        Schema::AnyOf(Box::new(schema))
    }

    pub fn new_boxed_any_of_schema(schema: AnyOfSchema) -> Box<Self> {
        Box::new(Schema::AnyOf(Box::new(schema)))
    }

    pub fn new_any_of_schema_ref(schema: AnyOfSchema) -> RefOr<Self> {
        RefOr::new_item(Schema::AnyOf(Box::new(schema)))
    }

    pub fn new_boxed_any_of_schema_ref(schema: AnyOfSchema) -> RefOr<Box<Self>> {
        RefOr::new_item(Box::new(Schema::AnyOf(Box::new(schema))))
    }

    pub fn new_all_of_schema(schema: AllOfSchema) -> Self {
        Schema::AllOf(Box::new(schema))
    }

    pub fn new_boxed_all_of_schema(schema: AllOfSchema) -> Box<Self> {
        Box::new(Schema::AllOf(Box::new(schema)))
    }

    pub fn new_all_of_schema_ref(schema: AllOfSchema) -> RefOr<Self> {
        RefOr::new_item(Schema::AllOf(Box::new(schema)))
    }

    pub fn new_boxed_all_of_schema_ref(schema: AllOfSchema) -> RefOr<Box<Self>> {
        RefOr::new_item(Box::new(Schema::AllOf(Box::new(schema))))
    }

    pub fn new_one_of_schema(schema: OneOfSchema) -> Self {
        Schema::OneOf(Box::new(schema))
    }

    pub fn new_boxed_one_of_schema(schema: OneOfSchema) -> Box<Self> {
        Box::new(Schema::OneOf(Box::new(schema)))
    }

    pub fn new_one_of_schema_ref(schema: OneOfSchema) -> RefOr<Self> {
        RefOr::new_item(Schema::OneOf(Box::new(schema)))
    }

    pub fn new_boxed_one_of_schema_ref(schema: OneOfSchema) -> RefOr<Box<Self>> {
        RefOr::new_item(Box::new(Schema::OneOf(Box::new(schema))))
    }

    pub fn new_not_schema(schema: NotSchema) -> Self {
        Schema::Not(Box::new(schema))
    }

    pub fn new_boxed_not_schema(schema: NotSchema) -> Box<Self> {
        Box::new(Schema::Not(Box::new(schema)))
    }

    pub fn new_not_schema_ref(schema: NotSchema) -> RefOr<Self> {
        RefOr::new_item(Schema::Not(Box::new(schema)))
    }

    pub fn new_boxed_not_schema_ref(schema: NotSchema) -> RefOr<Box<Self>> {
        RefOr::new_item(Box::new(Schema::Not(Box::new(schema))))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AllOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "allOf")]
    pub all_of: Vec<RefOr<Box<Schema>>>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Adds support for polymorphism.
    /// The discriminator is an object name that is used to differentiate between other schemas
    /// which may satisfy the payload description.
    /// See Composition and Inheritance for more details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Discriminator>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AnyOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "anyOf")]
    pub any_of: Vec<RefOr<Box<Schema>>>,

    /// Adds support for polymorphism.
    /// The discriminator is an object name that is used to differentiate between other schemas
    /// which may satisfy the payload description.
    /// See Composition and Inheritance for more details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Discriminator>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct OneOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "oneOf")]
    pub one_of: Vec<RefOr<Box<Schema>>>,

    /// Adds support for polymorphism.
    /// The discriminator is an object name that is used to differentiate between other schemas
    /// which may satisfy the payload description.
    /// See Composition and Inheritance for more details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Discriminator>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NotSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "allOf")]
    pub not: RefOr<Box<Schema>>,

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SingleSchema {
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

    #[serde(rename = "null")]
    Null(Box<NullSchema>),

    #[serde(rename = "object")]
    Object(Box<ObjectSchema>), // must be last
}

impl Default for SingleSchema {
    fn default() -> Self {
        SingleSchema::Object(Box::default())
    }
}

impl Display for SingleSchema {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SingleSchema::String(_) => write!(f, "string"),
            SingleSchema::Integer(_) => write!(f, "integer"),
            SingleSchema::Number(_) => write!(f, "number"),
            SingleSchema::Boolean(_) => write!(f, "boolean"),
            SingleSchema::Array(_) => write!(f, "array"),
            SingleSchema::Null(_) => write!(f, "null"),
            SingleSchema::Object(_) => write!(f, "object"),
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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
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
    pub items: Option<BoolOr<RefOr<Box<Schema>>>>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ObjectSchema {
    #[serde(rename = "type")]
    pub schema_type: MustBe!("object"),

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The properties (key-value pairs) on an object are defined using the properties keyword.
    /// The value of properties is an object, where each key is the name of a property and each value is
    /// a schema used to validate that property.
    /// Any property that doesn't match any of the property names in the properties keyword is ignored by this keyword.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, RefOr<Box<Schema>>>>,

    /// Sometimes you want to say that, given a particular kind of property name, the value should match a particular schema.
    /// That’s where patternProperties comes in: it maps regular expressions to schemas.
    /// If a property name matches the given regular expression, the property value must validate against the corresponding schema.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#pattern-properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern_properties: Option<BTreeMap<String, RefOr<Box<Schema>>>>,

    /// Declares the values of the header that the server will use if none is provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<BTreeMap<String, serde_json::Value>>,

    /// Declares the maximum number of items that are allowed in the array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<u64>,

    /// Declares the minimum number of items that are allowed in the array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<u64>,

    /// The additionalProperties keyword is used to control the handling of extra stuff, that is,
    /// properties whose names are not listed in the properties keyword or match any of the regular expressions
    /// in the patternProperties keyword.
    /// By default any additional properties are allowed.
    ///
    /// The value of the additionalProperties keyword is a schema that will be used to validate any properties in the instance
    /// that are not matched by properties or patternProperties.
    /// Setting the additionalProperties schema to false means no additional properties will be allowed.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#additional-properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<BoolOr<RefOr<Box<Schema>>>>,

    /// The unevaluatedProperties keyword is similar to additionalProperties except that it can recognize properties declared in subschemas.
    /// So, the example from the previous section can be rewritten without the need to redeclare properties.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#unevaluated-properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unevaluated_properties: Option<BoolOr<RefOr<Box<Schema>>>>,

    /// The names of properties can be validated against a schema, irrespective of their values.
    /// This can be useful if you don’t want to enforce specific properties, but you want to make sure that
    /// the names of those properties follow a specific convention.
    /// You might, for example, want to enforce that all names are valid ASCII tokens so they can be used
    /// as attributes in a particular programming language.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#property-names
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property_names: Option<RefOr<Box<Schema>>>,

    /// A list of required properties.
    /// If the object is defined at the root of the document,
    /// the `required` property MUST be omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// Struct provides a primitive support for a schema with multiple types
///
/// Example:
///
/// ```json
/// { "type": ["number", "string"] }
/// ```
///
/// ```yaml
/// type:
///   - number
///   - string
/// ```
///
/// Fo more details see: https://json-schema.org/understanding-json-schema/reference/type#type-specific-keywords
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MultiSchema {
    #[serde(rename = "type")]
    pub schema_types: Vec<String>,

    /// A title to explain the purpose of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A short description of the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Declares the value of the header that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

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

    /// Additional external documentation for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// A free-form property to include an example of an instance for this schema.
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A fre-form list to include the examples of instances for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Schema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            Schema::Single(s) => s.validate_with_context(ctx, path),
            Schema::Multi(s) => s.validate_with_context(ctx, path),
            Schema::AllOf(s) => {
                for (i, schema) in s.all_of.iter().enumerate() {
                    schema.validate_with_context_boxed(ctx, format!("{path}.allOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::AnyOf(s) => {
                for (i, schema) in s.any_of.iter().enumerate() {
                    schema.validate_with_context_boxed(ctx, format!("{path}.anyOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::OneOf(s) => {
                for (i, schema) in s.one_of.iter().enumerate() {
                    schema.validate_with_context_boxed(ctx, format!("{path}.oneOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::Not(s) => {
                s.not
                    .validate_with_context_boxed(ctx, format!("{path}.not"));
            }
        }
    }
}

impl ValidateWithContext<Spec> for SingleSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            SingleSchema::String(s) => s.validate_with_context(ctx, path),
            SingleSchema::Integer(s) => s.validate_with_context(ctx, path),
            SingleSchema::Number(s) => s.validate_with_context(ctx, path),
            SingleSchema::Boolean(s) => s.validate_with_context(ctx, path),
            SingleSchema::Array(s) => s.validate_with_context(ctx, path),
            SingleSchema::Object(s) => s.validate_with_context(ctx, path),
            SingleSchema::Null(s) => s.validate_with_context(ctx, path),
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

        if let Some(items) = &self.items {
            items.validate_with_context_boxed(ctx, format!("{path}.items"));
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
                schema.validate_with_context_boxed(ctx, format!("{path}.properties.{name}"));
            }
        }

        if let Some(properties) = &self.pattern_properties {
            for (pattern, schema) in properties {
                let path = format!("{path}.pattern_properties[{pattern}]");
                schema.validate_with_context_boxed(ctx, path.clone());
                validate_pattern(pattern, ctx, path);
            }
        }

        if let Some(additional_properties) = &self.additional_properties {
            match additional_properties {
                BoolOr::Bool(_) => {}
                BoolOr::Item(schema) => {
                    schema.validate_with_context_boxed(ctx, format!("{path}.additionalProperties"));
                }
            }
        }

        if let Some(unevaluated_properties) = &self.unevaluated_properties {
            match unevaluated_properties {
                BoolOr::Bool(_) => {}
                BoolOr::Item(schema) => {
                    schema
                        .validate_with_context_boxed(ctx, format!("{path}.unevaluatedProperties"));
                }
            }
        }

        if let Some(property_names) = &self.property_names {
            property_names.validate_with_context_boxed(ctx, format!("{path}.propertyNames"));
        }
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

impl ValidateWithContext<Spec> for MultiSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        let allowed_types: HashSet<String> = HashSet::from_iter(vec![
            "string".into(),
            "number".into(),
            "object".into(),
            "array".into(),
            "boolean".into(),
            "null".into(),
        ]);
        let mut unique_types: HashSet<String> = HashSet::with_capacity(self.schema_types.len());
        self.schema_types.iter().for_each(|t| {
            if !allowed_types.contains(t) {
                ctx.error(
                    format!("{path}.type"),
                    format_args!("type `{t}` is not supported"),
                );
            }
            if !unique_types.insert(t.clone()) {
                ctx.error(
                    format!("{path}.type"),
                    format_args!("type `{t}` is not unique"),
                );
            }
        });
        if let Some(docs) = &self.external_docs {
            docs.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
    }
}

mod extensions {
    use std::collections::BTreeMap;
    use std::fmt;

    use serde::de::{Error, MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serialize, Serializer};

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<BTreeMap<String, serde_json::Value>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ExtensionsVisitor;
        impl<'de> Visitor<'de> for ExtensionsVisitor {
            type Value = BTreeMap<String, serde_json::Value>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("extensions: Option<BTreeMap<String, serde_json::Value>>")
            }

            fn visit_map<V>(
                self,
                mut map: V,
            ) -> Result<BTreeMap<String, serde_json::Value>, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if ext.contains_key(key.as_str()) {
                        return Err(Error::custom(format_args!("duplicate field `{key}`")));
                    }
                    let value: serde_json::Value = map.next_value()?;
                    ext.insert(key, value);
                }
                Ok(ext)
            }
        }

        let map = deserializer.deserialize_map(ExtensionsVisitor)?;
        Ok(if map.is_empty() { None } else { Some(map) })
    }

    pub(super) fn serialize<S>(
        ext: &Option<BTreeMap<String, serde_json::Value>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(ext) = ext {
            let mut map = serializer.serialize_map(Some(ext.len()))?;
            for (k, v) in ext.clone() {
                map.serialize_entry(&k, &v)?;
            }
            map.end()
        } else {
            None::<BTreeMap<String, serde_json::Value>>.serialize(serializer)
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
        if let Schema::Single(o) = &spec {
            if let SingleSchema::String(string) = &**o {
                assert_eq!(string.title, Some("foo".to_owned()));
            } else {
                panic!("expected StringSchema");
            }
        } else {
            panic!("expected Single");
        }
        assert_eq!(
            spec,
            Schema::Single(Box::new(SingleSchema::String(Box::new(StringSchema {
                title: Some("foo".to_owned()),
                ..Default::default()
            })))),
        );
    }

    #[test]
    fn test_single_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::Single(Box::new(SingleSchema::String(Box::new(
                StringSchema {
                    title: Some("foo".to_owned()),
                    ..Default::default()
                }
            )))))
            .unwrap(),
            serde_json::json!({
                "type": "string",
                "title": "foo",
            }),
        );
        assert_eq!(
            serde_json::to_value(Schema::Single(Box::new(SingleSchema::Object(Box::new(
                ObjectSchema {
                    title: Some("foo".to_owned()),
                    required: Some(vec!["bar".to_owned()]),
                    properties: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "bar".to_owned(),
                            RefOr::new_item(Box::new(Schema::Single(Box::new(
                                SingleSchema::String(Box::new(StringSchema {
                                    title: Some("foo bar".to_owned()),
                                    ..Default::default()
                                })),
                            )))),
                        );
                        map
                    }),
                    ..Default::default()
                }
            )))))
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
        }))
        .unwrap();
        if let Schema::AllOf(schema) = &spec {
            assert_eq!(schema.all_of.len(), 2);
            match schema.all_of[0].clone() {
                RefOr::Ref(r) => {
                    assert_eq!(r.reference, "#/definitions/bar".to_owned());
                }
                _ => panic!("expected Ref"),
            }
            match schema.all_of[1].clone() {
                RefOr::Item(o) => {
                    if let Schema::Single(o) = *o {
                        if let SingleSchema::Object(o) = *o {
                            assert_eq!(o.title, Some("foo".to_owned()));
                        } else {
                            panic!("expected ObjectSchema");
                        }
                    } else {
                        panic!("expected Single");
                    }
                }
                _ => panic!("expected Schema"),
            }
        } else {
            panic!("expected AllOf schema, but got {spec:?}");
        }
    }

    #[test]
    fn test_all_of_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::AllOf(Box::new(AllOfSchema {
                all_of: vec![
                    RefOr::new_ref("#/definitions/bar".to_owned()),
                    RefOr::new_item(Box::new(Schema::Single(Box::new(SingleSchema::Object(
                        Box::new(ObjectSchema {
                            title: Some("foo".to_owned()),
                            ..Default::default()
                        })
                    ))))),
                ],
                ..Default::default()
            })))
            .unwrap(),
            serde_json::json!({
                "allOf": [
                    {
                        "$ref": "#/definitions/bar"
                    },
                    {
                        "type": "object",
                        "title": "foo",
                    },
                ],
            }),
        );
    }

    #[test]
    fn test_string_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::String(Box::new(StringSchema {
            title: Some("foo".to_string()),
            format: Some(StringFormat::Custom("custom".to_string())),
            default: Some("d".to_string()),
            enum_values: Some(vec!["a".to_string(), "b".to_string(), "d".to_string()]),
            max_length: Some(1),
            min_length: Some(1),
            examples: Some(vec![serde_json::json!("a"), serde_json::json!("b")]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "string",
            "title": "foo",
            "format": "custom",
            "default": "d",
            "enum": ["a", "b", "d"],
            "maxLength": 1,
            "minLength": 1,
            "examples": ["a", "b"],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_integer_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Integer(Box::new(IntegerSchema {
            title: Some("foo".to_string()),
            format: Some(IntegerFormat::Int32),
            default: Some(42),
            enum_values: Some(vec![1, 42, 105]),
            minimum: Some(1),
            maximum: Some(105),
            examples: Some(vec![serde_json::json!(1), serde_json::json!(42)]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "integer",
            "title": "foo",
            "format": "int32",
            "default": 42,
            "enum": [1, 42, 105],
            "minimum": 1,
            "maximum": 105,
            "examples": [1, 42],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_number_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Number(Box::new(NumberSchema {
            title: Some("foo".to_string()),
            format: Some(NumberFormat::Float),
            default: Some(42.0),
            enum_values: Some(vec![1.0, 42.0, 105.0]),
            minimum: Some(1.0),
            maximum: Some(105.0),
            examples: Some(vec![serde_json::json!(1.0), serde_json::json!(42.0)]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "number",
            "title": "foo",
            "format": "float",
            "default": 42.0,
            "enum": [1.0, 42.0, 105.0],
            "minimum": 1.0,
            "maximum": 105.0,
            "examples": [1.0, 42.0],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_boolean_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Boolean(Box::new(BooleanSchema {
            title: Some("foo".to_string()),
            default: Some(false),
            examples: Some(vec![serde_json::json!(true), serde_json::json!(false)]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "boolean",
            "title": "foo",
            "default": false,
            "examples": [true, false],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_array_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Array(Box::new(ArraySchema {
            title: Some("foo".to_string()),
            items: Some(BoolOr::Item(Schema::new_boxed_single_schema_ref(
                SingleSchema::Integer(Box::new(IntegerSchema {
                    title: Some("bar".into()),
                    ..Default::default()
                })),
            ))),
            default: Some(vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3),
            ]),
            examples: Some(vec![
                serde_json::json!([1, 42, 105]),
                serde_json::json!([0, 25, 43]),
            ]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "array",
            "title": "foo",
            "items": {
                "type": "integer",
                "title": "bar",
            },
            "default": [1, 2, 3],
            "examples": [[1, 42, 105], [0, 25, 43]],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_object_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Object(Box::new(ObjectSchema {
            title: Some("foo".to_string()),
            properties: Some(BTreeMap::from_iter(vec![(
                "bar".into(),
                Schema::new_boxed_single_schema_ref(SingleSchema::Integer(Box::default())),
            )])),
            default: Some(BTreeMap::from_iter(vec![(
                "bar".into(),
                serde_json::json!(42),
            )])),
            examples: Some(vec![
                serde_json::json!({"bar": 42}),
                serde_json::json!({"bar": 105}),
            ]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "object",
            "title": "foo",
            "properties": {
                "bar": {
                    "type": "integer",
                },
            },
            "default": {"bar": 42},
            "examples": [{"bar": 42}, {"bar": 105}],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_null_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Null(Box::new(NullSchema {
            title: Some("foo".to_string()),
            examples: Some(vec![serde_json::json!(null)]),
            ..Default::default()
        }))));
        let value = serde_json::json!({
            "type": "null",
            "title": "foo",
            "examples": [null],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_multi_serialize_deserialize() {
        let spec = Schema::Multi(Box::new(MultiSchema {
            schema_types: vec!["string".into(), "integer".into()],
            title: Some("foo".to_string()),
            examples: Some(vec![serde_json::json!("bar"), serde_json::json!(42)]),
            ..Default::default()
        }));
        let value = serde_json::json!({
            "type": ["string", "integer"],
            "title": "foo",
            "examples": ["bar", 42],
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_one_of_serialize_deserialize() {
        let spec = Schema::OneOf(Box::new(OneOfSchema {
            one_of: vec![
                RefOr::new_ref("#/components/schemas/Cat".into()),
                RefOr::new_ref("#/components/schemas/Dog".into()),
                RefOr::new_ref("#/components/schemas/Lizard".into()),
            ],
            discriminator: Some(Discriminator {
                property_name: "petType".into(),
                ..Default::default()
            }),
            ..Default::default()
        }));
        let value = serde_json::json!({
            "oneOf": [
                {"$ref": "#/components/schemas/Cat"},
                {"$ref": "#/components/schemas/Dog"},
                {"$ref": "#/components/schemas/Lizard"},
            ],
            "discriminator": {
                "propertyName": "petType",
            }
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_any_of_serialize_deserialize() {
        let spec = Schema::AnyOf(Box::new(AnyOfSchema {
            any_of: vec![
                RefOr::new_ref("#/components/schemas/Cat".into()),
                RefOr::new_ref("#/components/schemas/Dog".into()),
                RefOr::new_ref("#/components/schemas/Lizard".into()),
            ],
            discriminator: Some(Discriminator {
                property_name: "petType".into(),
                ..Default::default()
            }),
            ..Default::default()
        }));
        let value = serde_json::json!({
            "anyOf": [
                {"$ref": "#/components/schemas/Cat"},
                {"$ref": "#/components/schemas/Dog"},
                {"$ref": "#/components/schemas/Lizard"},
            ],
            "discriminator": {
                "propertyName": "petType",
            }
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }

    #[test]
    fn test_all_of_serialize_deserialize() {
        let spec = Schema::AllOf(Box::new(AllOfSchema {
            all_of: vec![
                RefOr::new_ref("#/components/schemas/Cat".into()),
                RefOr::new_ref("#/components/schemas/Dog".into()),
                RefOr::new_ref("#/components/schemas/Lizard".into()),
            ],
            discriminator: Some(Discriminator {
                property_name: "petType".into(),
                ..Default::default()
            }),
            ..Default::default()
        }));
        let value = serde_json::json!({
            "allOf": [
                {"$ref": "#/components/schemas/Cat"},
                {"$ref": "#/components/schemas/Dog"},
                {"$ref": "#/components/schemas/Lizard"},
            ],
            "discriminator": {
                "propertyName": "petType",
            }
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), value);
        assert_eq!(serde_json::from_value::<Schema>(value).unwrap(), spec);
    }
}
