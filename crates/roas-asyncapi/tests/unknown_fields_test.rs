//! Integration tests for unknown-field preservation, validation, and
//! conversion stripping across AsyncAPI v2.6, v3.0, and v3.1.

#[cfg(any(feature = "v2_6", feature = "v3_0", feature = "v3_1"))]
const UNKNOWN_FIELD_MSG: &str = "not a field of this object; only `x-` extensions may be added";

#[cfg(feature = "v2_6")]
mod v2_6 {
    use super::UNKNOWN_FIELD_MSG;
    use enumset::EnumSet;
    use roas_asyncapi::v2_6::{Document, Message};
    use roas_asyncapi::validation::{Validate, ValidationOptions};
    use serde_json::json;

    fn doc_with_unknowns_everywhere() -> serde_json::Value {
        json!({
            "asyncapi": "2.6.0",
            "docExtra": 1,
            "x-doc": "kept",
            "info": {
                "title": "Demo",
                "version": "1.0.0",
                "infoExtra": 1,
                "x-info": "kept",
                "contact": { "name": "Ops", "contactExtra": 1, "x-contact": "kept" },
                "license": { "name": "MIT", "licenseExtra": 1, "x-license": "kept" }
            },
            "servers": {
                "prod": {
                    "url": "kafka://{env}.example.com",
                    "protocol": "kafka",
                    "serverExtra": 1,
                    "x-server": "kept",
                    "variables": {
                        "env": {
                            "default": "prod",
                            "varExtra": 1,
                            "x-var": "kept"
                        }
                    }
                }
            },
            "tags": [
                { "name": "root", "tagExtra": 1, "x-tag": "kept" }
            ],
            "externalDocs": {
                "url": "https://example.com/docs",
                "docsExtra": 1,
                "x-docs": "kept"
            },
            "channels": {
                "events/{id}": {
                    "channelExtra": 1,
                    "x-channel": "kept",
                    "parameters": {
                        "id": {
                            "description": "Event id",
                            "paramExtra": 1,
                            "x-param": "kept"
                        }
                    },
                    "publish": {
                        "operationId": "recvEvent",
                        "opExtra": 1,
                        "x-op": "kept",
                        "traits": [
                            { "summary": "tr", "opTraitExtra": 1, "x-op-trait": "kept" }
                        ],
                        "message": {
                            "oneOf": [
                                {
                                    "name": "EventA",
                                    "msgExtra": 1,
                                    "x-msg": "kept",
                                    "correlationId": {
                                        "location": "$message.header#/id",
                                        "corrExtra": 1,
                                        "x-corr": "kept"
                                    },
                                    "examples": [
                                        {
                                            "name": "ex",
                                            "payload": { "a": 1 },
                                            "exampleExtra": 1,
                                            "x-example": "kept"
                                        }
                                    ],
                                    "traits": [
                                        {
                                            "summary": "mtr",
                                            "msgTraitExtra": 1,
                                            "x-msg-trait": "kept"
                                        }
                                    ]
                                }
                            ],
                            "oneOfExtra": 1,
                            "x-one-of": "kept"
                        }
                    }
                }
            },
            "components": {
                "componentsExtra": 1,
                "x-components": "kept",
                "securitySchemes": {
                    "oauth": {
                        "type": "oauth2",
                        "secExtra": 1,
                        "x-sec": "kept",
                        "flows": {
                            "flowsExtra": 1,
                            "x-flows": "kept",
                            "implicit": {
                                "authorizationUrl": "https://example.com/auth",
                                "scopes": { "read": "Read" },
                                "flowExtra": 1,
                                "x-flow": "kept"
                            }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn round_trips_and_reports_unknown_fields_on_every_object() {
        let value = doc_with_unknowns_everywhere();
        let doc: Document = serde_json::from_value(value.clone()).expect("parses");

        // All unclaimed keys and `x-` extensions round-trip without loss.
        assert_eq!(serde_json::to_value(&doc).expect("serializes"), value);

        let err = doc.validate(EnumSet::empty()).unwrap_err();
        let expected_paths = [
            "#.docExtra",
            "#.info.infoExtra",
            "#.info.contact.contactExtra",
            "#.info.license.licenseExtra",
            "#.servers.prod.serverExtra",
            "#.servers.prod.variables.env.varExtra",
            "#.tags[0].tagExtra",
            "#.externalDocs.docsExtra",
            "#.channels.events/{id}.channelExtra",
            "#.channels.events/{id}.parameters.id.paramExtra",
            "#.channels.events/{id}.publish.opExtra",
            "#.channels.events/{id}.publish.traits[0].opTraitExtra",
            "#.channels.events/{id}.publish.message.oneOfExtra",
            "#.channels.events/{id}.publish.message.oneOf[0].msgExtra",
            "#.channels.events/{id}.publish.message.oneOf[0].correlationId.corrExtra",
            "#.channels.events/{id}.publish.message.oneOf[0].examples[0].exampleExtra",
            "#.channels.events/{id}.publish.message.oneOf[0].traits[0].msgTraitExtra",
            "#.components.componentsExtra",
            "#.components.securitySchemes.oauth.secExtra",
            "#.components.securitySchemes.oauth.flows.flowsExtra",
            "#.components.securitySchemes.oauth.flows.implicit.flowExtra",
        ];
        assert_eq!(
            err.errors.len(),
            expected_paths.len(),
            "unexpected error list: {err}"
        );
        for path in expected_paths {
            assert!(
                err.errors
                    .iter()
                    .any(|e| e.path == path && e.message == UNKNOWN_FIELD_MSG),
                "missing unknown-field error at `{path}` in: {err}"
            );
        }

        // `IgnoreUnknownFields` silences all of them.
        doc.validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .expect("valid when unknown fields are ignored");
    }

    #[test]
    fn schema_keywords_are_exempt_but_nested_external_docs_are_validated() {
        let value = json!({
            "asyncapi": "2.6.0",
            "info": { "title": "Demo", "version": "1.0.0" },
            "channels": {
                "events": {
                    "publish": {
                        "message": {
                            "payload": {
                                "type": "object",
                                "customKeyword": "allowed-in-json-schema",
                                "properties": {
                                    "nested": {
                                        "type": "string",
                                        "anotherKeyword": true,
                                        "externalDocs": {
                                            "url": "https://example.com",
                                            "badDocsField": 1
                                        }
                                    }
                                },
                                "externalDocs": {
                                    "url": "https://example.com",
                                    "topDocsExtra": 2
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "User": {
                        "type": "object",
                        "nullable": true,
                        "x-schema": 1
                    }
                }
            }
        });
        let doc: Document = serde_json::from_value(value.clone()).expect("parses");
        assert_eq!(serde_json::to_value(&doc).expect("serializes"), value);

        let err = doc.validate(EnumSet::empty()).unwrap_err();
        assert_eq!(err.errors.len(), 2, "got: {err}");
        assert!(err.errors.iter().any(|e| {
            e.path == "#.channels.events.publish.message.payload.externalDocs.topDocsExtra"
                && e.message == UNKNOWN_FIELD_MSG
        }));
        assert!(err.errors.iter().any(|e| {
            e.path
                == "#.channels.events.publish.message.payload.properties.nested.externalDocs.badDocsField"
                && e.message == UNKNOWN_FIELD_MSG
        }));

        doc.validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .expect("passes when ignored");
    }

    #[test]
    fn manual_message_deserializer_preserves_unknown_keys_and_rejects_duplicates() {
        let msg: Message = serde_json::from_str(
            r##"{"name":"M","unknownKey":42,"x-tag":true,"payload":{"$ref":"#/components/schemas/A","ignoredSibling":1}}"##,
        )
        .expect("parses");
        let ext = msg.extensions.as_ref().expect("extensions");
        assert_eq!(ext["unknownKey"], json!(42));
        assert_eq!(ext["x-tag"], json!(true));
        // Reference Object payload still strips siblings per spec.
        assert_eq!(
            msg.payload,
            Some(json!({ "$ref": "#/components/schemas/A" }))
        );

        let err = serde_json::from_str::<Message>(r#"{"name":"M","dup":1,"dup":2}"#).unwrap_err();
        assert!(
            err.to_string().contains("duplicate field `dup`"),
            "got: {err}"
        );
    }

    #[test]
    fn reference_drops_siblings_and_local_pointer_resolution_still_works() {
        let doc: Document = serde_json::from_value(json!({
            "asyncapi": "2.6.0",
            "rootUnknown": "kept",
            "info": { "title": "Demo", "version": "1.0.0" },
            "channels": {
                "events": {
                    "publish": {
                        "message": { "$ref": "#/components/messages/Signup", "ignoredSibling": 1 }
                    }
                }
            },
            "components": {
                "messages": {
                    "Signup": { "name": "Signup", "msgUnknown": true }
                }
            }
        }))
        .expect("parses");

        // Reference sibling was dropped; unknown fields on Document and Message are reported,
        // and the local `$ref` resolves cleanly without false errors.
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        assert_eq!(err.errors.len(), 2, "got: {err}");
        assert!(err.errors.iter().any(|e| e.path == "#.rootUnknown"));
        assert!(
            err.errors
                .iter()
                .any(|e| e.path == "#.components.messages.Signup.msgUnknown")
        );
    }
}

#[cfg(any(feature = "v3_0", feature = "v3_1"))]
macro_rules! v3_unknown_field_tests {
    ($mod_name:ident, $version:ident, $version_str:literal) => {
        mod $mod_name {
            use super::UNKNOWN_FIELD_MSG;
            use enumset::EnumSet;
            use roas_asyncapi::$version::Document;
            use roas_asyncapi::validation::{Validate, ValidationOptions};
            use serde_json::json;

            fn doc_with_unknowns_everywhere() -> serde_json::Value {
                json!({
                    "asyncapi": $version_str,
                    "docExtra": 1,
                    "x-doc": "kept",
                    "info": {
                        "title": "Demo",
                        "version": "1.0.0",
                        "infoExtra": 1,
                        "x-info": "kept",
                        "contact": { "name": "Ops", "contactExtra": 1, "x-contact": "kept" },
                        "license": { "name": "MIT", "licenseExtra": 1, "x-license": "kept" },
                        "tags": [ { "name": "root", "tagExtra": 1, "x-tag": "kept" } ],
                        "externalDocs": {
                            "url": "https://example.com/docs",
                            "docsExtra": 1,
                            "x-docs": "kept"
                        }
                    },
                    "servers": {
                        "prod": {
                            "host": "{env}.example.com:9092",
                            "protocol": "kafka",
                            "serverExtra": 1,
                            "x-server": "kept",
                            "variables": {
                                "env": {
                                    "default": "prod",
                                    "varExtra": 1,
                                    "x-var": "kept"
                                }
                            },
                            "security": [
                                {
                                    "type": "oauth2",
                                    "secExtra": 1,
                                    "x-sec": "kept",
                                    "flows": {
                                        "flowsExtra": 1,
                                        "x-flows": "kept",
                                        "implicit": {
                                            "authorizationUrl": "https://example.com/auth",
                                            "availableScopes": { "read": "Read" },
                                            "flowExtra": 1,
                                            "x-flow": "kept"
                                        }
                                    }
                                }
                            ]
                        }
                    },
                    "channels": {
                        "events": {
                            "address": "events/{id}",
                            "channelExtra": 1,
                            "x-channel": "kept",
                            "parameters": {
                                "id": {
                                    "description": "Event id",
                                    "paramExtra": 1,
                                    "x-param": "kept"
                                }
                            },
                            "messages": {
                                "signup": {
                                    "name": "Signup",
                                    "msgExtra": 1,
                                    "x-msg": "kept",
                                    "payload": {
                                        "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
                                        "schema": { "type": "record", "name": "Signup" },
                                        "mfExtra": 1,
                                        "x-mf": "kept"
                                    },
                                    "correlationId": {
                                        "location": "$message.header#/id",
                                        "corrExtra": 1,
                                        "x-corr": "kept"
                                    },
                                    "examples": [
                                        {
                                            "name": "ex",
                                            "payload": { "a": 1 },
                                            "exampleExtra": 1,
                                            "x-example": "kept"
                                        }
                                    ],
                                    "traits": [
                                        {
                                            "summary": "mtr",
                                            "msgTraitExtra": 1,
                                            "x-msg-trait": "kept"
                                        }
                                    ]
                                }
                            }
                        },
                        "replies": {
                            "address": null
                        }
                    },
                    "operations": {
                        "sendSignup": {
                            "action": "send",
                            "channel": { "$ref": "#/channels/events" },
                            "messages": [ { "$ref": "#/channels/events/messages/signup" } ],
                            "opExtra": 1,
                            "x-op": "kept",
                            "traits": [
                                { "summary": "otr", "opTraitExtra": 1, "x-op-trait": "kept" }
                            ],
                            "reply": {
                                "channel": { "$ref": "#/channels/replies" },
                                "replyExtra": 1,
                                "x-reply": "kept",
                                "address": {
                                    "location": "$message.header#/replyTo",
                                    "replyAddrExtra": 1,
                                    "x-reply-addr": "kept"
                                }
                            }
                        }
                    },
                    "components": {
                        "componentsExtra": 1,
                        "x-components": "kept"
                    }
                })
            }

            #[test]
            fn round_trips_and_reports_unknown_fields_on_every_object() {
                let value = doc_with_unknowns_everywhere();
                let doc: Document = serde_json::from_value(value.clone()).expect("parses");

                // Lossless round-trip preserves unknown keys and `x-` extensions.
                assert_eq!(serde_json::to_value(&doc).expect("serializes"), value);

                let err = doc.validate(EnumSet::empty()).unwrap_err();
                let expected_paths = [
                    "#.docExtra",
                    "#.info.infoExtra",
                    "#.info.contact.contactExtra",
                    "#.info.license.licenseExtra",
                    "#.info.tags[0].tagExtra",
                    "#.info.externalDocs.docsExtra",
                    "#.servers.prod.serverExtra",
                    "#.servers.prod.variables.env.varExtra",
                    "#.servers.prod.security[0].secExtra",
                    "#.servers.prod.security[0].flows.flowsExtra",
                    "#.servers.prod.security[0].flows.implicit.flowExtra",
                    "#.channels.events.channelExtra",
                    "#.channels.events.parameters.id.paramExtra",
                    "#.channels.events.messages.signup.msgExtra",
                    "#.channels.events.messages.signup.payload.mfExtra",
                    "#.channels.events.messages.signup.correlationId.corrExtra",
                    "#.channels.events.messages.signup.examples[0].exampleExtra",
                    "#.channels.events.messages.signup.traits[0].msgTraitExtra",
                    "#.operations.sendSignup.opExtra",
                    "#.operations.sendSignup.traits[0].opTraitExtra",
                    "#.operations.sendSignup.reply.replyExtra",
                    "#.operations.sendSignup.reply.address.replyAddrExtra",
                    "#.components.componentsExtra",
                ];
                assert_eq!(
                    err.errors.len(),
                    expected_paths.len(),
                    "unexpected error list: {err}"
                );
                for path in expected_paths {
                    assert!(
                        err.errors
                            .iter()
                            .any(|e| e.path == path && e.message == UNKNOWN_FIELD_MSG),
                        "missing unknown-field error at `{path}` in: {err}"
                    );
                }

                // `IgnoreUnknownFields` silences all of them.
                doc.validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
                    .expect("valid when unknown fields are ignored");
            }

            #[test]
            fn schema_keywords_are_exempt_but_nested_external_docs_are_validated() {
                let value = json!({
                    "asyncapi": $version_str,
                    "info": { "title": "Demo", "version": "1.0.0" },
                    "components": {
                        "schemas": {
                            "User": {
                                "type": "object",
                                "customKeyword": "kept",
                                "x-schema": true,
                                "properties": {
                                    "id": {
                                        "type": "string",
                                        "nestedKeyword": 123,
                                        "externalDocs": {
                                            "url": "https://example.com/id",
                                            "nestedDocsExtra": 1
                                        }
                                    }
                                },
                                "externalDocs": {
                                    "url": "https://example.com/user",
                                    "topDocsExtra": 2
                                }
                            }
                        }
                    }
                });
                let doc: Document = serde_json::from_value(value.clone()).expect("parses");
                assert_eq!(serde_json::to_value(&doc).expect("serializes"), value);

                let err = doc.validate(EnumSet::empty()).unwrap_err();
                assert_eq!(err.errors.len(), 2, "got: {err}");
                assert!(err.errors.iter().any(|e| {
                    e.path == "#.components.schemas.User.externalDocs.topDocsExtra"
                        && e.message == UNKNOWN_FIELD_MSG
                }));
                assert!(err.errors.iter().any(|e| {
                    e.path
                        == "#.components.schemas.User.properties.id.externalDocs.nestedDocsExtra"
                        && e.message == UNKNOWN_FIELD_MSG
                }));

                doc.validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
                    .expect("passes when ignored");
            }

            #[test]
            fn local_refs_resolve_with_unknown_fields_and_reference_drops_siblings() {
                let doc: Document = serde_json::from_value(json!({
                    "asyncapi": $version_str,
                    "rootExtra": 1,
                    "info": { "title": "Demo", "version": "1.0.0" },
                    "channels": {
                        "events": {
                            "address": "events",
                            "messages": {
                                "signup": {
                                    "$ref": "#/components/messages/Signup",
                                    "ignoredSibling": true
                                }
                            }
                        }
                    },
                    "operations": {
                        "send": {
                            "action": "send",
                            "channel": { "$ref": "#/channels/events" },
                            "messages": [ { "$ref": "#/channels/events/messages/signup" } ]
                        }
                    },
                    "components": {
                        "messages": {
                            "Signup": { "name": "Signup", "msgExtra": 1 }
                        }
                    }
                }))
                .expect("parses");

                let err = doc.validate(EnumSet::empty()).unwrap_err();
                assert_eq!(err.errors.len(), 2, "got: {err}");
                assert!(err.errors.iter().any(|e| e.path == "#.rootExtra"));
                assert!(
                    err.errors
                        .iter()
                        .any(|e| e.path == "#.components.messages.Signup.msgExtra")
                );
            }
        }
    };
}

#[cfg(feature = "v3_0")]
v3_unknown_field_tests!(v3_0, v3_0, "3.0.0");

#[cfg(feature = "v3_1")]
v3_unknown_field_tests!(v3_1, v3_1, "3.1.0");

#[cfg(all(feature = "v2_6", feature = "v3_0"))]
mod convert_v2_6_to_v3_0 {
    use enumset::EnumSet;
    use roas_asyncapi::v2_6;
    use roas_asyncapi::v3_0::from_v2_6::{NoteKind, convert};
    use roas_asyncapi::validation::Validate;
    use serde_json::json;

    #[test]
    fn strips_unknown_fields_while_keeping_extensions_and_schema_keywords() {
        let source: v2_6::Document = serde_json::from_value(json!({
            "asyncapi": "2.6.0",
            "docUnknown": 1,
            "x-doc": "kept",
            "info": {
                "title": "Demo",
                "version": "1.0.0",
                "infoUnknown": 1,
                "x-info": "kept",
                "contact": { "name": "Ops", "contactUnknown": 1, "x-contact": "kept" },
                "license": { "name": "MIT", "licenseUnknown": 1, "x-license": "kept" }
            },
            "servers": {
                "prod": {
                    "url": "{env}.example.com",
                    "protocol": "kafka",
                    "serverUnknown": 1,
                    "x-server": "kept",
                    "variables": {
                        "env": { "default": "prod", "varUnknown": 1, "x-var": "kept" }
                    }
                }
            },
            "tags": [ { "name": "root", "tagUnknown": 1, "x-tag": "kept" } ],
            "externalDocs": { "url": "https://example.com", "docsUnknown": 1, "x-docs": "kept" },
            "channels": {
                "events": {
                    "channelUnknown": 1,
                    "x-channel": "kept",
                    "publish": {
                        "operationId": "recvEvent",
                        "opUnknown": 1,
                        "x-op": "kept",
                        "traits": [ { "summary": "ot", "opTraitUnknown": 1, "x-ot": "kept" } ],
                        "message": {
                            "oneOf": [
                                {
                                    "name": "Ev",
                                    "msgUnknown": 1,
                                    "x-msg": "kept",
                                    "correlationId": {
                                        "location": "$message.header#/id",
                                        "corrUnknown": 1,
                                        "x-corr": "kept"
                                    },
                                    "examples": [
                                        {
                                            "payload": { "a": 1 },
                                            "exUnknown": 1,
                                            "x-ex": "kept"
                                        }
                                    ],
                                    "traits": [
                                        {
                                            "summary": "mt",
                                            "msgTraitUnknown": 1,
                                            "x-mt": "kept"
                                        }
                                    ],
                                    "payload": {
                                        "type": "object",
                                        "customKeyword": "preserved",
                                        "x-schema": "kept",
                                        "properties": {
                                            "a": {
                                                "type": "string",
                                                "propKeyword": true,
                                                "externalDocs": {
                                                    "url": "https://example.com/a",
                                                    "nestedDocsUnknown": 1,
                                                    "x-nested-docs": "kept"
                                                }
                                            }
                                        },
                                        "externalDocs": {
                                            "url": "https://example.com/s",
                                            "schemaDocsUnknown": 1,
                                            "x-schema-docs": "kept"
                                        }
                                    }
                                }
                            ],
                            "oneOfUnknown": 1,
                            "x-oneof": "kept"
                        }
                    }
                }
            },
            "components": {
                "compUnknown": 1,
                "x-comp": "kept",
                "parameters": {
                    "p": { "description": "d", "paramUnknown": 1, "x-param": "kept" }
                },
                "correlationIds": {
                    "c": { "location": "$message.header#/id", "cUnknown": 1, "x-c": "kept" }
                },
                "securitySchemes": {
                    "oauth": {
                        "type": "oauth2",
                        // Collides with v3.0 `SecurityScheme.scopes: Vec<String>` if not stripped!
                        "scopes": 123,
                        "secUnknown": 1,
                        "x-sec": "kept",
                        "flows": {
                            "flowsUnknown": 1,
                            "x-flows": "kept",
                            "implicit": {
                                "authorizationUrl": "https://example.com/auth",
                                "scopes": { "read": "Read" },
                                // Collides with v3.0 `availableScopes` if not stripped!
                                "availableScopes": 123,
                                "flowUnknown": 1,
                                "x-flow": "kept"
                            }
                        }
                    }
                }
            }
        }))
        .expect("parses");

        let (converted, report) = convert(source);

        // The converted v3.0 document must pass strict validation (no leaked unknown fields!).
        converted
            .validate(EnumSet::empty())
            .expect("converted document must validate cleanly");

        // `oneOfUnknown` was stripped silently, while `x-oneof` was reported as dropped.
        assert!(report.notes.iter().any(|n| matches!(
            &n.kind,
            NoteKind::FieldsDropped { fields } if fields == &vec!["x-oneof".to_owned()]
        )));

        let out = serde_json::to_value(&converted).expect("serializes");
        let text = out.to_string();
        for stripped in [
            "docUnknown",
            "infoUnknown",
            "contactUnknown",
            "licenseUnknown",
            "serverUnknown",
            "varUnknown",
            "tagUnknown",
            "docsUnknown",
            "channelUnknown",
            "opUnknown",
            "opTraitUnknown",
            "msgUnknown",
            "corrUnknown",
            "exUnknown",
            "msgTraitUnknown",
            "nestedDocsUnknown",
            "schemaDocsUnknown",
            "oneOfUnknown",
            "compUnknown",
            "paramUnknown",
            "cUnknown",
            "secUnknown",
            "flowsUnknown",
            "flowUnknown",
        ] {
            assert!(!text.contains(stripped), "`{stripped}` leaked into: {text}");
        }
        for preserved in [
            "x-doc",
            "x-info",
            "x-contact",
            "x-license",
            "x-server",
            "x-var",
            "x-tag",
            "x-docs",
            "x-channel",
            "x-op",
            "x-ot",
            "x-msg",
            "x-corr",
            "x-ex",
            "x-mt",
            "x-schema",
            "x-nested-docs",
            "x-schema-docs",
            "x-comp",
            "x-param",
            "x-c",
            "x-sec",
            "x-flows",
            "x-flow",
            "customKeyword",
            "propKeyword",
        ] {
            assert!(
                text.contains(preserved),
                "`{preserved}` should survive in: {text}"
            );
        }
    }

    #[test]
    fn schema_external_docs_with_stray_dollar_ref_and_nested_subschemas_convert_cleanly() {
        let source: v2_6::Document = serde_json::from_value(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "Demo", "version": "1.0.0" },
            "channels": {},
            "components": {
                "schemas": {
                    "StrRef": {
                        "type": "object",
                        "externalDocs": {
                            "url": "https://example.com/top",
                            "description": "Top",
                            "$ref": "#/components/externalDocs/nope",
                            "extra": 1,
                            "x-docs": "kept"
                        },
                        "dependencies": {
                            "a": {
                                "type": "object",
                                "externalDocs": {
                                    "url": "https://example.com/dep",
                                    "$ref": 123,
                                    "depUnknown": true
                                }
                            },
                            "b": ["a"]
                        },
                        "items": {
                            "type": "string",
                            "externalDocs": {
                                "url": "https://example.com/item",
                                "$ref": "#/foo",
                                "itemUnknown": 1
                            }
                        },
                        "allOf": [{
                            "type": "object",
                            "externalDocs": { "url": "https://example.com/all", "allUnknown": 1 }
                        }],
                        "anyOf": [{
                            "type": "object",
                            "externalDocs": { "url": "https://example.com/any", "anyUnknown": 1 }
                        }],
                        "oneOf": [{
                            "type": "object",
                            "externalDocs": { "url": "https://example.com/one", "oneUnknown": 1 }
                        }],
                        "additionalProperties": {
                            "type": "string",
                            "externalDocs": { "url": "https://example.com/ap", "apUnknown": 1 }
                        },
                        "not": {
                            "type": "null",
                            "externalDocs": { "url": "https://example.com/not", "notUnknown": 1 }
                        },
                        "if": {
                            "type": "object",
                            "externalDocs": { "url": "https://example.com/if", "ifUnknown": 1 }
                        },
                        "then": {
                            "type": "object",
                            "externalDocs": { "url": "https://example.com/then", "thenUnknown": 1 }
                        },
                        "else": {
                            "type": "object",
                            "externalDocs": { "url": "https://example.com/else", "elseUnknown": 1 }
                        }
                    },
                    "TupleItems": {
                        "type": "array",
                        "items": [{
                            "type": "string",
                            "externalDocs": {
                                "url": "https://example.com/tuple",
                                "$ref": 456,
                                "tupleUnknown": 1
                            }
                        }]
                    }
                }
            }
        }))
        .expect("parses");

        let (converted, report) = convert(source);
        assert!(
            !report
                .notes
                .iter()
                .any(|n| matches!(n.kind, NoteKind::NotConverted { .. })),
            "schemas must not fail conversion: {:?}",
            report.notes
        );
        converted
            .validate(EnumSet::empty())
            .expect("converted schemas must validate cleanly");

        let schemas = &converted.components.as_ref().expect("components").schemas;
        assert!(schemas.contains_key("StrRef"));
        assert!(schemas.contains_key("TupleItems"));

        let out = serde_json::to_value(&converted).expect("serializes");
        let top_docs = &out["components"]["schemas"]["StrRef"]["externalDocs"];
        assert_eq!(
            top_docs,
            &json!({
                "url": "https://example.com/top",
                "description": "Top",
                "x-docs": "kept"
            })
        );
        let text = out.to_string();
        for stripped in [
            "$ref",
            "extra",
            "depUnknown",
            "itemUnknown",
            "allUnknown",
            "anyUnknown",
            "oneUnknown",
            "apUnknown",
            "notUnknown",
            "ifUnknown",
            "thenUnknown",
            "elseUnknown",
            "tupleUnknown",
        ] {
            assert!(!text.contains(stripped), "`{stripped}` leaked into: {text}");
        }
    }

    #[test]
    fn schema_with_custom_schema_keyword_is_not_misclassified_as_multi_format() {
        let source: v2_6::Document = serde_json::from_value(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "Demo", "version": "1.0.0" },
            "channels": {
                "events": {
                    "publish": {
                        "operationId": "recv",
                        "message": {
                            "name": "Ev",
                            "payload": {
                                "type": "object",
                                "schema": "custom-annotation",
                                "properties": { "id": { "type": "string" } }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "Annotated": {
                        "type": "object",
                        "schema": "custom-component-annotation",
                        "properties": { "name": { "type": "string" } }
                    }
                }
            }
        }))
        .expect("parses");

        let (converted, report) = convert(source);
        assert!(
            !report
                .notes
                .iter()
                .any(|n| matches!(n.kind, NoteKind::NotConverted { .. })),
            "got notes: {:?}",
            report.notes
        );
        converted
            .validate(EnumSet::empty())
            .expect("converted document must validate cleanly");

        let comp = converted.components.as_ref().expect("components").schemas["Annotated"]
            .item()
            .expect("inline schema");
        match comp {
            roas_asyncapi::v3_0::SchemaOrMultiFormat::Schema(schema) => {
                assert!(schema.properties.contains_key("name"));
                assert_eq!(
                    schema.extensions.as_ref().and_then(|e| e.get("schema")),
                    Some(&json!("custom-component-annotation"))
                );
            }
            other => panic!("expected Schema, got {other:?}"),
        }

        let msg_payload = converted.channels["events"]
            .item()
            .expect("inline channel")
            .messages["Ev"]
            .item()
            .expect("inline message")
            .payload
            .as_ref()
            .and_then(|p| p.item())
            .expect("inline payload");
        match msg_payload {
            roas_asyncapi::v3_0::SchemaOrMultiFormat::Schema(schema) => {
                assert!(schema.properties.contains_key("id"));
                assert_eq!(
                    schema.extensions.as_ref().and_then(|e| e.get("schema")),
                    Some(&json!("custom-annotation"))
                );
            }
            other => panic!("expected Schema, got {other:?}"),
        }
    }

    #[test]
    fn external_channel_ref_siblings_note_ignores_unknown_keys_but_reports_x_extensions() {
        let only_unknown: v2_6::Document = serde_json::from_value(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "Demo", "version": "1.0.0" },
            "channels": {
                "ext": {
                    "$ref": "./other.yaml#/channels/ext",
                    "unknownSibling": 1
                }
            }
        }))
        .expect("parses");
        let (_, report) = convert(only_unknown);
        assert!(
            !report
                .notes
                .iter()
                .any(|n| matches!(n.kind, NoteKind::ChannelReferenceSiblingsDropped)),
            "unknown key alone should not trigger ChannelReferenceSiblingsDropped: {:?}",
            report.notes
        );

        let with_ext: v2_6::Document = serde_json::from_value(json!({
            "asyncapi": "2.6.0",
            "info": { "title": "Demo", "version": "1.0.0" },
            "channels": {
                "ext": {
                    "$ref": "./other.yaml#/channels/ext",
                    "x-sibling": 1
                }
            }
        }))
        .expect("parses");
        let (_, report) = convert(with_ext);
        assert!(
            report
                .notes
                .iter()
                .any(|n| matches!(n.kind, NoteKind::ChannelReferenceSiblingsDropped)),
            "x- extension sibling should trigger ChannelReferenceSiblingsDropped: {:?}",
            report.notes
        );
    }
}

#[cfg(all(feature = "v3_0", feature = "v3_1"))]
mod convert_v3_0_to_v3_1 {
    use enumset::EnumSet;
    use roas_asyncapi::validation::Validate;
    use roas_asyncapi::{v3_0, v3_1};
    use serde_json::json;

    #[test]
    fn strips_unknown_fields_while_keeping_extensions_and_schema_keywords() {
        let source: v3_0::Document = serde_json::from_value(json!({
            "asyncapi": "3.0.0",
            "docUnknown": 1,
            "x-doc": "kept",
            "info": {
                "title": "Demo",
                "version": "1.0.0",
                "infoUnknown": 1,
                "x-info": "kept",
                "contact": { "name": "Ops", "contactUnknown": 1, "x-contact": "kept" },
                "license": { "name": "MIT", "licenseUnknown": 1, "x-license": "kept" },
                "tags": [ { "name": "root", "tagUnknown": 1, "x-tag": "kept" } ],
                "externalDocs": { "url": "https://example.com", "docsUnknown": 1, "x-docs": "kept" }
            },
            "servers": {
                "prod": {
                    "host": "{env}.example.com",
                    "protocol": "kafka",
                    "serverUnknown": 1,
                    "x-server": "kept",
                    "variables": {
                        "env": { "default": "prod", "varUnknown": 1, "x-var": "kept" }
                    },
                    "security": [
                        {
                            "type": "oauth2",
                            "secUnknown": 1,
                            "x-sec": "kept",
                            "flows": {
                                "flowsUnknown": 1,
                                "x-flows": "kept",
                                "implicit": {
                                    "authorizationUrl": "https://example.com/auth",
                                    "availableScopes": {},
                                    "flowUnknown": 1,
                                    "x-flow": "kept"
                                }
                            }
                        }
                    ]
                }
            },
            "channels": {
                "events": {
                    "address": "events/{id}",
                    "channelUnknown": 1,
                    "x-channel": "kept",
                    "parameters": {
                        "id": { "description": "id", "paramUnknown": 1, "x-param": "kept" }
                    },
                    "messages": {
                        "ev": {
                            "name": "Ev",
                            "msgUnknown": 1,
                            "x-msg": "kept",
                            "correlationId": {
                                "location": "$message.header#/id",
                                "corrUnknown": 1,
                                "x-corr": "kept"
                            },
                            "examples": [
                                { "payload": {}, "exUnknown": 1, "x-ex": "kept" }
                            ],
                            "traits": [
                                { "summary": "mt", "msgTraitUnknown": 1, "x-mt": "kept" }
                            ],
                            "payload": {
                                "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
                                "schema": { "type": "record", "name": "Ev" },
                                "mfUnknown": 1,
                                "x-mf": "kept"
                            }
                        }
                    }
                },
                "replies": { "address": null }
            },
            "operations": {
                "sendEv": {
                    "action": "send",
                    "channel": { "$ref": "#/channels/events" },
                    "opUnknown": 1,
                    "x-op": "kept",
                    "traits": [ { "summary": "ot", "opTraitUnknown": 1, "x-ot": "kept" } ],
                    "reply": {
                        "channel": { "$ref": "#/channels/replies" },
                        "replyUnknown": 1,
                        "x-reply": "kept",
                        "address": {
                            "location": "$message.header#/replyTo",
                            "replyAddrUnknown": 1,
                            "x-reply-addr": "kept"
                        }
                    }
                }
            },
            "components": {
                "compUnknown": 1,
                "x-comp": "kept",
                "schemas": {
                    "User": {
                        "type": "object",
                        "customKeyword": "preserved",
                        "x-schema": "kept",
                        "properties": {
                            "a": {
                                "type": "string",
                                "propKeyword": true,
                                "externalDocs": {
                                    "url": "https://example.com/a",
                                    "nestedDocsUnknown": 1,
                                    "x-nested-docs": "kept"
                                }
                            }
                        },
                        "externalDocs": {
                            "url": "https://example.com/s",
                            "schemaDocsUnknown": 1,
                            "x-schema-docs": "kept"
                        }
                    }
                }
            }
        }))
        .expect("parses");

        let converted = v3_1::Document::from(source);
        converted
            .validate(EnumSet::empty())
            .expect("converted v3.1 document must validate cleanly");

        let out = serde_json::to_value(&converted).expect("serializes");
        let text = out.to_string();
        for stripped in [
            "docUnknown",
            "infoUnknown",
            "contactUnknown",
            "licenseUnknown",
            "tagUnknown",
            "docsUnknown",
            "serverUnknown",
            "varUnknown",
            "secUnknown",
            "flowsUnknown",
            "flowUnknown",
            "channelUnknown",
            "paramUnknown",
            "msgUnknown",
            "corrUnknown",
            "exUnknown",
            "msgTraitUnknown",
            "mfUnknown",
            "opUnknown",
            "opTraitUnknown",
            "replyUnknown",
            "replyAddrUnknown",
            "compUnknown",
            "nestedDocsUnknown",
            "schemaDocsUnknown",
        ] {
            assert!(!text.contains(stripped), "`{stripped}` leaked into: {text}");
        }
        for preserved in [
            "x-doc",
            "x-info",
            "x-contact",
            "x-license",
            "x-tag",
            "x-docs",
            "x-server",
            "x-var",
            "x-sec",
            "x-flows",
            "x-flow",
            "x-channel",
            "x-param",
            "x-msg",
            "x-corr",
            "x-ex",
            "x-mt",
            "x-mf",
            "x-op",
            "x-ot",
            "x-reply",
            "x-reply-addr",
            "x-comp",
            "x-schema",
            "x-nested-docs",
            "x-schema-docs",
            "customKeyword",
            "propKeyword",
        ] {
            assert!(
                text.contains(preserved),
                "`{preserved}` should survive in: {text}"
            );
        }
    }
}
