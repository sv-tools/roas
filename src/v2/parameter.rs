//! Parameter Object

use crate::common::formats::{CollectionFormat, IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{validate_pattern, validate_required_string};
use crate::common::reference::RefOr;
use crate::v2::items::Items;
use crate::v2::schema::Schema;
use crate::v2::spec::Spec;
use crate::validation::{Context, PushError, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "in")]
pub enum Parameter {
    #[serde(rename = "body")]
    Body(Box<InBody>),

    #[serde(rename = "header")]
    Header(Box<InHeader>),

    #[serde(rename = "query")]
    Query(Box<InQuery>),

    #[serde(rename = "path")]
    Path(Box<InPath>),

    #[serde(rename = "formData")]
    FormData(Box<InFormData>),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InBody {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// ***Required*** The schema defining the type used for the body parameter.
    pub schema: RefOr<Schema>,

    /// ReDoc extension containing named examples for the request body.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum InHeader {
    #[serde(rename = "string")]
    String(StringParameter),

    #[serde(rename = "integer")]
    Integer(IntegerParameter),

    #[serde(rename = "number")]
    Number(NumberParameter),

    #[serde(rename = "boolean")]
    Boolean(BooleanParameter),

    #[serde(rename = "array")]
    Array(ArrayParameter),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum InPath {
    #[serde(rename = "string")]
    String(StringParameter),

    #[serde(rename = "integer")]
    Integer(IntegerParameter),

    #[serde(rename = "number")]
    Number(NumberParameter),

    #[serde(rename = "boolean")]
    Boolean(BooleanParameter),

    #[serde(rename = "array")]
    Array(ArrayParameter),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum InQuery {
    #[serde(rename = "string")]
    String(StringParameter),

    #[serde(rename = "integer")]
    Integer(IntegerParameter),

    #[serde(rename = "number")]
    Number(NumberParameter),

    #[serde(rename = "boolean")]
    Boolean(BooleanParameter),

    #[serde(rename = "array")]
    Array(ArrayParameter),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum InFormData {
    #[serde(rename = "string")]
    String(StringParameter),

    #[serde(rename = "integer")]
    Integer(IntegerParameter),

    #[serde(rename = "number")]
    Number(NumberParameter),

    #[serde(rename = "boolean")]
    Boolean(BooleanParameter),

    #[serde(rename = "array")]
    Array(ArrayParameter),

    #[serde(rename = "file")]
    File(FileParameter),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct StringParameter {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Sets the ability to pass empty-valued parameters and
    /// allows you to send a parameter with a name only or an empty value.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowEmptyValue")]
    pub allow_empty_value: Option<bool>,

    /// The extending format for the string type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,

    /// Declares the value of the parameter that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required parameters.
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

    /// ReDoc extension containing named examples for this parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct IntegerParameter {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Sets the ability to pass empty-valued parameters and
    /// allows you to send a parameter with a name only or an empty value.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowEmptyValue")]
    pub allow_empty_value: Option<bool>,

    /// The extending format for the integer type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<IntegerFormat>,

    /// Declares the value of the parameter that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required parameters.
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

    /// ReDoc extension containing named examples for this parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct NumberParameter {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Sets the ability to pass empty-valued parameters and
    /// allows you to send a parameter with a name only or an empty value.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowEmptyValue")]
    pub allow_empty_value: Option<bool>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// The extending format for the number type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<NumberFormat>,

    /// Declares the value of the parameter that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required parameters.
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

    /// ReDoc extension containing named examples for this parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct BooleanParameter {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Sets the ability to pass empty-valued parameters and
    /// allows you to send a parameter with a name only or an empty value.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowEmptyValue")]
    pub allow_empty_value: Option<bool>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Declares the value of the parameter that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,

    /// ReDoc extension containing named examples for this parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ArrayParameter {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Sets the ability to pass empty-valued parameters and
    /// allows you to send a parameter with a name only or an empty value.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowEmptyValue")]
    pub allow_empty_value: Option<bool>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// **Required** Describes the type of items in the array.
    pub items: Items,

    /// Declares the values of the header that the server will use if none is provided.
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

    /// ReDoc extension containing named examples for this parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct FileParameter {
    /// ***Required*** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Determines whether this parameter is mandatory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Declares the value of the parameter that the server will use if none is provided.
    ///
    /// **Note**: "default" has no meaning for required parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// ReDoc extension containing named examples for this parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Parameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            Parameter::Body(p) => p.validate_with_context(ctx, path),
            Parameter::Header(p) => p.validate_with_context(ctx, path),
            Parameter::Query(p) => p.validate_with_context(ctx, path),
            Parameter::Path(p) => p.validate_with_context(ctx, path),
            Parameter::FormData(p) => p.validate_with_context(ctx, path),
        }
    }
}

impl ValidateWithContext<Spec> for InBody {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}name"));
        self.schema
            .validate_with_context(ctx, format!("{path}.schema"));
    }
}

impl ValidateWithContext<Spec> for InHeader {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            InHeader::String(p) => {
                p.validate_with_context(ctx, path.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
                if let Some(pattern) = &p.pattern {
                    validate_pattern(pattern, ctx, format!("{path}.pattern"));
                }
            }
            InHeader::Integer(p) => {
                p.validate_with_context(ctx, path.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InHeader::Number(p) => {
                p.validate_with_context(ctx, path.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InHeader::Boolean(p) => {
                p.validate_with_context(ctx, path.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InHeader::Array(p) => {
                p.validate_with_context(ctx, path.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
                must_not_use_multi_collection_format(&p.collection_format, ctx, path.clone());
            }
        }
    }
}

impl ValidateWithContext<Spec> for InQuery {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            InQuery::String(p) => {
                p.validate_with_context(ctx, path);
            }
            InQuery::Integer(p) => {
                p.validate_with_context(ctx, path);
            }
            InQuery::Number(p) => {
                p.validate_with_context(ctx, path);
            }
            InQuery::Boolean(p) => {
                p.validate_with_context(ctx, path);
            }
            InQuery::Array(p) => {
                p.validate_with_context(ctx, path);
            }
        }
    }
}

impl ValidateWithContext<Spec> for InPath {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            InPath::String(p) => {
                must_be_required(&p.required, ctx, path.clone(), p.name.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InPath::Integer(p) => {
                must_be_required(&p.required, ctx, path.clone(), p.name.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InPath::Number(p) => {
                must_be_required(&p.required, ctx, path.clone(), p.name.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InPath::Boolean(p) => {
                must_be_required(&p.required, ctx, path.clone(), p.name.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
            }
            InPath::Array(p) => {
                p.validate_with_context(ctx, path.clone());
                must_be_required(&p.required, ctx, path.clone(), p.name.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
                must_not_use_multi_collection_format(&p.collection_format, ctx, path.clone());
            }
        }
    }
}

impl ValidateWithContext<Spec> for InFormData {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            InFormData::String(p) => {
                p.validate_with_context(ctx, path);
            }
            InFormData::Integer(p) => {
                p.validate_with_context(ctx, path);
            }
            InFormData::Number(p) => {
                p.validate_with_context(ctx, path);
            }
            InFormData::Boolean(p) => {
                p.validate_with_context(ctx, path);
            }
            InFormData::Array(p) => {
                p.validate_with_context(ctx, path);
            }
            InFormData::File(p) => {
                p.validate_with_context(ctx, path);
            }
        }
    }
}

impl ValidateWithContext<Spec> for StringParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

impl ValidateWithContext<Spec> for IntegerParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

impl ValidateWithContext<Spec> for NumberParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

impl ValidateWithContext<Spec> for BooleanParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

impl ValidateWithContext<Spec> for ArrayParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        self.items
            .validate_with_context(ctx, format!("{path}.items"));
    }
}

impl ValidateWithContext<Spec> for FileParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

fn must_be_required(p: &Option<bool>, ctx: &mut Context<Spec>, path: String, name: String) {
    if !p.is_some_and(|x| x) {
        ctx.error(format!("{path}.{name}"), "must be required");
    }
}

fn must_not_allow_empty_value(
    p: &Option<bool>,
    ctx: &mut Context<Spec>,
    path: String,
    name: String,
) {
    if p.is_some_and(|x| x) {
        ctx.error(format!("{path}.{name}"), "must not allow empty value");
    }
}

/// Per the OAS 2.0 schema, `collectionFormat: "multi"` is allowed only on
/// `query` and `formData` parameters. `header` and `path` parameters must
/// use the `collectionFormat` enum (csv/ssv/tsv/pipes) — `multi` here is
/// a spec violation.
fn must_not_use_multi_collection_format(
    format: &Option<CollectionFormat>,
    ctx: &mut Context<Spec>,
    path: String,
) {
    if let Some(fmt) = format
        && fmt.is_multi()
    {
        ctx.error(
            format!("{path}.collectionFormat"),
            "`multi` is only allowed on `query` and `formData` parameters",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::items::{ArrayItem, Items, StringItem};
    use crate::v2::schema::{Schema, StringSchema};
    use crate::validation::Context;
    use crate::validation::Options;
    use crate::validation::ValidationErrorsExt;
    use serde_json::json;

    fn ctx() -> Context<'static, Spec> {
        // Returns a context with a default Spec held by the static box leaked.
        // Using a leak keeps the borrow checker simple in tests; the spec lives
        // for the test process which is fine.
        let spec: &'static Spec = Box::leak(Box::new(Spec::default()));
        Context::new(spec, Options::new())
    }

    #[test]
    fn body_parameter_roundtrip_and_validate() {
        let raw = json!({
            "in": "body",
            "name": "user",
            "description": "the user",
            "required": true,
            "schema": {"type": "string"},
            "x-foo": "bar",
        });
        let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
        match &p {
            Parameter::Body(b) => {
                assert_eq!(b.name, "user");
                assert_eq!(b.required, Some(true));
            }
            _ => panic!("expected body"),
        }
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, raw);

        let mut c = ctx();
        p.validate_with_context(&mut c, "x".into());
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);

        // Empty name produces an error.
        let bad = Parameter::Body(Box::new(InBody {
            name: String::new(),
            description: None,
            required: None,
            schema: RefOr::new_item(Schema::from(StringSchema::default())),
            x_examples: None,
            extensions: None,
        }));
        let mut c = ctx();
        bad.validate_with_context(&mut c, "p".into());
        assert!(
            c.errors.mentions("must not be empty"),
            "errors: {:?}",
            c.errors
        );
    }

    #[test]
    fn header_parameter_all_variants_roundtrip_and_validate() {
        // String
        let raw = json!({"in": "header", "type": "string", "name": "h"});
        let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(&p, Parameter::Header(h) if matches!(**h, InHeader::String(_))));
        assert_eq!(serde_json::to_value(&p).unwrap(), raw);
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(c.errors.is_empty(), "errors: {:?}", c.errors);

        // Integer
        let raw = json!({"in": "header", "type": "integer", "name": "h"});
        let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), raw);
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(c.errors.is_empty());

        // Number
        let raw = json!({"in": "header", "type": "number", "name": "h"});
        let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), raw);
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(c.errors.is_empty());

