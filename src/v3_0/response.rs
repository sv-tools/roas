//! Response Object

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_required_string};
use crate::common::reference::RefOr;
use crate::v3_0::header::Header;
use crate::v3_0::link::Link;
use crate::v3_0::media_type::MediaType;
use crate::v3_0::spec::Spec;

/// A container for the expected responses of an operation.
/// The container maps a HTTP response code to the expected response.
///
/// The documentation is not necessarily expected to cover all possible HTTP response codes
/// because they may not be known in advance.
/// However, documentation is expected to cover a successful operation response and any known errors.
///
/// The `default` MAY be used as a default response object for all HTTP codes that are
/// not covered individually by the specification.
///
/// The `Responses Object` MUST contain at least one response code,
/// and it SHOULD be the response for a successful operation call.
///
/// Specification example:
/// ```yaml
/// '200':
///   description: a pet to be returned
///   content:
///     application/json:
///       schema:
///         $ref: '#/components/schemas/Pet'
/// default:
///   description: Unexpected error
///   content:
///     application/json:
///       schema:
///         $ref: '#/components/schemas/ErrorModel'
/// ```
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Responses {
    /// The documentation of responses other than the ones declared for specific HTTP response codes.
    /// Use this field to cover undeclared responses.
    /// A Reference Object can link to a response that the OpenAPI Object’s components/responses
    /// section defines.
    pub default: Option<RefOr<Response>>,

    /// Any HTTP status code can be used as the property name,
    /// but only one property per code,
    /// to describe the expected response for that HTTP status code.
    /// A Reference Object can link to a response that is defined in the OpenAPI Object’s
    /// components/responses section.
    /// This field MUST be enclosed in quotation marks (for example, “200”) for compatibility
    /// between JSON and YAML.
    /// To define a range of response codes, this field MAY contain the uppercase wildcard character `X`.
    /// For example, `2XX` represents all response codes between `[200-299]`.
    /// Only the following range definitions are allowed: `1XX`, `2XX`, `3XX`, `4XX`, and `5XX`.
    /// If a response is defined using an explicit code,
    /// the explicit code definition takes precedence over the range definition for that code.
    pub responses: Option<BTreeMap<String, RefOr<Response>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Response {
    /// **Required** A short description of the response.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    pub description: String,

    /// Maps a header name to its definition.
    /// [RFC7230](https://www.rfc-editor.org/rfc/rfc7230) states header names are case insensitive.
    /// If a response header is defined with the name `"Content-Type"`, it SHALL be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, RefOr<Header>>>,

    /// A map containing descriptions of potential response payloads.
    /// The key is a media type or media type range and the value describes it.
    /// For responses that match multiple keys, only the most specific key is applicable.
    /// e.g. `text/plain` overrides `text/*`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<BTreeMap<String, MediaType>>,

    /// Maps a header name to its definition.
    /// [RFC7230](https://www.rfc-editor.org/rfc/rfc7230) states header names are case insensitive.
    /// If a response header is defined with the name `"Content-Type"`, it SHALL be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, RefOr<Link>>>,

    /// A map of operations links that can be followed from the response.
    /// The key of the map is a short name for the link,
    /// following the naming constraints of the names for Component Objects.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Serialize for Responses {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        if let Some(ref default) = self.default {
            map.serialize_entry("default", default)?;
        }

        if let Some(ref responses) = self.responses {
            for (k, v) in responses {
                map.serialize_entry(&k, &v)?;
            }
        }

        if let Some(ref ext) = self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(&k, &v)?;
                }
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for Responses {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const FIELDS: &[&str] = &["default", "x-...", "1xx", "2xx", "3xx", "4xx", "5xx"];

        struct ResponsesVisitor;

        impl<'de> Visitor<'de> for ResponsesVisitor {
            type Value = Responses;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Responses")
            }

            fn visit_map<V>(self, mut map: V) -> Result<Responses, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut res = Responses::default();
                let mut responses: BTreeMap<String, RefOr<Response>> = BTreeMap::new();
                let mut extensions: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "default" {
                        if res.default.is_some() {
                            return Err(Error::duplicate_field("default"));
                        }
                        res.default = Some(map.next_value()?);
                    } else if key.starts_with("x-") {
                        if extensions.contains_key(key.as_str()) {
                            return Err(Error::custom(format_args!("duplicate field `{}`", key)));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else {
                        match key.parse::<u16>() {
                            Ok(100..=599) => {
                                if responses.contains_key(key.as_str()) {
                                    return Err(Error::custom(format_args!(
                                        "duplicate field `{}`",
                                        key
                                    )));
                                }
                                responses.insert(key, map.next_value()?);
                            }
                            _ => return Err(Error::unknown_field(key.as_str(), FIELDS)),
                        }
                    }
                }
                if !responses.is_empty() {
                    res.responses = Some(responses);
                }
                if !extensions.is_empty() {
                    res.extensions = Some(extensions);
                }
                Ok(res)
            }
        }

        deserializer.deserialize_struct("Responses", FIELDS, ResponsesVisitor)
    }
}

impl ValidateWithContext<Spec> for Response {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.description, ctx, format!("{}.description", path));
        if let Some(headers) = &self.headers {
            for (name, header) in headers {
                header.validate_with_context(ctx, format!("{}.headers[{}]", path, name));
            }
        }
        if let Some(media_types) = &self.content {
            for (name, media_type) in media_types {
                media_type.validate_with_context(ctx, format!("{}.mediaTypes[{}]", path, name));
            }
        }
        if let Some(links) = &self.links {
            for (name, link) in links {
                link.validate_with_context(ctx, format!("{}.links[{}]", path, name));
            }
        }
    }
}

impl ValidateWithContext<Spec> for Responses {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(response) = &self.default {
            response.validate_with_context(ctx, format!("{}.default", path));
        }
        if let Some(responses) = &self.responses {
            for (name, response) in responses {
                match name.parse::<u16>() {
                    Ok(100..=599) => {}
                    _ => {
                        ctx.error(
                            path.clone(),
                            format_args!(
                                "name must be an integer within [100..599] range, found `{}`",
                                name
                            ),
                        );
                    }
                }
                response.validate_with_context(ctx, format!("{}.{}", path, name));
            }
        }
    }
}
