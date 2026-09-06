//! The executor's documented profile, not an assertion that unresolved Arazzo
//! semantics are identical in every implementation. All requests use a fake.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    CriterionError, ExecutionError, ExecutionReport, ExpressionError, Options, Run, execute,
    testing::Fake,
};
use serde_json::{Value, json};

fn document(steps: Value) -> Value {
    json!({
        "arazzo": "1.1.0",
        "info": { "title": "Condition profile", "version": "1" },
        "sourceDescriptions": [{ "name": "api", "url": "https://example.com/openapi.json", "type": "openapi" }],
        "workflows": [{ "workflowId": "w", "steps": steps }]
    })
}

fn options() -> Options {
    Options::new().source(
        "api",
        "https://example.com/openapi.json",
        json!({
            "openapi": "3.1.0",
            "servers": [{ "url": "https://example.com" }],
            "paths": { "/check": { "get": { "operationId": "check" } } }
        }),
    )
}

fn run(condition: &str, body: &Value) -> Result<ExecutionReport, ExecutionError> {
    let description = serde_json::from_value(document(json!([{
        "stepId": "check", "operationId": "check", "successCriteria": [{ "condition": condition }]
    }])))
    .unwrap();
    execute(&description, &options(), &mut Fake::new().reply(200, body))
}

#[test]
fn the_profile_corpus_runs_as_v1_1() {
    let corpus: Value = serde_json::from_str(include_str!("data/condition-profile.json")).unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        let condition = case["condition"].as_str().unwrap();
        let report =
            run(condition, &case["body"]).unwrap_or_else(|error| panic!("{condition}: {error}"));
        assert_eq!(
            report.is_success(),
            case["passed"].as_bool().unwrap(),
            "{condition}"
        );
    }
}

#[test]
#[cfg(feature = "v1_0")]
fn the_shared_language_has_the_same_profile_after_v1_0_conversion() {
    let corpus: Value = serde_json::from_str(include_str!("data/condition-profile.json")).unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        let mut value = document(json!([{
            "stepId": "check", "operationId": "check", "successCriteria": [{ "condition": case["condition"] }]
        }]));
        value["arazzo"] = json!("1.0.1");
        let description = serde_json::from_value(value).unwrap();
        let report = roas_arazzo_executor::execute_v1_0(
            &description,
            &options(),
            &mut Fake::new().reply(200, &case["body"]),
        )
        .unwrap();
        assert_eq!(
            report.is_success(),
            case["passed"].as_bool().unwrap(),
            "{}",
            case["condition"]
        );
    }
}

#[test]
fn malformed_conditions_are_located_before_any_request_even_in_skipped_branches() {
    for condition in [
        "true || $response.body[",
        "true || $unknown.foo",
        "false && $response.body#/bad~2",
        "$request.query.a=b == 1",
        "$response.body#/a=b == 1",
        "$request.path.a&b == 1",
        "$response.body#/a>b == 1",
        "$sourceDescriptions.api.a=b == 1",
        "$response.body#/no space == 1",
        "$response.header.a=b == 1",
        "1 < 2 < 3",
        "$response.body[]",
        "$response.body[-1]",
        "$response.body[01]",
        "$response.body[1.0]",
        "$response.body[999999999999999999999999999999999999]",
        "$response.body..name",
        "$response.body[0]suffix",
        "$response.body#not-a-pointer",
        "$response.body#/bad~",
        "$response.body#/a{b}",
        "'unclosed",
        "true | false",
        "!",
        "()",
        "true &&",
        "01 == 1",
        "+1 == 1",
        ".5 == 0.5",
        "1. == 1",
        "NaN == null",
        "inf == null",
        "Infinity == null",
        "1e9999 == null",
        "1e == 1",
        "1word == '1word'",
    ] {
        let description: Description = serde_json::from_value(document(json!([
            { "stepId": "first", "operationId": "check" },
            { "stepId": "bad", "operationId": "check", "successCriteria": [{ "condition": condition }] }
        ]))).unwrap();
        let mut client = Fake::new().reply(200, &json!({}));
        let error = execute(&description, &options(), &mut client).unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionError::Criterion(CriterionError::Syntax { .. })
            ),
            "{condition}: {error}"
        );
        assert!(
            error.to_string().contains("at byte"),
            "{condition}: {error}"
        );
        assert!(client.sent().is_empty(), "{condition}");
    }
}