        // Boolean
        let raw = json!({"in": "header", "type": "boolean", "name": "h"});
        let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), raw);
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(c.errors.is_empty());

        // Array
        let raw =
            json!({"in": "header", "type": "array", "name": "h", "items": {"type": "string"}});
        let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), raw);
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(c.errors.is_empty());

        // Header with allowEmptyValue=true should produce a "must not allow empty value" error
        let p = Parameter::Header(Box::new(InHeader::String(StringParameter {
            name: "h".into(),
            allow_empty_value: Some(true),
            ..Default::default()
        })));
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(
            c.errors
                .iter()
                .any(|e| e.contains("must not allow empty value")),
            "errors: {:?}",
            c.errors
        );

        // Header with bad pattern triggers pattern error.
        let p = Parameter::Header(Box::new(InHeader::String(StringParameter {
            name: "h".into(),
            pattern: Some("[".into()),
            ..Default::default()
        })));
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(c.errors.mentions("pattern"), "errors: {:?}", c.errors);

        // Each non-string variant — exercise allow-empty-value on each.
        for variant in [
            Parameter::Header(Box::new(InHeader::Integer(IntegerParameter {
                name: "h".into(),
                allow_empty_value: Some(true),
                ..Default::default()
            }))),
            Parameter::Header(Box::new(InHeader::Number(NumberParameter {
                name: "h".into(),
                allow_empty_value: Some(true),
                ..Default::default()
            }))),
            Parameter::Header(Box::new(InHeader::Boolean(BooleanParameter {
                name: "h".into(),
                allow_empty_value: Some(true),
                ..Default::default()
            }))),
            Parameter::Header(Box::new(InHeader::Array(ArrayParameter {
                name: "h".into(),
                allow_empty_value: Some(true),
                items: Items::String(Box::default()),
                ..Default::default()
            }))),
        ] {
            let mut c = ctx();
            variant.validate_with_context(&mut c, "p".into());
            assert!(
                c.errors
                    .iter()
                    .any(|e| e.contains("must not allow empty value")),
                "errors: {:?}",
                c.errors
            );
        }
    }

    #[test]
    fn query_parameter_all_variants_roundtrip_and_validate() {
        for raw in [
            json!({"in": "query", "type": "string", "name": "q"}),
            json!({"in": "query", "type": "integer", "name": "q"}),
            json!({"in": "query", "type": "number", "name": "q"}),
            json!({"in": "query", "type": "boolean", "name": "q"}),
            json!({"in": "query", "type": "array", "name": "q", "items": {"type": "string"}}),
        ] {
            let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(&p).unwrap(), raw);
            let mut c = ctx();
            p.validate_with_context(&mut c, "p".into());
            assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
        }

        // Validate empty name reports an error for each kind.
        for inner in [
            InQuery::String(StringParameter::default()),
            InQuery::Integer(IntegerParameter::default()),
            InQuery::Number(NumberParameter::default()),
            InQuery::Boolean(BooleanParameter::default()),
            InQuery::Array(ArrayParameter {
                items: Items::String(Box::default()),
                ..Default::default()
            }),
        ] {
            let p = Parameter::Query(Box::new(inner));
            let mut c = ctx();
            p.validate_with_context(&mut c, "p".into());
            assert!(
                c.errors.mentions("must not be empty"),
                "errors: {:?}",
                c.errors
            );
        }
    }

    #[test]
    fn path_parameter_all_variants_validate() {
        // Each path parameter variant must have required=true; otherwise an error is reported.
        let cases = [
            Parameter::Path(Box::new(InPath::String(StringParameter {
                name: "id".into(),
                required: Some(true),
                ..Default::default()
            }))),
            Parameter::Path(Box::new(InPath::Integer(IntegerParameter {
                name: "id".into(),
                required: Some(true),
                ..Default::default()
            }))),
            Parameter::Path(Box::new(InPath::Number(NumberParameter {
                name: "id".into(),
                required: Some(true),
                ..Default::default()
            }))),
            Parameter::Path(Box::new(InPath::Boolean(BooleanParameter {
                name: "id".into(),
                required: Some(true),
                ..Default::default()
            }))),
            Parameter::Path(Box::new(InPath::Array(ArrayParameter {
                name: "id".into(),
                required: Some(true),
                items: Items::String(Box::default()),
                ..Default::default()
            }))),
        ];
        for p in &cases {
            let raw = serde_json::to_value(p).unwrap();
            let p2: Parameter = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(p, &p2);
            let mut c = ctx();
            p.validate_with_context(&mut c, "p".into());
            assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
        }

        // Path without required produces an error
        let p = Parameter::Path(Box::new(InPath::String(StringParameter {
            name: "id".into(),
            required: None,
            ..Default::default()
        })));
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(
            c.errors.mentions("must be required"),
            "errors: {:?}",
            c.errors
        );

        // Path with allow_empty_value triggers extra error.
        let p = Parameter::Path(Box::new(InPath::String(StringParameter {
            name: "id".into(),
            required: Some(true),
            allow_empty_value: Some(true),
            ..Default::default()
        })));
        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(
            c.errors
                .iter()
                .any(|e| e.contains("must not allow empty value")),
            "errors: {:?}",
            c.errors
        );

        // Hit each Path variant's must_be_required + allow-empty-value branches.
        for inner in [
            InPath::Integer(IntegerParameter::default()),
            InPath::Number(NumberParameter::default()),
            InPath::Boolean(BooleanParameter::default()),
            InPath::Array(ArrayParameter {
                items: Items::String(Box::default()),
                ..Default::default()
            }),
        ] {
            let p = Parameter::Path(Box::new(inner));
            let mut c = ctx();
            p.validate_with_context(&mut c, "p".into());
            assert!(
                c.errors.mentions("must be required"),
                "errors: {:?}",
                c.errors
            );
        }
    }

    #[test]
    fn formdata_parameter_all_variants_roundtrip_and_validate() {
        for raw in [
            json!({"in": "formData", "type": "string", "name": "f"}),
            json!({"in": "formData", "type": "integer", "name": "f"}),
            json!({"in": "formData", "type": "number", "name": "f"}),
            json!({"in": "formData", "type": "boolean", "name": "f"}),
            json!({"in": "formData", "type": "array", "name": "f", "items": {"type": "string"}}),
            json!({"in": "formData", "type": "file", "name": "f"}),
        ] {
            let p: Parameter = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(&p).unwrap(), raw);
            let mut c = ctx();
            p.validate_with_context(&mut c, "p".into());
            assert!(c.errors.is_empty(), "errors: {:?}", c.errors);
        }

        // Empty-name validation for each variant.
        for inner in [
            InFormData::String(StringParameter::default()),
            InFormData::Integer(IntegerParameter::default()),
            InFormData::Number(NumberParameter::default()),
            InFormData::Boolean(BooleanParameter::default()),
            InFormData::Array(ArrayParameter {
                items: Items::String(Box::default()),
                ..Default::default()
            }),
            InFormData::File(FileParameter::default()),
        ] {
            let p = Parameter::FormData(Box::new(inner));
            let mut c = ctx();
            p.validate_with_context(&mut c, "p".into());
            assert!(
                c.errors.mentions("must not be empty"),
                "errors: {:?}",
                c.errors
            );
        }
    }

    #[test]
    fn x_examples_round_trip() {
        let body = json!({
            "in": "body",
            "name": "payload",
            "schema": { "type": "string" },
            "x-examples": {
                "application/json": { "value": "demo" }
            }
        });
        let p: Parameter = serde_json::from_value(body.clone()).unwrap();
        match &p {
            Parameter::Body(body) => assert_eq!(
                body.x_examples,
                Some(BTreeMap::from_iter([(
                    "application/json".to_owned(),
                    serde_json::json!({ "value": "demo" })
                )]))
            ),
            _ => panic!("expected body parameter"),
        }
        assert_eq!(serde_json::to_value(&p).unwrap(), body);

        let query = json!({
            "in": "query",
            "type": "string",
            "name": "q",
            "x-examples": {
                "default": "demo"
            }
        });
        let p: Parameter = serde_json::from_value(query.clone()).unwrap();
        match &p {
            Parameter::Query(query) => match query.as_ref() {
                InQuery::String(query) => assert_eq!(
                    query.x_examples,
                    Some(BTreeMap::from_iter([(
                        "default".to_owned(),
                        serde_json::json!("demo")
                    )]))
                ),
                _ => panic!("expected string query parameter"),
            },
            _ => panic!("expected query parameter"),
        }
        assert_eq!(serde_json::to_value(&p).unwrap(), query);
    }

    #[test]
    fn header_and_path_array_params_reject_multi_collection_format() {
        // `multi` is allowed only for query/formData; header and path
        // parameters must use the `collectionFormat` enum without `multi`.
        for inner in [
            Parameter::Header(Box::new(InHeader::Array(ArrayParameter {
                name: "h".into(),
                items: Items::String(Box::default()),
                collection_format: Some(CollectionFormat::Multi),
                ..Default::default()
            }))),
            Parameter::Path(Box::new(InPath::Array(ArrayParameter {
                name: "id".into(),
                required: Some(true),
                items: Items::String(Box::default()),
                collection_format: Some(CollectionFormat::Multi),
                ..Default::default()
            }))),
        ] {
            let mut c = ctx();
            inner.validate_with_context(&mut c, "p".into());
            assert!(
                c.errors
                    .iter()
                    .any(|e| e.contains("`multi` is only allowed")),
                "errors: {:?}",
                c.errors
            );
        }

        // Query and formData accept `multi`.
        for inner in [
            Parameter::Query(Box::new(InQuery::Array(ArrayParameter {
                name: "q".into(),
                items: Items::String(Box::default()),
                collection_format: Some(CollectionFormat::Multi),
                ..Default::default()
            }))),
            Parameter::FormData(Box::new(InFormData::Array(ArrayParameter {
                name: "f".into(),
                items: Items::String(Box::default()),
                collection_format: Some(CollectionFormat::Multi),
                ..Default::default()
            }))),
        ] {
            let mut c = ctx();
            inner.validate_with_context(&mut c, "p".into());
            assert!(
                c.errors.iter().all(|e| !e.contains("`multi`")),
                "errors: {:?}",
                c.errors
            );
        }
    }

    #[test]
    fn array_parameter_validates_items() {
        let p = Parameter::Query(Box::new(InQuery::Array(ArrayParameter {
            name: "q".into(),
            items: Items::String(Box::new(StringItem {
                pattern: Some("[".into()),
                ..Default::default()
            })),
            ..Default::default()
        })));

        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(
            c.errors.mentions("p.items.pattern"),
            "errors: {:?}",
            c.errors
        );
    }

    #[test]
    fn path_array_parameter_validates_items() {
        let p = Parameter::Path(Box::new(InPath::Array(ArrayParameter {
            name: "id".into(),
            required: Some(true),
            items: Items::Array(Box::new(ArrayItem {
                items: Items::String(Box::default()),
                default: None,
                collection_format: Some(CollectionFormat::Multi),
                max_items: None,
                min_items: None,
                unique_items: None,
                extensions: None,
            })),
            ..Default::default()
        })));

        let mut c = ctx();
        p.validate_with_context(&mut c, "p".into());
        assert!(
            c.errors
                .iter()
                .any(|e| e.contains("p.items.collectionFormat")),
            "errors: {:?}",
            c.errors
        );
    }
}
