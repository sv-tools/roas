//! Schema Object

use crate::common::bool_or::BoolOr;
use crate::common::formats::{IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{
    Context, PushError, ValidateWithContext, validate_pattern, validate_required_string,
};
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
    /// JSON Schema 2020-12 boolean schema: `true` matches anything,
    /// `false` matches nothing. Must be first so a bare boolean JSON value
    /// dispatches here before the typed variants try (and fail) to parse it
    /// as an object.
    Bool(bool),
    AllOf(Box<AllOfSchema>),
    AnyOf(Box<AnyOfSchema>),
    OneOf(Box<OneOfSchema>),
    Not(Box<NotSchema>),
    Multi(Box<MultiSchema>),
    /// The literal empty schema `{}`. Per JSON Schema 2020-12 this is
    /// semantically equivalent to `true` ("matches anything") but
    /// preserves the `{}` JSON representation.
    ///
    /// Must come before [`Schema::Single`]: `SingleSchema::Object`
    /// uses `MustBe!("object")` with a `default`, so the typed
    /// variant happily matches a bare `{}`. Putting `Empty` first
    /// captures the empty-object idiom while leaving anything with at
    /// least one field (even just `description`) to fall through to
    /// `Single::Object`.
    Empty(EmptySchema),
    Single(Box<SingleSchema>), // must be last
}

/// Marker for the empty schema `{}`. Round-trips to `{}` and rejects
/// any non-empty object on deserialization, so a schema with even one
/// field stays a typed `Single` / composition variant.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EmptySchema;

impl Serialize for EmptySchema {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let map: BTreeMap<String, ()> = BTreeMap::new();
        map.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for EmptySchema {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let map: BTreeMap<String, serde::de::IgnoredAny> = BTreeMap::deserialize(de)?;
        if !map.is_empty() {
            return Err(serde::de::Error::custom(
                "expected empty schema object `{}`",
            ));
        }
        Ok(EmptySchema)
    }
}

impl From<EmptySchema> for Schema {
    fn from(s: EmptySchema) -> Self {
        Schema::Empty(s)
    }
}

