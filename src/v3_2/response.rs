//! Response Object

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v3_2::header::Header;
use crate::v3_2::link::Link;
use crate::v3_2::media_type::MediaType;
use crate::v3_2::spec::Spec;
use crate::validation::Options;
use lazy_regex::regex;
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// True if `key` is a 3-digit HTTP status code (100-599) or a wildcard
/// range token `1XX/2XX/3XX/4XX/5XX` (uppercase X). Per OAS 3.2.0 the
/// `Responses` object's patterned keys are exactly this union.
fn is_response_code_key(key: &str) -> bool {
    if let Ok(n) = key.parse::<u16>() {
        return (100..=599).contains(&n);
    }
    regex!(r"^[1-5]XX$").is_match(key)
}

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
    /// A short summary of the response (added in OAS 3.2). Intended for
    /// quick UI display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A description of the response.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    /// Optional in OAS 3.2 (the JSON Schema does not require it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

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
    pub content: Option<BTreeMap<String, RefOr<MediaType>>>,

    /// A map of operations links that can be followed from the response.
    /// The key of the map is a short name for the link, following the
    /// component-name pattern (`^[a-zA-Z0-9.\-_]+$`). Each value is a Link
    /// Object or a Reference Object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, RefOr<Link>>>,

    /// `^x-` Specification Extensions.
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
        const FIELDS: &[&str] = &[
            "default",
            "x-...",
            "<3-digit status code>",
            "1XX",
            "2XX",
            "3XX",
            "4XX",
            "5XX",
        ];

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
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else if is_response_code_key(key.as_str()) {
                        if responses.contains_key(key.as_str()) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        responses.insert(key, map.next_value()?);
                    } else {
                        return Err(Error::unknown_field(key.as_str(), FIELDS));
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
        // OAS 3.2: `description` is optional per the JSON Schema. When set,
        // the legacy `IgnoreEmptyResponseDescription` toggle is moot — but
        // we still flag a present-but-empty value (the field was set so it
        // ought to carry text), unless the toggle is on.
        if let Some(desc) = &self.description
            && desc.is_empty()
            && !ctx.is_option(Options::IgnoreEmptyResponseDescription)
        {
            ctx.error(format!("{path}.description"), "must not be empty");
        }
        if let Some(headers) = &self.headers {
            for (name, header) in headers {
                header.validate_with_context(ctx, format!("{path}.headers[{name}]"));
            }
        }
        if let Some(media_types) = &self.content {
            for (name, media_type) in media_types {
                media_type.validate_with_context(ctx, format!("{path}.content[{name}]"));
            }
        }
        if let Some(links) = &self.links {
            for (name, link) in links {
                link.validate_with_context(ctx, format!("{path}.links[{name}]"));
            }
        }
    }
}

