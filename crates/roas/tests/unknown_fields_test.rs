//! Keys an object does not define: kept on parsing, reported by validation unless
//! `Options::IgnoreUnknownFields` is set, and left out by the version conversions.

#![cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2"))]

use roas::validation::{Options, Validate};
use serde_json::json;

/// The paths validation reports as unknown fields.
fn unknown_field_paths(spec: &impl Validate, options: enumset::EnumSet<Options>) -> Vec<String> {
    let Err(error) = spec.validate(options, None) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = error
        .errors
        .iter()
        .filter(|error| error.message.starts_with("not a field of this object"))
        .map(|error| error.path.clone())
        .collect();
    paths.sort();
    paths
}

/// A v3.x document with a key OpenAPI does not define (`embed`) on several objects, an `x-`
/// extension beside it, and a schema keyword the model does not know.
#[cfg(any(feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
fn v3_document(version: &str) -> serde_json::Value {
    json!({
        "openapi": version,
        "info": {"title": "t", "version": "1", "embed": true, "x-keep": 1},
        "paths": {"/x": {"post": {
            "embed": true,
            "parameters": [{"name": "q", "in": "query", "schema": {"type": "string"}, "embed": true}],
            "requestBody": {"embed": true, "content": {"application/json": {
                "schema": {"$ref": "#/components/schemas/Cat"}, "embed": true
            }}},
            "responses": {"200": {"description": "ok", "embed": true}}
        }}},
        "components": {"schemas": {"Cat": {"type": "object", "embed": true}}},
        "embed": true
    })
}

/// Where `v3_document` puts `embed` outside its schema.
#[cfg(any(feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
const V3_UNKNOWN_FIELDS: [&str; 7] = [
    "#.embed",
    "#.info.embed",
    "#.paths[/x].post.embed",
    "#.paths[/x].post.parameters[0].embed",
    "#.paths[/x].post.requestBody.content[application/json].embed",
    "#.paths[/x].post.requestBody.embed",
    "#.paths[/x].post.responses.200.embed",
];

#[cfg(feature = "v3_0")]
#[test]
fn v3_0_reports_every_unknown_field_but_not_extensions_or_schema_keywords() {
    let spec: roas::v3_0::spec::Spec = serde_json::from_value(v3_document("3.0.4")).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        V3_UNKNOWN_FIELDS
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

#[cfg(feature = "v3_1")]
#[test]
fn v3_1_reports_every_unknown_field_but_not_extensions_or_schema_keywords() {
    let spec: roas::v3_1::spec::Spec = serde_json::from_value(v3_document("3.1.2")).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        V3_UNKNOWN_FIELDS
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

#[cfg(feature = "v3_2")]
#[test]
fn v3_2_reports_every_unknown_field_but_not_extensions_or_schema_keywords() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(v3_document("3.2.0")).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        V3_UNKNOWN_FIELDS
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

#[cfg(feature = "v3_2")]
#[test]
fn unknown_fields_round_trip() {
    let document = v3_document("3.2.0");
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(spec).unwrap(), document);
}

/// A v2.0 document with `embed` on several objects, its schema among them: a v2.0 schema is a
/// fixed subset of JSON Schema, so an unknown keyword there is reported too.
#[cfg(feature = "v2")]
#[test]
fn v2_reports_every_unknown_field_including_schemas() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1", "embed": true, "x-keep": 1},
        "paths": {"/x": {"post": {
            "embed": true,
            "parameters": [{"name": "q", "in": "query", "type": "string", "embed": true}],
            "responses": {"200": {
                "description": "ok", "schema": {"type": "object", "embed": true}, "embed": true
            }}
        }}},
        "embed": true
    });
    let spec: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.embed",
            "#.info.embed",
            "#.paths[/x].post.embed",
            "#.paths[/x].post.parameters[0].embed",
            "#.paths[/x].post.responses.200.embed",
            "#.paths[/x].post.responses.200.schema.embed",
        ]
    );
    let ignored = Options::new() | Options::IgnoreUnknownFields;
    assert!(unknown_field_paths(&spec, ignored).is_empty());
}

/// `summary` is not a v3.0 Info field and is a string in v3.1: a number there would not fit,
/// so carrying it would fail the conversion.
#[cfg(all(feature = "v3_0", feature = "v3_1"))]
#[test]
fn v3_0_to_v3_1_leaves_out_unknown_fields_and_keeps_extensions_and_schema_keywords() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1", "summary": 5, "embed": true, "x-keep": 1},
        "paths": {},
        "components": {"schemas": {"Choice": {"type": "string", "const": "a"}}}
    });
    let v30: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    let v31 = roas::v3_1::spec::Spec::try_from(v30).unwrap();
    let converted = serde_json::to_value(v31).unwrap();
    assert_eq!(
        converted["info"],
        json!({"title": "t", "version": "1", "x-keep": 1})
    );
    assert_eq!(converted["components"]["schemas"]["Choice"]["const"], "a");
}

