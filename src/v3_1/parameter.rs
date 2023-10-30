//! Describes a single operation parameter.

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_required_string};
use crate::common::reference::RefOr;
use crate::v3_1::example::Example;
use crate::v3_1::media_type::MediaType;
use crate::v3_1::schema::Schema;
use crate::v3_1::spec::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Describes a single operation parameter.
///
/// A unique parameter is defined by a combination of a name and location.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "in")]
pub enum Parameter {
    /// Used together with Path Templating, where the parameter value is actually part of
    /// the operation’s URL.
    /// This does not include the host or base path of the API.
    /// For example, in `/items/{itemId}`, the path parameter is `itemId`.
    #[serde(rename = "path")]
    Path(InPath),

    /// Parameters that are appended to the URL.
    /// For example, in `/items?id=###`, the query parameter is `id`.
    #[serde(rename = "query")]
    Query(InQuery),

    /// Custom headers that are expected as part of the request.
    /// Note that [RFC7230](https://www.rfc-editor.org/rfc/rfc7230) states header names are case insensitive.
    #[serde(rename = "header")]
    Header(InHeader),

    /// Used to pass a specific cookie value to the API.
    #[serde(rename = "cookie")]
    Cookie(InCookie),
}

/// Holds a parameter with `in: path` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InPath {
    /// **Required** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Determines whether this parameter is mandatory.
    /// If the parameter location is "path", this property is **REQUIRED** and its value MUST be `true`.
    /// Otherwise, the property MAY be included and its default value is `false`.
    pub required: bool,

    /// Specifies that a parameter is deprecated and SHOULD be transitioned out of usage.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    /// Describes how the parameter value will be serialized depending on the type of
    /// the parameter value.
    /// Default values is `simple`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<InPathStyle>,

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

/// Holds the style information for a parameter with `in: path` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum InPathStyle {
    /// Path-style parameters defined by [RFC6570](https://www.rfc-editor.org/rfc/rfc6570).
    #[serde(rename = "matrix")]
    Matrix,

    /// Label style parameters defined by [RFC6570](https://www.rfc-editor.org/rfc/rfc6570).
    #[serde(rename = "label")]
    Label,

    /// Simple style parameters defined by [RFC6570](https://www.rfc-editor.org/rfc/rfc6570).
    /// This option replaces collectionFormat with a csv value from OpenAPI 2.0.
    #[serde(rename = "simple")]
    Simple,
}

/// Holds a parameter with `in: query` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InQuery {
    /// **Required** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

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

    /// Sets the ability to pass empty-valued parameters.
    /// This allows sending a parameter with an empty value.
    /// Default value is `false`.
    /// If style is used, and if behavior is `n/a` (cannot be serialized),
    /// the value of `allowEmptyValue` SHALL be ignored.
    /// Use of this property is NOT RECOMMENDED, as it is likely to be removed in a later revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowEmptyValue")]
    pub allow_empty_value: Option<bool>,

    /// Describes how the parameter value will be serialized depending on the type of
    /// the parameter value.
    /// Default values is `form`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<InQueryStyle>,

    /// When this is `true`, parameter values of type `array` or `object` generate separate parameters
    /// for each value of the array or key-value pair of the map.
    /// For other types of parameters this property has no effect.
    /// When `style` is `form`, the default value is `true`.
    /// For all other styles, the default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,

    /// Determines whether the parameter value SHOULD allow reserved characters,
    /// as defined by [RFC3986](https://www.rfc-editor.org/rfc/rfc3986)
    /// `:/?#[]@!$&'()*+,;=` to be included without percent-encoding.
    /// The default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowReserved")]
    pub allow_reserved: Option<bool>,

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