#[test]
fn syntax_offsets_are_utf8_byte_offsets_in_the_original_condition() {
    let condition = "'🐈' == '🐈' && $response.body[bad]";
    let error = run(condition, &json!({})).unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("at byte {}:", condition.find('[').unwrap())),
        "{error}"
    );
}

#[test]
fn missing_values_are_not_null_and_wrong_type_navigation_is_distinct() {
    for condition in [
        "$response.body.missing",
        "$response.body.missing != 1",
        "$response.body.missing == null",
        "$response.body.items[1]",
    ] {
        let error = run(condition, &json!({ "items": [] })).unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionError::Criterion(CriterionError::Expression(
                    ExpressionError::Missing { .. }
                ))
            ),
            "{condition}: {error}"
        );
    }
    for condition in [
        "$statusCode.garbage == 200",
        "$response.body.items.name",
        "$response.body[0]",
        "$response.body.nil.name",
        "$response.body.items[0][0]",
    ] {
        let error = run(condition, &json!({ "items": [1], "nil": null })).unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionError::Criterion(CriterionError::Expression(
                    ExpressionError::Navigation { .. }
                ))
            ),
            "{condition}: {error}"
        );
    }
    assert!(
        run("$response.body.nil == null", &json!({ "nil": null }))
            .unwrap()
            .is_success()
    );
}

#[test]
fn bare_value_truthiness_is_an_explicit_compatibility_policy() {
    for (body, expected) in [
        (json!(null), false),
        (json!(false), false),
        (json!(true), true),
        (json!(0), false),
        (json!(-0.0), false),
        (json!(-1), true),
        (json!(0.5), true),
        (json!(""), false),
        (json!("false"), true),
        (json!([]), false),
        (json!([null]), true),
        (json!({}), false),
        (json!({ "x": false }), true),
    ] {
        assert_eq!(
            run("$response.body", &body).unwrap().is_success(),
            expected,
            "{body}"
        );
        assert_eq!(
            run("!$response.body", &body).unwrap().is_success(),
            !expected,
            "{body}"
        );
    }
}

#[test]
fn a_jsonpath_node_is_not_subject_to_simple_truthiness() {
    for value in [json!(false), json!(null), json!([]), json!({})] {
        let description = serde_json::from_value(document(json!([{
            "stepId": "check", "operationId": "check", "successCriteria": [{
                "type": "jsonpath", "context": "$response.body", "condition": "$.value"
            }]
        }])))
        .unwrap();
        assert!(
            execute(
                &description,
                &options(),
                &mut Fake::new().reply(200, &json!({ "value": value }))
            )
            .unwrap()
            .is_success()
        );
    }
}

#[test]
fn a_separate_context_or_selector_can_address_an_operator_in_a_pointer_key() {
    let description = serde_json::from_value(document(json!([{
        "stepId": "check", "operationId": "check",
        "successCriteria": [{ "type": "regex", "context": "$response.body#/a=b", "condition": "^1$" }],
        "outputs": { "selected": { "context": "$response.body", "selector": "/a=b", "type": "jsonpointer" } }
    }]))).unwrap();
    let report = execute(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({ "a=b": 1 })),
    )
    .unwrap();
    assert!(report.is_success());
    assert_eq!(report.steps[0].outputs["selected"], json!(1));
}

#[test]
fn standalone_names_keep_operator_characters_spaces_dots_and_hashes() {
    for name in ["a=b", "a&b", "a<b", "a>b", "a b", "a.b", "a#b", "🐈"] {
        let description = serde_json::from_value(document(json!([{
            "stepId": "check", "operationId": "check",
            "parameters": [{ "name": name, "in": "query", "value": 7 }],
            "outputs": { "query": format!("$request.query.{name}"), "pointer": format!("$response.body#/{name}") }
        }]))).unwrap();
        let report = execute(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({ name: 8 })),
        )
        .unwrap();
        assert_eq!(report.steps[0].outputs["query"], json!(7), "{name}");
        assert_eq!(report.steps[0].outputs["pointer"], json!(8), "{name}");
    }
    let description = serde_json::from_value(document(json!([{
        "stepId": "check", "operationId": "check",
        "parameters": [{ "name": "X.a!b#c&d", "in": "header", "value": "yes" }],
        "outputs": { "header": "$request.header.x.a!b#c&d" }
    }])))
    .unwrap();
    let report = execute(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap();
    assert_eq!(report.steps[0].outputs["header"], json!("yes"));
}

