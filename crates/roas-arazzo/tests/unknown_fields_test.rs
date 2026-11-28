//! Integration tests for unknown-field preservation, validation,
//! `ValidationOptions::IgnoreUnknownFields`, `Description::unknown_members`,
//! and `v1_0 -> v1_1` upconversion stripping across every extensible object
//! type in Arazzo v1.0 and v1.1.

#[cfg(any(feature = "v1_0", feature = "v1_1"))]
const UNKNOWN_MSG: &str = "not a field of this object; only `x-` extensions may be added";

#[cfg(feature = "v1_0")]
mod v1_0_unknown_fields {
    use super::UNKNOWN_MSG;
    use enumset::EnumSet;
    use roas_arazzo::v1_0::Description;
    use roas_arazzo::validation::{Validate, ValidationOptions};
    use serde_json::{Value, json};

    /// A v1.0 document with an unknown field AND an `x-` extension on every
    /// extensible object type in the v1.0 model.
    fn rich_v1_0_with_unknown_fields() -> Value {
        json!({
            "arazzo": "1.0.1",
            "x-root": 1,
            "bogusRoot": "r",
            "info": {
                "title": "Pets",
                "version": "1.0.0",
                "x-info": 1,
                "bogusInfo": "i"
            },
            "sourceDescriptions": [
                {
                    "name": "petStore",
                    "url": "https://api.example.com/openapi.json",
                    "type": "openapi",
                    "x-sd": 1,
                    "bogusSource": "sd"
                }
            ],
            "workflows": [
                {
                    "workflowId": "buyPet",
                    "x-wf": 1,
                    "bogusWorkflow": "wf",
                    "parameters": [
                        {
                            "name": "wfParam",
                            "in": "header",
                            "value": "v",
                            "x-wp": 1,
                            "bogusWfParam": "wp"
                        }
                    ],
                    "steps": [
                        {
                            "stepId": "order",
                            "operationId": "placeOrder",
                            "x-step": 1,
                            "bogusStep": "st",
                            "parameters": [
                                {
                                    "name": "petId",
                                    "in": "path",
                                    "value": "$inputs.petId",
                                    "x-param": 1,
                                    "bogusParam": "p"
                                },
                                {
                                    "reference": "$components.parameters.locale"
                                }
                            ],
                            "requestBody": {
                                "contentType": "application/json",
                                "payload": { "petId": 1, "arbitraryPayloadKey": true },
                                "x-rb": 1,
                                "bogusRequestBody": "rb",
                                "replacements": [
                                    {
                                        "target": "/petId",
                                        "value": "$inputs.petId",
                                        "x-rep": 1,
                                        "bogusReplacement": "rep"
                                    }
                                ]
                            },
                            "successCriteria": [
                                {
                                    "context": "$response.body",
                                    "condition": "$.ok",
                                    "type": {
                                        "type": "jsonpath",
                                        "version": "draft-goessner-dispatch-jsonpath-00",
                                        "x-et": 1,
                                        "bogusExprType": "et"
                                    },
                                    "x-crit": 1,
                                    "bogusCriterion": "cr"
                                }
                            ],
                            "onSuccess": [
                                {
                                    "name": "done",
                                    "type": "end",
                                    "x-sa": 1,
                                    "bogusSuccess": "sa"
                                }
                            ],
                            "onFailure": [
                                {
                                    "name": "retry",
                                    "type": "retry",
                                    "retryLimit": 1,
                                    "x-fa": 1,
                                    "bogusFailure": "fa"
                                }
                            ]
                        }
                    ]
                }
            ],
            "components": {
                "x-comp": 1,
                "bogusComponents": "c",
                "inputs": {
                    "pet": { "type": "object", "arbitrarySchemaKeyword": true }
                },
                "parameters": {
                    "locale": {
                        "name": "Accept-Language",
                        "in": "header",
                        "value": "en",
                        "x-cp": 1,
                        "bogusCompParam": "cp"
                    }
                },
                "successActions": {
                    "ok": {
                        "name": "ok",
                        "type": "end",
                        "x-csa": 1,
                        "bogusCompSuccess": "csa"
                    }
                },
                "failureActions": {
                    "fail": {
                        "name": "fail",
                        "type": "end",
                        "x-cfa": 1,
                        "bogusCompFailure": "cfa"
                    }
                }
            }
        })
    }