/// `kind` is not a v3.1 Tag field and is a string in v3.2.
#[cfg(all(feature = "v3_1", feature = "v3_2"))]
#[test]
fn v3_1_to_v3_2_leaves_out_unknown_fields_and_keeps_extensions() {
    let document = json!({
        "openapi": "3.1.2",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "tags": [{"name": "t", "kind": 5, "x-keep": 1}]
    });
    let v31: roas::v3_1::spec::Spec = serde_json::from_value(document).unwrap();
    let v32 = roas::v3_2::spec::Spec::try_from(v31).unwrap();
    let converted = serde_json::to_value(v32).unwrap();
    assert_eq!(converted["tags"], json!([{"name": "t", "x-keep": 1}]));
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
#[test]
fn v2_to_v3_0_leaves_out_unknown_fields_and_keeps_extensions() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1", "embed": true, "x-keep": 1},
        "paths": {}
    });
    let v2: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    let v30 = roas::v3_0::spec::Spec::try_from(v2).unwrap();
    let converted = serde_json::to_value(v30).unwrap();
    assert_eq!(
        converted["info"],
        json!({"title": "t", "version": "1", "x-keep": 1})
    );
}

#[cfg(feature = "v2")]
#[test]
fn v2_allows_oas_2_0_cross_type_keywords_and_reports_truly_unknown_ones() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "parameters": [
                {"name": "flag", "in": "query", "type": "boolean", "enum": [true]},
                {"name": "tags", "in": "query", "type": "array", "items": {"type": "boolean", "enum": [true, false]}},
                {"name": "bad", "in": "query", "type": "boolean", "embed": true, "schema": {}},
                {"name": "body", "in": "body", "schema": {"type": "string", "maximum": 5}, "enum": ["nope"]}
            ],
            "responses": {"200": {
                "description": "ok",
                "headers": {
                    "X-Flag": {"type": "boolean", "enum": [true]},
                    "X-Bad": {"type": "boolean", "embed": true}
                },
                "schema": {
                    "type": "object",
                    "properties": {
                        "flag": {"type": "boolean", "enum": [true]},
                        "strNum": {"type": "string", "maximum": 5},
                        "bad": {"type": "boolean", "embed": true}
                    }
                }
            }}
        }}}
    });
    let spec: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.paths[/x].get.parameters[2].embed",
            "#.paths[/x].get.parameters[2].schema",
            "#.paths[/x].get.parameters[3].enum",
            "#.paths[/x].get.responses.200.headers.X-Bad.embed",
            "#.paths[/x].get.responses.200.schema.properties.bad.embed",
        ]
    );
}

#[cfg(feature = "v2")]
#[test]
fn v2_operation_external_docs_validates_unknown_fields_and_url() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "externalDocs": {"url": "not-a-url", "embed": true},
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    let spec: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    let err = spec.validate(Options::new(), None).unwrap_err();
    assert!(
        err.errors
            .iter()
            .any(|e| e.path == "#.paths[/x].get.externalDocs.embed")
    );
    assert!(
        err.errors
            .iter()
            .any(|e| e.path == "#.paths[/x].get.externalDocs.url")
    );
}

#[cfg(feature = "v3_0")]
#[test]
fn v3_0_parameter_walks_schema_examples_and_content() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "parameters": [
                {
                    "name": "a",
                    "in": "query",
                    "schema": {"type": "object", "discriminator": {"propertyName": "kind", "embed": true}},
                    "examples": {"ex": {"value": 1, "embed": true}}
                },
                {
                    "name": "b",
                    "in": "query",
                    "content": {"application/json": {"schema": {"type": "string"}, "embed": true}}
                }
            ],
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    let spec: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.paths[/x].get.parameters[0].examples[ex].embed",
            "#.paths[/x].get.parameters[0].schema.discriminator.embed",
            "#.paths[/x].get.parameters[1].content[application/json].embed",
        ]
    );
}

#[cfg(any(feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
#[test]
fn oauth2_security_scheme_reports_errors_under_flows_path() {
    fn doc(version: &str) -> serde_json::Value {
        json!({
            "openapi": version,
            "info": {"title": "t", "version": "1"},
            "paths": {},
            "components": {"securitySchemes": {"oauth": {
                "type": "oauth2",
                "flows": {
                    "embed": true,
                    "implicit": {
                        "authorizationUrl": "https://example.com/auth",
                        "scopes": {},
                        "embed": true
                    }
                }
            }}}
        })
    }
    let expected = [
        "#.components.securitySchemes[oauth].flows.embed",
        "#.components.securitySchemes[oauth].flows.implicit.embed",
    ];
    let opts = Options::new() | Options::IgnoreUnusedSecuritySchemes;

    #[cfg(feature = "v3_0")]
    {
        let spec: roas::v3_0::spec::Spec = serde_json::from_value(doc("3.0.4")).unwrap();
        assert_eq!(unknown_field_paths(&spec, opts), expected);
    }
    #[cfg(feature = "v3_1")]
    {
        let spec: roas::v3_1::spec::Spec = serde_json::from_value(doc("3.1.2")).unwrap();
        assert_eq!(unknown_field_paths(&spec, opts), expected);
    }
    #[cfg(feature = "v3_2")]
    {
        let spec: roas::v3_2::spec::Spec = serde_json::from_value(doc("3.2.0")).unwrap();
        assert_eq!(unknown_field_paths(&spec, opts), expected);
    }
}