#[test]
fn dotted_identifiers_are_single_names_not_data_dependent_navigation() {
    let mut value = document(json!([
        { "stepId": "produce", "operationId": "check", "outputs": { "pet.name": "$response.body" } },
        { "stepId": "consume", "operationId": "check", "outputs": {
            "input": "$inputs.auth.token", "nested": "$inputs.auth#/token", "output": "$steps.produce.outputs.pet.name",
            "component": "$components.parameters.org.locale#/value"
        }, "successCriteria": [{ "condition": "$inputs.auth.token == 'flat' && $steps.produce.outputs.pet.name[0].name == 'Rex'" }] }
    ]));
    value["components"] = json!({ "parameters": { "org.locale": { "name": "locale", "in": "query", "value": "en" } } });
    let description = serde_json::from_value(value).unwrap();
    let options = options()
        .input("auth.token", "flat")
        .input("auth", json!({ "token": "nested" }));
    let report = execute(
        &description,
        &options,
        &mut Fake::new()
            .reply(200, &json!([{ "name": "Rex" }]))
            .reply(200, &json!({})),
    )
    .unwrap();
    assert_eq!(report.steps[1].outputs["input"], json!("flat"));
    assert_eq!(report.steps[1].outputs["nested"], json!("nested"));
    assert_eq!(
        report.steps[1].outputs["output"],
        json!([{ "name": "Rex" }])
    );
    assert_eq!(report.steps[1].outputs["component"], json!("en"));
}

#[test]
fn standalone_expressions_do_not_ignore_suffixes_or_bad_pointer_escapes() {
    for expression in [
        "$statusCode.garbage",
        "$url.extra",
        "$method.extra",
        "$self.extra",
        "$response.body.extra",
        "$request.body[0]",
        "$response.body#/a~2b",
        "$inputs.",
        "$steps.a.outputs.",
        "$response.header.bad name",
        "$components.noSuchGroup.x",
    ] {
        let description = serde_json::from_value(document(json!([{
            "stepId": "check", "operationId": "check", "outputs": { "value": expression }
        }])))
        .unwrap();
        let error = match Run::start(&description, &options()) {
            Ok(_) => panic!("accepted {expression}"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                ExecutionError::Expression(ExpressionError::Syntax { .. })
            ),
            "{expression}: {error}"
        );
    }
}

#[test]
fn dependencies_visit_short_circuited_branches_and_navigation_bases() {
    for condition in [
        "true || $steps.produce.outputs.pet[0].name == 'Rex'",
        "$steps.produce.response.body[0].name == 'Rex'",
    ] {
        let description = serde_json::from_value(document(json!([
            { "stepId": "consume", "operationId": "check", "successCriteria": [{ "condition": condition }] },
            { "stepId": "produce", "operationId": "check", "outputs": { "pet": "$response.body" } }
        ]))).unwrap();
        let report = execute(
            &description,
            &options(),
            &mut Fake::new()
                .reply(200, &json!([{ "name": "Rex" }]))
                .reply(200, &json!({})),
        )
        .unwrap();
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect::<Vec<_>>(),
            ["produce", "consume"]
        );
    }
}

#[test]
fn effective_inherited_parameters_are_shared_by_dependencies_and_requests() {
    let mut value = document(json!([
        { "stepId": "consume", "operationId": "check" },
        { "stepId": "produce", "operationId": "check", "parameters": [{ "name": "id", "in": "query", "value": 0 }], "outputs": { "id": "$response.body#/id" } }
    ]));
    value["workflows"][0]["parameters"] =
        json!([{ "reference": "$components.parameters.fromProducer" }]);
    value["components"] = json!({ "parameters": { "fromProducer": { "name": "id", "in": "query", "value": "$steps.produce.outputs.id" } } });
    let description = serde_json::from_value(value).unwrap();
    let mut client = Fake::new()
        .reply(200, &json!({ "id": 7 }))
        .reply(200, &json!({}));
    let report = execute(&description, &options(), &mut client).unwrap();
    assert_eq!(report.steps[0].step_id, "produce");
    assert!(client.sent()[0].url.ends_with("?id=0"));
    assert!(client.sent()[1].url.ends_with("?id=7"));
}