    const EXPECTED_PATHS: &[&str] = &[
        "#.bogusRoot",
        "#.info.bogusInfo",
        "#.sourceDescriptions[0].bogusSource",
        "#.workflows[0].bogusWorkflow",
        "#.workflows[0].parameters[0].bogusWfParam",
        "#.workflows[0].steps[0].bogusStep",
        "#.workflows[0].steps[0].parameters[0].bogusParam",
        "#.workflows[0].steps[0].requestBody.bogusRequestBody",
        "#.workflows[0].steps[0].requestBody.replacements[0].bogusReplacement",
        "#.workflows[0].steps[0].successCriteria[0].bogusCriterion",
        "#.workflows[0].steps[0].successCriteria[0].type.bogusExprType",
        "#.workflows[0].steps[0].onSuccess[0].bogusSuccess",
        "#.workflows[0].steps[0].onFailure[0].bogusFailure",
        "#.components.bogusComponents",
        "#.components.parameters.locale.bogusCompParam",
        "#.components.successActions.ok.bogusCompSuccess",
        "#.components.failureActions.fail.bogusCompFailure",
    ];

    #[test]
    fn round_trip_preserves_unknown_fields_and_validation_reports_every_object() {
        let raw = rich_v1_0_with_unknown_fields();
        let doc: Description = serde_json::from_value(raw.clone()).unwrap();

        // Lossless JSON round-trip preserves both `x-` extensions and unknown keys.
        assert_eq!(serde_json::to_value(&doc).unwrap(), raw);

        // Validation reports every unknown key and flags each as about a key.
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        let mut actual_paths: Vec<&str> = err
            .errors
            .iter()
            .map(|e| {
                assert_eq!(e.message, UNKNOWN_MSG, "unexpected error: {e}");
                assert!(e.is_about_key(), "expected is_about_key for {e}");
                e.path.as_str()
            })
            .collect();
        actual_paths.sort_unstable();
        let mut expected: Vec<&str> = EXPECTED_PATHS.to_vec();
        expected.sort_unstable();
        assert_eq!(actual_paths, expected);

        // IgnoreUnknownFields suppresses all of them.
        doc.validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .expect("IgnoreUnknownFields should allow unknown fields");

        // `unknown_members(&raw)` still lists every one of them!
        let unknown = doc.unknown_members(&raw).unwrap();
        assert_eq!(unknown.version, "1.0.1");
        let mut unknown_paths: Vec<&str> =
            unknown.members.iter().map(|m| m.path.as_str()).collect();
        unknown_paths.sort_unstable();
        assert_eq!(unknown_paths, expected);
    }

    #[test]
    fn criterion_expression_type_checks_unknown_fields_even_with_empty_version() {
        let raw = json!({
            "arazzo": "1.0.1",
            "info": { "title": "T", "version": "1" },
            "sourceDescriptions": [{ "name": "api", "url": "api.json" }],
            "workflows": [{
                "workflowId": "w",
                "steps": [{
                    "stepId": "s",
                    "operationId": "op",
                    "successCriteria": [{
                        "context": "$response.body",
                        "condition": "$.ok",
                        "type": { "type": "jsonpath", "version": "", "bogus": 1 }
                    }]
                }]
            }]
        });
        let doc: Description = serde_json::from_value(raw).unwrap();
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.path == "#.workflows[0].steps[0].successCriteria[0].type.version")
        );
        assert!(err.errors.iter().any(|e| {
            e.path == "#.workflows[0].steps[0].successCriteria[0].type.bogus"
                && e.message == UNKNOWN_MSG
        }));
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn upconversion_from_v1_0_to_v1_1_strips_unknown_fields_and_keeps_extensions() {
        let raw = rich_v1_0_with_unknown_fields();
        let v0: Description = serde_json::from_value(raw).unwrap();
        let v1: roas_arazzo::v1_1::Description = v0.into();

        // Upconverted v1.1 document validates cleanly with NO unknown-field errors.
        v1.validate(EnumSet::empty())
            .expect("upconversion must strip unknown fields");

        // `x-` extensions are preserved everywhere, while bogus keys are gone.
        let serialized = serde_json::to_value(&v1).unwrap();
        assert_eq!(serialized["x-root"], json!(1));
        assert!(serialized.get("bogusRoot").is_none());
        assert_eq!(serialized["info"]["x-info"], json!(1));
        assert!(serialized["info"].get("bogusInfo").is_none());
        assert_eq!(serialized["sourceDescriptions"][0]["x-sd"], json!(1));
        assert!(
            serialized["sourceDescriptions"][0]
                .get("bogusSource")
                .is_none()
        );
        assert_eq!(serialized["workflows"][0]["x-wf"], json!(1));
        assert!(serialized["workflows"][0].get("bogusWorkflow").is_none());
        assert_eq!(
            serialized["workflows"][0]["steps"][0]["successCriteria"][0]["type"]["x-et"],
            json!(1)
        );
        assert!(
            serialized["workflows"][0]["steps"][0]["successCriteria"][0]["type"]
                .get("bogusExprType")
                .is_none()
        );
        assert_eq!(serialized["components"]["x-comp"], json!(1));
        assert!(serialized["components"].get("bogusComponents").is_none());
        // Free-form JSON (payload & input schema) is untouched.
        assert_eq!(
            serialized["workflows"][0]["steps"][0]["requestBody"]["payload"]["arbitraryPayloadKey"],
            json!(true)
        );
        assert_eq!(
            serialized["components"]["inputs"]["pet"]["arbitrarySchemaKeyword"],
            json!(true)
        );
    }
}

