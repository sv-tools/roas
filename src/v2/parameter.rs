//! Parameter Object

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::formats::{CollectionFormat, IntegerFormat, NumberFormat, StringFormat};
use crate::common::helpers::{
    Context, ValidateWithContext, validate_pattern, validate_required_string,
};
use crate::common::reference::RefOr;
use crate::v2::items::Items;
use crate::v2::schema::Schema;
use crate::v2::spec::Spec;

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
                must_be_required(&p.required, ctx, path.clone(), p.name.clone());
                must_not_allow_empty_value(&p.allow_empty_value, ctx, path.clone(), p.name.clone());
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
    }
}

impl ValidateWithContext<Spec> for FileParameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

fn must_be_required(p: &Option<bool>, ctx: &mut Context<Spec>, path: String, name: String) {
    if !p.is_some_and(|x| x) {
        ctx.errors.push(format!("{path}.{name}: must be required"));
    }
}

fn must_not_allow_empty_value(
    p: &Option<bool>,
    ctx: &mut Context<Spec>,
    path: String,
    name: String,
) {
    if p.is_some_and(|x| x) {
        ctx.errors
            .push(format!("{path}.{name}: must not allow empty value"));
    }
}