#[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1"))]
#[test]
fn vendor_extension_payloads_allow_extra_fields_and_round_trip() {
    #[cfg(feature = "v2")]
    {
        let doc = json!({
            "swagger": "2.0",
            "info": {
                "title": "t",
                "version": "1",
                "x-logo": {"url": "https://example.com/logo.png", "customLogoOpt": true}
            },
            "x-servers": [{"url": "https://api.example.com", "variables": {"env": {"default": "prod"}}}],
            "x-tagGroups": [{"name": "Core", "tags": ["users"], "description": "Core APIs"}],
            "paths": {"/x": {"get": {
                "x-codeSamples": [{"lang": "curl", "source": "curl /x", "extra": 1}],
                "responses": {"200": {"description": "ok"}}
            }}}
        });
        let spec: roas::v2::spec::Spec = serde_json::from_value(doc.clone()).unwrap();
        assert!(unknown_field_paths(&spec, Options::new()).is_empty());
        assert_eq!(serde_json::to_value(spec).unwrap(), doc);
    }

    #[cfg(feature = "v3_0")]
    {
        let doc = json!({
            "openapi": "3.0.4",
            "info": {
                "title": "t",
                "version": "1",
                "x-logo": {"url": "https://example.com/logo.png", "customLogoOpt": true}
            },
            "x-tagGroups": [{"name": "Core", "tags": ["users"], "description": "Core APIs"}],
            "paths": {"/x": {"get": {
                "x-codeSamples": [{"lang": "curl", "source": "curl /x", "extra": 1}],
                "x-badges": [{"name": "Beta", "customBadge": true}],
                "responses": {"200": {"description": "ok"}}
            }}}
        });
        let spec: roas::v3_0::spec::Spec = serde_json::from_value(doc.clone()).unwrap();
        assert!(unknown_field_paths(&spec, Options::new()).is_empty());
        assert_eq!(serde_json::to_value(spec).unwrap(), doc);
    }

    #[cfg(feature = "v3_1")]
    {
        let doc = json!({
            "openapi": "3.1.2",
            "info": {
                "title": "t",
                "version": "1",
                "x-logo": {"url": "https://example.com/logo.png", "customLogoOpt": true}
            },
            "x-tagGroups": [{"name": "Core", "tags": ["users"], "description": "Core APIs"}],
            "paths": {"/x": {"get": {
                "x-codeSamples": [{"lang": "curl", "source": "curl /x", "extra": 1}],
                "responses": {"200": {"description": "ok"}}
            }}}
        });
        let spec: roas::v3_1::spec::Spec = serde_json::from_value(doc.clone()).unwrap();
        assert!(unknown_field_paths(&spec, Options::new()).is_empty());
        assert_eq!(serde_json::to_value(spec).unwrap(), doc);
    }
}

#[cfg(all(feature = "v3_0", feature = "v3_1"))]
#[test]
fn v3_0_to_v3_1_strips_unknown_fields_inside_schema_nested_objects() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {"Pet": {
            "type": "object",
            "const": "keep-schema-keyword",
            "discriminator": {"propertyName": "kind", "embed": true, "x-disc": 1},
            "xml": {"name": "pet", "embed": true, "x-xml": 2},
            "externalDocs": {"url": "https://example.com", "embed": true, "x-doc": 3}
        }}}
    });
    let v30: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    let v31 = roas::v3_1::spec::Spec::try_from(v30).unwrap();
    let pet = &serde_json::to_value(v31).unwrap()["components"]["schemas"]["Pet"];
    assert_eq!(pet["const"], "keep-schema-keyword");
    assert_eq!(
        pet["discriminator"],
        json!({"propertyName": "kind", "x-disc": 1})
    );
    assert_eq!(pet["xml"], json!({"name": "pet", "x-xml": 2}));
    assert_eq!(
        pet["externalDocs"],
        json!({"url": "https://example.com", "x-doc": 3})
    );
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
#[test]
fn v2_to_v3_0_preserves_oas_2_0_cross_type_keywords_and_strips_unknown_ones() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "definitions": {
            "Flag": {"type": "boolean", "enum": [true], "embed": true, "x-keep": 1}
        }
    });
    let v2: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    let v30 = roas::v3_0::spec::Spec::try_from(v2).unwrap();
    let flag = &serde_json::to_value(v30).unwrap()["components"]["schemas"]["Flag"];
    assert_eq!(flag["enum"], json!([true]));
    assert_eq!(flag["x-keep"], 1);
    assert!(flag.get("embed").is_none());
}

