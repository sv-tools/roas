use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{ExecutionError, InputError, InputValidation, Options, Run, prepare};
use serde_json::{Value, json};

fn value(schema: Value) -> Value {
    json!({"arazzo":"1.1.0","info":{"title":"Inputs","version":"1"},
        "sourceDescriptions":[{"name":"api","url":"https://example.com/api.json","type":"openapi"}],
        "workflows":[{"workflowId":"root","inputs":schema,
            "steps":[{"stepId":"request","operationId":"check"}]}]})
}

fn document(schema: Value) -> Description {
    serde_json::from_value(value(schema)).unwrap()
}

fn options() -> Options {
    Options::new().source(
        "api",
        "https://example.com/api.json",
        json!({
            "openapi":"3.1.0","servers":[{"url":"https://example.com"}],
            "paths":{"/check":{"get":{"operationId":"check"}}}
        }),
    )
}

#[test]
fn non_object_batches_are_never_silently_ignored() {
    let description = document(json!({}));
    for input in [
        Value::Null,
        json!([]),
        json!(7),
        json!("secret"),
        json!(true),
    ] {
        let options = options().input("kept", 1).inputs(input);
        assert!(matches!(
            Run::start(&description, &options),
            Err(ExecutionError::Input(InputError::NotObject))
        ));
        assert!(
            prepare(&description, &options)
                .unwrap_err()
                .to_string()
                .contains("must be a JSON object")
        );
        assert!(Run::start(&description, &options.inputs(json!({}))).is_ok());
    }
}

#[test]
fn disabled_mode_remains_pass_through() {
    let description = document(json!({"type":"object","required":["missing"]}));
    let options = options();
    let plan = prepare(&description, &options).unwrap();
    assert_eq!(options.input_validation_mode(), InputValidation::Disabled);
    assert_eq!(plan.input_validation(), InputValidation::Disabled);
    assert!(plan.start().is_ok());
}

#[test]
fn schema_registration_rejects_invalid_uris_and_conflicts() {
    assert!(Options::new().input_schema_base("relative.json").is_err());
    assert!(
        Options::new()
            .schema_document("https://example.com/schema#anchor", json!({}))
            .is_err()
    );
    let options = Options::new()
        .schema_document("https://example.com/schema", json!({}))
        .unwrap();
    assert!(
        options
            .clone()
            .schema_document("https://example.com/schema#", json!({}))
            .is_ok()
    );
    assert!(
        options
            .schema_document("https://example.com/schema", json!(false))
            .is_err()
    );
}

#[cfg(not(feature = "input-validation"))]
#[test]
fn requesting_unavailable_validation_fails_even_without_an_input_schema() {
    let mut description = document(json!({}));
    description.workflows[0].inputs = None;
    let options = options().input_validation(InputValidation::Draft202012);
    assert!(matches!(
        Run::start(&description, &options),
        Err(ExecutionError::Input(InputError::Unavailable))
    ));
    assert!(
        prepare(&description, &options)
            .unwrap_err()
            .to_string()
            .contains("Cargo feature")
    );
}

#[cfg(feature = "input-validation")]
mod enabled {
    use super::*;
    use roas_arazzo_executor::{Outcome, Progress, execute, execute_with_report, testing::Fake};

    fn checked() -> Options {
        options().input_validation(InputValidation::Draft202012)
    }
    fn fake() -> Fake {
        Fake::new()
            .reply(200, &json!({}))
            .reply(200, &json!({}))
            .reply(200, &json!({}))
    }

    #[test]
    fn required_type_nested_and_additional_properties_reject_before_sending() {
        let description = document(json!({"type":"object","properties":{"user":{
            "type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false
        }},"required":["user"],"additionalProperties":false}));
        for inputs in [
            json!({}),
            json!({"user":7}),
            json!({"user":{"name":1}}),
            json!({"user":{"name":"ok","extra":1}}),
        ] {
            let options = checked().inputs(inputs);
            let plan = prepare(&description, &options).unwrap();
            assert_eq!(plan.input_validation(), InputValidation::Draft202012);
            let mut lazy = fake();
            let mut prepared = fake();
            let failure = execute_with_report(&description, &options, &mut lazy).unwrap_err();
            assert!(failure.report.is_none());
            assert!(
                matches!(&failure.error, ExecutionError::Input(InputError::Invalid { workflow, violations }) if workflow == "root" && !violations.is_empty())
            );
            assert_eq!(
                failure.error.to_string(),
                plan.execute(&mut prepared).unwrap_err().error.to_string()
            );
            assert!(lazy.sent().is_empty() && prepared.sent().is_empty());
        }
    }