#[cfg(feature = "v1_1")]
mod v1_1_unknown_fields {
    use super::UNKNOWN_MSG;
    use enumset::EnumSet;
    use roas_arazzo::v1_1::Description;
    use roas_arazzo::validation::{Validate, ValidationOptions};
    use serde_json::{Value, json};

    /// A v1.1 document with an unknown field AND an `x-` extension on every
    /// extensible object type in the v1.1 model (including `Selector`,
    /// `ExpressionType` inside both `Criterion` and `Selector`, and action
    /// `parameters`).
    fn rich_v1_1_with_unknown_fields() -> Value {
        json!({
            "arazzo": "1.1.0",
            "$self": "urn:example:arazzo:pets",
            "x-root": 1,
            "bogusRoot": "r",
            "info": {
                "title": "Pets",
                "version": "1.0.0",
                "x-info": 1,
                "bogusInfo": "i"
            },
            "sourceDescriptions": [
                {
                    "name": "petStore",
                    "url": "https://api.example.com/openapi.json",
                    "type": "openapi",
                    "x-sd": 1,
                    "bogusSource": "sd"
                }
            ],
            "workflows": [
                {
                    "workflowId": "buyPet",
                    "x-wf": 1,
                    "bogusWorkflow": "wf",
                    "outputs": {
                        "selected": {
                            "context": "$steps.order.outputs.body",
                            "selector": "$.id",
                            "type": {
                                "type": "jsonpath",
                                "version": "rfc9535",
                                "x-wf-et": 1,
                                "bogusWfSelExprType": "et"
                            },
                            "x-wf-sel": 1,
                            "bogusWfSelector": "sel"
                        }
                    },
                    "steps": [
                        {
                            "stepId": "order",
                            "operationId": "placeOrder",
                            "x-step": 1,
                            "bogusStep": "st",
                            "parameters": [
                                {
                                    "name": "petId",
                                    "in": "path",
                                    "value": {
                                        "context": "$inputs",
                                        "selector": "$.petId",
                                        "type": "jsonpath",
                                        "x-p-sel": 1,
                                        "bogusParamSelector": "psel"
                                    },
                                    "x-param": 1,
                                    "bogusParam": "p"
                                }
                            ],
                            "requestBody": {
                                "contentType": "application/json",
                                "payload": { "petId": 1, "arbitraryPayloadKey": true },
                                "x-rb": 1,
                                "bogusRequestBody": "rb",
                                "replacements": [
                                    {
                                        "target": "$.petId",
                                        "targetSelectorType": {
                                            "type": "jsonpath",
                                            "version": "rfc9535",
                                            "x-tst": 1,
                                            "bogusTargetSelectorType": "tst"
                                        },
                                        "value": {
                                            "context": "$inputs",
                                            "selector": "/petId",
                                            "type": "jsonpointer",
                                            "x-rv-sel": 1,
                                            "bogusReplacementSelector": "rsel"
                                        },
                                        "x-rep": 1,
                                        "bogusReplacement": "rep"
                                    }
                                ]
                            },
                            "successCriteria": [
                                {
                                    "context": "$response.body",
                                    "condition": "$.ok",
                                    "type": {
                                        "type": "jsonpath",
                                        "version": "rfc9535",
                                        "x-et": 1,
                                        "bogusExprType": "et"
                                    },
                                    "x-crit": 1,
                                    "bogusCriterion": "cr"
                                }
                            ],
                            "onSuccess": [
                                {
                                    "name": "nextWf",
                                    "type": "goto",
                                    "workflowId": "buyPet",
                                    "parameters": [
                                        {
                                            "name": "p",
                                            "value": 1,
                                            "x-sap": 1,
                                            "bogusSuccessParam": "sap"
                                        }
                                    ],
                                    "x-sa": 1,
                                    "bogusSuccess": "sa"
                                }
                            ],
                            "onFailure": [
                                {
                                    "name": "fallback",
                                    "type": "goto",
                                    "workflowId": "buyPet",
                                    "parameters": [
                                        {
                                            "name": "p",
                                            "value": 2,
                                            "x-fap": 1,
                                            "bogusFailureParam": "fap"
                                        }
                                    ],
                                    "x-fa": 1,
                                    "bogusFailure": "fa"
                                }
                            ],
                            "outputs": {
                                "stepOut": {
                                    "context": "$response.body",
                                    "selector": "$.id",
                                    "type": "jsonpath",
                                    "x-so": 1,
                                    "bogusStepOutSelector": "sosel"
                                }
                            }
                        }
                    ]
                }
            ],
            "components": {
                "x-comp": 1,
                "bogusComponents": "c",
                "parameters": {
                    "locale": {
                        "name": "Accept-Language",
                        "in": "header",
                        "value": "en",
                        "x-cp": 1,
                        "bogusCompParam": "cp"
                    }
                },
                "successActions": {
                    "ok": {
                        "name": "ok",
                        "type": "end",
                        "x-csa": 1,
                        "bogusCompSuccess": "csa"
                    }
                },
                "failureActions": {
                    "fail": {
                        "name": "fail",
                        "type": "end",
                        "x-cfa": 1,
                        "bogusCompFailure": "cfa"
                    }
                }
            }
        })
    }

