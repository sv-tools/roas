//! Provides schema and examples for the media type

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{Context, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v3_0::example::Example;
use crate::v3_0::header::Header;
use crate::v3_0::parameter::InQueryStyle;
use crate::v3_0::schema::Schema;
use crate::v3_0::spec::Spec;

/// Each Media Type Object provides schema and examples for the media type identified by its key.
///
/// Specification example:
/// ```yaml
/// application/json:
///   schema:
///     $ref: "#/components/schemas/Pet"
///   examples:
///     cat:
///       summary: An example of a cat
///       value:
///         name: Fluffy
///         petType: Cat
///         color: White
///         gender: male
///         breed: Persian
///     dog:
///       summary: An example of a dog with a cat's name
///       value:
///         name: Puma
///         petType: Dog
///         color: Black
///         gender: Female
///         breed: Mixed
///     frog:
///       $ref: "#/components/examples/frog-example"
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MediaType {
    /// The schema defining the content of the request, response, or parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<RefOr<Schema>>,

    /// Example of the media type.
    /// The example SHOULD match the specified schema and encoding properties if present.
    /// The `example` field is mutually exclusive of the `examples` field.
    /// Furthermore, if referencing a `schema` that contains an example,
    /// the `example` value SHALL override the example provided by the schema.
    /// To represent examples of media types that cannot naturally be represented in JSON or YAML,
    /// a string value can contain the example with escaping where necessary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,

    /// Examples of the media type.
    /// Each example SHOULD contain a value in the correct format as specified in the parameter encoding.
    /// The `examples` field is mutually exclusive of the `example` field.
    /// Furthermore, if referencing a `schema` that contains an example,
    /// the `examples` value SHALL override the example provided by the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<BTreeMap<String, RefOr<Example>>>,

    /// A map between a property name and its encoding information.
    /// The key, being the property name, MUST exist in the schema as a property.
    /// The encoding object SHALL only apply to `requestBody` objects when
    /// the media type is `multipart` or `application/x-www-form-urlencoded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<BTreeMap<String, Encoding>>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// A single encoding definition applied to a single schema property.
///
/// Specification example:
/// ```yaml
/// requestBody:
///   content:
///     multipart/mixed:
///       schema:
///         type: object
///         properties:
///           id:
///             # default is text/plain
///             type: string
///             format: uuid
///           address:
///             # default is application/json
///             type: object
///             properties: {}
///           historyMetadata:
///             # need to declare XML format!
///             description: metadata in XML format
///             type: object
///             properties: {}
///           profileImage:
///             # default is application/octet-stream, need to declare an image type only!
///             type: string
///             format: binary
///       encoding:
///         historyMetadata:
///           # require XML Content-Type in utf-8 encoding
///           contentType: application/xml; charset=utf-8
///         profileImage:
///           # only accept png/jpeg
///           contentType: image/png, image/jpeg
///           headers:
///             X-Rate-Limit-Limit:
///               description: The number of allowed requests in the current period
///               schema:
///                 type: integer
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Encoding {
    /// The Content-Type for encoding a specific property.
    /// Default value depends on the property type:
    /// - for `string` with `format` being `binary` – `application/octet-stream`;
    /// - for other primitive types – `text/plain`;
    /// - for `object` - `application/json`;
    /// - for `array` – the default is defined based on the inner type.
    ///
    /// The value can be a specific media type (e.g. `application/json`),
    /// a wildcard media type (e.g. `image/*`),
    /// or a comma-separated list of the two types.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,

    /// A map allowing additional information to be provided as headers, for example `Content-Disposition`.
    /// `Content-Type` is described separately and SHALL be ignored in this section.
    /// This property SHALL be ignored if the request body media type is not a `multipart`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, RefOr<Header>>>,

    /// Describes how a specific property value will be serialized depending on its type.
    /// See Parameter Object for details on the `style` property.
    /// The behavior follows the same values as query parameters, including default values.
    /// This property SHALL be ignored if the request body media type is not `application/x-www-form-urlencoded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<InQueryStyle>,

    /// When this is true, property values of type `array` or `object` generate separate parameters
    /// for each value of the array, or key-value-pair of the map.
    /// For other types of properties this property has no effect.
    /// When `style` is `form`, the default value is `true`.
    /// For all other styles, the default value is `false`.
    /// This property SHALL be ignored if the request body media type is not `application/x-www-form-urlencoded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,

    /// Determines whether the parameter value SHOULD allow reserved characters,
    /// as defined by [RFC3986](https://www.rfc-editor.org/rfc/rfc3986)
    /// `:/?#[]@!$&'()*+,;=` to be included without percent-encoding.
    /// The default value is `false`.
    /// This property SHALL be ignored if the request body media type is not `application/x-www-form-urlencoded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowReserved")]
    pub allow_reserved: Option<bool>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for MediaType {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(schema) = &self.schema {
            schema.validate_with_context(ctx, format!("{path}.schema"));
        }
        if let Some(examples) = &self.examples {
            for (name, example) in examples {
                example.validate_with_context(ctx, format!("{path}.examples[{name}]"));
            }
        }
        if let Some(encoding) = &self.encoding {
            for (name, encoding) in encoding {
                encoding.validate_with_context(ctx, format!("{path}.encoding[{name}]"));
            }
        }
    }
}

impl ValidateWithContext<Spec> for Encoding {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(headers) = &self.headers {
            for (name, header) in headers {
                header.validate_with_context(ctx, format!("{path}.headers[{name}]"));
            }
        }
    }
}