impl ValidateWithContext<Spec> for Responses {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        // Per the OAS 3.2 JSON Schema, a Responses Object satisfies the
        // anyOf with either a `default` entry OR at least one status-code /
        // wildcard entry. Both shapes are valid; only an entirely empty
        // object is flagged.
        let has_default = self.default.is_some();
        let has_status_code = self.responses.as_ref().is_some_and(|m| !m.is_empty());
        if !has_default && !has_status_code {
            ctx.error(
                path.clone(),
                "must declare at least one response (`default` or a status code like `200` / wildcard like `2XX`)",
            );
        }
        if let Some(response) = &self.default {
            response.validate_with_context(ctx, format!("{path}.default"));
        }
        if let Some(responses) = &self.responses {
            for (name, response) in responses {
                if !is_response_code_key(name) {
                    ctx.error(
                        path.clone(),
                        format_args!(
                            "key must be a 3-digit status code (100-599) or one of `1XX/2XX/3XX/4XX/5XX`, found `{name}`"
                        ),
                    );
                }
                response.validate_with_context(ctx, format!("{path}.{name}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_2::parameter::InHeaderStyle;
    use crate::v3_2::schema::{ObjectSchema, Schema, SingleSchema};

    #[test]
    fn test_response_deserialize() {
        assert_eq!(
            serde_json::from_value::<Response>(serde_json::json!({
                "description": "A simple response",
                "headers": {
                    "Authorization": {
                        "description": "A short description of the header.",
                        "style": "simple",
                        "required": true,
                    },
                },
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "title": "foo"
                        }
                    }
                },
                "links": {
                    "next": {
                        "operationRef": "getNextPage",
                        "description": "Get the next page of results"
                    }
                },
                "x-extra": "extension",
            }))
            .unwrap(),
            Response {
                summary: None,
                description: Some("A simple response".to_owned()),
                headers: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "Authorization".to_owned(),
                        RefOr::new_item(Header {
                            description: Some("A short description of the header.".to_owned()),
                            required: Some(true),
                            style: Some(InHeaderStyle::Simple),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                content: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "application/json".to_owned(),
                        RefOr::new_item(MediaType {
                            schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                SingleSchema::Object(ObjectSchema {
                                    title: Some("foo".to_owned()),
                                    ..Default::default()
                                }),
                            )))),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                links: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "next".to_owned(),
                        RefOr::new_item(Link {
                            operation_ref: Some("getNextPage".to_owned()),
                            description: Some("Get the next page of results".to_owned()),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                    map
                }),
            },
            "response deserialization",
        );
    }

    #[test]
    fn test_response_serialization() {
        assert_eq!(
            serde_json::to_value(Response {
                summary: None,
                description: Some("A simple response".to_owned()),
                headers: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "Authorization".to_owned(),
                        RefOr::new_item(Header {
                            description: Some("A short description of the header.".to_owned()),
                            required: Some(true),
                            style: Some(InHeaderStyle::Simple),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                content: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "application/json".to_owned(),
                        RefOr::new_item(MediaType {
                            schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                SingleSchema::Object(ObjectSchema {
                                    title: Some("foo".to_owned()),
                                    ..Default::default()
                                }),
                            )))),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                links: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "next".to_owned(),
                        RefOr::new_item(Link {
                            operation_ref: Some("getNextPage".to_owned()),
                            description: Some("Get the next page of results".to_owned()),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                    map
                }),
            })
            .unwrap(),
            serde_json::json!({
                "description": "A simple response",
                "headers": {
                    "Authorization": {
                        "description": "A short description of the header.",
                        "style": "simple",
                        "required": true,
                    },
                },
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "title": "foo"
                        }
                    }
                },
                "links": {
                    "next": {
                        "operationRef": "getNextPage",
                        "description": "Get the next page of results"
                    }
                },
                "x-extra": "extension",
            }),
            "response serialization",
        );
    }

    #[test]
    fn test_responses_deserialize() {
        assert_eq!(
            serde_json::from_value::<Responses>(serde_json::json!({
                "default": {
                    "description": "A simple response",
                    "headers": {
                        "Authorization": {
                            "description": "A short description of the header.",
                            "style": "simple",
                            "required": true,
                        },
                    },
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "title": "foo"
                            }
                        }
                    },
                    "links": {
                        "next": {
                            "operationRef": "getNextPage",
                            "description": "Get the next page of results"
                        }
                    },
                    "x-extra": "extension",
                },
                "200": {
                    "description": "200 OK"
                },
                "x-extra": "extension",
            }))
            .unwrap(),
            Responses {
                default: Some(RefOr::new_item(Response {
                    summary: None,
                    description: Some("A simple response".to_owned()),
                    headers: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "Authorization".to_owned(),
                            RefOr::new_item(Header {
                                description: Some("A short description of the header.".to_owned()),
                                required: Some(true),
                                style: Some(InHeaderStyle::Simple),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    content: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "application/json".to_owned(),
                            RefOr::new_item(MediaType {
                                schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                    SingleSchema::Object(ObjectSchema {
                                        title: Some("foo".to_owned()),
                                        ..Default::default()
                                    }),
                                )))),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    links: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "next".to_owned(),
                            RefOr::new_item(Link {
                                operation_ref: Some("getNextPage".to_owned()),
                                description: Some("Get the next page of results".to_owned()),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    extensions: Some({
                        let mut map = BTreeMap::new();
                        map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                        map
                    }),
                })),
                responses: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            summary: None,
                            description: Some("200 OK".to_owned()),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                    map
                }),
            },
            "responses deserialization",
        );
    }

    #[test]
    fn test_responses_serialization() {
        assert_eq!(
            serde_json::to_value(Responses {
                default: Some(RefOr::new_item(Response {
                    summary: None,
                    description: Some("A simple response".to_owned()),
                    headers: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "Authorization".to_owned(),
                            RefOr::new_item(Header {
                                description: Some("A short description of the header.".to_owned()),
                                required: Some(true),
                                style: Some(InHeaderStyle::Simple),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    content: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "application/json".to_owned(),
                            RefOr::new_item(MediaType {
                                schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                    SingleSchema::Object(ObjectSchema {
                                        title: Some("foo".to_owned()),
                                        ..Default::default()
                                    }),
                                )))),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    links: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "next".to_owned(),
                            RefOr::new_item(Link {
                                operation_ref: Some("getNextPage".to_owned()),
                                description: Some("Get the next page of results".to_owned()),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    extensions: Some({
                        let mut map = BTreeMap::new();
                        map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                        map
                    }),
                })),
                responses: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            summary: None,
                            description: Some("200 OK".to_owned()),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                    map
                }),
            })
            .unwrap(),
            serde_json::json!({
                "default": {
                    "description": "A simple response",
                    "headers": {
                        "Authorization": {
                            "description": "A short description of the header.",
                            "style": "simple",
                            "required": true,
                        },
                    },
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "title": "foo"
                            }
                        }
                    },
                    "links": {
                        "next": {
                            "operationRef": "getNextPage",
                            "description": "Get the next page of results"
                        }
                    },
                    "x-extra": "extension",
                },
                "200": {
                    "description": "200 OK"
                },
                "x-extra": "extension",
            }),
            "response serialization",
        );
    }

    #[test]
    fn test_response_validate() {
        let spec = Spec::default();

        let mut ctx = Context::new(&spec, Options::new());
        Response {
            summary: None,
            description: Some("A simple response".to_owned()),
            headers: Some({
                let mut map = BTreeMap::new();
                map.insert(
                    "Authorization".to_owned(),
                    RefOr::new_item(Header {
                        description: Some("A short description of the header.".to_owned()),
                        required: Some(true),
                        style: Some(InHeaderStyle::Simple),
                        schema: Some(RefOr::new_item(Schema::Single(Box::new(
                            SingleSchema::Object(ObjectSchema::default()),
                        )))),
                        ..Default::default()
                    }),
                );
                map
            }),
            content: Some({
                let mut map = BTreeMap::new();
                map.insert(
                    "application/json".to_owned(),
                    RefOr::new_item(MediaType {
                        schema: Some(RefOr::new_item(Schema::Single(Box::new(
                            SingleSchema::Object(ObjectSchema {
                                title: Some("foo".to_owned()),
                                ..Default::default()
                            }),
                        )))),
                        ..Default::default()
                    }),
                );
                map
            }),
            links: Some({
                let mut map = BTreeMap::new();
                map.insert(
                    "next".to_owned(),
                    RefOr::new_item(Link {
                        operation_id: Some("getNextPage".to_owned()),
                        description: Some("Get the next page of results".to_owned()),
                        ..Default::default()
                    }),
                );
                map
            }),
            extensions: Some({
                let mut map = BTreeMap::new();
                map.insert("x-extra".to_owned(), serde_json::json!("extension"));
                map
            }),
        }
        .validate_with_context(&mut ctx, "response".to_owned());
        // The unknown operationId surfaces a single Link error; for this
        // test we just confirm Response itself does not emit anything else.
        assert!(
            ctx.errors
                .iter()
                .all(|e| e.contains("missing operation with id")),
            "unexpected errors: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&spec, Options::new());
        Response {
            summary: None,
            description: Some("A simple response".to_owned()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "response".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        // OAS 3.2 makes `description` optional — a Response without
        // it is now valid.
        let mut ctx = Context::new(&spec, Options::new());
        Response::default().validate_with_context(&mut ctx, "response".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        // But a present-but-empty description still flags (the field
        // was set, so it ought to carry text).
        let mut ctx = Context::new(&spec, Options::new());
        Response {
            description: Some(String::new()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "response".to_owned());
        assert!(
            ctx.errors
                .contains(&"response.description: must not be empty".to_string()),
            "expected error: {:?}",
            ctx.errors
        );

        // IgnoreEmptyResponseDescription silences the present-but-empty case.
        let mut ctx = Context::new(
            &spec,
            Options::only(&Options::IgnoreEmptyResponseDescription),
        );
        Response {
            description: Some(String::new()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "response".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }
}