    const EXPECTED_PATHS: &[&str] = &[
        "#.bogusRoot",
        "#.info.bogusInfo",
        "#.sourceDescriptions[0].bogusSource",
        "#.workflows[0].bogusWorkflow",
        "#.workflows[0].outputs.selected.bogusWfSelector",
        "#.workflows[0].outputs.selected.type.bogusWfSelExprType",
        "#.workflows[0].steps[0].bogusStep",
        "#.workflows[0].steps[0].parameters[0].bogusParam",
        "#.workflows[0].steps[0].parameters[0].value.bogusParamSelector",
        "#.workflows[0].steps[0].requestBody.bogusRequestBody",
        "#.workflows[0].steps[0].requestBody.replacements[0].bogusReplacement",
        "#.workflows[0].steps[0].requestBody.replacements[0].targetSelectorType.bogusTargetSelectorType",
        "#.workflows[0].steps[0].requestBody.replacements[0].value.bogusReplacementSelector",
        "#.workflows[0].steps[0].successCriteria[0].bogusCriterion",
        "#.workflows[0].steps[0].successCriteria[0].type.bogusExprType",
        "#.workflows[0].steps[0].onSuccess[0].bogusSuccess",
        "#.workflows[0].steps[0].onSuccess[0].parameters[0].bogusSuccessParam",
        "#.workflows[0].steps[0].onFailure[0].bogusFailure",
        "#.workflows[0].steps[0].onFailure[0].parameters[0].bogusFailureParam",
        "#.workflows[0].steps[0].outputs.stepOut.bogusStepOutSelector",
        "#.components.bogusComponents",
        "#.components.parameters.locale.bogusCompParam",
        "#.components.successActions.ok.bogusCompSuccess",
        "#.components.failureActions.fail.bogusCompFailure",
    ];

