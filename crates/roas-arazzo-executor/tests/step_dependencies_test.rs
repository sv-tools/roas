//! A step that depends on a step of another workflow:
//! `$workflows.<workflowId>.steps.<stepId>`.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    ExecutionError, ExecutionReport, Options, Progress, Run, execute, prepare, testing::Fake,
};
use serde_json::{Value, json};

fn description(workflows: Value) -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": workflows,
    }))
    .expect("a v1.1 description")
}

fn options(workflow: &str) -> Options {
    Options::new().workflow(workflow).source(
        "api",
        "https://api.example.com/openapi.json",
        json!({
            "openapi": "3.0.3",
            "servers": [{ "url": "https://api.example.com" }],
            "paths": {
                "/a": { "get": { "operationId": "a" } },
                "/b": { "get": { "operationId": "b" } },
                "/c": { "get": { "operationId": "c" } }
            }
        }),
    )
}

fn step(id: &str, depends_on: &[&str]) -> Value {
    json!({ "stepId": id, "operationId": id, "dependsOn": depends_on })
}

fn steps(report: &ExecutionReport) -> Vec<(&str, &str)> {
    report
        .steps
        .iter()
        .map(|record| (record.workflow_id.as_str(), record.step_id.as_str()))
        .collect()
}

/// Both ways a run can go: prepared, and lazily.
fn both(description: &Description, workflow: &str, replies: usize) -> Vec<ExecutionReport> {
    let fake = || (0..replies).fold(Fake::new(), |fake, _| fake.reply(200, &json!({})));
    let options = options(workflow);
    let plan = prepare(description, &options).unwrap();
    vec![
        plan.execute(&mut fake()).unwrap(),
        execute(description, &options, &mut fake()).unwrap(),
    ]
}

#[test]
fn the_workflow_of_a_step_depended_on_runs_first() {
    let description = description(json!([
        { "workflowId": "w", "steps": [step("c", &["$workflows.auth.steps.a"])] },
        { "workflowId": "auth", "steps": [step("a", &[]), step("b", &[])] },
    ]));
    for report in both(&description, "w", 3) {
        assert!(report.is_success(), "{report}");
        assert_eq!(steps(&report), [("auth", "a"), ("auth", "b"), ("w", "c")]);
    }
}

#[test]
fn a_step_of_its_own_workflow_may_be_named_in_full() {
    let description = description(json!([
        { "workflowId": "w", "steps": [step("b", &["$workflows.w.steps.a"]), step("a", &[])] },
    ]));
    for report in both(&description, "w", 2) {
        assert_eq!(steps(&report), [("w", "a"), ("w", "b")]);
    }
}

#[test]
fn a_workflow_depended_on_twice_runs_once() {
    let description = description(json!([
        {
            "workflowId": "w",
            "dependsOn": ["auth"],
            "steps": [step("b", &["$workflows.auth.steps.a"]), step("c", &["$workflows.auth.steps.a"])]
        },
        { "workflowId": "auth", "steps": [step("a", &[])] },
    ]));
    for report in both(&description, "w", 3) {
        assert_eq!(steps(&report), [("auth", "a"), ("w", "b"), ("w", "c")]);
    }
}

#[test]
fn a_step_that_failed_ran() {
    let description = description(json!([
        { "workflowId": "w", "steps": [step("c", &["$workflows.auth.steps.a"])] },
        { "workflowId": "auth", "steps": [step("a", &[])] },
    ]));
    let report = execute(
        &description,
        &options("w"),
        &mut Fake::new().reply(500, &json!({})).reply(200, &json!({})),
    )
    .unwrap();
    assert_eq!(steps(&report), [("auth", "a"), ("w", "c")]);
    assert!(report.steps[1].passed);
}

#[test]
fn a_step_its_workflow_passed_over_stops_the_run() {
    let mut auth = json!({ "workflowId": "auth", "steps": [step("a", &[]), step("b", &[])] });
    auth["steps"][0]["onSuccess"] = json!([{ "name": "stop", "type": "end" }]);
    let description = description(json!([
        { "workflowId": "w", "steps": [step("c", &["$workflows.auth.steps.b"])] },
        auth,
    ]));
    let error = execute(
        &description,
        &options("w"),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("step `b` of workflow `auth`"),
        "got: {error}"
    );
}

#[test]
fn a_step_the_other_workflow_has_not_got_is_refused() {
    let description = description(json!([
        { "workflowId": "w", "steps": [step("c", &["$workflows.auth.steps.nope"])] },
        { "workflowId": "auth", "steps": [step("a", &[])] },
    ]));
    let error = prepare(&description, &options("w")).unwrap_err();
    assert!(error.to_string().contains("nope"), "got: {error}");
    assert_eq!(
        error.diagnostics[0].path, "#.workflows[0].steps[0].dependsOn[0]",
        "{error}"
    );
    let error = execute(
        &description,
        &options("w"),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownStep { ref step, .. } if step == "nope"),
        "got: {error}"
    );
}

#[test]
fn a_workflow_the_description_has_not_got_is_refused() {
    let description = description(json!([
        { "workflowId": "w", "steps": [step("c", &["$workflows.nope.steps.a"])] },
    ]));
    assert!(prepare(&description, &options("w")).is_err());
    assert!(matches!(
        Run::start(&description, &options("w")),
        Err(ExecutionError::UnknownWorkflow(_))
    ));
}

#[test]
fn a_step_depending_on_its_callers_workflow_is_a_circle() {
    let description = description(json!([
        { "workflowId": "w", "steps": [step("a", &[]), { "stepId": "call", "workflowId": "x" }] },
        { "workflowId": "x", "steps": [step("b", &["$workflows.w.steps.a"])] },
    ]));
    let error = execute(
        &description,
        &options("w"),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap_err();
    assert!(
        matches!(error, ExecutionError::Circular(ref id) if id == "w"),
        "got: {error}"
    );
}

#[test]
fn a_called_workflow_runs_what_its_steps_depend_on_first() {
    let description = description(json!([
        { "workflowId": "w", "steps": [{ "stepId": "call", "workflowId": "x" }] },
        { "workflowId": "x", "steps": [step("b", &["$workflows.auth.steps.a"])] },
        { "workflowId": "auth", "steps": [step("a", &[])] },
    ]));
    for report in both(&description, "w", 2) {
        assert_eq!(steps(&report), [("auth", "a"), ("x", "b"), ("w", "call")]);
    }
}

#[test]
fn a_root_dependency_waits_for_the_workflow_its_steps_depend_on() {
    let together = |depends_on: &[&str]| {
        let description = description(json!([
            { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [step("c", &[])] },
            { "workflowId": "one", "steps": [step("a", &[])] },
            { "workflowId": "two", "steps": [step("b", depends_on)] },
        ]));
        let options = options("t").parallel(4);
        let mut run = Run::start(&description, &options).unwrap();
        assert!(matches!(run.advance(), Ok(Progress::Send(_))));
        matches!(run.advance(), Ok(Progress::Send(_)))
    };
    assert!(together(&[]));
    assert!(!together(&["$workflows.one.steps.a"]));
}