    #[test]
    fn diagnostics_have_locations_and_mask_instance_secrets() {
        let description =
            document(json!({"type":"object","properties":{"password":{"type":"integer"}}}));
        let options = checked().input("password", "do-not-log-me");
        let error = Run::start(&description, &options).err().unwrap();
        let ExecutionError::Input(InputError::Invalid { violations, .. }) = error else {
            panic!("{error}")
        };
        assert_eq!(violations[0].instance_path, "/password");
        assert!(
            violations[0]
                .schema_path
                .ends_with("/properties/password/type"),
            "{:?}",
            violations
        );
        assert!(!violations[0].to_string().contains("do-not-log-me"));
    }

    #[test]
    fn all_violations_are_reported_in_stable_location_order() {
        let description =
            document(json!({"properties":{"z":{"type":"integer"},"a":{"type":"integer"}}}));
        let options = checked().inputs(json!({"z":"z-secret","a":"a-secret"}));
        let error = Run::start(&description, &options).err().unwrap();
        let ExecutionError::Input(InputError::Invalid { violations, .. }) = error else {
            panic!("{error}")
        };
        assert_eq!(
            violations
                .iter()
                .map(|v| v.instance_path.as_str())
                .collect::<Vec<_>>(),
            ["/a", "/z"]
        );
        assert!(!format!("{violations:?}").contains("secret"));
    }

    #[test]
    fn malformed_uri_scopes_and_schema_keyword_shapes_are_configuration_errors() {
        for schema in [
            json!({"$id":"https://["}),
            json!({"$ref":"https://["}),
            json!({"properties":{"x":{"$ref":"https://["}}}),
            json!({"allOf":17}),
            json!({"properties":17}),
        ] {
            let description = document(schema);
            assert!(prepare(&description, &checked()).is_err());
        }
        let mut raw = value(json!({}));
        raw["$self"] = json!("relative.json");
        let description: Description = serde_json::from_value(raw).unwrap();
        assert!(
            prepare(&description, &checked())
                .unwrap_err()
                .to_string()
                .contains("absolute base")
        );
    }

    #[test]
    fn duplicate_supplied_resources_must_be_identical() {
        let description = document(json!({}));
        let api = json!({"openapi":"3.1.0","servers":[{"url":"https://example.com"}],
            "paths":{"/check":{"get":{"operationId":"check"}}}});
        let options = checked()
            .schema_document("https://example.com/api.json", api)
            .unwrap();
        assert!(prepare(&description, &options).is_ok());
        let options = checked()
            .schema_document("https://example.com/api.json", json!({}))
            .unwrap();
        assert!(
            prepare(&description, &options)
                .unwrap_err()
                .to_string()
                .contains("different documents")
        );
    }