    #[test]
    fn round_trip_preserves_unknown_fields_and_validation_reports_every_object() {
        let raw = rich_v1_1_with_unknown_fields();
        let doc: Description = serde_json::from_value(raw.clone()).unwrap();

        // Lossless JSON round-trip preserves both `x-` extensions and unknown keys.
        assert_eq!(serde_json::to_value(&doc).unwrap(), raw);

        // Validation reports every unknown key and flags each as about a key.
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        let mut actual_paths: Vec<&str> = err
            .errors
            .iter()
            .map(|e| {
                assert_eq!(e.message, UNKNOWN_MSG, "unexpected error: {e}");
                assert!(e.is_about_key(), "expected is_about_key for {e}");
                e.path.as_str()
            })
            .collect();
        actual_paths.sort_unstable();
        let mut expected: Vec<&str> = EXPECTED_PATHS.to_vec();
        expected.sort_unstable();
        assert_eq!(actual_paths, expected);

        // IgnoreUnknownFields suppresses all of them.
        doc.validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
            .expect("IgnoreUnknownFields should allow unknown fields");

        // `unknown_members(&raw)` still lists every one of them!
        let unknown = doc.unknown_members(&raw).unwrap();
        assert_eq!(unknown.version, "1.1.0");
        let mut unknown_paths: Vec<&str> =
            unknown.members.iter().map(|m| m.path.as_str()).collect();
        unknown_paths.sort_unstable();
        assert_eq!(unknown_paths, expected);
    }

    #[cfg(feature = "editor")]
    #[test]
    fn editor_reports_unknown_fields_once_and_honours_ignore_option() {
        use roas_arazzo::editor::{Document, Severity};

        let text = "arazzo: 1.1.0
info:
  title: Pets
  version: '1'
  summery: typo
sourceDescriptions:
  - name: api
    url: api.yaml
workflows:
  - workflowId: buy
    steps:
      - stepId: find
        operationId: findPet
";
        let doc = Document::read(text, EnumSet::empty());
        assert!(!doc.has_errors(), "{:?}", doc.diagnostics());
        assert_eq!(doc.diagnostics().len(), 1, "{:?}", doc.diagnostics());
        assert_eq!(doc.diagnostics()[0].severity, Severity::Warning);
        assert_eq!(doc.diagnostics()[0].path.as_deref(), Some("#.info.summery"));

        let ignored = Document::read(text, EnumSet::only(ValidationOptions::IgnoreUnknownFields));
        assert!(
            ignored.diagnostics().is_empty(),
            "{:?}",
            ignored.diagnostics()
        );
    }

    #[cfg(feature = "editor")]
    #[test]
    fn editor_reports_empty_valued_and_reusable_unknown_fields_as_warnings_unless_ignored() {
        use roas_arazzo::editor::{Document, Severity};

        let text = "arazzo: 1.1.0
info:
  title: Pets
  version: '1'
  summery: typo
  bogusNull: null
  bogusArr: []
  bogusObj: {}
sourceDescriptions:
  - name: api
    url: api.yaml
workflows:
  - workflowId: buy
    steps:
      - stepId: find
        operationId: findPet
        parameters:
          - reference: $components.parameters.p
            x-note: 1
components:
  parameters:
    p:
      name: p
      in: query
      value: 1
";
        let doc = Document::read(text, EnumSet::empty());
        assert!(!doc.has_errors(), "{:?}", doc.diagnostics());
        let mut paths: Vec<&str> = doc
            .diagnostics()
            .iter()
            .map(|d| {
                assert_eq!(d.severity, Severity::Warning, "{d:?}");
                assert!(d.range.is_some(), "expected source location for {d:?}");
                d.path.as_deref().unwrap()
            })
            .collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            [
                "#.info.bogusArr",
                "#.info.bogusNull",
                "#.info.bogusObj",
                "#.info.summery",
                "#.workflows[0].steps[0].parameters[0].x-note",
            ]
        );

        let ignored = Document::read(text, EnumSet::only(ValidationOptions::IgnoreUnknownFields));
        assert!(
            ignored.diagnostics().is_empty(),
            "{:?}",
            ignored.diagnostics()
        );
    }
}