/// Holds the style information for a parameter with `in: query` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum InQueryStyle {
    /// Form style parameters defined by [RFC6570](https://www.rfc-editor.org/rfc/rfc6570).
    /// This option replaces `collectionFormat` with a `csv` (when `explode` is `false`)
    /// or `multi` (when `explode` is `true`) value from OpenAPI 2.0.
    #[serde(rename = "form")]
    Form,

    /// Space separated array values.
    /// This option replaces `collectionFormat` equal to `ssv` from OpenAPI 2.0.
    #[serde(rename = "spaceDelimited")]
    SpaceDelimited,

    /// Pipe separated array values.
    /// This option replaces `collectionFormat` equal to `pipes` from OpenAPI 2.0.
    #[serde(rename = "pipeDelimited")]
    PipeDelimited,

    /// Provides a simple way of rendering nested objects using form parameters.
    #[serde(rename = "deepObject")]
    DeepObject,
}

/// Holds a parameter with `in: header` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InHeader {
    /// **Required** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

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

/// Holds the style information for a parameter with `in: header` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum InHeaderStyle {
    /// Simple style parameters defined by [RFC6570](https://www.rfc-editor.org/rfc/rfc6570).
    /// This option replaces collectionFormat with a csv value from OpenAPI 2.0.
    #[serde(rename = "simple")]
    Simple,
}

/// Holds a parameter with `in: cookie` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InCookie {
    /// **Required** The name of the parameter.
    /// Parameter names are case sensitive.
    pub name: String,

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
    /// Default values is `form`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<InCookieStyle>,

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

/// Holds the style information for a parameter with `in: cookie` property.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum InCookieStyle {
    /// Form style parameters defined by [RFC6570](https://www.rfc-editor.org/rfc/rfc6570).
    /// This option replaces `collectionFormat` with a `csv` (when `explode` is `false`)
    /// or `multi` (when `explode` is `true`) value from OpenAPI 2.0.
    #[serde(rename = "form")]
    Form,
}

impl ValidateWithContext<Spec> for Parameter {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        match self {
            Parameter::Path(p) => p.validate_with_context(ctx, path),
            Parameter::Query(p) => p.validate_with_context(ctx, path),
            Parameter::Header(p) => p.validate_with_context(ctx, path),
            Parameter::Cookie(p) => p.validate_with_context(ctx, path),
        }
    }
}

impl ValidateWithContext<Spec> for InPath {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        must_be_required(&Some(self.required), ctx, path.clone(), self.name.clone());
        either_example_or_examples(ctx, &self.example, &self.examples, path.clone());
        either_schema_or_content(ctx, &self.schema, &self.content, path.clone());
    }
}

impl ValidateWithContext<Spec> for InQuery {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        either_example_or_examples(ctx, &self.example, &self.examples, path.clone());
        either_schema_or_content(ctx, &self.schema, &self.content, path.clone());
    }
}

impl ValidateWithContext<Spec> for InHeader {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        either_example_or_examples(ctx, &self.example, &self.examples, path.clone());
        either_schema_or_content(ctx, &self.schema, &self.content, path.clone());
    }
}

impl ValidateWithContext<Spec> for InCookie {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        either_example_or_examples(ctx, &self.example, &self.examples, path.clone());
        either_schema_or_content(ctx, &self.schema, &self.content, path.clone());
    }
}

fn must_be_required(p: &Option<bool>, ctx: &mut Context<Spec>, path: String, name: String) {
    if !p.is_some_and(|x| x) {
        ctx.error(path, format_args!(".{name}: must be required"));
    }
}

fn either_example_or_examples(
    ctx: &mut Context<Spec>,
    example: &Option<serde_json::Value>,
    examples: &Option<BTreeMap<String, RefOr<Example>>>,
    path: String,
) {
    if example.is_some() && examples.is_some() {
        ctx.error(path, "example and examples are mutually exclusive");
    }
}

fn either_schema_or_content(
    ctx: &mut Context<Spec>,
    schema: &Option<RefOr<Schema>>,
    content: &Option<BTreeMap<String, MediaType>>,
    path: String,
) {
    if schema.is_some() && content.is_some() {
        ctx.error(path, "schema and content are mutually exclusive");
    }
}