    #[test]
    fn openapi_embedded_schema_anchors_keep_their_original_locations() {
        let description = document(json!({"$ref":"https://example.com/api.json#params"}));
        let mut api = json!({"openapi":"3.1.0","servers":[{"url":"https://example.com"}],
            "paths":{"/check":{"get":{"operationId":"check"}}},
            "components":{"schemas":{"Params":{"$anchor":"params","required":["name"]}}},
            "$defs":{"roas-input-index":{}}});
        let options = Options::new()
            .input_validation(InputValidation::Draft202012)
            .source("api", "https://example.com/api.json", api.clone());
        let error = prepare(&description, &options)
            .unwrap()
            .start()
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("/components/schemas/Params/required"),
            "{error}"
        );
        api["$defs"] = json!(17);
        let options = Options::new()
            .input_validation(InputValidation::Draft202012)
            .source("api", "https://example.com/api.json", api);
        assert!(
            prepare(&description, &options)
                .unwrap_err()
                .to_string()
                .contains("non-object $defs")
        );
    }

    #[test]
    fn unrelated_workflows_without_inputs_and_relative_legacy_sources_still_work() {
        let mut raw = value(json!({}));
        raw["workflows"].as_array_mut().unwrap().push(
            json!({"workflowId":"unrelated","steps":[{"stepId":"other","operationId":"check"}]}),
        );
        let description: Description = serde_json::from_value(raw).unwrap();
        let options = Options::new()
            .input_validation(InputValidation::Draft202012)
            .source(
                "api",
                "api.json",
                json!({"openapi":"3.1.0","servers":[{"url":"https://example.com"}],
                "paths":{"/check":{"get":{"operationId":"check"}}}}),
            );
        let report = prepare(&description, &options)
            .unwrap()
            .execute(&mut fake())
            .unwrap();
        assert_eq!(report.input_validation, InputValidation::Draft202012);
    }

    #[test]
    fn schemas_compile_during_preparation_but_replacement_inputs_are_checked_at_start() {
        let description = document(
            json!({"required":["count"],"properties":{"count":{"type":"integer","minimum":1}}}),
        );
        let options = checked();
        let plan = prepare(&description, &options).unwrap();
        assert!(plan.start().is_err());
        assert!(
            plan.start_with_inputs(json!({"count":1}).as_object().unwrap().clone())
                .is_ok()
        );
        assert!(
            plan.start_with_inputs(json!({"count":"1"}).as_object().unwrap().clone())
                .is_err()
        );
        assert!(plan.start().is_err());
    }

    #[test]
    fn malformed_and_unresolved_schemas_are_not_instance_errors() {
        for schema in [
            json!({"type":17}),
            json!({"required":17}),
            json!({"$ref":"https://127.0.0.1:1/missing"}),
            json!({"$ref":"file:///definitely-not-supplied.json"}),
            json!({"$ref":"#absent"}),
        ] {
            let description = document(schema);
            let options = checked();
            let error = prepare(&description, &options).unwrap_err();
            assert!(
                error
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.path == "#.workflows[0].inputs")
            );
            let error = Run::start(&description, &options).err().unwrap();
            assert!(
                matches!(
                    error,
                    ExecutionError::Input(InputError::Schema { .. } | InputError::Configuration(_))
                ),
                "{error}"
            );
        }
    }

    #[test]
    fn no_default_insertion_coercion_or_format_assertion() {
        let description = document(
            json!({"properties":{"email":{"type":"string","format":"email"},"n":{"default":3}},"required":["email"]}),
        );
        let mut description = description;
        description.workflows[0].outputs.insert(
            "inputs".into(),
            serde_json::from_value(json!("$inputs")).unwrap(),
        );
        let options = checked().input("email", "not-an-email");
        let report = execute(&description, &options, &mut fake()).unwrap();
        assert_eq!(report.outputs["inputs"], json!({"email":"not-an-email"}));
        assert!(Run::start(&description, &checked().input("email", 3)).is_err());
    }

    #[test]
    fn dependencies_and_selected_root_are_checked_before_any_requests() {
        let mut raw = value(json!({"required":["root"]}));
        raw["workflows"][0]["dependsOn"] = json!(["dependency"]);
        raw["workflows"].as_array_mut().unwrap().push(json!({"workflowId":"dependency","inputs":{"required":["dep"]},"steps":[{"stepId":"d","operationId":"check"}]}));
        let description: Description = serde_json::from_value(raw).unwrap();
        for inputs in [json!({"dep":1}), json!({"root":1})] {
            let options = checked().inputs(inputs);
            let mut client = fake();
            assert!(execute(&description, &options, &mut client).is_err());
            assert!(
                prepare(&description, &options)
                    .unwrap()
                    .execute(&mut client)
                    .is_err()
            );
            assert!(client.sent().is_empty());
        }
        let options = checked().inputs(json!({"dep":1,"root":1}));
        let mut client = fake();
        assert!(
            prepare(&description, &options)
                .unwrap()
                .execute(&mut client)
                .unwrap()
                .is_success()
        );
        assert_eq!(client.sent().len(), 2);
    }

    fn child_document(argument: Value) -> Description {
        let mut raw = value(json!({}));
        raw["workflows"][0]["steps"].as_array_mut().unwrap().push(json!({"stepId":"call","workflowId":"child","parameters":[{"name":"count","value":argument}],
            "onFailure":[{"name":"recover","type":"end"}]}));
        raw["workflows"].as_array_mut().unwrap().push(json!({"workflowId":"child","inputs":{"required":["count"],"properties":{"count":{"type":"integer"}}},"steps":[{"stepId":"childRequest","operationId":"check"}]}));
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn child_entry_uses_bound_arguments_and_preserves_parent_history_on_rejection() {
        let description = child_document(json!("bad"));
        let options = checked().input("count", 42);
        let plan = prepare(&description, &options).unwrap();
        let mut lazy = fake();
        let mut prepared = fake();
        let a = execute_with_report(&description, &options, &mut lazy).unwrap_err();
        let b = plan.execute(&mut prepared).unwrap_err();
        assert_eq!(a.error.to_string(), b.error.to_string());
        for failure in [a, b] {
            assert!(
                matches!(failure.error, ExecutionError::Input(InputError::Invalid { workflow, .. }) if workflow == "child")
            );
            let report = failure.report.unwrap();
            assert_eq!(report.outcome, Outcome::Incomplete);
            assert_eq!(report.steps.len(), 1);
            assert_eq!(report.steps[0].step_id, "request");
        }
        assert_eq!(lazy.sent().len(), 1);
        assert_eq!(prepared.sent().len(), 1);
        let good = child_document(json!(42));
        assert!(execute(&good, &options, &mut fake()).unwrap().is_success());
    }

    #[test]
    fn terminal_entry_failure_stops_manual_runs() {
        let description = child_document(json!(false));
        let options = checked();
        let mut run = Run::start(&description, &options).unwrap();
        assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
        run.supply(roas_arazzo_executor::HttpResponse::json(200, &json!({})))
            .unwrap();
        assert!(matches!(run.advance(), Err(ExecutionError::Input(_))));
        assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
        assert_eq!(run.partial_report().steps.len(), 1);
    }

    #[test]
    fn reusable_inputs_resolve_by_pointer_and_anchor() {
        for reference in ["#/components/inputs/Common", "#common", "types/common.json"] {
            let mut raw = value(json!({"$ref":reference}));
            raw["$self"] = json!("https://example.com/workflow.json");
            let schema = if reference.starts_with("types/") {
                json!({"$id":"types/common.json","required":["name"]})
            } else {
                json!({"$anchor":"common","required":["name"]})
            };
            raw["components"] = json!({"inputs":{"Common":schema}});
            let description: Description = serde_json::from_value(raw).unwrap();
            let options = checked();
            let plan = prepare(&description, &options).unwrap();
            assert!(plan.start().is_err(), "{reference}");
            if reference == "#common" {
                let error = plan.start().err().unwrap();
                assert!(
                    error
                        .to_string()
                        .contains("/components/inputs/Common/required"),
                    "{error}"
                );
            }
            assert!(
                plan.start_with_inputs(json!({"name":"ok"}).as_object().unwrap().clone())
                    .is_ok()
            );
        }
    }

    #[test]
    fn schema_ids_rebase_relative_refs_and_anchors_are_not_pointers() {
        let mut raw = value(json!({"$id":"schemas/root.json","$ref":"child.json#payload"}));
        raw["$self"] = json!("../canonical/flow.json");
        let description: Description = serde_json::from_value(raw).unwrap();
        let options = checked()
            .input_schema_base("https://example.com/download/flow.json")
            .unwrap()
            .schema_document(
                "https://example.com/canonical/schemas/child.json",
                json!({"$defs":{"payload":{"$anchor":"payload","required":["token"]}}}),
            )
            .unwrap();
        let plan = prepare(&description, &options).unwrap();
        assert!(plan.start().is_err());
        assert!(
            plan.start_with_inputs(json!({"token":"ok"}).as_object().unwrap().clone())
                .is_ok()
        );
    }

    #[test]
    fn external_retrieval_uri_and_schema_identity_both_work() {
        for reference in [
            "https://example.com/download.json#payload",
            "https://example.com/canonical.json#payload",
        ] {
            let description = document(json!({"$ref":reference}));
            let options = checked().schema_document("https://example.com/download.json", json!({"$id":"https://example.com/canonical.json","$defs":{"Payload":{"$anchor":"payload","type":"object","required":["x"]}}})).unwrap();
            let plan = prepare(&description, &options).unwrap();
            assert!(plan.start().is_err());
            let error = plan.start().err().unwrap().to_string();
            assert!(
                error.contains("canonical.json#/$defs/Payload/required"),
                "{error}"
            );
        }
    }

    #[test]
    fn anchor_targets_preserve_escaped_pointer_and_uri_characters() {
        let description = document(json!({"$ref":"https://example.com/schema#payload"}));
        let options = checked()
            .schema_document(
                "https://example.com/schema",
                json!({
                    "$defs":{"a/b~c%20":{"$anchor":"payload","required":["x"]}}
                }),
            )
            .unwrap();
        let plan = prepare(&description, &options).unwrap();
        assert!(plan.start().is_err());
        assert!(
            plan.start_with_inputs(json!({"x":1}).as_object().unwrap().clone())
                .is_ok()
        );
    }

    #[test]
    fn schema_identity_cannot_shadow_a_different_supplied_document() {
        let description = document(json!({"$ref":"https://example.com/b"}));
        let options = checked()
            .schema_document(
                "https://example.com/a",
                json!({"$id":"https://example.com/b","type":"integer"}),
            )
            .unwrap()
            .schema_document("https://example.com/b", json!({"type":"object"}))
            .unwrap();
        let error = prepare(&description, &options).unwrap_err().to_string();
        assert!(
            error.contains("conflicts") || error.contains("claim"),
            "{error}"
        );
    }

    #[test]
    fn boolean_schemas_and_dynamic_recursion_have_2020_12_semantics() {
        assert!(Run::start(&document(json!(true)), &checked()).is_ok());
        assert!(matches!(
            Run::start(&document(json!(false)), &checked()),
            Err(ExecutionError::Input(InputError::Invalid { .. }))
        ));
        let description = document(
            json!({"$id":"https://example.com/dynamic", "$dynamicAnchor":"node",
            "type":"object", "required":["value"], "properties":{"value":{"type":"integer"},"child":{"$dynamicRef":"#node"}}}),
        );
        assert!(
            Run::start(
                &description,
                &checked().inputs(json!({"value":1,"child":{"value":2}}))
            )
            .is_ok()
        );
        assert!(
            Run::start(
                &description,
                &checked().inputs(json!({"value":1,"child":{"value":"wrong"}}))
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn async_entry_validation_matches_sync() {
        let description = child_document(json!(true));
        let options = checked();
        let mut client = fake();
        let error =
            roas_arazzo_executor::execute_async_with_report(&description, &options, &mut client)
                .await
                .unwrap_err();
        assert!(matches!(
            error.error,
            ExecutionError::Input(InputError::Invalid { .. })
        ));
        assert_eq!(client.sent().len(), 1);
    }

    #[test]
    fn recovery_workflow_entries_validate_their_explicit_parameters() {
        for kind in ["goto", "retry"] {
            let mut raw = value(json!({}));
            raw["workflows"][0]["steps"][0]["successCriteria"] =
                json!([{"condition":"$statusCode == 200"}]);
            let mut action = json!({"name":"recover","type":kind,"workflowId":"child","parameters":[{"name":"count","value":"bad"}]});
            if kind == "retry" {
                action["retryLimit"] = json!(1);
            }
            raw["workflows"][0]["steps"][0]["onFailure"] = json!([action]);
            raw["workflows"].as_array_mut().unwrap().push(json!({"workflowId":"child","inputs":{"properties":{"count":{"type":"integer"}}},"steps":[{"stepId":"childRequest","operationId":"check"}]}));
            let description: Description = serde_json::from_value(raw).unwrap();
            let options = checked();
            for prepared in [false, true] {
                let mut client = Fake::new().reply(500, &json!({}));
                let error = if prepared {
                    prepare(&description, &options)
                        .unwrap()
                        .execute(&mut client)
                } else {
                    execute_with_report(&description, &options, &mut client)
                }
                .unwrap_err();
                assert!(
                    matches!(
                        error.error,
                        ExecutionError::Input(InputError::Invalid { .. })
                    ),
                    "{kind}: {error}"
                );
                assert_eq!(client.sent().len(), 1);
                assert_eq!(error.report.unwrap().steps.len(), 1);
            }
        }
    }

    #[test]
    fn nested_scope_and_schema_keywords_do_not_interpret_annotation_data() {
        let mut raw = value(
            json!({"type":"object","properties":{"value":{"$ref":"#amount"}},
            "$defs":{"Value":{"$id":"types/amount.json","$anchor":"amount","type":"integer"}},
            "default":{"$id":"not a URI","$ref":"not a ref"}}),
        );
        raw["$self"] = json!("https://example.com/flow.json");
        // The anchor belongs to amount.json, not the enclosing Arazzo resource.
        let wrong: Description = serde_json::from_value(raw.clone()).unwrap();
        assert!(prepare(&wrong, &checked()).is_err());
        raw["workflows"][0]["inputs"]["properties"]["value"]["$ref"] =
            json!("types/amount.json#amount");
        let description: Description = serde_json::from_value(raw).unwrap();
        let options = checked().input("value", 1);
        assert!(prepare(&description, &options).unwrap().start().is_ok());
        assert!(Run::start(&description, &checked().input("value", "1")).is_err());
    }

    #[test]
    fn draft202012_assertions_and_recursive_refs_are_active() {
        let description = document(json!({"$id":"https://example.com/node","type":"object",
            "properties":{"value":{"type":"integer"},"child":{"$ref":"#"}},"required":["value"],"unevaluatedProperties":false}));
        let good = checked().inputs(json!({"value":1,"child":{"value":2}}));
        assert!(prepare(&description, &good).unwrap().start().is_ok());
        for inputs in [json!({"value":1,"child":{}}), json!({"value":1,"extra":1})] {
            assert!(Run::start(&description, &checked().inputs(inputs)).is_err());
        }
    }

    #[test]
    fn conflicting_ids_anchors_and_unsupported_dialects_fail_preparation() {
        for components in [
            json!({"A":{"$anchor":"same","type":"integer"},"B":{"$anchor":"same","type":"string"}}),
            json!({"A":{"$id":"https://example.com/same","type":"integer"},"B":{"$id":"https://example.com/same","type":"string"}}),
            json!({"A":{"$schema":"http://json-schema.org/draft-07/schema#"}}),
        ] {
            let mut raw = value(json!({"$ref":"#/components/inputs/A"}));
            raw["components"] = json!({"inputs":components});
            let description: Description = serde_json::from_value(raw).unwrap();
            assert!(prepare(&description, &checked()).is_err());
        }
    }

    #[cfg(feature = "source-graph")]
    #[test]
    fn registry_metadata_and_raw_schema_documents_supply_the_offline_universe() {
        use roas_arazzo_executor::SourceRegistry;
        let mut raw = value(json!({"$ref":"types/schema.json#payload"}));
        raw["$self"] = json!("../canonical/flow.json");
        let description: Description = serde_json::from_value(raw.clone()).unwrap();
        let mut registry = SourceRegistry::new();
        let owner = registry
            .insert("https://example.com/download/flow.json", raw)
            .unwrap();
        registry
            .insert_reference_document(
                "https://example.com/canonical/types/schema.json",
                json!({"$defs":{"Input":{"$anchor":"payload","required":["name"]}}}),
            )
            .unwrap();
        let options = checked().source_registry(&registry, owner).unwrap();
        let plan = prepare(&description, &options).unwrap();
        assert!(plan.start().is_err());
        assert!(
            plan.start_with_inputs(json!({"name":"ok"}).as_object().unwrap().clone())
                .is_ok()
        );
        assert!(
            registry
                .document(owner)
                .unwrap()
                .value()
                .get("$defs")
                .is_none()
        );
    }

    #[cfg(feature = "v1_0")]
    #[test]
    fn v1_0_upconversion_preserves_input_validation() {
        let mut raw = value(json!({"required":["missing"]}));
        raw["arazzo"] = json!("1.0.1");
        let description: roas_arazzo::v1_0::Description = serde_json::from_value(raw).unwrap();
        let mut client = fake();
        let error =
            roas_arazzo_executor::execute_v1_0(&description, &checked(), &mut client).unwrap_err();
        assert!(matches!(
            error,
            ExecutionError::Input(InputError::Invalid { .. })
        ));
        assert!(client.sent().is_empty());
    }
}