impl From<bool> for Schema {
    fn from(b: bool) -> Self {
        Schema::Bool(b)
    }
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

impl From<MultiSchema> for Schema {
    fn from(s: MultiSchema) -> Self {
        Schema::Multi(Box::new(s))
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

    /// A free-form list to include the examples of instances for this schema.
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
    pub any_of: Vec<RefOr<Schema>>,

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

    /// A free-form list to include the examples of instances for this schema.
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
    pub one_of: Vec<RefOr<Schema>>,

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

    /// A free-form list to include the examples of instances for this schema.
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
    /// **Required** The schema that this schema must not match.
    pub not: RefOr<Schema>,

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

    /// A free-form list to include the examples of instances for this schema.
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

// SingleSchema is always heap-allocated via `Schema::Single(Box<SingleSchema>)`,
// so the size variance between variants is intentional and harmless.
#[allow(clippy::large_enum_variant)]
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
    Object(ObjectSchema), // must be last
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

    /// Documentation/codegen extension with descriptions for enum values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumDescriptions")]
    pub x_enum_descriptions: Option<Vec<String>>,

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
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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

    /// Documentation/codegen extension with descriptions for enum values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumDescriptions")]
    pub x_enum_descriptions: Option<Vec<String>>,

    /// Inclusive lower bound for the value.
    /// Per JSON Schema 2020-12 §6.2.4, this keyword is any number even when
    /// the parent schema's `type` is `"integer"`, so a fractional bound such
    /// as `0.5` is valid (and constrains the integer instance to `>= 1`).
    /// Stored as [`serde_json::Number`] so integer-shaped values like `100`
    /// round-trip without becoming `100.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<serde_json::Number>,

    /// Strict (exclusive) lower bound for the value.
    /// Per JSON Schema 2020-12 §6.2.5, the keyword value is any number — not
    /// the boolean modifier from JSON Schema draft-04 / OAS 3.0.
    #[serde(rename = "exclusiveMinimum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<serde_json::Number>,

    /// Inclusive upper bound for the value.
    /// See [`Self::minimum`] for the rationale on the type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<serde_json::Number>,

    /// Strict (exclusive) upper bound for the value.
    /// Per JSON Schema 2020-12 §6.2.3, the keyword value is any number — not
    /// the boolean modifier from JSON Schema draft-04 / OAS 3.0.
    #[serde(rename = "exclusiveMaximum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<serde_json::Number>,

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
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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

    /// Documentation/codegen extension with descriptions for enum values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-enumDescriptions")]
    pub x_enum_descriptions: Option<Vec<String>>,

    /// Declares the minimum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,

    /// Declares that the value of the parameter is strictly greater than this value.
    /// In OpenAPI 3.1 / JSON Schema 2020-12 this is a numeric bound, not a boolean modifier.
    #[serde(rename = "exclusiveMinimum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<f64>,

    /// Declares the maximum value of the parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,

    /// Declares that the value of the parameter is strictly less than this value.
    /// In OpenAPI 3.1 / JSON Schema 2020-12 this is a numeric bound, not a boolean modifier.
    #[serde(rename = "exclusiveMaximum")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<f64>,

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
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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
    pub items: Option<BoolOr<RefOr<Schema>>>,

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
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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

    /// The properties (key-value pairs) on an object are defined using the properties keyword.
    /// The value of properties is an object, where each key is the name of a property and each value is
    /// a schema used to validate that property.
    /// Any property that doesn't match any of the property names in the properties keyword is ignored by this keyword.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, RefOr<Schema>>>,

    /// Sometimes you want to say that, given a particular kind of property name, the value should match a particular schema.
    /// That’s where patternProperties comes in: it maps regular expressions to schemas.
    /// If a property name matches the given regular expression, the property value must validate against the corresponding schema.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#pattern-properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern_properties: Option<BTreeMap<String, RefOr<Schema>>>,

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
    pub additional_properties: Option<BoolOr<RefOr<Schema>>>,

    /// The unevaluatedProperties keyword is similar to additionalProperties except that it can recognize properties declared in subschemas.
    /// So, the example from the previous section can be rewritten without the need to redeclare properties.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#unevaluated-properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unevaluated_properties: Option<BoolOr<RefOr<Schema>>>,

    /// The names of properties can be validated against a schema, irrespective of their values.
    /// This can be useful if you don’t want to enforce specific properties, but you want to make sure that
    /// the names of those properties follow a specific convention.
    /// You might, for example, want to enforce that all names are valid ASCII tokens so they can be used
    /// as attributes in a particular programming language.
    ///
    /// https://json-schema.org/understanding-json-schema/reference/object.html#property-names
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property_names: Option<RefOr<Schema>>,

    /// Codegen/documentation extension with tags associated with this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-tags")]
    pub x_tags: Option<Vec<String>>,

    /// Codegen extension overriding the discriminator value for this schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-discriminator-value")]
    pub x_discriminator_value: Option<String>,

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
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

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
    ///
    /// Deprecated: The example property has been deprecated in favor of the JSON Schema examples keyword.
    /// Use of example is discouraged, and later versions of this specification may remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// A free-form list to include the examples of instances for this schema.
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

    /// Relevant only for Schema "properties" definitions.
    /// Declares the property as "write only". Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "writeOnly")]
    pub write_only: Option<bool>,

    /// Specifies that the schema is deprecated and SHOULD be transitioned out
    /// of usage. Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

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

    /// A free-form list to include the examples of instances for this schema.
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
            // A boolean schema (true / false) has no fields to validate.
            Schema::Bool(_) => {}
            // The empty schema `{}` is the unconstrained schema — nothing to validate.
            Schema::Empty(_) => {}
            Schema::Single(s) => s.validate_with_context(ctx, path),
            Schema::Multi(s) => s.validate_with_context(ctx, path),
            Schema::AllOf(s) => {
                // JSON Schema 2020-12 §10.2.1.1: `allOf` MUST be a non-empty
                // array. The same MUST applies to `anyOf` (§10.2.1.2) and
                // `oneOf` (§10.2.1.3).
                if s.all_of.is_empty() {
                    ctx.error(path.clone(), "`allOf` must be a non-empty array");
                }
                for (i, schema) in s.all_of.iter().enumerate() {
                    schema.validate_with_context(ctx, format!("{path}.allOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::AnyOf(s) => {
                if s.any_of.is_empty() {
                    ctx.error(path.clone(), "`anyOf` must be a non-empty array");
                }
                for (i, schema) in s.any_of.iter().enumerate() {
                    schema.validate_with_context(ctx, format!("{path}.anyOf[{i}]"));
                }
                if let Some(discriminator) = &s.discriminator {
                    discriminator.validate_with_context(ctx, format!("{path}.discriminator"));
                }
            }
            Schema::OneOf(s) => {
                if s.one_of.is_empty() {
                    ctx.error(path.clone(), "`oneOf` must be a non-empty array");
                }
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
        // Spec: `readOnly` and `writeOnly` MUST NOT both be true on the same
        // schema. Centralised here so every variant goes through it.
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

fn validate_enum_descriptions_len(
    enum_len: Option<usize>,
    descriptions: Option<&Vec<String>>,
    ctx: &mut Context<Spec>,
    path: &str,
) {
    if let (Some(enum_len), Some(descriptions)) = (enum_len, descriptions)
        && descriptions.len() != enum_len
    {
        ctx.error(
            format!("{path}.x-enumDescriptions"),
            format_args!(
                "must contain exactly one description per enum value ({enum_len} expected, {} found)",
                descriptions.len()
            ),
        );
    }
}

fn validate_extension_tags(tags: &Option<Vec<String>>, ctx: &mut Context<Spec>, path: &str) {
    if let Some(tags) = tags {
        for (i, tag) in tags.iter().enumerate() {
            validate_required_string(tag, ctx, format!("{path}.x-tags[{i}]"));
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
        // Spec: minLength MUST be ≤ maxLength when both are present.
        if let (Some(min), Some(max)) = (self.min_length, self.max_length)
            && min > max
        {
            ctx.error(
                path.clone(),
                format_args!("`minLength` ({min}) must be ≤ `maxLength` ({max})"),
            );
        }
        validate_enum_descriptions_len(
            self.enum_values.as_ref().map(Vec::len),
            self.x_enum_descriptions.as_ref(),
            ctx,
            &path,
        );
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
        // Spec: multipleOf MUST be > 0 (JSON Schema 2020-12 §6.2.1).
        if let Some(m) = self.multiple_of
            && m <= 0.0
        {
            ctx.error(path.clone(), format_args!("`multipleOf` ({m}) must be > 0"));
        }
        validate_enum_descriptions_len(
            self.enum_values.as_ref().map(Vec::len),
            self.x_enum_descriptions.as_ref(),
            ctx,
            &path,
        );
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
        if let Some(m) = self.multiple_of
            && m <= 0.0
        {
            ctx.error(path.clone(), format_args!("`multipleOf` ({m}) must be > 0"));
        }
        validate_enum_descriptions_len(
            self.enum_values.as_ref().map(Vec::len),
            self.x_enum_descriptions.as_ref(),
            ctx,
            &path,
        );
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

        // Spec: minItems MUST be ≤ maxItems when both are present.
        if let (Some(min), Some(max)) = (self.min_items, self.max_items)
            && min > max
        {
            ctx.error(
                path.clone(),
                format_args!("`minItems` ({min}) must be ≤ `maxItems` ({max})"),
            );
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

        // Spec: minProperties MUST be ≤ maxProperties when both are present.
        if let (Some(min), Some(max)) = (self.min_properties, self.max_properties)
            && min > max
        {
            ctx.error(
                path.clone(),
                format_args!("`minProperties` ({min}) must be ≤ `maxProperties` ({max})"),
            );
        }

        if let Some(properties) = &self.properties {
            for (name, schema) in properties {
                schema.validate_with_context(ctx, format!("{path}.properties.{name}"));
            }
        }

        if let Some(properties) = &self.pattern_properties {
            for (pattern, schema) in properties {
                let path = format!("{path}.pattern_properties[{pattern}]");
                schema.validate_with_context(ctx, path.clone());
                validate_pattern(pattern, ctx, path);
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

        if let Some(unevaluated_properties) = &self.unevaluated_properties {
            match unevaluated_properties {
                BoolOr::Bool(_) => {}
                BoolOr::Item(schema) => {
                    schema.validate_with_context(ctx, format!("{path}.unevaluatedProperties"));
                }
            }
        }

        if let Some(property_names) = &self.property_names {
            property_names.validate_with_context(ctx, format!("{path}.propertyNames"));
        }
        validate_extension_tags(&self.x_tags, ctx, &path);
        if let Some(value) = &self.x_discriminator_value {
            validate_required_string(value, ctx, format!("{path}.x-discriminator-value"));
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
        // JSON Schema 2020-12 §6.1.1: when `type` is an array it MUST contain
        // at least one element. An empty `type: []` is not a valid schema.
        if self.schema_types.is_empty() {
            ctx.error(format!("{path}.type"), "must contain at least one element");
        }
        let allowed_types: HashSet<String> = HashSet::from_iter(vec![
            "string".into(),
            "number".into(),
            "integer".into(),
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
            Schema::Single(Box::new(SingleSchema::String(StringSchema {
                title: Some("foo".to_owned()),
                ..Default::default()
            }))),
        );
    }

    #[test]
    fn schema_without_type_parses_as_object() {
        // A schema with no `type` field is treated as an object schema in v3.1
        // (matches Spectral / Stoplight / Redocly tooling). Note: v3.1 has
        // both `SingleSchema` and `MultiSchema`; missing-type must hit Single
        // (Object) — not be misidentified as a MultiSchema.
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
            _ => panic!("expected Schema::Single, got {parsed:?}"),
        }

        // A literal `{}` now matches `Schema::Empty` — see the
        // `empty_schema_*` tests below for full coverage. Anything
        // with at least one field (even without `type`) still
        // dispatches to `Single::Object` because that variant has a
        // default `schema_type`.
        let parsed: Schema = serde_json::from_value(serde_json::json!({})).expect("must parse");
        assert_eq!(parsed, Schema::Empty(EmptySchema));
    }

    #[test]
    fn schema_typed_string_still_dispatches_correctly_v31() {
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
    fn schema_with_type_array_still_routes_to_multi() {
        // v3.1's MultiSchema (type as array) MUST keep priority over the
        // missing-type-as-object fallback for a doc that explicitly lists
        // a type array.
        let parsed: Schema =
            serde_json::from_value(serde_json::json!({"type": ["string", "null"]}))
                .expect("must parse");
        assert!(
            matches!(parsed, Schema::Multi(_)),
            "expected Schema::Multi, got {parsed:?}"
        );
    }

    #[test]
    fn test_single_serialize() {
        assert_eq!(
            serde_json::to_value(Schema::from(SingleSchema::from(StringSchema {
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
            serde_json::to_value(Schema::from(SingleSchema::from(ObjectSchema {
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
    fn test_string_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::String(StringSchema {
            title: Some("foo".to_string()),
            format: Some(StringFormat::Custom("custom".to_string())),
            default: Some("d".to_string()),
            enum_values: Some(vec!["a".to_string(), "b".to_string(), "d".to_string()]),
            max_length: Some(1),
            min_length: Some(1),
            examples: Some(vec![serde_json::json!("a"), serde_json::json!("b")]),
            ..Default::default()
        })));
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
        let spec = Schema::Single(Box::new(SingleSchema::Integer(IntegerSchema {
            title: Some("foo".to_string()),
            format: Some(IntegerFormat::Int32),
            default: Some(42),
            enum_values: Some(vec![1, 42, 105]),
            minimum: Some(1.into()),
            maximum: Some(105.into()),
            examples: Some(vec![serde_json::json!(1), serde_json::json!(42)]),
            ..Default::default()
        })));
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
    fn test_numeric_bounds_are_numbers() {
        // OpenAPI 3.1 / JSON Schema 2020-12: minimum, maximum, exclusiveMinimum,
        // and exclusiveMaximum are numbers — not booleans (the draft-04 form),
        // and not constrained to integers when the parent type is "integer"
        // (the keyword value is independent of the instance constraint).

        // a) Integer-shaped JSON numbers (e.g. `0`, `100`) parse and
        //    round-trip *without* becoming floats. This is required for
        //    real-world OpenAPI documents (e.g. petstore.json) that write
        //    `"maximum": 100` on an integer schema.
        let json = serde_json::json!({
            "type": "integer",
            "exclusiveMinimum": 0,
            "exclusiveMaximum": 100,
        });
        let parsed: Schema = serde_json::from_value(json.clone()).expect("must parse");
        match &parsed {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Integer(int) => {
                    assert_eq!(int.exclusive_minimum.as_ref().unwrap().as_i64(), Some(0));
                    assert_eq!(int.exclusive_maximum.as_ref().unwrap().as_i64(), Some(100));
                }
                _ => panic!("expected Integer schema"),
            },
            _ => panic!("expected Single schema"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        // b) Fractional bounds on `type: integer` MUST parse — JSON Schema
        //    permits this; `minimum: 0.5` on an integer simply means
        //    "instance is integer AND >= 0.5", i.e. >= 1.
        let json = serde_json::json!({
            "type": "integer",
            "minimum": 0.5,
            "maximum": 99.5,
            "exclusiveMinimum": 0.5,
            "exclusiveMaximum": 99.5,
        });
        let parsed: Schema = serde_json::from_value(json.clone()).expect("must parse");
        match &parsed {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Integer(int) => {
                    assert_eq!(int.minimum.as_ref().unwrap().as_f64(), Some(0.5));
                    assert_eq!(int.maximum.as_ref().unwrap().as_f64(), Some(99.5));
                    assert_eq!(int.exclusive_minimum.as_ref().unwrap().as_f64(), Some(0.5));
                    assert_eq!(int.exclusive_maximum.as_ref().unwrap().as_f64(), Some(99.5));
                }
                _ => panic!("expected Integer schema"),
            },
            _ => panic!("expected Single schema"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);

        // c) Number schema accepts fractional bounds (sanity check).
        let json = serde_json::json!({
            "type": "number",
            "exclusiveMinimum": 0.5,
            "exclusiveMaximum": 1.5,
        });
        let parsed: Schema = serde_json::from_value(json.clone()).expect("must parse");
        match &parsed {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Number(n) => {
                    assert_eq!(n.exclusive_minimum, Some(0.5));
                    assert_eq!(n.exclusive_maximum, Some(1.5));
                }
                _ => panic!("expected Number schema"),
            },
            _ => panic!("expected Single schema"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn test_number_serialize_deserialize() {
        let spec = Schema::Single(Box::new(SingleSchema::Number(NumberSchema {
            title: Some("foo".to_string()),
            format: Some(NumberFormat::Float),
            default: Some(42.0),
            enum_values: Some(vec![1.0, 42.0, 105.0]),
            minimum: Some(1.0),
            maximum: Some(105.0),
            examples: Some(vec![serde_json::json!(1.0), serde_json::json!(42.0)]),
            ..Default::default()
        })));
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
        let spec = Schema::Single(Box::new(SingleSchema::Boolean(BooleanSchema {
            title: Some("foo".to_string()),
            default: Some(false),
            examples: Some(vec![serde_json::json!(true), serde_json::json!(false)]),
            ..Default::default()
        })));
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
        let spec = Schema::from(SingleSchema::from(ArraySchema {
            title: Some("foo".to_string()),
            items: Some(BoolOr::Item(RefOr::new_item(Schema::from(
                SingleSchema::from(IntegerSchema {
                    title: Some("bar".into()),
                    ..Default::default()
                }),
            )))),
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
        }));
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
        let spec = Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
            title: Some("foo".to_string()),
            properties: Some(BTreeMap::from_iter(vec![(
                "bar".into(),
                RefOr::new_item(Schema::from(
                    SingleSchema::Integer(IntegerSchema::default()),
                )),
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
        })));
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
        let spec = Schema::Single(Box::new(SingleSchema::Null(NullSchema {
            title: Some("foo".to_string()),
            examples: Some(vec![serde_json::json!(null)]),
            ..Default::default()
        })));
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

        // Round-trip alone is not enough: validate that all primitive type
        // names accepted by the deserializer are also accepted by the validator.
        let spec_owner = Spec::default();
        let mut ctx = Context::new(&spec_owner, crate::validation::Options::empty());
        let multi = MultiSchema {
            schema_types: vec![
                "string".into(),
                "integer".into(),
                "number".into(),
                "boolean".into(),
                "object".into(),
                "array".into(),
                "null".into(),
            ],
            ..Default::default()
        };
        multi.validate_with_context(&mut ctx, "schema".into());
        assert!(
            ctx.errors.is_empty(),
            "all primitive types should be valid: {:?}",
            ctx.errors
        );
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

    #[test]
    fn boolean_schema_true_and_false_round_trip() {
        // JSON Schema 2020-12 boolean schemas.
        let t: Schema = serde_json::from_value(serde_json::json!(true)).unwrap();
        assert!(matches!(t, Schema::Bool(true)));
        assert_eq!(serde_json::to_value(&t).unwrap(), serde_json::json!(true));

        let f: Schema = serde_json::from_value(serde_json::json!(false)).unwrap();
        assert!(matches!(f, Schema::Bool(false)));
        assert_eq!(serde_json::to_value(&f).unwrap(), serde_json::json!(false));

        // Validate is a no-op on Bool.
        let spec = crate::v3_1::spec::Spec::default();
        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        Schema::Bool(true).validate_with_context(&mut ctx, "s".into());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn schema_from_bool_helper() {
        let _: Schema = true.into();
        let _: Schema = false.into();
    }

    #[test]
    fn from_conversions_each_variant() {
        // Cover the From impls + Default impls that are otherwise only
        // exercised through serde dispatch.
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
            external_docs: None,
            example: None,
            examples: None,
            extensions: None,
        }
        .into();
        let _: Schema = MultiSchema::default().into();

        // Defaults
        assert!(matches!(Schema::default(), Schema::Single(_)));
        assert!(matches!(SingleSchema::default(), SingleSchema::Object(_)));
    }

    #[test]
    fn single_schema_display_each_variant() {
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
    fn composition_validate_dispatches_with_discriminator() {
        // Each composition variant's validate dispatch + discriminator walk
        // (lines 1085-1107).
        let spec = crate::v3_1::spec::Spec::default();

        let bad_disc = || crate::v3_1::discriminator::Discriminator::default();
        for s in [
            Schema::AllOf(Box::new(AllOfSchema {
                all_of: vec![],
                discriminator: Some(bad_disc()),
                ..Default::default()
            })),
            Schema::AnyOf(Box::new(AnyOfSchema {
                any_of: vec![],
                discriminator: Some(bad_disc()),
                ..Default::default()
            })),
            Schema::OneOf(Box::new(OneOfSchema {
                one_of: vec![],
                discriminator: Some(bad_disc()),
                ..Default::default()
            })),
        ] {
            let mut ctx =
                crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
            s.validate_with_context(&mut ctx, "s".into());
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains("propertyName") && e.contains("must not be empty")),
                "expected discriminator empty-propertyName error: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn boolean_and_null_variants_validate() {
        // BooleanSchema and NullSchema have no consistency rules to fire,
        // but the dispatch path still needs to walk them — exercised via
        // an external_docs URL coming back invalid.
        let spec = crate::v3_1::spec::Spec::default();
        let bad_docs = || crate::v3_1::external_documentation::ExternalDocumentation {
            url: "".into(),
            description: None,
            extensions: None,
        };
        for s in [
            Schema::Single(Box::new(SingleSchema::Boolean(BooleanSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            }))),
            Schema::Single(Box::new(SingleSchema::Null(NullSchema {
                external_docs: Some(bad_docs()),
                ..Default::default()
            }))),
        ] {
            let mut ctx =
                crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
            s.validate_with_context(&mut ctx, "s".into());
            assert!(
                ctx.errors.iter().any(|e| e.contains("externalDocs.url")),
                "expected externalDocs walk: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn keyword_consistency_violations_reported() {
        let spec = crate::v3_1::spec::Spec::default();

        let cases: Vec<(&str, Schema, &str)> = vec![
            (
                "string min/max",
                Schema::Single(Box::new(SingleSchema::String(StringSchema {
                    min_length: Some(10),
                    max_length: Some(5),
                    ..Default::default()
                }))),
                "minLength",
            ),
            (
                "integer multipleOf <= 0",
                Schema::Single(Box::new(SingleSchema::Integer(IntegerSchema {
                    multiple_of: Some(0.0),
                    ..Default::default()
                }))),
                "multipleOf",
            ),
            (
                "number multipleOf < 0",
                Schema::Single(Box::new(SingleSchema::Number(NumberSchema {
                    multiple_of: Some(-1.0),
                    ..Default::default()
                }))),
                "multipleOf",
            ),
            (
                "array min/max items",
                Schema::Single(Box::new(SingleSchema::Array(ArraySchema {
                    min_items: Some(10),
                    max_items: Some(5),
                    ..Default::default()
                }))),
                "minItems",
            ),
            (
                "object min/max properties",
                Schema::Single(Box::new(SingleSchema::Object(ObjectSchema {
                    min_properties: Some(10),
                    max_properties: Some(5),
                    ..Default::default()
                }))),
                "minProperties",
            ),
        ];
        for (label, schema, needle) in cases {
            let mut ctx =
                crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
            schema.validate_with_context(&mut ctx, "s".into());
            assert!(
                ctx.errors.iter().any(|e| e.contains(needle)),
                "case `{label}`: expected `{needle}` error: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn read_only_write_only_mutex() {
        // OAS spec rule (also a JSON Schema interaction): both flags
        // MUST NOT be true on the same schema. Centralised in
        // SingleSchema dispatch.
        let json = serde_json::json!({
            "type": "string",
            "readOnly": true,
            "writeOnly": true,
        });
        let s: Schema = serde_json::from_value(json).unwrap();
        let spec = crate::v3_1::spec::Spec::default();
        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        s.validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("readOnly and .writeOnly are mutually exclusive")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn composition_arrays_must_be_non_empty() {
        let spec = crate::v3_1::spec::Spec::default();
        for (json, kw) in [
            (serde_json::json!({"allOf": []}), "allOf"),
            (serde_json::json!({"anyOf": []}), "anyOf"),
            (serde_json::json!({"oneOf": []}), "oneOf"),
        ] {
            let s: Schema = serde_json::from_value(json).unwrap();
            let mut ctx =
                crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
            s.validate_with_context(&mut ctx, "s".into());
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains(&format!("`{kw}` must be a non-empty array"))),
                "{kw} errors: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn multi_schema_type_array_must_be_non_empty() {
        // Build via the struct: `serde_json::from_value` on `{"type": []}`
        // would not route to `MultiSchema`.
        let spec = crate::v3_1::spec::Spec::default();
        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        let m = MultiSchema {
            schema_types: vec![],
            ..Default::default()
        };
        let s = Schema::Multi(Box::new(m));
        s.validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("s.type") && e.contains("must contain at least one element")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn common_codegen_extensions_round_trip_and_validate() {
        let enum_json = serde_json::json!({
            "type": "string",
            "enum": ["open", "closed"],
            "x-enumDescriptions": ["Open state", "Closed state"]
        });
        let schema: Schema = serde_json::from_value(enum_json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&schema).unwrap(), enum_json);

        let spec = crate::v3_1::spec::Spec::default();
        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        schema.validate_with_context(&mut ctx, "s".into());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let object_json = serde_json::json!({
            "type": "object",
            "x-tags": ["models"],
            "x-discriminator-value": "pet"
        });
        let schema: Schema = serde_json::from_value(object_json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&schema).unwrap(), object_json);

        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        schema.validate_with_context(&mut ctx, "s".into());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let schema = Schema::Single(Box::new(SingleSchema::String(StringSchema {
            enum_values: Some(vec!["open".to_owned(), "closed".to_owned()]),
            x_enum_descriptions: Some(vec!["Open state".to_owned()]),
            ..Default::default()
        })));
        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        schema.validate_with_context(&mut ctx, "s".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("s.x-enumDescriptions")),
            "enum descriptions length: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn empty_schema_default_is_unit_value() {
        // `EmptySchema` is a unit-like marker; `default()` must
        // produce the same singular value every time. Use the trait
        // path so the test still exercises the `Default` impl
        // (clippy flags `EmptySchema::default()` as redundant on a
        // unit struct, but we want to pin down the impl exists).
        assert_eq!(EmptySchema, <EmptySchema as Default>::default());
    }

    #[test]
    fn empty_schema_serializes_as_empty_object() {
        // The on-the-wire form is the literal JSON `{}`. Use a string
        // round-trip so we observe exactly what serde writes (rather
        // than going through `Value` which loses ordering / duplicate
        // info).
        let s = serde_json::to_string(&EmptySchema).unwrap();
        assert_eq!(s, "{}");
        // `to_value` agrees and produces an empty object (not null,
        // not array).
        let v = serde_json::to_value(EmptySchema).unwrap();
        assert!(v.is_object());
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn empty_schema_deserializes_from_empty_object() {
        let from_str: EmptySchema = serde_json::from_str("{}").unwrap();
        assert_eq!(from_str, EmptySchema);
        let from_value: EmptySchema = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(from_value, EmptySchema);
    }

    #[test]
    fn empty_schema_rejects_populated_object() {
        // Any populated object is rejected — that's the whole point.
        // The error message names the expected shape.
        let err = serde_json::from_value::<EmptySchema>(serde_json::json!({"k": 1}))
            .expect_err("populated object must reject");
        assert!(
            err.to_string().contains("expected empty schema object"),
            "error must explain the constraint: {err}"
        );
        // Even a single key with a null value still counts as
        // populated.
        let err = serde_json::from_value::<EmptySchema>(serde_json::json!({"x": null}))
            .expect_err("single null entry must reject");
        assert!(err.to_string().contains("expected empty schema object"));
    }

    #[test]
    fn empty_schema_rejects_non_object_shapes() {
        // Anything other than a JSON object is a hard parse error.
        for value in [
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!(false),
            serde_json::json!(0),
            serde_json::json!("{}"),
            serde_json::json!([]),
        ] {
            assert!(
                serde_json::from_value::<EmptySchema>(value.clone()).is_err(),
                "{value} must not deserialize as EmptySchema",
            );
        }
    }

    #[test]
    fn empty_schema_round_trip_via_string_is_stable() {
        let original = EmptySchema;
        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: EmptySchema = serde_json::from_str(&encoded).unwrap();
        assert_eq!(original, decoded);
        // Re-encoding produces the same bytes.
        assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
    }

    #[test]
    fn empty_schema_round_trips_as_literal_empty_object() {
        let json = serde_json::json!({});
        let schema: Schema = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(schema, Schema::Empty(EmptySchema));
        assert_eq!(serde_json::to_value(&schema).unwrap(), json);
    }

    #[test]
    fn schema_with_only_description_remains_object() {
        // Anything beyond a literal `{}` (even a `description` with no
        // `type`) falls through to the typed `Single::Object` variant
        // — `{type: "object"}` is the canonical re-serialised form.
        let json = serde_json::json!({"description": "just metadata"});
        let schema: Schema = serde_json::from_value(json).unwrap();
        match &schema {
            Schema::Single(s) => match s.as_ref() {
                SingleSchema::Object(o) => {
                    assert_eq!(o.description.as_deref(), Some("just metadata"));
                }
                other => panic!("expected ObjectSchema, got {other}"),
            },
            other => panic!("expected Single, got {other:?}"),
        }
        let value = serde_json::to_value(&schema).unwrap();
        assert_eq!(value["type"], "object");
        assert_eq!(value["description"], "just metadata");
    }

    #[test]
    fn explicit_object_schema_still_picks_object_variant() {
        let json = serde_json::json!({"type": "object", "title": "T"});
        let schema: Schema = serde_json::from_value(json.clone()).unwrap();
        let Schema::Single(s) = &schema else {
            panic!("expected Single, got {schema:?}");
        };
        let SingleSchema::Object(_) = s.as_ref() else {
            panic!("expected ObjectSchema, got {s}");
        };
        assert_eq!(serde_json::to_value(&schema).unwrap(), json);
    }

    #[test]
    fn empty_schema_validates_clean() {
        let schema = Schema::Empty(EmptySchema);
        let spec = crate::v3_1::spec::Spec::default();
        let mut ctx =
            crate::common::helpers::Context::new(&spec, crate::validation::Options::new());
        schema.validate_with_context(&mut ctx, "s".into());
        assert!(ctx.errors.is_empty(), "errors: {:?}", ctx.errors);
    }

    #[test]
    fn empty_schema_distinct_from_bool_true() {
        // Both are semantically "matches anything" but the JSON
        // representation is what users author, so they round-trip
        // distinctly.
        let bool_schema: Schema = serde_json::from_value(serde_json::json!(true)).unwrap();
        let empty_schema: Schema = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(bool_schema, Schema::Bool(true));
        assert_eq!(empty_schema, Schema::Empty(EmptySchema));
        assert_ne!(bool_schema, empty_schema);
    }
}