#[test]
fn replaced_defaults_are_not_parsed_or_evaluated() {
    let mut value = document(json!([{
        "stepId": "check", "operationId": "check", "parameters": [{ "name": "id", "in": "query", "value": 1 }]
    }]));
    value["workflows"][0]["parameters"] =
        json!([{ "name": "id", "in": "query", "value": "$not_a_runtime_expression" }]);
    let description = serde_json::from_value(value).unwrap();
    assert!(
        execute(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({}))
        )
        .unwrap()
        .is_success()
    );
}

#[test]
fn nesting_is_bounded_and_flat_logical_chains_do_not_recurse() {
    let deeply_nested = format!("{}true{}", "(".repeat(65), ")".repeat(65));
    assert!(
        run(&deeply_nested, &json!({}))
            .unwrap_err()
            .to_string()
            .contains("nesting exceeds")
    );
    let nested = format!("{}true{}", "(".repeat(64), ")".repeat(64));
    assert!(run(&nested, &json!({})).unwrap().is_success());
    let chain = std::iter::repeat_n("false", 10_000)
        .collect::<Vec<_>>()
        .join(" || ");
    assert!(!run(&chain, &json!({})).unwrap().is_success());
}

#[test]
fn short_circuiting_does_not_hide_undeclared_steps_or_workflows() {
    for condition in [
        "true || $steps.typo.outputs.value",
        "false && $workflows.typo.outputs.value",
    ] {
        let error = run(condition, &json!({})).unwrap_err();
        assert!(
            matches!(
                error,
                ExecutionError::Criterion(CriterionError::Expression(
                    ExpressionError::Missing { .. }
                ))
            ),
            "{error}"
        );
    }
}

#[test]
fn a_workflow_call_applies_overrides_before_reading_inherited_arguments() {
    let mut value = document(json!([{
        "stepId": "call", "workflowId": "child", "parameters": [{ "name": "id", "value": 7 }]
    }]));
    value["workflows"][0]["parameters"] =
        json!([{ "name": "id", "in": "header", "value": "$inputs.notProvided" }]);
    value["workflows"].as_array_mut().unwrap().push(json!({
        "workflowId": "child", "steps": [{ "stepId": "read", "operationId": "check", "successCriteria": [{ "condition": "$inputs.id == 7" }] }]
    }));
    let description = serde_json::from_value(value).unwrap();
    assert!(
        execute(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({}))
        )
        .unwrap()
        .is_success()
    );
}

#[test]
fn shared_action_syntax_is_checked_without_reordering_steps() {
    for field in ["successActions", "failureActions"] {
        let mut value = document(json!([
            { "stepId": "consume", "operationId": "check" },
            { "stepId": "produce", "operationId": "check", "outputs": { "ready": true } }
        ]));
        value["workflows"][0][field] = json!([{ "name": "never", "type": "end", "criteria": [{ "condition": "false && $steps.produce.outputs.ready" }] }]);
        let description = serde_json::from_value(value.clone()).unwrap();
        let report = execute(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
        )
        .unwrap();
        assert_eq!(report.steps[0].step_id, "consume");
        value["workflows"][0][field][0]["criteria"][0]["condition"] =
            json!("true || $statusCode ==");
        let description = serde_json::from_value(value).unwrap();
        assert!(matches!(
            Run::start(&description, &options()),
            Err(ExecutionError::Criterion(CriterionError::Syntax { .. }))
        ));
    }
}

