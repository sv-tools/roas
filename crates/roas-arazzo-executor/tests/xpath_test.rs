//! XPath criteria and selectors over an XML response body, lazily and
//! through a prepared plan.
#![cfg(feature = "xpath")]

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    CriterionError, ExecutionError, HttpResponse, Options, SelectError, execute, prepare,
    testing::Fake,
};
use serde_json::{Value, json};

fn description(steps: Value) -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [{
            "workflowId": "w",
            "steps": steps,
            "outputs": { "total": "$steps.read.outputs.total" }
        }],
    }))
    .expect("a v1.1 description")
}

fn options() -> Options {
    Options::new().source(
        "api",
        "https://api.example.com/openapi.json",
        json!({
            "openapi": "3.0.3",
            "servers": [{ "url": "https://api.example.com" }],
            "paths": { "/order": { "get": { "operationId": "order" } } }
        }),
    )
}

fn xml(body: &str) -> HttpResponse {
    let mut response = HttpResponse::json(200, &Value::Null);
    response.headers = vec![("content-type".to_owned(), "application/xml".to_owned())];
    response.body = body.as_bytes().to_vec();
    response
}

const ORDER: &str = r#"<order id="7"><item>a</item><item>b</item><total>12.5</total></order>"#;

fn reading(criteria: Value) -> Description {
    description(json!([{
        "stepId": "read",
        "operationId": "order",
        "successCriteria": criteria,
        "outputs": {
            "total": { "type": "xpath", "context": "$response.body", "selector": "number(/order/total)" },
            "items": { "type": { "type": "xpath", "version": "xpath-30" }, "context": "$response.body", "selector": "/order/item" },
            "id": { "type": "xpath", "context": "$response.body", "selector": "/order/@id" }
        }
    }]))
}

#[test]
fn an_xml_body_is_judged_and_read_with_xpath_lazily_and_prepared() {
    let description = reading(json!([
        { "type": "xpath", "context": "$response.body", "condition": "count(/order/item) = 2" },
        { "type": { "type": "xpath", "version": "xpath-20" }, "context": "$response.body", "condition": "/order/total > 10" }
    ]));
    let options = options();
    let lazy = execute(
        &description,
        &options,
        &mut Fake::new().reply_with(xml(ORDER)),
    )
    .unwrap();
    let plan = prepare(&description, &options).unwrap();
    let prepared = plan
        .execute(&mut Fake::new().reply_with(xml(ORDER)))
        .unwrap();
    for report in [lazy, prepared] {
        assert!(report.is_success(), "{report}");
        let outputs = &report.steps[0].outputs;
        assert_eq!(outputs["total"], json!(12.5));
        assert_eq!(outputs["items"], json!(["a", "b"]));
        assert_eq!(outputs["id"], json!("7"));
        assert_eq!(report.outputs["total"], json!(12.5));
    }
}

#[test]
fn a_criterion_that_does_not_hold_fails_the_step() {
    let description = reading(json!([
        { "type": "xpath", "context": "$response.body", "condition": "/order/missing" }
    ]));
    let report = execute(
        &description,
        &options(),
        &mut Fake::new().reply_with(xml(ORDER)),
    )
    .unwrap();
    assert!(!report.is_success());
    assert_eq!(report.steps[0].criteria[0].error, None);
    assert!(!report.steps[0].criteria[0].passed);
}

#[test]
fn a_json_body_is_no_xpath_context() {
    let description = reading(json!([
        { "type": "xpath", "context": "$response.body", "condition": "/order" }
    ]));
    let report = execute(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({ "order": 1 })),
    )
    .unwrap();
    assert!(!report.is_success());
    assert!(matches!(
        report.steps[0].criteria[0].error,
        Some(CriterionError::Select(SelectError::Evaluation { .. }))
    ));
}

#[test]
fn preparation_checks_xpath_syntax_and_version() {
    let error = prepare(
        &reading(json!([
            { "type": "xpath", "context": "$response.body", "condition": "/order/[" }
        ])),
        &options(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("XPST0003"), "{error}");
    let error = prepare(
        &reading(json!([
            { "type": { "type": "xpath", "version": "xpath-40" }, "context": "$response.body", "condition": "/order" }
        ])),
        &options(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("XPath version `xpath-40`"),
        "{error}"
    );
    // Each version's syntax is its own: `let` is XPath 3.0.
    let error = prepare(
        &reading(json!([
            { "type": { "type": "xpath", "version": "xpath-20" }, "context": "$response.body", "condition": "let $x := 1 return $x" }
        ])),
        &options(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("XPST0003"), "{error}");
    // A condition with an expression in it is checked once it is filled in.
    assert!(
        prepare(
            &reading(json!([
                { "type": "xpath", "context": "$response.body", "condition": "/{$inputs.root}" }
            ])),
            &options()
        )
        .is_ok()
    );
}

#[test]
fn xpath_one_is_evaluated_as_itself() {
    // `'12.5' + 1` is a number in XPath 1.0 and a type error later.
    let description = reading(json!([
        { "type": { "type": "xpath", "version": "xpath-10" }, "context": "$response.body", "condition": "/order/total + 1 = 13.5" }
    ]));
    let report = execute(
        &description,
        &options(),
        &mut Fake::new().reply_with(xml(ORDER)),
    )
    .unwrap();
    assert!(report.is_success(), "{report}");
}

#[test]
fn a_lazy_run_refuses_an_unknown_xpath_version_and_an_xpath_replacement_target() {
    let refused = reading(json!([
        { "type": { "type": "xpath", "version": "xpath-40" }, "context": "$response.body", "condition": "/order" }
    ]));
    let failure = execute(
        &refused,
        &options(),
        &mut Fake::new().reply_with(xml(ORDER)),
    )
    .unwrap_err();
    assert!(matches!(
        failure,
        ExecutionError::Criterion(CriterionError::Unsupported("unknown XPath version"))
    ));
    // A refused function is unsupported too, so a failure action is not
    // taken in its place.
    let mut unsafe_function = reading(json!([
        { "type": "xpath", "context": "$response.body", "condition": "matches(/order/item, 'a')" }
    ]));
    unsafe_function.workflows[0].steps[0].on_failure = serde_json::from_value(json!([
        { "name": "give-up", "type": "end" }
    ]))
    .unwrap();
    let failure = execute(
        &unsafe_function,
        &options(),
        &mut Fake::new().reply_with(xml(ORDER)),
    )
    .unwrap_err();
    assert!(
        failure.to_string().contains("regular-expression functions"),
        "{failure}"
    );
    let replacing = description(json!([{
        "stepId": "read",
        "operationId": "order",
        "requestBody": { "payload": "<a/>", "replacements": [
            { "target": "/a", "targetSelectorType": "xpath", "value": "x" }
        ] }
    }]));
    let error = execute(&replacing, &options(), &mut Fake::new()).unwrap_err();
    assert!(
        error.to_string().contains("XPath payload replacement"),
        "{error}"
    );
    let error = prepare(&replacing, &options()).unwrap_err();
    assert!(error.to_string().contains("targetSelectorType"), "{error}");
}