#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
fn contains_key(value: &serde_json::Value, key: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key(key) || map.values().any(|v| contains_key(v, key))
        }
        serde_json::Value::Array(arr) => arr.iter().any(|v| contains_key(v, key)),
        _ => false,
    }
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
#[test]
fn v2_to_v3_0_strips_unknown_fields_across_all_object_types() {
    let xml = || json!({"name": "x", "embed": true});
    let docs = || json!({"url": "https://example.com", "embed": true});
    let document = json!({
        "swagger": "2.0",
        "embed": true,
        "info": {
            "title": "t",
            "version": "1",
            "contact": {"name": "c", "embed": true},
            "license": {"name": "MIT", "embed": true},
            "embed": true
        },
        "externalDocs": docs(),
        "tags": [{"name": "pets", "externalDocs": docs(), "embed": true}],
        "securityDefinitions": {
            "basic": {"type": "basic", "embed": true},
            "key": {"type": "apiKey", "name": "k", "in": "header", "embed": true},
            "oauth": {
                "type": "oauth2",
                "flow": "implicit",
                "authorizationUrl": "https://example.com/auth",
                "scopes": {"read": "r"},
                "embed": true
            }
        },
        "parameters": {
            "pHeader": {"name": "h", "in": "header", "type": "integer", "embed": true},
            "pQuery": {"name": "q", "in": "query", "type": "number", "embed": true}
        },
        "responses": {
            "NotFound": {
                "description": "nf",
                "embed": true,
                "headers": {
                    "H-Str": {"type": "string", "embed": true},
                    "H-Int": {"type": "integer", "embed": true},
                    "H-Num": {"type": "number", "embed": true},
                    "H-Bool": {"type": "boolean", "embed": true},
                    "H-Arr": {"type": "array", "items": {"type": "string", "embed": true}, "embed": true}
                }
            }
        },
        "definitions": {
            "Str": {"type": "string", "xml": xml(), "externalDocs": docs(), "embed": true},
            "Int": {"type": "integer", "xml": xml(), "externalDocs": docs(), "embed": true},
            "Num": {"type": "number", "xml": xml(), "externalDocs": docs(), "embed": true},
            "Bool": {"type": "boolean", "xml": xml(), "externalDocs": docs(), "embed": true},
            "Null": {"type": "null", "xml": xml(), "externalDocs": docs(), "embed": true},
            "Arr": {"type": "array", "items": {"type": "string", "embed": true}, "xml": xml(), "externalDocs": docs(), "embed": true},
            "Obj": {
                "type": "object",
                "properties": {"a": {"type": "string", "embed": true}},
                "additionalProperties": {"type": "integer", "embed": true},
                "allOf": [{"type": "object", "embed": true}],
                "xml": xml(),
                "externalDocs": docs(),
                "embed": true
            },
            "AllOf": {
                "allOf": [{"type": "string", "embed": true}],
                "externalDocs": docs(),
                "embed": true
            }
        },
        "paths": {"/items/{id}": {
            "parameters": [
                {"name": "id", "in": "path", "required": true, "type": "string", "embed": true},
                {"name": "pInt", "in": "path", "required": true, "type": "integer", "embed": true},
                {"name": "pNum", "in": "path", "required": true, "type": "number", "embed": true},
                {"name": "pBool", "in": "path", "required": true, "type": "boolean", "embed": true},
                {"name": "pArr", "in": "path", "required": true, "type": "array", "items": {"type": "integer", "embed": true}, "embed": true}
            ],
            "post": {
                "externalDocs": docs(),
                "embed": true,
                "parameters": [
                    {"name": "hStr", "in": "header", "type": "string", "embed": true},
                    {"name": "hInt", "in": "header", "type": "integer", "embed": true},
                    {"name": "hNum", "in": "header", "type": "number", "embed": true},
                    {"name": "hBool", "in": "header", "type": "boolean", "embed": true},
                    {"name": "hArr", "in": "header", "type": "array", "items": {"type": "number", "embed": true}, "embed": true},
                    {"name": "qStr", "in": "query", "type": "string", "embed": true},
                    {"name": "qInt", "in": "query", "type": "integer", "embed": true},
                    {"name": "qNum", "in": "query", "type": "number", "embed": true},
                    {"name": "qBool", "in": "query", "type": "boolean", "embed": true},
                    {"name": "qArr", "in": "query", "type": "array", "items": {"type": "boolean", "embed": true}, "embed": true},
                    {"name": "fStr", "in": "formData", "type": "string", "embed": true},
                    {"name": "fInt", "in": "formData", "type": "integer", "embed": true},
                    {"name": "fNum", "in": "formData", "type": "number", "embed": true},
                    {"name": "fBool", "in": "formData", "type": "boolean", "embed": true},
                    {"name": "fArr", "in": "formData", "type": "array", "items": {"type": "array", "items": {"type": "string", "embed": true}, "embed": true}, "embed": true},
                    {"name": "fFile", "in": "formData", "type": "file", "embed": true}
                ],
                "responses": {
                    "default": {"description": "err", "schema": {"type": "string", "embed": true}, "embed": true},
                    "200": {"description": "ok", "embed": true}
                }
            },
            "put": {
                "parameters": [
                    {"name": "body", "in": "body", "schema": {"type": "object", "embed": true}, "embed": true}
                ],
                "responses": {"200": {"description": "ok"}}
            }
        }}
    });
    let v2: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    let v30 = roas::v3_0::spec::Spec::try_from(v2).unwrap();
    let converted = serde_json::to_value(v30).unwrap();
    assert!(
        !contains_key(&converted, "embed"),
        "converted v3.0 still contains `embed`: {converted}"
    );
}