#[test]
fn shared_actions_naming_multiple_steps_do_not_create_cycles() {
    for field in ["successActions", "failureActions"] {
        for reusable in [false, true] {
            let mut value = document(json!([
                { "stepId": "a", "operationId": "check", "outputs": { "ready": true } },
                { "stepId": "b", "operationId": "check", "outputs": { "ready": true } }
            ]));
            let action = json!({
                "name": "never", "type": "end",
                "criteria": [{ "condition": "false && $steps.a.outputs.ready && $steps.b.outputs.ready" }],
                "parameters": [{ "name": "ignored", "value": "$steps.b.outputs.ready" }]
            });
            value["workflows"][0][field] = if reusable {
                value["components"] = json!({ field: { "shared": action } });
                json!([{ "reference": format!("$components.{field}.shared") }])
            } else {
                json!([action])
            };
            let description = serde_json::from_value(value).unwrap();
            let report = execute(
                &description,
                &options(),
                &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
            )
            .unwrap();
            assert_eq!(
                report
                    .steps
                    .iter()
                    .map(|step| step.step_id.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"],
                "{field}, reusable={reusable}"
            );
        }
    }
}

#[test]
fn unresolved_actions_are_reported_only_when_their_branch_is_considered() {
    for (step_field, workflow_field, inactive_status, active_status) in [
        ("onFailure", "failureActions", 200, 500),
        ("onSuccess", "successActions", 500, 200),
    ] {
        for shared in [false, true] {
            let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
            let entry = json!([{ "reference": format!("$components.{workflow_field}.missing") }]);
            if shared {
                value["workflows"][0][workflow_field] = entry;
            } else {
                value["workflows"][0]["steps"][0][step_field] = entry;
            }
            let description = serde_json::from_value(value).unwrap();
            let mut client = Fake::new().reply(inactive_status, &json!({}));
            execute(&description, &options(), &mut client)
                .expect("an unused action is not resolved eagerly");
            assert_eq!(client.sent().len(), 1);
            let mut client = Fake::new().reply(active_status, &json!({}));
            let error = execute(&description, &options(), &mut client).unwrap_err();
            assert!(matches!(error, ExecutionError::Unsupported(_)), "{error}");
            assert_eq!(
                client.sent().len(),
                1,
                "the reached action still reports its missing component"
            );
        }
    }
}

#[test]
fn an_action_parameter_is_not_resolved_when_its_criterion_is_false() {
    for (step_field, status) in [("onSuccess", 200), ("onFailure", 500)] {
        for enabled in [false, true] {
            let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
            value["workflows"][0]["steps"][0][step_field] = json!([{
                "name": "call", "type": "goto", "workflowId": "child",
                "criteria": [{ "condition": enabled.to_string() }],
                "parameters": [{ "reference": "$components.parameters.missing" }]
            }]);
            value["workflows"].as_array_mut().unwrap().push(json!({ "workflowId": "child", "steps": [{ "stepId": "c", "operationId": "check" }] }));
            let description = serde_json::from_value(value).unwrap();
            let mut client = Fake::new().reply(status, &json!({}));
            let result = execute(&description, &options(), &mut client);
            if enabled {
                assert!(matches!(result, Err(ExecutionError::Unsupported(_))));
            } else {
                result.expect("unselected arguments need not resolve");
            }
            assert_eq!(
                client.sent().len(),
                1,
                "arguments resolve only after selecting the action"
            );
        }
    }
}

#[test]
fn unresolved_action_arguments_do_not_erase_other_step_local_dependencies() {
    for field in ["onSuccess", "onFailure"] {
        let mut value = document(json!([
            { "stepId": "consume", "operationId": "check" },
            { "stepId": "produce", "operationId": "check", "outputs": { "ready": true } }
        ]));
        value["workflows"][0]["steps"][0][field] = json!([{
            "name": "never", "type": "end", "criteria": [{ "condition": "false" }],
            "parameters": [
                { "reference": "$components.parameters.missing" },
                { "name": "ready", "value": "$steps.produce.outputs.ready" }
            ]
        }]);
        let description = serde_json::from_value(value).unwrap();
        let report = execute(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
        )
        .unwrap();
        assert_eq!(report.steps[0].step_id, "produce");
    }
}

#[test]
fn resolved_step_action_syntax_errors_are_still_reported_before_io() {
    for (field, collection) in [
        ("onSuccess", "successActions"),
        ("onFailure", "failureActions"),
    ] {
        let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
        value["workflows"][0]["steps"][0][field] =
            json!([{ "reference": format!("$components.{collection}.broken") }]);
        value["components"] = json!({ collection: { "broken": {
            "name": "broken", "type": "end", "criteria": [{ "condition": "false && $response.body[" }]
        } } });
        let description = serde_json::from_value(value).unwrap();
        assert!(matches!(
            Run::start(&description, &options()),
            Err(ExecutionError::Criterion(CriterionError::Syntax { .. }))
        ));
    }
}

#[test]
fn an_earlier_selected_action_can_make_a_dangling_action_unreachable() {
    for (field, collection, status) in [
        ("onSuccess", "successActions", 200),
        ("onFailure", "failureActions", 500),
    ] {
        for shared in [false, true] {
            let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
            value["workflows"][0]["steps"][0][field] = json!([{ "name": "stop", "type": "end" }]);
            let missing = json!({ "reference": format!("$components.{collection}.missing") });
            if shared {
                value["workflows"][0][collection] = json!([missing]);
            } else {
                value["workflows"][0]["steps"][0][field]
                    .as_array_mut()
                    .unwrap()
                    .push(missing);
            }
            let description = serde_json::from_value(value).unwrap();
            let report = execute(
                &description,
                &options(),
                &mut Fake::new().reply(status, &json!({})),
            )
            .unwrap();
            assert_eq!(report.steps.len(), 1);
        }
    }
}

#[test]
fn shared_action_parameter_syntax_errors_are_reported_before_io() {
    for field in ["successActions", "failureActions"] {
        for reusable in [false, true] {
            for parameter in [
                json!({ "name": "bad", "value": "$statusCode.extra" }),
                json!({ "name": "bad", "value": { "nested": ["code={$statusCode.extra}"] } }),
                json!({ "name": "bad", "value": { "context": "$statusCode.extra", "selector": "/code", "type": "jsonpointer" } }),
                json!({ "reference": "$components.parameters.bad" }),
                json!({ "reference": "$components.parameters.good", "value": "$statusCode.extra" }),
            ] {
                let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
                value["components"] = json!({ "parameters": {
                    "bad": { "name": "bad", "value": "$statusCode.extra" },
                    "good": { "name": "good", "value": 200 }
                } });
                let action = json!({
                    "name": "never", "type": "end", "criteria": [{ "condition": "false" }],
                    "parameters": [
                        { "reference": "$components.parameters.missing" },
                        parameter
                    ]
                });
                value["workflows"][0][field] = if reusable {
                    value["components"][field] = json!({ "shared": action });
                    json!([{ "reference": format!("$components.{field}.shared") }])
                } else {
                    json!([action])
                };
                let description = serde_json::from_value(value).unwrap();
                let mut client = Fake::new();
                let error = execute(&description, &options(), &mut client).unwrap_err();
                assert!(
                    matches!(&error, ExecutionError::Expression(ExpressionError::Syntax { expression, .. }) if expression == "$statusCode.extra"),
                    "{field}, reusable={reusable}: {error}"
                );
                assert!(client.sent().is_empty());
            }
        }
    }
}

#[test]
fn shared_action_parameters_do_not_add_dependencies_or_look_up_values() {
    for field in ["successActions", "failureActions"] {
        let mut value = document(json!([
            { "stepId": "a", "operationId": "check", "outputs": { "ready": true } },
            { "stepId": "b", "operationId": "check", "outputs": { "ready": true } }
        ]));
        value["components"] = json!({ "parameters": {
            "overridden": { "name": "overridden", "value": "$statusCode.extra" }
        } });
        value["workflows"][0][field] = json!([{
            "name": "never", "type": "end", "criteria": [{ "condition": "false" }],
            "parameters": [
                { "reference": "$components.parameters.missing" },
                { "reference": "$components.parameters.overridden", "value": "$steps.b.outputs.ready" },
                { "name": "first", "value": "$steps.a.outputs.ready" },
                { "name": "unknown", "value": "$steps.undeclared.outputs.ready" },
                { "name": "absent", "value": { "nested": ["input={$inputs.absent}", false, 0, null] } },
                { "name": "selector", "value": { "context": "$steps.b.outputs.ready", "selector": "", "type": "jsonpointer" } }
            ]
        }]);
        let description = serde_json::from_value(value).unwrap();
        let report = execute(
            &description,
            &options(),
            &mut Fake::new().reply(200, &json!({})).reply(200, &json!({})),
        )
        .unwrap();
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"],
            "{field}"
        );
    }
}

