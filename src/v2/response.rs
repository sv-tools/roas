//! Response Object

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::common::helpers::{validate_required_string, Context, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v2::header::Header;
use crate::v2::schema::Schema;
use crate::v2::spec::Spec;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Responses {
    /// The documentation of responses other than the ones declared for specific HTTP response codes.
    /// It can be used to cover undeclared responses.
    /// Reference Object can be used to link to a response that is defined
    /// at the Swagger Object's responses section.
    pub default: Option<RefOr<Response>>,

    /// Any HTTP status code can be used as the property name (one property per HTTP status code).
    /// Describes the expected response for that HTTP status code.
    /// Reference Object can be used to link to a response that is defined
    /// at the Swagger Object's responses section.
    pub responses: Option<BTreeMap<String, RefOr<Response>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Response {
    /// **Required** A short description of the response.
    /// [GFM](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#-git-hub-flavored-markdown) syntax can be used for rich text representation.
    pub description: String,

    /// A definition of the response structure.
    /// It can be a primitive, an array or an object.
    /// If this field does not exist, it means no content is returned as part of the response.
    /// As an extension to the Schema Object, its root type value may also be "file".
    /// This SHOULD be accompanied by a relevant produces mime-type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<RefOr<Schema>>,

    /// A list of headers that are sent with the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, Header>>,

    /// An example of the response message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
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
        if let Some(schema) = &self.schema {
            schema.validate_with_context(ctx, format!("{}.schema", path));
        }
        if let Some(headers) = &self.headers {
            for (name, header) in headers {
                header.validate_with_context(ctx, format!("{}.headers.{}", path, name));
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
                        ctx.errors.push(format!(
                            "{}: name must be an integer within [100..599] range, found `{}`",
                            path.clone(),
                            name
                        ));
                    }
                }
                response.validate_with_context(ctx, format!("{}.{}", path, name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::common::reference::Ref;
    use crate::v2::header::{IntegerHeader, StringHeader};

    use super::*;

    #[test]
    fn test_response_deserialize() {
        assert_eq!(
            serde_json::from_value::<Response>(serde_json::json!({
                "description": "A simple response",
                "headers": {
                    "Authorization": {
                        "description": "The bearer token to use in all other requests",
                        "type": "string",
                        "pattern": r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#
                    },
                    "X-Rate-Limit-Limit": {
                        "description": "The number of allowed requests in the current period",
                        "type": "integer"
                    }
                },
                "examples": {
                    "foo": "bar",
                    "baz": 42,
                },
                "x-extra": "extension",
            }))
            .unwrap(),
            Response {
                description: "A simple response".to_string(),
                schema: None,
                headers: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "Authorization".to_string(),
                        Header::String(StringHeader {
                            description: Some(
                                "The bearer token to use in all other requests".to_string(),
                            ),
                            pattern: Some(r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#.to_string()),
                            ..Default::default()
                        }),
                    );
                    map.insert(
                        "X-Rate-Limit-Limit".to_string(),
                        Header::Integer(IntegerHeader {
                            description: Some(
                                "The number of allowed requests in the current period".to_string(),
                            ),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                examples: Some({
                    let mut map = BTreeMap::new();
                    map.insert("foo".to_string(), serde_json::json!("bar"));
                    map.insert("baz".to_string(), serde_json::json!(42));
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_string(), serde_json::json!("extension"));
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
                description: "A simple response".to_string(),
                schema: None,
                headers: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "Authorization".to_string(),
                        Header::String(StringHeader {
                            description: Some(
                                "The bearer token to use in all other requests".to_string(),
                            ),
                            pattern: Some(r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#.to_string()),
                            ..Default::default()
                        }),
                    );
                    map.insert(
                        "X-Rate-Limit-Limit".to_string(),
                        Header::Integer(IntegerHeader {
                            description: Some(
                                "The number of allowed requests in the current period".to_string(),
                            ),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                examples: Some({
                    let mut map = BTreeMap::new();
                    map.insert("foo".to_string(), serde_json::json!("bar"));
                    map.insert("baz".to_string(), serde_json::json!(42));
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_string(), serde_json::json!("extension"));
                    map
                }),
            })
            .unwrap(),
            serde_json::json!({
                "description": "A simple response",
                "headers": {
                    "Authorization": {
                        "description": "The bearer token to use in all other requests",
                        "type": "string",
                        "pattern": r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#
                    },
                    "X-Rate-Limit-Limit": {
                        "description": "The number of allowed requests in the current period",
                        "type": "integer"
                    }
                },
                "examples": {
                    "foo": "bar",
                    "baz": 42,
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
                            "description": "The bearer token to use in all other requests",
                            "type": "string",
                            "pattern": r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#
                        },
                        "X-Rate-Limit-Limit": {
                            "description": "The number of allowed requests in the current period",
                            "type": "integer"
                        }
                    },
                    "examples": {
                        "foo": "bar",
                        "baz": 42,
                    },
                    "x-extra": "extension",
                },
                "200": {
                    "description": "A simple response",
                    "headers": {
                        "Authorization": {
                            "description": "The bearer token to use in all other requests",
                            "type": "string",
                            "pattern": r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#
                        },
                        "X-Rate-Limit-Limit": {
                            "description": "The number of allowed requests in the current period",
                            "type": "integer"
                        }
                    },
                    "examples": {
                        "foo": "bar",
                        "baz": 42,
                    },
                    "x-extra": "extension",
                },
                "404": {
                    "$ref": "#/components/responses/NotFound",
                },
                "x-extra": "extension",
            }))
            .unwrap(),
            Responses {
                default: Some(RefOr::Item(Response {
                    description: "A simple response".to_string(),
                    schema: None,
                    headers: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "Authorization".to_string(),
                            Header::String(StringHeader {
                                description: Some(
                                    "The bearer token to use in all other requests".to_string(),
                                ),
                                pattern: Some(r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#.to_string()),
                                ..Default::default()
                            }),
                        );
                        map.insert(
                            "X-Rate-Limit-Limit".to_string(),
                            Header::Integer(IntegerHeader {
                                description: Some(
                                    "The number of allowed requests in the current period"
                                        .to_string(),
                                ),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    examples: Some({
                        let mut map = BTreeMap::new();
                        map.insert("foo".to_string(), serde_json::json!("bar"));
                        map.insert("baz".to_string(), serde_json::json!(42));
                        map
                    }),
                    extensions: Some({
                        let mut map = BTreeMap::new();
                        map.insert("x-extra".to_string(), serde_json::json!("extension"));
                        map
                    }),
                })),
                responses: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "200".to_string(),
                        RefOr::Item(Response {
                            description: "A simple response".to_string(),
                            schema: None,
                            headers: Some({
                                let mut map = BTreeMap::new();
                                map.insert(
                                    "Authorization".to_string(),
                                    Header::String(StringHeader {
                                        description: Some(
                                            "The bearer token to use in all other requests"
                                                .to_string(),
                                        ),
                                        pattern: Some(
                                            r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#.to_string(),
                                        ),
                                        ..Default::default()
                                    }),
                                );
                                map.insert(
                                    "X-Rate-Limit-Limit".to_string(),
                                    Header::Integer(IntegerHeader {
                                        description: Some(
                                            "The number of allowed requests in the current period"
                                                .to_string(),
                                        ),
                                        ..Default::default()
                                    }),
                                );
                                map
                            }),
                            examples: Some({
                                let mut map = BTreeMap::new();
                                map.insert("foo".to_string(), serde_json::json!("bar"));
                                map.insert("baz".to_string(), serde_json::json!(42));
                                map
                            }),
                            extensions: Some({
                                let mut map = BTreeMap::new();
                                map.insert("x-extra".to_string(), serde_json::json!("extension"));
                                map
                            }),
                        }),
                    );
                    map.insert(
                        "404".to_string(),
                        RefOr::Ref(Ref {
                            reference: "#/components/responses/NotFound".to_string(),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_string(), serde_json::json!("extension"));
                    map
                }),
            },
            "responses deserialization",
        );

        assert_eq!(
            serde_json::from_value::<Responses>(serde_json::json!({
                "foo": {
                    "description": "A simple response",
                },
            }))
                .unwrap_err()
                .to_string(),
            "unknown field `foo`, expected one of `default`, `x-...`, `1xx`, `2xx`, `3xx`, `4xx`, `5xx`",
            "responses deserialization with invalid status code",
        );

        assert_eq!(
            serde_json::from_value::<Responses>(serde_json::json!({
                "600": {
                    "description": "A simple response",
                },
            }))
                .unwrap_err()
                .to_string(),
            "unknown field `600`, expected one of `default`, `x-...`, `1xx`, `2xx`, `3xx`, `4xx`, `5xx`",
            "responses deserialization with 600 as status code",
        );

        assert_eq!(
            serde_json::from_value::<Responses>(serde_json::json!({
                "42": {
                    "description": "A simple response",
                },
            }))
                .unwrap_err()
                .to_string(),
            "unknown field `42`, expected one of `default`, `x-...`, `1xx`, `2xx`, `3xx`, `4xx`, `5xx`",
            "responses deserialization with 42 as status code",
        );
        assert_eq!(
            serde_json::from_str::<Responses>(r#"{"200":{"description":"A simple response"},"200":{"description":"A duplicate response"}}"#)
                .unwrap_err()
                .to_string(),
            "duplicate field `200` at line 1 column 48",
            "responses deserialization with duplicate field",
        );
    }

    #[test]
    fn test_responses_serialize() {
        assert_eq!(
            serde_json::to_value(Responses {
                default: Some(RefOr::Item(Response {
                    description: "A simple response".to_string(),
                    schema: None,
                    headers: Some({
                        let mut map = BTreeMap::new();
                        map.insert(
                            "Authorization".to_string(),
                            Header::String(StringHeader {
                                description: Some(
                                    "The bearer token to use in all other requests".to_string(),
                                ),
                                pattern: Some(r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#.to_string()),
                                ..Default::default()
                            }),
                        );
                        map.insert(
                            "X-Rate-Limit-Limit".to_string(),
                            Header::Integer(IntegerHeader {
                                description: Some(
                                    "The number of allowed requests in the current period"
                                        .to_string(),
                                ),
                                ..Default::default()
                            }),
                        );
                        map
                    }),
                    examples: Some({
                        let mut map = BTreeMap::new();
                        map.insert("foo".to_string(), serde_json::json!("bar"));
                        map.insert("baz".to_string(), serde_json::json!(42));
                        map
                    }),
                    extensions: Some({
                        let mut map = BTreeMap::new();
                        map.insert("x-extra".to_string(), serde_json::json!("extension"));
                        map
                    }),
                })),
                responses: Some({
                    let mut map = BTreeMap::new();
                    map.insert(
                        "200".to_string(),
                        RefOr::Item(Response {
                            description: "A simple response".to_string(),
                            schema: None,
                            headers: Some({
                                let mut map = BTreeMap::new();
                                map.insert(
                                    "Authorization".to_string(),
                                    Header::String(StringHeader {
                                        description: Some(
                                            "The bearer token to use in all other requests"
                                                .to_string(),
                                        ),
                                        pattern: Some(
                                            r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#.to_string(),
                                        ),
                                        ..Default::default()
                                    }),
                                );
                                map.insert(
                                    "X-Rate-Limit-Limit".to_string(),
                                    Header::Integer(IntegerHeader {
                                        description: Some(
                                            "The number of allowed requests in the current period"
                                                .to_string(),
                                        ),
                                        ..Default::default()
                                    }),
                                );
                                map
                            }),
                            examples: Some({
                                let mut map = BTreeMap::new();
                                map.insert("foo".to_string(), serde_json::json!("bar"));
                                map.insert("baz".to_string(), serde_json::json!(42));
                                map
                            }),
                            extensions: Some({
                                let mut map = BTreeMap::new();
                                map.insert("x-extra".to_string(), serde_json::json!("extension"));
                                map
                            }),
                        }),
                    );
                    map.insert(
                        "404".to_string(),
                        RefOr::Ref(Ref {
                            reference: "#/components/responses/NotFound".to_string(),
                            ..Default::default()
                        }),
                    );
                    map
                }),
                extensions: Some({
                    let mut map = BTreeMap::new();
                    map.insert("x-extra".to_string(), serde_json::json!("extension"));
                    map
                }),
            })
            .unwrap(),
            serde_json::json!({
                "default": {
                    "description": "A simple response",
                    "headers": {
                        "Authorization": {
                            "description": "The bearer token to use in all other requests",
                            "type": "string",
                            "pattern": r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#
                        },
                        "X-Rate-Limit-Limit": {
                            "description": "The number of allowed requests in the current period",
                            "type": "integer"
                        }
                    },
                    "examples": {
                        "foo": "bar",
                        "baz": 42,
                    },
                    "x-extra": "extension",
                },
                "200": {
                    "description": "A simple response",
                    "headers": {
                        "Authorization": {
                            "description": "The bearer token to use in all other requests",
                            "type": "string",
                            "pattern": r#""^Bearer [a-zA-Z0-9-._~+/]+={0,2}$""#
                        },
                        "X-Rate-Limit-Limit": {
                            "description": "The number of allowed requests in the current period",
                            "type": "integer"
                        }
                    },
                    "examples": {
                        "foo": "bar",
                        "baz": 42,
                    },
                    "x-extra": "extension",
                },
                "404": {
                    "$ref": "#/components/responses/NotFound",
                },
                "x-extra": "extension",
            }),
            "responses serialization",
        );
    }
}
