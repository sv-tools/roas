//! Schema Object

use monostate::MustBe;
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::common::bool_or::BoolOr;
use crate::common::formats::{IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::validate_pattern;
use crate::common::reference::RefOr;
use crate::v3_0::discriminator::Discriminator;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::spec::Spec;
use crate::v3_0::xml::XML;
use crate::validation::{Context, PushError, ValidateWithContext};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Schema {
    AllOf(Box<AllOfSchema>),
    AnyOf(Box<AnyOfSchema>),
    OneOf(Box<OneOfSchema>),
    Not(Box<NotSchema>),
    Single(Box<SingleSchema>), // must be last
}

impl Default for Schema {
    fn default() -> Self {
        Schema::Single(Box::default())
    }
}

impl From<SingleSchema> for Schema {
    fn from(s: SingleSchema) -> Self {
        Schema::Single(Box::new(s))
    }
}

impl From<AllOfSchema> for Schema {
    fn from(s: AllOfSchema) -> Self {
        Schema::AllOf(Box::new(s))
    }
}

impl From<AnyOfSchema> for Schema {
    fn from(s: AnyOfSchema) -> Self {
        Schema::AnyOf(Box::new(s))
    }
}

impl From<OneOfSchema> for Schema {
    fn from(s: OneOfSchema) -> Self {
        Schema::OneOf(Box::new(s))
    }
}

impl From<NotSchema> for Schema {
    fn from(s: NotSchema) -> Self {
        Schema::Not(Box::new(s))
    }
}

impl From<StringSchema> for SingleSchema {
    fn from(s: StringSchema) -> Self {
        SingleSchema::String(s)
    }
}

impl From<IntegerSchema> for SingleSchema {
    fn from(s: IntegerSchema) -> Self {
        SingleSchema::Integer(s)
    }
}

impl From<NumberSchema> for SingleSchema {
    fn from(s: NumberSchema) -> Self {
        SingleSchema::Number(s)
    }
}

impl From<BooleanSchema> for SingleSchema {
    fn from(s: BooleanSchema) -> Self {
        SingleSchema::Boolean(s)
    }
}

impl From<ArraySchema> for SingleSchema {
    fn from(s: ArraySchema) -> Self {
        SingleSchema::Array(s)
    }
}

impl From<NullSchema> for SingleSchema {
    fn from(s: NullSchema) -> Self {
        SingleSchema::Null(s)
    }
}

impl From<ObjectSchema> for SingleSchema {
    fn from(s: ObjectSchema) -> Self {
        SingleSchema::Object(s)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AllOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "allOf")]
    pub all_of: Vec<RefOr<Schema>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,

    /// Adds support for polymorphism.
    /// The discriminator is an object name that is used to differentiate between other schemas
    /// which may satisfy the payload description.
    /// See Composition and Inheritance for more details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Discriminator>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AnyOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "anyOf")]
    pub any_of: Vec<RefOr<Schema>>,

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
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct OneOfSchema {
    /// **Required** The list of schemas that this schema is composed of.
    #[serde(rename = "oneOf")]
    pub one_of: Vec<RefOr<Schema>>,

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
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NotSchema {
    /// **Required** The schema that this schema must not match.
    pub not: RefOr<Schema>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SingleSchema {
    #[serde(rename = "string")]
    String(StringSchema),

    #[serde(rename = "integer")]
    Integer(IntegerSchema),

    #[serde(rename = "number")]
    Number(NumberSchema),

    #[serde(rename = "boolean")]
    Boolean(BooleanSchema),

    #[serde(rename = "array")]
    Array(ArraySchema),

    #[serde(rename = "null")]
    Null(NullSchema),

    #[serde(rename = "object")]
    Object(ObjectSchema),
}

impl Default for SingleSchema {
    fn default() -> Self {
        SingleSchema::Object(ObjectSchema::default())
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
            SingleSchema::Object(_) => write!(f, "object"),
            SingleSchema::Null(_) => write!(f, "null"),
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

    /// Documentation/codegen extension with descriptions for enum values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumDescriptions")]
    pub x_enum_descriptions: Option<Vec<String>>,

    /// Codegen extension with Rust/Java-style enum variant names.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enum-varnames")]
    pub x_enum_varnames: Option<Vec<String>>,

    /// Codegen extension with enum member names used by several generators.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumNames")]
    pub x_enum_names: Option<Vec<String>>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

    /// Documentation/codegen extension with descriptions for enum values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumDescriptions")]
    pub x_enum_descriptions: Option<Vec<String>>,

    /// Codegen extension with Rust/Java-style enum variant names.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enum-varnames")]
    pub x_enum_varnames: Option<Vec<String>>,

    /// Codegen extension with enum member names used by several generators.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumNames")]
    pub x_enum_names: Option<Vec<String>>,

    /// Inclusive lower bound for the value.
    /// Per JSON Schema draft-04 §5.1.3, this keyword is any number even when
    /// the parent schema's `type` is `"integer"`. Stored as
    /// [`serde_json::Number`] so integer-shaped values like `100` round-trip
    /// without becoming `100.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<serde_json::Number>,

    /// If `true`, the value of the parameter is strictly greater than the
    /// value of `minimum` (the draft-04 boolean modifier form, retained in
    /// OpenAPI 3.0).
    #[serde(rename = "exclusiveMinimum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<bool>,

    /// Inclusive upper bound for the value.
    /// See [`Self::minimum`] for the rationale on the type.
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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

    /// Documentation/codegen extension with descriptions for enum values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumDescriptions")]
    pub x_enum_descriptions: Option<Vec<String>>,

    /// Codegen extension with Rust/Java-style enum variant names.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enum-varnames")]
    pub x_enum_varnames: Option<Vec<String>>,

    /// Codegen extension with enum member names used by several generators.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumNames")]
    pub x_enum_names: Option<Vec<String>>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ObjectSchema {
    /// `type: "object"`. The field is also accepted as **absent** —
    /// per common practice (Spectral, Stoplight, Redocly), a Schema
    /// with no declared `type` is treated as an object schema. When
    /// missing, serde fills in the default value via `MustBe::default()`,
    /// so the parsed value round-trips with an explicit `type: "object"`.
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

    /// Declares the values of the header that the server will use if none is provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<BTreeMap<String, serde_json::Value>>,

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

    /// ReDoc/Redocly extension with a display name for additional properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-additionalPropertiesName")]
    pub x_additional_properties_name: Option<String>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only", meaning it MAY be sent as part
    /// of a request but MUST NOT be sent as part of a response.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Allows the value to be `null` in addition to its declared type.
    /// OpenAPI 3.0-only — 3.1 uses the `type` array form (e.g.
    /// `type: ["<type>", "null"]`) instead. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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
            Schema::Single(s) => s.validate_with_context(ctx, path),
            Schema::AllOf(s) => {
                for (i, schema) in s.all_of.iter().enumerate() {
                    schema.validate_with_context(ctx, format!("{path}.allOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::AnyOf(s) => {
                for (i, schema) in s.any_of.iter().enumerate() {
                    schema.validate_with_context(ctx, format!("{path}.anyOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::OneOf(s) => {
                for (i, schema) in s.one_of.iter().enumerate() {
                    schema.validate_with_context(ctx, format!("{path}.oneOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::Not(s) => {
                s.not.validate_with_context(ctx, format!("{path}.not"));
            }
        }
    }
}

impl ValidateWithContext<Spec> for SingleSchema {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        // OAS 3.0 §4.7.21: `readOnly` and `writeOnly` MUST NOT both be true.
        // Centralised here so every variant's dispatch goes through it.
        let (read_only, write_only) = match self {
            SingleSchema::String(s) => (s.read_only, s.write_only),
            SingleSchema::Integer(s) => (s.read_only, s.write_only),
            SingleSchema::Number(s) => (s.read_only, s.write_only),
            SingleSchema::Boolean(s) => (s.read_only, s.write_only),
            SingleSchema::Array(s) => (s.read_only, s.write_only),
            SingleSchema::Object(s) => (s.read_only, s.write_only),
            SingleSchema::Null(s) => (s.read_only, s.write_only),
        };
        if read_only == Some(true) && write_only == Some(true) {
            ctx.error(
                path.clone(),
                ".readOnly and .writeOnly are mutually exclusive",
            );
        }

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

        if let Some(items) = &self.items {
            items.validate_with_context(ctx, format!("{path}.items"));
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
    use crate::validation::ValidationErrorsExt;

    #[test]
    fn test_single_deserialize() {
        let spec = serde_json::from_value::<Schema>(serde_json::json!({
            "type": "string",
            "title": "foo",
        }))
        .unwrap();
        if let Schema::Single(val) = &spec {
            if let SingleSchema::String(string) = &**val {
                assert_eq!(string.title, Some("foo".to_owned()));
            } else {
                panic!("expected StringSchema");
            }
        } else {
            panic!("expected Schema::Single");
        }
        assert_eq!(
            spec,
            Schema::Single(Box::new(SingleSchema::String(StringSchema {
                title: Some("foo".to_owned()),
                ..Default::default()
            }))),
        );
    }

    #[test]
    fn schema_without_type_parses_as_object() {
        // A schema with no `type` field is treated as an object schema
        // (matches Spectral / Stoplight / Redocly tooling behavior). The
        // deserialized value is `Schema::Single(Object(...))`; presence of
        // `properties` / `required` round-trips through the object form.
        let json = serde_json::json!({
            "title": "Untyped",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let parsed: Schema = serde_json::from_value(json).expect("must parse");
        match &parsed {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Object(o) => {
                    assert_eq!(o.title.as_deref(), Some("Untyped"));
                    assert_eq!(o.required.as_deref(), Some(&["name".to_owned()][..]));
                    assert!(o.properties.is_some());
                }
                other => panic!("expected Object, got {other:?}"),
            },
            _ => panic!("expected Schema::Single"),
        }

        // Bare `{}` also parses (as the default ObjectSchema).
        let parsed: Schema = serde_json::from_value(serde_json::json!({})).expect("must parse");
        assert!(matches!(
            parsed,
            Schema::Single(ref s) if matches!(s.as_ref(), SingleSchema::Object(_))
        ));
    }

    #[test]
    fn schema_typed_string_still_dispatches_correctly() {
        // Sanity: making `type` optional on ObjectSchema must NOT route
        // typed schemas to the object variant.
        let parsed: Schema =
            serde_json::from_value(serde_json::json!({"type": "string"})).expect("must parse");
        match parsed {
            Schema::Single(s) => match *s {
                SingleSchema::String(_) => {}
                other => panic!("expected String, got {other:?}"),
            },
            _ => panic!("expected Schema::Single"),
        }
    }

    #[test]
    fn test_single_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::Single(Box::new(SingleSchema::String(
                StringSchema {
                    title: Some("foo".to_owned()),
                    ..Default::default()
                }
            ))))
            .unwrap(),
            serde_json::json!({
                "type": "string",
                "title": "foo",
            }),
        );
        assert_eq!(
            serde_json::to_value(Schema::Single(Box::new(SingleSchema::Object(
                ObjectSchema {
                    title: Some("foo".to_owned()),
                    required: Some(vec!["bar".to_owned()]),
                    properties: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "bar".to_owned(),
                            RefOr::new_item(Schema::from(SingleSchema::from(StringSchema {
                                title: Some("foo bar".to_owned()),
                                ..Default::default()
                            }))),
                        );
                        map
                    }),
                    ..Default::default()
                }
            ))))
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
                    if let Schema::Single(o) = o {
                        if let SingleSchema::Object(o) = *o {
                            assert_eq!(o.title, Some("foo".to_owned()));
                        } else {
                            panic!("expected SingleSchema::Object");
                        }
                    } else {
                        panic!("expected Schema::Single");
                    }
                }
                _ => panic!("expected Schema"),
            }
        } else {
            panic!("expected AllOf schema");
        }
    }

    #[test]
    fn test_all_of_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::from(AllOfSchema {
                all_of: vec![
                    RefOr::new_ref("#/definitions/bar".to_owned()),
                    RefOr::new_item(Schema::from(SingleSchema::from(ObjectSchema {
                        title: Some("foo".to_owned()),
                        ..Default::default()
                    }))),
                ],
                ..Default::default()
            }))
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
    fn test_integer_bounds_round_trip() {
        // OpenAPI 3.0 / JSON Schema draft-04: minimum and maximum on
        // `IntegerSchema` are numbers, not integers. Integer-shaped JSON
        // input must round-trip as integers (real-world specs like
        // `tests/v3_0_data/petstore.json` write `"maximum": 100`),
        // and fractional values must parse.
        let json = serde_json::json!({
            "type": "integer",
            "minimum": 0,
            "maximum": 100,
        });
        let parsed: Schema = serde_json::from_value(json.clone()).expect("must parse");
        match &parsed {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Integer(int) => {
                    assert_eq!(int.minimum.as_ref().unwrap().as_i64(), Some(0));
                    assert_eq!(int.maximum.as_ref().unwrap().as_i64(), Some(100));
                }
                _ => panic!("expected Integer schema"),
            },
            _ => panic!("expected Single schema"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        let json = serde_json::json!({
            "type": "integer",
            "minimum": 0.5,
            "maximum": 99.5,
        });
        let parsed: Schema = serde_json::from_value(json.clone()).expect("must parse");
        match &parsed {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Integer(int) => {
                    assert_eq!(int.minimum.as_ref().unwrap().as_f64(), Some(0.5));
                    assert_eq!(int.maximum.as_ref().unwrap().as_f64(), Some(99.5));
                }
                _ => panic!("expected Integer schema"),
            },
            _ => panic!("expected Single schema"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn read_only_write_only_mutex_each_variant() {
        // Spec: a Schema MUST NOT have both readOnly and writeOnly set to
        // true. Build one of each `SingleSchema` variant with both flags
        // and run them through the dispatch path; each should produce the
        // error.
        fn case<T: Into<SingleSchema>>(s: T) -> Schema {
            Schema::from(s.into())
        }
        let schemas: Vec<(String, Schema)> = vec![
            (
                "string".into(),
                case(StringSchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
            (
                "integer".into(),
                case(IntegerSchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
            (
                "number".into(),
                case(NumberSchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
            (
                "boolean".into(),
                case(BooleanSchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
            (
                "array".into(),
                case(ArraySchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
            (
                "object".into(),
                case(ObjectSchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
            (
                "null".into(),
                case(NullSchema {
                    read_only: Some(true),
                    write_only: Some(true),
                    ..Default::default()
                }),
            ),
        ];
        let spec = Spec::default();
        for (name, schema) in &schemas {
            let mut ctx = Context::new(&spec, crate::validation::Options::new());
            schema.validate_with_context(&mut ctx, format!("s.{name}"));
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains(".readOnly and .writeOnly are mutually exclusive")),
                "variant `{name}` should reject readOnly+writeOnly: errors {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn read_only_xor_write_only_individually_ok() {
        // Only one of readOnly / writeOnly is fine.
        let only_read = Schema::from(SingleSchema::from(StringSchema {
            read_only: Some(true),
            ..Default::default()
        }));
        let only_write = Schema::from(SingleSchema::from(StringSchema {
            write_only: Some(true),
            ..Default::default()
        }));
        let spec = Spec::default();
        for s in [only_read, only_write] {
            let mut ctx = Context::new(&spec, crate::validation::Options::new());
            s.validate_with_context(&mut ctx, "s".into());
            assert!(
                !ctx.errors.mentions("mutually exclusive"),
                "single flag should not error: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn from_conversions_each_variant() {
        let _: Schema = SingleSchema::from(StringSchema::default()).into();
        let _: Schema = SingleSchema::from(IntegerSchema::default()).into();
        let _: Schema = SingleSchema::from(NumberSchema::default()).into();
        let _: Schema = SingleSchema::from(BooleanSchema::default()).into();
        let _: Schema = SingleSchema::from(ArraySchema::default()).into();
        let _: Schema = SingleSchema::from(ObjectSchema::default()).into();
        let _: Schema = SingleSchema::from(NullSchema::default()).into();

        let _: Schema = AllOfSchema::default().into();
        let _: Schema = AnyOfSchema::default().into();
        let _: Schema = OneOfSchema::default().into();
        let _: Schema = NotSchema {
            not: RefOr::new_item(Schema::default()),
            extensions: None,
        }
        .into();

        let s = Schema::default();
        // Default is the empty Object schema.
        if let Schema::Single(inner) = s {
            assert!(matches!(*inner, SingleSchema::Object(_)));
        } else {
            panic!("default should be Single");
        }
    }

    #[test]
    fn validate_dispatches_to_each_single_variant() {
        // Each variant validator runs at least one optional walk (xml /
        // externalDocs); seed each with a malformed XML namespace to surface
        // the dispatch path.
        let bad_xml = || crate::v3_0::xml::XML {
            namespace: Some("not-a-url".into()),
            ..Default::default()
        };
        let cases: Vec<Schema> = vec![
            Schema::from(SingleSchema::from(StringSchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(IntegerSchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(NumberSchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(BooleanSchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(ArraySchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(ObjectSchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(NullSchema {
                xml: Some(bad_xml()),
                ..Default::default()
            })),
        ];
        let spec = Spec::default();
        for (i, schema) in cases.iter().enumerate() {
            let mut ctx = Context::new(&spec, crate::validation::Options::new());
            schema.validate_with_context(&mut ctx, format!("c[{i}]"));
            assert!(
                ctx.errors.mentions("namespace"),
                "case {i}: errors: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn validate_walks_external_docs_per_variant() {
        // ExternalDocumentation requires a non-empty URL — picks up empty.
        let bad_docs = || crate::v3_0::external_documentation::ExternalDocumentation {
            url: "".into(),
            description: None,
            extensions: None,
        };
        let cases: Vec<Schema> = vec![
            Schema::from(SingleSchema::from(StringSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(IntegerSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(NumberSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(BooleanSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(ArraySchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(ObjectSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
            Schema::from(SingleSchema::from(NullSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            })),
        ];
        let spec = Spec::default();
        for (i, schema) in cases.iter().enumerate() {
            let mut ctx = Context::new(&spec, crate::validation::Options::new());
            schema.validate_with_context(&mut ctx, format!("c[{i}]"));
            assert!(
                ctx.errors.mentions("externalDocs"),
                "case {i}: errors: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn object_validate_walks_properties() {
        let json = serde_json::json!({
            "type": "object",
            "properties": {
                "bad": {"type": "string", "pattern": "["}
            }
        });
        let s: Schema = serde_json::from_value(json).unwrap();
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "obj".into());
        assert!(
            ctx.errors.mentions("obj.properties.bad"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn null_boolean_number_schemas_round_trip() {
        let json = serde_json::json!({"type": "null", "title": "n"});
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        let json = serde_json::json!({"type": "boolean", "default": true});
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        let json = serde_json::json!({
            "type": "number",
            "minimum": 0.5,
            "maximum": 99.5,
            "exclusiveMinimum": true,
            "exclusiveMaximum": false,
            "multipleOf": 0.5,
            "enum": [1.0, 2.5]
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn one_of_any_of_not_round_trip_and_validate() {
        let json = serde_json::json!({
            "oneOf": [{"type": "string"}, {"$ref": "#/components/schemas/X"}],
            "discriminator": {"propertyName": "kind"}
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(parsed, Schema::OneOf(_)));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        let json = serde_json::json!({
            "anyOf": [{"type": "string"}, {"type": "integer"}]
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(parsed, Schema::AnyOf(_)));

        let json = serde_json::json!({"not": {"type": "string"}});
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(parsed, Schema::Not(_)));

        // Validate dispatches into composition arms.
        let spec = Spec::default();
        let composition: Schema = serde_json::from_value(serde_json::json!({
            "allOf": [{"type": "string", "pattern": "["}],
            "discriminator": {"propertyName": ""}
        }))
        .unwrap();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        composition.validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors.mentions("pattern"),
            "expected nested string pattern error: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors.mentions("propertyName"),
            "expected discriminator empty propertyName: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn additional_properties_bool_and_schema_round_trip() {
        let json = serde_json::json!({
            "type": "object",
            "additionalProperties": false
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        let json = serde_json::json!({
            "type": "object",
            "additionalProperties": {"type": "string", "pattern": "["}
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        // Validate walks into additionalProperties Item form to surface nested
        // pattern errors.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        parsed.validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("additionalProperties")),
            "expected additionalProperties walk error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn nullable_writeonly_deprecated_round_trip() {
        let json = serde_json::json!({
            "type": "string",
            "nullable": true,
            "writeOnly": true,
            "deprecated": true,
            "readOnly": false
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn array_items_validate_walks() {
        let json = serde_json::json!({
            "type": "array",
            "items": {"type": "string", "pattern": "["}
        });
        let parsed: Schema = serde_json::from_value(json).unwrap();
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        parsed.validate_with_context(&mut ctx, "arr".into());
        assert!(
            ctx.errors.mentions("arr.items.pattern"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn schema_display_impl() {
        assert_eq!(
            SingleSchema::String(StringSchema::default()).to_string(),
            "string"
        );
        assert_eq!(
            SingleSchema::Integer(IntegerSchema::default()).to_string(),
            "integer"
        );
        assert_eq!(
            SingleSchema::Number(NumberSchema::default()).to_string(),
            "number"
        );
        assert_eq!(
            SingleSchema::Boolean(BooleanSchema::default()).to_string(),
            "boolean"
        );
        assert_eq!(
            SingleSchema::Array(ArraySchema::default()).to_string(),
            "array"
        );
        assert_eq!(
            SingleSchema::Object(ObjectSchema::default()).to_string(),
            "object"
        );
        assert_eq!(
            SingleSchema::Null(NullSchema::default()).to_string(),
            "null"
        );
    }

    #[test]
    fn common_extension_fields_round_trip() {
        let json = serde_json::json!({
            "type": "string",
            "enum": ["open", "closed"],
            "x-enumDescriptions": ["Open state", "Closed state"],
            "x-enum-varnames": ["Open", "Closed"],
            "x-enumNames": ["OPEN", "CLOSED"],
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        let json = serde_json::json!({
            "type": "object",
            "additionalProperties": {
                "type": "string"
            },
            "x-additionalPropertiesName": "metadata",
        });
        let parsed: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }
}