#[cfg(all(feature = "v3_0", feature = "v3_1"))]
#[test]
fn v3_0_to_v3_1_strips_unknown_fields_across_all_object_types() {
    let xml = || json!({"name": "x", "embed": true});
    let docs = || json!({"url": "https://example.com", "embed": true});
    let disc = || json!({"propertyName": "kind", "embed": true});
    let srv = || json!({"url": "https://api.example.com/{env}", "variables": {"env": {"default": "p", "embed": true}}, "embed": true});
    let document = json!({
        "openapi": "3.0.4",
        "embed": true,
        "info": {
            "title": "t",
            "version": "1",
            "contact": {"name": "c", "embed": true},
            "license": {"name": "MIT", "embed": true},
            "embed": true
        },
        "servers": [srv()],
        "externalDocs": docs(),
        "tags": [{"name": "pets", "externalDocs": docs(), "embed": true}],
        "components": {
            "embed": true,
            "schemas": {
                "Str": {"type": "string", "xml": xml(), "externalDocs": docs()},
                "Int": {"type": "integer", "xml": xml(), "externalDocs": docs()},
                "Num": {"type": "number", "xml": xml(), "externalDocs": docs()},
                "Bool": {"type": "boolean", "xml": xml(), "externalDocs": docs()},
                "Null": {"type": "null", "xml": xml(), "externalDocs": docs()},
                "Arr": {"type": "array", "items": {"type": "string", "xml": xml()}, "xml": xml(), "externalDocs": docs()},
                "Obj": {
                    "type": "object",
                    "properties": {"a": {"type": "string", "xml": xml()}},
                    "additionalProperties": {"type": "integer", "xml": xml()},
                    "discriminator": disc(),
                    "xml": xml(),
                    "externalDocs": docs()
                },
                "All": {"allOf": [{"type": "string", "xml": xml()}], "discriminator": disc()},
                "Any": {"anyOf": [{"type": "string", "xml": xml()}], "discriminator": disc()},
                "One": {"oneOf": [{"type": "string", "xml": xml()}], "discriminator": disc()},
                "Not": {"not": {"type": "string", "xml": xml()}}
            },
            "responses": {"R": {"description": "ok", "embed": true}},
            "parameters": {"P": {"name": "p", "in": "query", "schema": {"type": "string"}, "embed": true}},
            "examples": {"E": {"value": 1, "embed": true}},
            "requestBodies": {"RB": {"content": {"application/json": {"schema": {"type": "string"}, "embed": true}}, "embed": true}},
            "headers": {"H": {"schema": {"type": "string"}, "embed": true}},
            "links": {"L": {"operationId": "op", "server": srv(), "embed": true}},
            "callbacks": {"CB": {"{$request.body#/url}": {"post": {"responses": {"200": {"description": "ok"}}, "embed": true}}}},
            "securitySchemes": {
                "http": {"type": "http", "scheme": "bearer", "embed": true},
                "key": {"type": "apiKey", "name": "k", "in": "header", "embed": true},
                "oidc": {"type": "openIdConnect", "openIdConnectUrl": "https://example.com/.well-known", "embed": true},
                "oauth": {
                    "type": "oauth2",
                    "embed": true,
                    "flows": {
                        "embed": true,
                        "implicit": {"authorizationUrl": "https://example.com/a", "scopes": {}, "embed": true},
                        "password": {"tokenUrl": "https://example.com/t", "scopes": {}, "embed": true},
                        "clientCredentials": {"tokenUrl": "https://example.com/t", "scopes": {}, "embed": true},
                        "authorizationCode": {"authorizationUrl": "https://example.com/a", "tokenUrl": "https://example.com/t", "scopes": {}, "embed": true}
                    }
                }
            }
        },
        "paths": {"/x/{id}": {
            "servers": [srv()],
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}, "examples": {"a": {"value": "1", "embed": true}}, "embed": true}],
            "post": {
                "externalDocs": docs(),
                "servers": [srv()],
                "embed": true,
                "parameters": [
                    {"name": "q", "in": "query", "content": {"application/json": {"schema": {"type": "string"}, "embed": true}}, "embed": true},
                    {"name": "h", "in": "header", "schema": {"type": "string"}, "embed": true},
                    {"name": "c", "in": "cookie", "schema": {"type": "string"}, "embed": true}
                ],
                "requestBody": {
                    "embed": true,
                    "content": {
                        "multipart/form-data": {
                            "schema": {"type": "object"},
                            "examples": {"e": {"value": {}, "embed": true}},
                            "encoding": {"f": {"headers": {"X-H": {"schema": {"type": "string"}, "embed": true}}, "embed": true}},
                            "embed": true
                        }
                    }
                },
                "callbacks": {"cb": {"{$request.body#/url}": {"post": {"responses": {"200": {"description": "ok"}}, "embed": true}}}},
                "responses": {
                    "default": {"description": "err", "embed": true},
                    "200": {
                        "description": "ok",
                        "headers": {"X-R": {"schema": {"type": "string"}, "embed": true}},
                        "content": {"application/json": {"schema": {"type": "string"}, "embed": true}},
                        "links": {"next": {"operationId": "op", "server": srv(), "embed": true}},
                        "embed": true
                    }
                }
            }
        }}
    });
    let v30: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    let v31 = roas::v3_1::spec::Spec::try_from(v30).unwrap();
    let converted = serde_json::to_value(v31).unwrap();
    assert!(
        !contains_key(&converted, "embed"),
        "converted v3.1 still contains `embed`: {converted}"
    );
}