#[test]
fn shared_action_arguments_resolve_only_when_the_action_is_selected() {
    for (field, status) in [("successActions", 200), ("failureActions", 500)] {
        for enabled in [false, true] {
            let mut value = document(json!([{ "stepId": "a", "operationId": "check" }]));
            value["workflows"][0][field] = json!([{
                "name": "call", "type": "goto", "workflowId": "child",
                "criteria": [{ "condition": enabled.to_string() }],
                "parameters": [{ "reference": "$components.parameters.missing" }]
            }]);
            value["workflows"].as_array_mut().unwrap().push(json!({
                "workflowId": "child", "steps": [{ "stepId": "c", "operationId": "check" }]
            }));
            let description = serde_json::from_value(value).unwrap();
            let mut client = Fake::new().reply(status, &json!({}));
            let result = execute(&description, &options(), &mut client);
            if enabled {
                assert!(matches!(result, Err(ExecutionError::Unsupported(_))));
            } else {
                result.expect("unselected shared arguments need not resolve");
            }
            assert_eq!(client.sent().len(), 1);
        }
    }
}

fn workflow_collection_document(outputs: Value, condition: &str) -> Description {
    let mut value = document(json!([
        { "stepId": "call", "workflowId": "inner" },
        {
            "stepId": "read", "operationId": "check",
            "requestBody": { "payload": "$workflows.inner.outputs" },
            "successCriteria": [{ "condition": condition }],
            "outputs": { "whole": "$workflows.inner.outputs" }
        }
    ]));
    value["workflows"].as_array_mut().unwrap().push(json!({
        "workflowId": "inner", "steps": [{ "stepId": "produce", "operationId": "check" }],
        "outputs": outputs
    }));
    serde_json::from_value(value).unwrap()
}

