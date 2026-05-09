//! Response Object

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_required_string};
use crate::v3_0::header::Header;
use crate::v3_0::link::Link;
use crate::v3_0::media_type::MediaType;
use crate::v3_0::reference::RefOr;
use crate::v3_0::spec::Spec;
use crate::validation::Options;
use lazy_regex::regex;
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// True if `key` is a 3-digit HTTP status code (100-599) or a wildcard
/// range token `1XX/2XX/3XX/4XX/5XX` (uppercase X). Per OAS 3.0.4 the
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

    /// Either a 3-digit HTTP status code (`"200"`) or one of the wildcard
    /// range tokens `1XX/2XX/3XX/4XX/5XX` (uppercase X). The string form is
    /// required because YAML would otherwise convert `200` to an integer.
    /// Explicit codes take precedence over the range that contains them.
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

    /// A map of operations links that can be followed from the response.
    /// The key is a short name for the link (constrained to the same naming
    /// pattern as Component map keys: `^[a-zA-Z0-9.\-_]+$`); the value is a
    /// `Link` Object or a `Reference` to one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, RefOr<Link>>>,

    /// ReDoc/Redocly extension with a short response summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-summary")]
    pub x_summary: Option<String>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
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
        if !ctx.is_option(Options::IgnoreEmptyResponseDescription) {
            validate_required_string(&self.description, ctx, format!("{path}.description"));
        }
        if let Some(headers) = &self.headers {
            for (name, header) in headers {
                header.validate_with_context(ctx, format!("{path}.headers[{name}]"));
            }
        }
        if let Some(media_types) = &self.content {
            for (name, media_type) in media_types {
                media_type.validate_with_context(ctx, format!("{path}.mediaTypes[{name}]"));
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
        // Spec: the Responses Object MUST contain at least one entry. The
        // 3.0.4 prose says "at least one response code", but tooling and the
        // spec's own examples treat a `default`-only Responses object as
        // valid (the OAS Linter accepts it; ReDoc renders it). We follow
        // that convention: any of `default` / one keyed code / one wildcard
        // satisfies the requirement.
        let has_any =
            self.default.is_some() || self.responses.as_ref().is_some_and(|m| !m.is_empty());
        if !has_any {
            ctx.error(
                path.clone(),
                "must declare at least one response (a status code, a wildcard like `2XX`, or `default`)",
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
    use crate::v3_0::parameter::InHeaderStyle;
    use crate::v3_0::schema::{ObjectSchema, Schema, SingleSchema};

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
                description: "A simple response".to_owned(),
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
                        MediaType {
                            schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                SingleSchema::Object(ObjectSchema {
                                    title: Some("foo".to_owned()),
                                    ..Default::default()
                                }),
                            )))),
                            ..Default::default()
                        },
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
                x_summary: None,
            },
            "response deserialization",
        );
    }

    #[test]
    fn test_response_serialization() {
        assert_eq!(
            serde_json::to_value(Response {
                description: "A simple response".to_owned(),
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
                        MediaType {
                            schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                SingleSchema::Object(ObjectSchema {
                                    title: Some("foo".to_owned()),
                                    ..Default::default()
                                }),
                            )))),
                            ..Default::default()
                        },
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
                x_summary: None,
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
                    description: "A simple response".to_owned(),
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
                            MediaType {
                                schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                    SingleSchema::Object(ObjectSchema {
                                        title: Some("foo".to_owned()),
                                        ..Default::default()
                                    }),
                                )))),
                                ..Default::default()
                            },
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
                    x_summary: None,
                })),
                responses: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            description: "200 OK".to_owned(),
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
                    description: "A simple response".to_owned(),
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
                            MediaType {
                                schema: Some(RefOr::new_item(Schema::Single(Box::new(
                                    SingleSchema::Object(ObjectSchema {
                                        title: Some("foo".to_owned()),
                                        ..Default::default()
                                    }),
                                )))),
                                ..Default::default()
                            },
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
                    x_summary: None,
                })),
                responses: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "200".to_owned(),
                        RefOr::new_item(Response {
                            description: "200 OK".to_owned(),
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
            description: "A simple response".to_owned(),
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
                    MediaType {
                        schema: Some(RefOr::new_item(Schema::Single(Box::new(
                            SingleSchema::Object(ObjectSchema {
                                title: Some("foo".to_owned()),
                                ..Default::default()
                            }),
                        )))),
                        ..Default::default()
                    },
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
            x_summary: None,
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
            description: "A simple response".to_owned(),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "response".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&spec, Options::new());
        Response::default().validate_with_context(&mut ctx, "response".to_owned());
        assert!(
            ctx.errors
                .contains(&"response.description: must not be empty".to_string()),
            "expected error: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(
            &spec,
            Options::only(&Options::IgnoreEmptyResponseDescription),
        );
        Response::default().validate_with_context(&mut ctx, "response".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }

    #[test]
    fn x_summary_round_trip() {
        let response: Response = serde_json::from_value(serde_json::json!({
            "description": "Created",
            "x-summary": "Created"
        }))
        .unwrap();
        assert_eq!(response.x_summary, Some("Created".to_owned()));
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({
                "description": "Created",
                "x-summary": "Created"
            })
        );
    }
}