#[cfg(all(feature = "v3_1", feature = "v3_2"))]
#[test]
fn v3_1_to_v3_2_strips_unknown_fields_across_all_object_types() {
    let xml = || json!({"name": "x", "embed": true});
    let docs = || json!({"url": "https://example.com", "embed": true});
    let disc = || json!({"propertyName": "kind", "embed": true});
    let srv = || json!({"url": "https://api.example.com/{env}", "variables": {"env": {"default": "p", "embed": true}}, "embed": true});
    let document = json!({
        "openapi": "3.1.2",
        "embed": true,
        "info": {
            "title": "t",
            "version": "1",
            "contact": {"name": "c", "embed": true},
            "license": {"name": "MIT", "embed": true},
            "embed": true
        },
        "servers": [srv()],
        "externalDocs": docs(),
        "tags": [{"name": "pets", "externalDocs": docs(), "embed": true}],
        "webhooks": {
            "onPet": {"post": {"responses": {"200": {"description": "ok", "embed": true}}, "embed": true}}
        },
        "components": {
            "embed": true,
            "pathItems": {"PI": {"get": {"responses": {"200": {"description": "ok", "embed": true}}, "embed": true}}},
            "schemas": {
                "BoolLit": true,
                "Empty": {},
                "Multi": {"type": ["string", "null"], "discriminator": disc(), "externalDocs": docs()},
                "Str": {"type": "string", "xml": xml(), "externalDocs": docs()},
                "Int": {"type": "integer", "xml": xml(), "externalDocs": docs()},
                "Num": {"type": "number", "xml": xml(), "externalDocs": docs()},
                "Bool": {"type": "boolean", "xml": xml(), "externalDocs": docs()},
                "Null": {"type": "null", "xml": xml(), "externalDocs": docs()},
                "Arr": {"type": "array", "items": {"type": "string", "xml": xml()}, "xml": xml(), "externalDocs": docs()},
                "Obj": {
                    "type": "object",
                    "properties": {"a": {"type": "string", "xml": xml()}},
                    "patternProperties": {"^x": {"type": "integer", "xml": xml()}},
                    "additionalProperties": {"type": "integer", "xml": xml()},
                    "unevaluatedProperties": {"type": "boolean", "xml": xml()},
                    "propertyNames": {"type": "string", "xml": xml()},
                    "discriminator": disc(),
                    "xml": xml(),
                    "externalDocs": docs()
                },
                "All": {"allOf": [{"type": "string", "xml": xml()}], "discriminator": disc(), "externalDocs": docs()},
                "Any": {"anyOf": [{"type": "string", "xml": xml()}], "discriminator": disc(), "externalDocs": docs()},
                "One": {"oneOf": [{"type": "string", "xml": xml()}], "discriminator": disc(), "externalDocs": docs()},
                "Not": {"not": {"type": "string", "xml": xml()}, "externalDocs": docs()}
            },
            "responses": {"R": {"description": "ok", "embed": true}},
            "parameters": {"P": {"name": "p", "in": "query", "schema": {"type": "string"}, "embed": true}},
            "examples": {"E": {"value": 1, "embed": true}},
            "requestBodies": {"RB": {"content": {"application/json": {"schema": {"type": "string"}, "embed": true}}, "embed": true}},
            "headers": {"H": {"schema": {"type": "string"}, "embed": true}},
            "links": {"L": {"operationId": "op", "server": srv(), "embed": true}},
            "callbacks": {"CB": {"{$request.body#/url}": {"post": {"responses": {"200": {"description": "ok"}}, "embed": true}}}},
            "securitySchemes": {
                "http": {"type": "http", "scheme": "bearer", "embed": true},
                "key": {"type": "apiKey", "name": "k", "in": "header", "embed": true},
                "oidc": {"type": "openIdConnect", "openIdConnectUrl": "https://example.com/.well-known", "embed": true},
                "mtls": {"type": "mutualTLS", "embed": true},
                "oauth": {
                    "type": "oauth2",
                    "embed": true,
                    "flows": {
                        "embed": true,
                        "implicit": {"authorizationUrl": "https://example.com/a", "scopes": {}, "embed": true},
                        "password": {"tokenUrl": "https://example.com/t", "scopes": {}, "embed": true},
                        "clientCredentials": {"tokenUrl": "https://example.com/t", "scopes": {}, "embed": true},
                        "authorizationCode": {"authorizationUrl": "https://example.com/a", "tokenUrl": "https://example.com/t", "scopes": {}, "embed": true}
                    }
                }
            }
        },
        "paths": {"/x/{id}": {
            "servers": [srv()],
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}, "examples": {"a": {"value": "1", "embed": true}}, "embed": true}],
            "post": {
                "externalDocs": docs(),
                "servers": [srv()],
                "embed": true,
                "parameters": [
                    {"name": "q", "in": "query", "content": {"application/json": {"schema": {"type": "string"}, "embed": true}}, "embed": true},
                    {"name": "h", "in": "header", "schema": {"type": "string"}, "embed": true},
                    {"name": "c", "in": "cookie", "schema": {"type": "string"}, "embed": true}
                ],
                "requestBody": {
                    "embed": true,
                    "content": {
                        "multipart/form-data": {
                            "schema": {"type": "object"},
                            "examples": {"e": {"value": {}, "embed": true}},
                            "encoding": {"f": {"headers": {"X-H": {"schema": {"type": "string"}, "embed": true}}, "embed": true}},
                            "embed": true
                        }
                    }
                },
                "callbacks": {"cb": {"{$request.body#/url}": {"post": {"responses": {"200": {"description": "ok"}}, "embed": true}}}},
                "responses": {
                    "default": {"description": "err", "embed": true},
                    "200": {
                        "description": "ok",
                        "headers": {"X-R": {"schema": {"type": "string"}, "embed": true}},
                        "content": {"application/json": {"schema": {"type": "string"}, "embed": true}},
                        "links": {"next": {"operationId": "op", "server": srv(), "embed": true}},
                        "embed": true
                    }
                }
            }
        }}
    });
    let v31: roas::v3_1::spec::Spec = serde_json::from_value(document).unwrap();
    let v32 = roas::v3_2::spec::Spec::try_from(v31).unwrap();
    let converted = serde_json::to_value(v32).unwrap();
    assert!(
        !contains_key(&converted, "embed"),
        "converted v3.2 still contains `embed`: {converted}"
    );
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
#[test]
fn v2_to_v3_0_guards_cross_type_schema_and_file_parameter_keywords() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "definitions": {
            "StrWithBadAllOf": {"type": "string", "allOf": 5, "discriminator": 5}
        },
        "paths": {"/upload": {"post": {
            "parameters": [
                {
                    "name": "file",
                    "in": "formData",
                    "type": "file",
                    "maxLength": "not-an-int",
                    "minLength": "not-an-int",
                    "pattern": 5,
                    "enum": 5,
                    "default": 123
                }
            ],
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    let v2: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    assert!(unknown_field_paths(&v2, Options::new()).is_empty());
    let v30 = roas::v3_0::spec::Spec::try_from(v2).unwrap();
    let converted = serde_json::to_value(&v30).unwrap();
    let str_schema = &converted["components"]["schemas"]["StrWithBadAllOf"];
    assert!(str_schema.get("allOf").is_none());
    assert!(str_schema.get("discriminator").is_none());
    assert!(unknown_field_paths(&v30, Options::new()).is_empty());
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
#[test]
fn v2_to_v3_0_sanitizes_promoted_x_servers_entries() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "x-servers": [
            {
                "url": "https://{env}.example.com",
                "description": "prod",
                "stage": "production",
                "x-region": "us",
                "variables": {
                    "env": {"default": "api", "extraVarKey": true, "x-var": 1}
                }
            },
            {
                "url": "https://fallback.example.com",
                "extra": true,
                "variables": "not-a-map"
            }
        ],
        "paths": {}
    });
    let v2: roas::v2::spec::Spec = serde_json::from_value(document).unwrap();
    assert!(unknown_field_paths(&v2, Options::new()).is_empty());
    let v30 = roas::v3_0::spec::Spec::try_from(v2).unwrap();
    assert!(unknown_field_paths(&v30, Options::new()).is_empty());
    let converted = serde_json::to_value(&v30).unwrap();
    assert_eq!(
        converted["servers"][0],
        json!({
            "url": "https://{env}.example.com",
            "description": "prod",
            "x-region": "us",
            "variables": {"env": {"default": "api", "x-var": 1}}
        })
    );
    assert_eq!(
        converted["servers"][1],
        json!({"url": "https://fallback.example.com"})
    );
}