#[test]
fn an_empty_workflow_output_collection_is_a_value_and_a_false_condition() {
    let description = workflow_collection_document(json!({}), "$workflows.inner.outputs");
    let mut client = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let report = execute(&description, &options(), &mut client).unwrap();
    assert!(!report.is_success());
    assert_eq!(client.sent().len(), 2);
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_deref().unwrap()).unwrap(),
        json!({})
    );
}

#[test]
fn a_populated_workflow_output_collection_supports_whole_named_and_pointer_reads() {
    let outputs = json!({ "code": 200, "meta": { "ok": true } });
    let description = workflow_collection_document(
        outputs.clone(),
        "$workflows.inner.outputs && $workflows.inner.outputs.code == 200 && $workflows.inner.outputs#/code == 200 && $workflows.inner.code == 200",
    );
    let mut client = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let report = execute(&description, &options(), &mut client).unwrap();
    assert!(report.is_success());
    assert_eq!(
        report
            .steps
            .iter()
            .find(|step| step.step_id == "read")
            .unwrap()
            .outputs["whole"],
        outputs
    );
    assert_eq!(
        serde_json::from_slice::<Value>(client.sent()[1].body.as_deref().unwrap()).unwrap(),
        outputs
    );
}

#[test]
fn workflow_output_collections_preserve_not_run_and_unknown_workflow_errors() {
    for declared in [false, true] {
        let mut value = document(json!([{
            "stepId": "read", "operationId": "check",
            "successCriteria": [{ "condition": "$workflows.inner.outputs" }]
        }]));
        if declared {
            value["workflows"].as_array_mut().unwrap().push(json!({
                "workflowId": "inner", "steps": [{ "stepId": "produce", "operationId": "check" }]
            }));
        }
        let description = serde_json::from_value(value).unwrap();
        let mut client = Fake::new().reply(200, &json!({}));
        let error = execute(&description, &options(), &mut client).unwrap_err();
        if declared {
            assert!(matches!(
                error,
                ExecutionError::Criterion(CriterionError::Expression(ExpressionError::NotRun { expression, .. }))
                    if expression == "$workflows.inner.outputs"
            ));
        } else {
            assert!(matches!(
                error,
                ExecutionError::Criterion(CriterionError::Expression(ExpressionError::Missing { expression, .. }))
                    if expression == "$workflows.inner.outputs"
            ));
        }
        assert_eq!(client.sent().len(), 1);
    }
}
