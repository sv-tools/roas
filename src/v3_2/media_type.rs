//! Provides schema and examples for the media type

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v3_2::example::Example;
use crate::v3_2::header::Header;
use crate::v3_2::parameter::InQueryStyle;
use crate::v3_2::schema::Schema;
use crate::v3_2::spec::Spec;

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
    /// A description of the media type entry (added in OAS 3.2).
    /// CommonMark syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The schema defining the content of the request, response, or parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<RefOr<Schema>>,

    /// The schema defining each item within a sequential media type
    /// (added in OAS 3.2). Used for line-delimited JSON, JSON Lines,
    /// Server-Sent Events, and other stream-of-records media types
    /// where `schema` would describe the whole stream.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "itemSchema")]
    pub item_schema: Option<RefOr<Schema>>,

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
    /// Mutually exclusive with `prefixEncoding` and `itemEncoding`
    /// (an entry MAY use one shape or the other, not both).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<BTreeMap<String, Encoding>>,

    /// Encoding for the prefix (header / framing) portion(s) of a sequential
    /// media type. Added in OAS 3.2. An array of Encoding Objects, applied
    /// in order to the leading prefix items. Mutually exclusive with
    /// `encoding`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "prefixEncoding")]
    pub prefix_encoding: Option<Vec<Encoding>>,

    /// Encoding applied per-item to a sequential media type
    /// (e.g. JSON Lines, Server-Sent Events). Added in OAS 3.2. A single
    /// Encoding Object, not a map. Mutually exclusive with `encoding`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "itemEncoding")]
    pub item_encoding: Option<Encoding>,

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

    /// Nested encoding for properties when this Encoding describes a
    /// `multipart/form-data` part whose body is itself structured.
    /// Added in OAS 3.2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<BTreeMap<String, Encoding>>,

    /// Nested prefix-encoding(s) for sequential parts. Added in OAS 3.2.
    /// Mutually exclusive with `encoding`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "prefixEncoding")]
    pub prefix_encoding: Option<Vec<Encoding>>,

    /// Nested per-item encoding for sequential parts. Added in OAS 3.2.
    /// Mutually exclusive with `encoding`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "itemEncoding")]
    pub item_encoding: Option<Box<Encoding>>,

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
        if self.example.is_some() && self.examples.is_some() {
            ctx.error(path.clone(), "example and examples are mutually exclusive");
        }
        if let Some(schema) = &self.schema {
            schema.validate_with_context(ctx, format!("{path}.schema"));
        }
        if let Some(schema) = &self.item_schema {
            schema.validate_with_context(ctx, format!("{path}.itemSchema"));
        }
        if let Some(examples) = &self.examples {
            for (name, example) in examples {
                example.validate_with_context(ctx, format!("{path}.examples[{name}]"));
            }
        }
        // OAS 3.2: `encoding` MUST NOT coexist with `prefixEncoding` or
        // `itemEncoding` — those describe sequential media types whereas
        // `encoding` describes a multipart/form-data property map.
        if self.encoding.is_some()
            && (self.prefix_encoding.is_some() || self.item_encoding.is_some())
        {
            ctx.error(
                path.clone(),
                "`encoding` is mutually exclusive with `prefixEncoding`/`itemEncoding`",
            );
        }
        if let Some(encoding) = &self.encoding {
            for (name, encoding) in encoding {
                encoding.validate_with_context(ctx, format!("{path}.encoding[{name}]"));
            }
        }
        if let Some(encodings) = &self.prefix_encoding {
            for (i, encoding) in encodings.iter().enumerate() {
                encoding.validate_with_context(ctx, format!("{path}.prefixEncoding[{i}]"));
            }
        }
        if let Some(encoding) = &self.item_encoding {
            encoding.validate_with_context(ctx, format!("{path}.itemEncoding"));
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
        // OAS 3.2 nested encoding: same `encoding` ⊕ `prefixEncoding`
        // / `itemEncoding` mutex as on MediaType.
        if self.encoding.is_some()
            && (self.prefix_encoding.is_some() || self.item_encoding.is_some())
        {
            ctx.error(
                path.clone(),
                "`encoding` is mutually exclusive with `prefixEncoding`/`itemEncoding`",
            );
        }
        if let Some(encoding) = &self.encoding {
            for (name, encoding) in encoding {
                encoding.validate_with_context(ctx, format!("{path}.encoding[{name}]"));
            }
        }
        if let Some(encodings) = &self.prefix_encoding {
            for (i, encoding) in encodings.iter().enumerate() {
                encoding.validate_with_context(ctx, format!("{path}.prefixEncoding[{i}]"));
            }
        }
        if let Some(encoding) = &self.item_encoding {
            encoding.validate_with_context(ctx, format!("{path}.itemEncoding"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::validation::Options;

    #[test]
    fn media_type_round_trip_full() {
        let v = serde_json::json!({
            "schema": {"type": "object"},
            "example": {"a": 1},
            "encoding": {
                "field": {
                    "contentType": "image/png, image/jpeg",
                    "headers": {
                        "X-Custom": {"description": "h", "schema": {"type": "string"}}
                    },
                    "style": "form",
                    "explode": true,
                    "allowReserved": false
                }
            },
            "x-extra": "yes"
        });
        let mt: MediaType = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&mt).unwrap(), v);
    }

    #[test]
    fn validate_walks_examples_and_encoding_headers() {
        // Examples and encoding-headers should each invoke their nested
        // validators (covered branches at media_type.rs:191-205).
        use crate::v3_2::header::Header;
        use crate::v3_2::schema::{ObjectSchema, Schema, SingleSchema};
        let mut examples = BTreeMap::new();
        examples.insert(
            "ex1".to_owned(),
            RefOr::new_item(crate::v3_2::example::Example {
                value: Some(serde_json::json!(1)),
                external_value: Some("https://example.com/x.json".into()),
                ..Default::default()
            }),
        );
        let mut headers = BTreeMap::new();
        headers.insert(
            "X-Bad".to_owned(),
            RefOr::new_item(Header {
                example: Some(serde_json::json!(1)),
                examples: Some(BTreeMap::from([(
                    "a".into(),
                    RefOr::new_item(crate::v3_2::example::Example::default()),
                )])),
                schema: Some(RefOr::new_item(Schema::Single(Box::new(
                    SingleSchema::Object(ObjectSchema::default()),
                )))),
                ..Default::default()
            }),
        );
        let mut encoding = BTreeMap::new();
        encoding.insert(
            "field".to_owned(),
            Encoding {
                content_type: None,
                headers: Some(headers),
                style: None,
                explode: None,
                allow_reserved: None,
                encoding: None,
                prefix_encoding: None,
                item_encoding: None,
                extensions: None,
            },
        );
        let mt = MediaType {
            description: None,
            schema: None,
            item_schema: None,
            example: None,
            examples: Some(examples),
            encoding: Some(encoding),
            prefix_encoding: None,
            item_encoding: None,
            extensions: None,
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        mt.validate_with_context(&mut ctx, "mt".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("examples[ex1]")),
            "expected nested example error: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("encoding[field].headers[X-Bad]")),
            "expected nested header error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_example_examples_xor() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        let mut examples = BTreeMap::new();
        examples.insert(
            "a".to_owned(),
            RefOr::new_item(crate::v3_2::example::Example::default()),
        );
        MediaType {
            example: Some(serde_json::json!("e")),
            examples: Some(examples),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "mt".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("example and examples are mutually exclusive")),
            "errors: {:?}",
            ctx.errors
        );
    }
}