#[cfg(all(feature = "v3_1", feature = "v3_2"))]
#[test]
fn v3_1_to_v3_2_filters_non_extension_keys_when_promoting_x_tag_groups() {
    let document = json!({
        "openapi": "3.1.2",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "x-tagGroups": [
            {
                "name": "Core",
                "tags": ["pets"],
                "description": "Core APIs",
                "collapsed": true,
                "summary": 123,
                "x-icon": "star"
            },
            {
                "name": "Other",
                "tags": ["misc"],
                "description": 456
            }
        ]
    });
    let v31: roas::v3_1::spec::Spec = serde_json::from_value(document).unwrap();
    assert!(unknown_field_paths(&v31, Options::new()).is_empty());
    let v32 = roas::v3_2::spec::Spec::try_from(v31).unwrap();
    assert!(unknown_field_paths(&v32, Options::new()).is_empty());
    let converted = serde_json::to_value(&v32).unwrap();
    let tags = converted["tags"].as_array().unwrap();
    let core = tags.iter().find(|t| t["name"] == "Core").unwrap();
    assert_eq!(core["kind"], "nav");
    assert_eq!(core["description"], "Core APIs");
    assert_eq!(core["x-icon"], "star");
    assert!(core.get("collapsed").is_none());
    assert!(core.get("summary").is_none());
    let other = tags.iter().find(|t| t["name"] == "Other").unwrap();
    assert!(other.get("description").is_none());
}

#[cfg(feature = "v3_0")]
#[test]
fn v3_0_header_walks_schema_for_unknown_fields() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "responses": {"200": {
                "description": "ok",
                "headers": {
                    "X-Id": {
                        "schema": {
                            "type": "object",
                            "discriminator": {"propertyName": "kind", "embed": true}
                        }
                    }
                }
            }}
        }}}
    });
    let spec: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        ["#.paths[/x].get.responses.200.headers[X-Id].schema.discriminator.embed"]
    );
}

#[cfg(all(feature = "v3_0", feature = "v3_1"))]
#[test]
fn v3_0_to_v3_1_strips_unknown_keys_from_schema_keyword_external_docs_and_discriminator() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {
            "Composed": {
                "allOf": [{"type": "string"}],
                "externalDocs": {"url": "https://example.com", "embed": true, "x-keep": 1}
            },
            "NullableStr": {
                "type": "string",
                "nullable": true,
                "discriminator": {"propertyName": "kind", "embed": true, "x-keep": 2}
            }
        }}
    });
    let v30: roas::v3_0::spec::Spec = serde_json::from_value(document).unwrap();
    let v31 = roas::v3_1::spec::Spec::try_from(v30).unwrap();
    assert!(unknown_field_paths(&v31, Options::new()).is_empty());
    let converted = serde_json::to_value(&v31).unwrap();
    assert_eq!(
        converted["components"]["schemas"]["Composed"]["externalDocs"],
        json!({"url": "https://example.com", "x-keep": 1})
    );
    assert_eq!(
        converted["components"]["schemas"]["NullableStr"]["discriminator"],
        json!({"propertyName": "kind", "x-keep": 2})
    );
}

#[cfg(feature = "v2")]
#[test]
fn v2_responses_keeps_and_reports_unknown_fields() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "responses": {
                "200": {"description": "ok"},
                "x-keep": 1,
                "600": {"description": "out of range"},
                "bogus": true
            }
        }}}
    });
    let spec: roas::v2::spec::Spec = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(&spec).unwrap(), document);
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.paths[/x].get.responses.600",
            "#.paths[/x].get.responses.bogus"
        ]
    );
    assert!(unknown_field_paths(&spec, Options::IgnoreUnknownFields.into()).is_empty());
}

#[cfg(feature = "v3_0")]
#[test]
fn v3_0_responses_keeps_and_reports_unknown_fields() {
    let document = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "responses": {
                "200": {"description": "ok"},
                "x-keep": 1,
                "2xx": {"description": "lowercase wildcard"},
                "bogus": true
            }
        }}}
    });
    let spec: roas::v3_0::spec::Spec = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(&spec).unwrap(), document);
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.paths[/x].get.responses.2xx",
            "#.paths[/x].get.responses.bogus"
        ]
    );
    assert!(unknown_field_paths(&spec, Options::IgnoreUnknownFields.into()).is_empty());
}

#[cfg(feature = "v3_1")]
#[test]
fn v3_1_path_item_and_responses_keep_and_report_unknown_fields() {
    let document = json!({
        "openapi": "3.1.2",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {
            "x-path-ext": 1,
            "gett": {"responses": {"200": {"description": "typo"}}},
            "GET": {"responses": {"200": {"description": "uppercase"}}},
            "get": {
                "responses": {
                    "200": {"description": "ok"},
                    "x-resp-ext": 2,
                    "bogus": true
                }
            }
        }}
    });
    let spec: roas::v3_1::spec::Spec = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(&spec).unwrap(), document);
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        [
            "#.paths[/x].GET",
            "#.paths[/x].get.responses.bogus",
            "#.paths[/x].gett"
        ]
    );
    assert!(unknown_field_paths(&spec, Options::IgnoreUnknownFields.into()).is_empty());
}

#[cfg(feature = "v3_2")]
#[test]
fn v3_2_path_item_and_responses_keep_and_report_unknown_fields() {
    let document = json!({
        "openapi": "3.2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {
            "x-path-ext": 1,
            "gett": {"responses": {"200": {"description": "typo"}}},
            "get": {
                "responses": {
                    "200": {"description": "ok"},
                    "x-resp-ext": 2,
                    "bogus": true
                }
            }
        }}
    });
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serde_json::to_value(&spec).unwrap(), document);
    assert_eq!(
        unknown_field_paths(&spec, Options::new()),
        ["#.paths[/x].get.responses.bogus", "#.paths[/x].gett"]
    );
    assert!(unknown_field_paths(&spec, Options::IgnoreUnknownFields.into()).is_empty());
}

#[cfg(all(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
#[test]
fn conversions_strip_unknown_keys_from_path_item_and_responses() {
    let v2_doc = json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "responses": {
                "200": {"description": "ok"},
                "x-keep": 1,
                "bogus": true
            }
        }}}
    });
    let v2_spec: roas::v2::spec::Spec = serde_json::from_value(v2_doc).unwrap();
    let v30_from_v2 = roas::v3_0::spec::Spec::try_from(v2_spec).unwrap();
    assert!(unknown_field_paths(&v30_from_v2, Options::new()).is_empty());
    let v30_json = serde_json::to_value(&v30_from_v2).unwrap();
    assert_eq!(v30_json["paths"]["/x"]["get"]["responses"]["x-keep"], 1);
    assert!(
        v30_json["paths"]["/x"]["get"]["responses"]
            .get("bogus")
            .is_none()
    );

    let v30_doc = json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {"get": {
            "responses": {
                "200": {"description": "ok"},
                "x-keep": 2,
                "bogus": true
            }
        }}}
    });
    let v30_spec: roas::v3_0::spec::Spec = serde_json::from_value(v30_doc).unwrap();
    let v31_from_v30 = roas::v3_1::spec::Spec::try_from(v30_spec).unwrap();
    assert!(unknown_field_paths(&v31_from_v30, Options::new()).is_empty());
    let v31_json = serde_json::to_value(&v31_from_v30).unwrap();
    assert_eq!(v31_json["paths"]["/x"]["get"]["responses"]["x-keep"], 2);
    assert!(
        v31_json["paths"]["/x"]["get"]["responses"]
            .get("bogus")
            .is_none()
    );

    let v31_doc = json!({
        "openapi": "3.1.2",
        "info": {"title": "t", "version": "1"},
        "paths": {"/x": {
            "x-path-keep": 3,
            "gett": {"responses": {"200": {"description": "typo"}}},
            "get": {
                "responses": {
                    "200": {"description": "ok"},
                    "x-resp-keep": 4,
                    "bogus": true
                }
            }
        }}
    });
    let v31_spec: roas::v3_1::spec::Spec = serde_json::from_value(v31_doc).unwrap();
    let v32_from_v31 = roas::v3_2::spec::Spec::try_from(v31_spec).unwrap();
    assert!(unknown_field_paths(&v32_from_v31, Options::new()).is_empty());
    let v32_json = serde_json::to_value(&v32_from_v31).unwrap();
    assert_eq!(v32_json["paths"]["/x"]["x-path-keep"], 3);
    assert!(v32_json["paths"]["/x"].get("gett").is_none());
    assert_eq!(
        v32_json["paths"]["/x"]["get"]["responses"]["x-resp-keep"],
        4
    );
    assert!(
        v32_json["paths"]["/x"]["get"]["responses"]
            .get("bogus")
            .is_none()
    );
}
