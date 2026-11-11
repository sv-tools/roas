//! Stage 7: workflows of external Arazzo documents run in their own document
//! scope, on the same engine as local calls.
#![cfg(feature = "source-graph")]

use roas::loader::{Loader, LoaderError, ResourceFetcher};
use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    DocumentId, ExecutionError, ExecutionReport, Options, Outcome, Performed, PreparationIssue,
    Progress, Run, SourceLoadOptions, SourceRegistry, SourceVersion, execute, prepare,
    required_sources, testing::Fake,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use url::Url;

const ROOT: &str = "https://example.test/root.arazzo.json";
const ROOT_API: &str = "https://example.test/api.openapi.json";
const SHARED: &str = "https://example.test/shared/shared.arazzo.json";
const SHARED_API: &str = "https://example.test/shared/api.openapi.json";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/data/external-workflows/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn standard() -> Vec<(&'static str, Value)> {
    vec![
        (ROOT, fixture("root.arazzo.json")),
        (ROOT_API, fixture("api.openapi.json")),
        (SHARED, fixture("shared.arazzo.json")),
        (SHARED_API, fixture("shared-api.openapi.json")),
    ]
}

struct Graph {
    registry: SourceRegistry,
    root: DocumentId,
    description: Description,
}

impl Graph {
    /// Every document is supplied up front; loading only links them, offline.
    fn new(documents: &[(&str, Value)]) -> Self {
        let mut registry = SourceRegistry::new();
        let ids: Vec<_> = documents
            .iter()
            .map(|(uri, value)| registry.insert(uri, value.clone()).unwrap())
            .collect();
        let root = ids[0];
        let report = registry
            .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
            .unwrap();
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        let description = registry.document(root).unwrap().arazzo().unwrap().clone();
        Self {
            registry,
            root,
            description,
        }
    }

    fn id(&self, uri: &str) -> DocumentId {
        self.registry
            .resolve(self.root, uri, false)
            .unwrap()
            .expect("a registered document")
    }

    fn options(&self, workflow: &str) -> Options {
        Options::new()
            .workflow(workflow)
            .input("id", "seed")
            .source_registry(&self.registry, self.root)
            .unwrap()
    }
}

fn ok(value: &str) -> Value {
    json!({ "value": value })
}

/// What a run did, without timings: comparable across execution paths.
fn trace(report: &ExecutionReport) -> Vec<(Option<String>, String, String, u32, String)> {
    report
        .steps
        .iter()
        .map(|step| {
            let performed = match &step.performed {
                Performed::Request { url, status, .. } => format!("{url} {status}"),
                Performed::Workflow {
                    workflow_id,
                    outcome,
                } => format!("{workflow_id} {outcome}"),
                other => format!("{other:?}"),
            };
            (
                step.document.clone(),
                step.workflow_id.clone(),
                step.step_id.clone(),
                step.attempt,
                format!("{performed} {:?} {:?}", step.action, step.outputs),
            )
        })
        .collect()
}

fn urls(client: &Fake) -> Vec<String> {
    client
        .sent()
        .into_iter()
        .map(|request| request.url)
        .collect()
}

#[test]
fn an_external_child_runs_with_its_own_sources_components_and_self() {
    let graph = Graph::new(&standard());
    let options = graph.options("main");
    let plan = prepare(&graph.description, &options).unwrap();
    let mut client = Fake::new()
        .reply(200, &ok("root"))
        .reply(200, &ok("local"))
        .reply(200, &ok("external"));
    let report = plan.execute(&mut client).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(
        urls(&client),
        [
            "https://entry.example.test/things?id=seed",
            "https://entry.example.test/things?id=local-root",
            "https://shared.example.test/v1/things?id=root",
        ]
    );
    // The shared document's reusable header parameter goes only with its request.
    let tenant = |at: usize| {
        client.sent()[at]
            .headers
            .iter()
            .any(|(name, value)| name == "X-Tenant" && value == "shared")
    };
    assert!(!tenant(0) && !tenant(1) && tenant(2));

    assert_eq!(report.outputs["local"], "local");
    assert_eq!(report.outputs["external"], "external");
    assert_eq!(report.outputs["origin"], SHARED, "`$self` is the child's");
    // `$workflows.child` in the entry names the entry's own `child`.
    assert_eq!(report.outputs["finished"], "local");

    let external = report
        .steps
        .iter()
        .find(|step| step.step_id == "external")
        .unwrap();
    // The reusable end action ended the child, so `never` was not sent.
    assert_eq!(
        external.performed,
        Performed::Workflow {
            workflow_id: "$sourceDescriptions.shared.child".into(),
            outcome: Outcome::Ended,
        }
    );
    // `$sourceDescriptions.api` inside the child is the child's alias.
    assert_eq!(external.outputs["api"], "api.openapi.json");

    // Identical workflow, step and source names stay distinguishable.
    let gets: Vec<_> = report
        .steps
        .iter()
        .filter(|step| step.step_id == "get")
        .map(|step| (step.document.as_deref(), step.workflow_id.as_str()))
        .collect();
    assert_eq!(
        gets,
        [(None, "main"), (None, "child"), (Some(SHARED), "child")]
    );
    assert!(
        report
            .to_string()
            .contains(&format!("(workflow `child` of {SHARED})")),
        "{report}"
    );
}

#[test]
fn checked_lazy_async_and_manual_runs_agree() {
    let graph = Graph::new(&standard());
    let options = graph.options("main");
    let client = || {
        Fake::new()
            .reply(200, &ok("root"))
            .reply(200, &ok("local"))
            .reply(200, &ok("external"))
    };
    let plan = prepare(&graph.description, &options).unwrap();
    let checked = plan.execute(&mut client()).unwrap();
    let lazy = execute(&graph.description, &options, &mut client()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let asynchronous = runtime.block_on(plan.execute_async(&mut client())).unwrap();
    assert_eq!(trace(&checked), trace(&lazy));
    assert_eq!(trace(&checked), trace(&asynchronous));
    assert_eq!(checked.outputs, lazy.outputs);

    let mut run = plan.start().unwrap();
    let mut fake = client();
    let manual = loop {
        match run.advance().unwrap() {
            roas_arazzo_executor::Progress::Send(request) => {
                let response = roas_arazzo_executor::HttpClient::send(&mut fake, &request).unwrap();
                run.supply(response).unwrap();
            }
            roas_arazzo_executor::Progress::Wait(_) => {}
            roas_arazzo_executor::Progress::Done(report) => break *report,
            progress => panic!("unexpected progress: {progress:?}"),
        }
    };
    assert_eq!(trace(&checked), trace(&manual));
}

#[test]
fn base_url_overrides_belong_to_the_document_that_declares_the_alias() {
    let mut graph = Graph::new(&standard());
    let shared = graph.id("shared/shared.arazzo.json");
    graph
        .registry
        .override_base_url(shared, "api", "https://staging-shared.test")
        .unwrap();
    let options = graph
        .options("main")
        .base_url("api", "https://staging-entry.test");
    let plan = prepare(&graph.description, &options).unwrap();
    let mut client = Fake::new()
        .reply(200, &ok("root"))
        .reply(200, &ok("local"))
        .reply(200, &ok("external"));
    plan.execute(&mut client).unwrap();
    assert_eq!(
        urls(&client),
        [
            "https://staging-entry.test/things?id=seed",
            "https://staging-entry.test/things?id=local-root",
            "https://staging-shared.test/things?id=root",
        ]
    );
}

fn with_workflow(mut root: Value, workflow: Value) -> Value {
    root["workflows"].as_array_mut().unwrap().push(workflow);
    root
}

#[test]
fn an_external_dependency_runs_first_with_the_root_inputs() {
    let mut documents = standard();
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "dependent",
            "dependsOn": ["$sourceDescriptions.shared.child"],
            "steps": [{ "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                "parameters": [{ "name": "id", "in": "query", "value": "$inputs.id" }] }] }),
    );
    let graph = Graph::new(&documents);
    let options = graph.options("dependent");
    let plan = prepare(&graph.description, &options).unwrap();
    for report in [
        plan.execute(
            &mut Fake::new()
                .reply(200, &ok("dependency"))
                .reply(200, &ok("root")),
        )
        .unwrap(),
        execute(
            &graph.description,
            &options,
            &mut Fake::new()
                .reply(200, &ok("dependency"))
                .reply(200, &ok("root")),
        )
        .unwrap(),
    ] {
        assert!(report.is_success(), "{report}");
        assert_eq!(report.workflow_id, "dependent");
        let steps: Vec<_> = report
            .steps
            .iter()
            .map(|step| {
                (
                    step.document.as_deref(),
                    step.workflow_id.as_str(),
                    step.url(),
                )
            })
            .collect();
        assert_eq!(
            steps,
            [
                (
                    Some(SHARED),
                    "child",
                    Some("https://shared.example.test/v1/things?id=seed")
                ),
                (
                    None,
                    "dependent",
                    Some("https://entry.example.test/things?id=seed")
                ),
            ]
        );
    }
}

#[test]
fn recovery_calls_and_goto_handoffs_bind_arguments_in_the_callers_scope() {
    let mut documents = standard();
    let root = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "recovering", "steps": [{
            "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
            "successCriteria": [{ "condition": "$statusCode == 200" }],
            "onFailure": [{ "name": "refresh", "type": "retry", "retryLimit": 1,
                "workflowId": "$sourceDescriptions.shared.child",
                "parameters": [{ "name": "id", "value": "retry-{$inputs.id}" }] }] }] }),
    );
    documents[0].1 = with_workflow(
        root,
        json!({ "workflowId": "handoff", "steps": [
            { "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
              "outputs": { "value": "$response.body#/value" },
              "onSuccess": [{ "name": "away", "type": "goto",
                  "workflowId": "$sourceDescriptions.shared.child",
                  "parameters": [{ "name": "id", "value": "$steps.get.outputs.value" }] }] },
            { "stepId": "after", "operationId": "$sourceDescriptions.api.getThing" }
        ] }),
    );
    let graph = Graph::new(&documents);

    let options = graph.options("recovering");
    let plan = prepare(&graph.description, &options).unwrap();
    let mut client = Fake::new()
        .reply(503, &json!({}))
        .reply(200, &ok("refreshed"))
        .reply(200, &ok("root"));
    let report = plan.execute(&mut client).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(
        urls(&client),
        [
            "https://entry.example.test/things",
            "https://shared.example.test/v1/things?id=retry-seed",
            "https://entry.example.test/things",
        ]
    );
    let steps: Vec<_> = report
        .steps
        .iter()
        .map(|step| {
            (
                step.document.as_deref(),
                step.step_id.as_str(),
                step.attempt,
                step.action.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        steps,
        [
            (
                None,
                "get",
                1,
                Some("retry via workflow `$sourceDescriptions.shared.child`")
            ),
            (Some(SHARED), "get", 1, Some("ended")),
            (None, "get", 2, None),
        ]
    );

    let options = graph.options("handoff");
    let plan = prepare(&graph.description, &options).unwrap();
    let mut client = Fake::new()
        .reply(200, &ok("token"))
        .reply(200, &ok("handed"));
    let report = plan.execute(&mut client).unwrap();
    // A goto does not come back: `after` is never sent, and the entry
    // workflow ends the way the external one did.
    assert_eq!(
        urls(&client),
        [
            "https://entry.example.test/things",
            "https://shared.example.test/v1/things?id=token",
        ]
    );
    assert_eq!(report.outcome, Outcome::Ended);
    assert_eq!(
        report.steps[0].action.as_deref(),
        Some("goto workflow `$sourceDescriptions.shared.child`")
    );
    assert_eq!(report.steps[1].document.as_deref(), Some(SHARED));
}

/// The same workflow body, run once as a local call and once as an external
/// one, makes the same attempts, detours and decisions.
#[test]
fn retry_detours_inside_an_external_workflow_follow_local_call_semantics() {
    let flaky = |api: &str| {
        json!({ "workflowId": "flaky", "steps": [
            { "stepId": "get", "operationId": format!("$sourceDescriptions.{api}.getThing"),
              "parameters": [{ "name": "id", "in": "query", "value": "$inputs.id" }],
              "successCriteria": [{ "condition": "$statusCode == 200" }],
              "outputs": { "value": "$response.body#/value" },
              "onFailure": [{ "name": "fix", "type": "retry", "stepId": "fix", "retryLimit": 1 }] },
            { "stepId": "fix", "operationId": format!("$sourceDescriptions.{api}.getThing"),
              "parameters": [{ "name": "id", "in": "query", "value": "fix" }] }
        ], "outputs": { "value": "$steps.get.outputs.value" } })
    };
    let mut documents = standard();
    let root = with_workflow(documents[0].1.clone(), flaky("api"));
    let caller = |name: &str, target: &str| {
        json!({ "workflowId": name, "steps": [{
            "stepId": "call", "workflowId": target,
            "parameters": [{ "name": "id", "value": "$inputs.id" }] }],
            "outputs": { "value": "$steps.call.outputs.value" } })
    };
    let root = with_workflow(root, caller("callLocal", "flaky"));
    documents[0].1 = with_workflow(
        root,
        caller("callExternal", "$sourceDescriptions.shared.flaky"),
    );
    documents[2].1 = with_workflow(documents[2].1.clone(), flaky("api"));
    let graph = Graph::new(&documents);

    let mut traces = Vec::new();
    for workflow in ["callLocal", "callExternal"] {
        let options = graph.options(workflow);
        let plan = prepare(&graph.description, &options).unwrap();
        let mut client = Fake::new()
            .reply(500, &json!({}))
            .reply(200, &json!({}))
            .reply(200, &ok("fixed"))
            .reply(200, &json!({}));
        let report = plan.execute(&mut client).unwrap();
        assert!(report.is_success(), "{report}");
        assert_eq!(report.outputs["value"], "fixed");
        let paths: Vec<_> = urls(&client)
            .iter()
            .map(|url| url.split_once("/things").unwrap().1.to_owned())
            .collect();
        let steps: Vec<_> = report
            .steps
            .iter()
            .map(|step| {
                (
                    step.step_id.clone(),
                    step.attempt,
                    step.passed,
                    step.action.clone(),
                    step.status(),
                )
            })
            .collect();
        traces.push((paths, steps));
    }
    assert_eq!(traces[0], traces[1]);
    assert_eq!(
        traces[0].0,
        ["?id=seed", "?id=fix", "?id=seed", "?id=fix"],
        "the detour hands control back to the failed step"
    );
}

fn api(server: &str) -> Value {
    json!({ "openapi": "3.1.0", "info": { "title": "API", "version": "1" },
        "servers": [{ "url": server }],
        "paths": { "/ping": { "get": { "operationId": "ping" } } } })
}

fn arazzo(version: &str, sources: Value, workflows: Value) -> Value {
    json!({ "arazzo": version, "info": { "title": "Doc", "version": "1" },
        "sourceDescriptions": sources, "workflows": workflows })
}

const A: &str = "https://example.test/cycle/a.arazzo.json";
const B: &str = "https://example.test/cycle/b.arazzo.json";

fn mutual(a_workflow: Value, b_workflow: Value) -> Graph {
    Graph::new(&[
        (
            A,
            arazzo(
                "1.1.0",
                json!([{ "name": "b", "url": "b.arazzo.json", "type": "arazzo" },
                       { "name": "api", "url": "a.openapi.json", "type": "openapi" }]),
                json!([a_workflow]),
            ),
        ),
        (
            B,
            arazzo(
                "1.1.0",
                json!([{ "name": "a", "url": "a.arazzo.json", "type": "arazzo" },
                       { "name": "api", "url": "b.openapi.json", "type": "openapi" }]),
                json!([b_workflow]),
            ),
        ),
        (
            "https://example.test/cycle/a.openapi.json",
            api("https://a.example.test"),
        ),
        (
            "https://example.test/cycle/b.openapi.json",
            api("https://b.example.test"),
        ),
    ])
}

#[test]
fn a_dependency_cycle_through_another_document_fails_preparation() {
    let graph = mutual(
        json!({ "workflowId": "w", "dependsOn": ["$sourceDescriptions.b.w"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }),
        json!({ "workflowId": "w", "dependsOn": ["$sourceDescriptions.a.w"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }),
    );
    let options = Options::new()
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    let error = prepare(&graph.description, &options).unwrap_err();
    assert!(
        error.diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.issue,
            PreparationIssue::Execution(ExecutionError::Circular(_))
        )),
        "{error}"
    );
    let mut client = Fake::new();
    let error = execute(&graph.description, &options, &mut client).unwrap_err();
    assert!(
        matches!(&error, ExecutionError::Circular(at) if at == "w"),
        "{error}"
    );
    assert!(client.sent().is_empty());
}

#[test]
fn source_cycles_are_harmless_and_recursive_calls_stop_at_the_depth_limit() {
    let graph = mutual(
        json!({ "workflowId": "w", "steps": [
            { "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" },
            { "stepId": "across", "workflowId": "$sourceDescriptions.b.w" }] }),
        json!({ "workflowId": "w", "steps": [
            { "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" },
            { "stepId": "back", "workflowId": "$sourceDescriptions.a.w" }] }),
    );
    let options = Options::new()
        .max_depth(3)
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    // Recursion is not a static error: the call graph is finite data.
    let plan = prepare(&graph.description, &options).unwrap();
    let mut client = Fake::new();
    for _ in 0..8 {
        client = client.reply(200, &json!({}));
    }
    let failure = plan.execute(&mut client).unwrap_err();
    assert!(
        matches!(
            failure.error,
            ExecutionError::Limit {
                limit: "workflow depth",
                at: 3
            }
        ),
        "{}",
        failure.error
    );
    assert_eq!(
        urls(&client),
        [
            "https://a.example.test/ping",
            "https://b.example.test/ping",
            "https://a.example.test/ping",
        ]
    );
    let report = failure.report.unwrap();
    assert_eq!(report.outcome, Outcome::Incomplete);
    let documents: Vec<_> = report
        .steps
        .iter()
        .map(|step| step.document.as_deref())
        .collect();
    assert_eq!(documents, [None, Some(B), None]);
}

#[derive(Clone, Default)]
struct Memory {
    documents: BTreeMap<String, Value>,
    reads: Rc<RefCell<Vec<String>>>,
}

impl ResourceFetcher for Memory {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        self.reads.borrow_mut().push(uri.to_string());
        self.documents
            .get(uri.as_str())
            .cloned()
            .ok_or_else(|| LoaderError::NoFetcherRegistered {
                uri: uri.to_string(),
            })
    }
}

#[test]
fn shared_documents_in_a_diamond_are_loaded_once_and_run_as_one_document() {
    let call = |id: &str, target: &str| json!({ "stepId": id, "workflowId": target });
    let root = arazzo(
        "1.1.0",
        json!([{ "name": "left", "url": "left.arazzo.json", "type": "arazzo" },
               { "name": "right", "url": "right.arazzo.json", "type": "arazzo" }]),
        json!([{ "workflowId": "w", "steps": [
            call("left", "$sourceDescriptions.left.w"),
            call("right", "$sourceDescriptions.right.w")] }]),
    );
    let side = || {
        arazzo(
            "1.1.0",
            json!([{ "name": "common", "url": "common.arazzo.json", "type": "arazzo" }]),
            json!([{ "workflowId": "w", "steps": [call("common", "$sourceDescriptions.common.w")] }]),
        )
    };
    let common = arazzo(
        "1.1.0",
        json!([{ "name": "api", "url": "api.openapi.json" }]),
        json!([{ "workflowId": "w", "steps": [
            { "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }]),
    );
    let memory = Memory {
        documents: [
            ("https://example.test/d/left.arazzo.json", side()),
            ("https://example.test/d/right.arazzo.json", side()),
            ("https://example.test/d/common.arazzo.json", common),
            (
                "https://example.test/d/api.openapi.json",
                api("https://api.example.test"),
            ),
        ]
        .into_iter()
        .map(|(uri, value)| (uri.to_owned(), value))
        .collect(),
        reads: Rc::default(),
    };
    let mut loader = Loader::new();
    loader.register_fetcher("https://", memory.clone());
    let mut registry = SourceRegistry::new();
    let id = registry
        .insert("https://example.test/d/root.arazzo.json", root.clone())
        .unwrap();
    let report = registry
        .load_sources(id, &mut loader, &SourceLoadOptions::default())
        .unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let reads = memory.reads.borrow().clone();
    assert_eq!(
        reads.iter().collect::<BTreeSet<_>>().len(),
        reads.len(),
        "{reads:?}"
    );

    let description: Description = serde_json::from_value(root).unwrap();
    let options = Options::new().source_registry(&registry, id).unwrap();
    let plan = prepare(&description, &options).unwrap();
    let mut client = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let report = plan.execute(&mut client).unwrap();
    assert!(report.is_success(), "{report}");
    let common: Vec<_> = report
        .steps
        .iter()
        .filter(|step| step.step_id == "ping")
        .map(|step| step.document.as_deref())
        .collect();
    assert_eq!(
        common,
        [Some("https://example.test/d/common.arazzo.json"); 2]
    );
}

#[test]
fn v1_0_and_v1_1_documents_combine_after_independent_upconversion() {
    let mut traces = Vec::new();
    for (root_version, child_version) in [
        ("1.0.1", "1.0.1"),
        ("1.0.1", "1.1.0"),
        ("1.1.0", "1.0.1"),
        ("1.1.0", "1.1.0"),
    ] {
        let graph = Graph::new(&[
            (
                ROOT,
                arazzo(
                    root_version,
                    json!([{ "name": "shared", "url": "shared/shared.arazzo.json", "type": "arazzo" }]),
                    json!([{ "workflowId": "main", "steps": [{
                        "stepId": "call", "workflowId": "$sourceDescriptions.shared.child",
                        "parameters": [{ "name": "id", "value": "$inputs.id" }] }],
                        "outputs": { "value": "$steps.call.outputs.value" } }]),
                ),
            ),
            (
                SHARED,
                arazzo(
                    child_version,
                    json!([{ "name": "api", "url": "api.openapi.json", "type": "openapi" }]),
                    json!([{ "workflowId": "child", "steps": [{
                        "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                        "parameters": [{ "name": "id", "in": "query", "value": "$inputs.id" }],
                        "successCriteria": [{ "condition": "$statusCode == 200" }],
                        "outputs": { "value": "$response.body#/value" } }],
                        "outputs": { "value": "$steps.get.outputs.value" } }]),
                ),
            ),
            (SHARED_API, fixture("shared-api.openapi.json")),
        ]);
        let versions = [graph.root, graph.id("shared/shared.arazzo.json")]
            .map(|id| graph.registry.document(id).unwrap().model());
        let expected = |version: &str| {
            if version == "1.0.1" {
                SourceVersion::Arazzo1_0
            } else {
                SourceVersion::Arazzo1_1
            }
        };
        assert_eq!(versions, [expected(root_version), expected(child_version)]);
        let options = graph.options("main");
        let plan = prepare(&graph.description, &options).unwrap();
        let report = plan
            .execute(&mut Fake::new().reply(200, &ok("value")))
            .unwrap();
        assert!(report.is_success(), "{report}");
        assert_eq!(report.outputs["value"], "value");
        // The v1.0 convenience wrapper upconverts the entry the same way.
        #[cfg(feature = "v1_0")]
        if root_version == "1.0.1" {
            let v1_0: roas_arazzo::v1_0::Description = serde_json::from_value(
                graph.registry.document(graph.root).unwrap().value().clone(),
            )
            .unwrap();
            let legacy = roas_arazzo_executor::execute_v1_0(
                &v1_0,
                &options,
                &mut Fake::new().reply(200, &ok("value")),
            )
            .unwrap();
            assert_eq!(trace(&legacy), trace(&report));
        }
        traces.push(trace(&report));
    }
    assert!(traces.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn preparation_locates_findings_inside_the_external_document() {
    let mut documents = standard();
    let shared = &mut documents[2].1;
    shared["workflows"][0]["steps"][0]["parameters"][1]["value"] = json!("$inputs.");
    shared["workflows"][0]["steps"][1]["stepId"] = json!("get");
    let graph = Graph::new(&documents);
    let error = prepare(&graph.description, &graph.options("main")).unwrap_err();
    let external: Vec<_> = error
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.document.as_deref() == Some(SHARED))
        .collect();
    assert!(
        external
            .iter()
            .any(|diagnostic| matches!(diagnostic.issue, PreparationIssue::Model(_))),
        "{error}"
    );
    assert!(
        external.iter().any(|diagnostic| {
            diagnostic.workflow_id.as_deref() == Some("child")
                && diagnostic.path.ends_with("parameters[1].value")
                && diagnostic.to_string().starts_with(SHARED)
        }),
        "{error}"
    );
    assert!(
        error
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.document.as_deref() == Some(SHARED)),
        "the entry document is clean: {error}"
    );
}

/// `shared.arazzo.json` as YAML, with a malformed condition behind
/// escapes and a multi-byte character.
const SHARED_YAML: &str = r#"arazzo: 1.1.0
$self: https://example.test/shared/shared.arazzo.json
info: { title: Shared, version: "1" }
sourceDescriptions:
  - { name: api, url: api.openapi.json, type: openapi }
components:
  parameters:
    tenant: { name: X-Tenant, in: header, value: shared }
  successActions:
    done:
      name: done
      type: end
      criteria: [{ condition: $statusCode == 200 }]
workflows:
  - workflowId: child
    steps:
      - stepId: get
        operationId: $sourceDescriptions.api.getThing
        parameters:
          - reference: $components.parameters.tenant
          - { name: id, in: query, value: $inputs.id }
        successCriteria:
          - condition: "$response.body#/n\u00e9 == \"café\" && ("
        onSuccess: [{ reference: $components.successActions.done }]
        outputs: { value: $response.body#/value, origin: $self }
    outputs:
      value: $steps.get.outputs.value
      origin: $steps.get.outputs.origin
      api: $sourceDescriptions.api.url
"#;

#[test]
fn preparation_places_findings_in_the_text_of_the_external_document() {
    use roas_arazzo::source_map::SourceMap;

    let mut documents = standard();
    documents[2].1 = serde_saphyr::from_str(SHARED_YAML).unwrap();
    let graph = Graph::new(&documents);
    let path = "#.workflows[0].steps[0].successCriteria[0].condition";

    let options = graph
        .options("main")
        .document_source_map(SHARED, SourceMap::parse(SHARED_YAML).unwrap());
    let error = prepare(&graph.description, &options).unwrap_err();
    let found = error
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path == path)
        .unwrap_or_else(|| panic!("no finding at {path}: {error}"));
    assert_eq!(found.document.as_deref(), Some(SHARED));
    let span = found.span.expect("the field is located");
    assert_eq!((span.start.line, span.start.column), (23, 24));
    let offset = found.offset.expect("a syntax error has an offset");
    let position = found.position.expect("the offset is placed in the text");
    // The decoded condition and where its bytes are written differ by
    // the escapes before the offending one.
    let condition = "$response.body#/n\u{e9} == \"caf\u{e9}\" && (";
    let raw_line = SHARED_YAML.lines().nth(22).unwrap();
    assert_eq!(
        &condition[offset..],
        &raw_line[raw_line.len() - (condition.len() - offset) - 1..raw_line.len() - 1]
    );
    assert_eq!(position.line, 23);
    assert_eq!(
        position.column,
        raw_line.chars().count() - condition[offset..].chars().count()
    );
    assert!(
        found
            .to_string()
            .starts_with(&format!("{SHARED} {path} at line 23, column ")),
        "{found}"
    );

    // A map of another identity, or one of the entry, does not apply.
    for options in [
        graph.options("main").document_source_map(
            "https://example.test/elsewhere.arazzo.yaml",
            SourceMap::parse(SHARED_YAML).unwrap(),
        ),
        graph
            .options("main")
            .source_map(SourceMap::parse(SHARED_YAML).unwrap()),
    ] {
        let error = prepare(&graph.description, &options).unwrap_err();
        let found = error
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.path == path)
            .unwrap();
        assert_eq!((found.span, found.position), (None, None), "{found}");
    }
}

#[test]
fn an_unknown_member_of_an_external_document_fails_preparation() {
    let mut documents = standard();
    documents[2].1["workflows"][0]["steps"][0]["retryLimit"] = json!(3);
    let graph = Graph::new(&documents);
    let shared = graph.registry.document(graph.id(SHARED)).unwrap();
    assert_eq!(shared.unknown_members().version, "1.1.0");
    let error = prepare(&graph.description, &graph.options("main")).unwrap_err();
    let found = error
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path == "#.workflows[0].steps[0].retryLimit")
        .unwrap_or_else(|| panic!("{error}"));
    assert_eq!(found.document.as_deref(), Some(SHARED));
    assert_eq!(found.workflow_id.as_deref(), Some("child"));
    assert!(
        matches!(&found.issue, PreparationIssue::UnknownMember { name, version, .. }
            if name == "retryLimit" && version == "1.1.0"),
        "{found}"
    );

    // A v1.0 document is compared with the v1.0 model, before upconversion.
    let mut registry = SourceRegistry::new();
    let id = registry
        .insert(
            "https://example.test/old.arazzo.json",
            json!({
                "arazzo": "1.0.1", "info": { "title": "O", "version": "1" },
                "sourceDescriptions": [{ "name": "api", "url": "api.json" }],
                "workflows": [{ "workflowId": "w", "steps": [{
                    "stepId": "s", "operationId": "op", "dependsOn": ["x"] }] }]
            }),
        )
        .unwrap();
    let unknown = registry.document(id).unwrap().unknown_members();
    assert_eq!(unknown.version, "1.0.1");
    let paths: Vec<_> = unknown
        .members
        .iter()
        .map(|member| member.path.as_str())
        .collect();
    assert_eq!(paths, ["#.workflows[0].steps[0].dependsOn"]);
}

#[test]
fn an_unsupplied_or_mistyped_workflow_source_is_named_before_any_request() {
    let mut documents = standard();
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "onlyExternal", "steps": [{
            "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] }),
    );
    // Only the entry and its API are supplied.
    let mut registry = SourceRegistry::new();
    let root = registry.insert(ROOT, documents[0].1.clone()).unwrap();
    registry.insert(ROOT_API, documents[1].1.clone()).unwrap();
    registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    let description: Description = serde_json::from_value(documents[0].1.clone()).unwrap();
    let options = |workflow: &str| {
        Options::new()
            .workflow(workflow)
            .source_registry(&registry, root)
            .unwrap()
    };
    assert_eq!(
        required_sources(&description, &options("onlyExternal")).unwrap(),
        BTreeSet::from(["shared".to_owned()])
    );
    assert_eq!(
        required_sources(&description, &options("main")).unwrap(),
        BTreeSet::from(["api".to_owned(), "shared".to_owned()])
    );
    let error = prepare(&description, &options("onlyExternal")).unwrap_err();
    assert!(
        error.diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.issue,
            PreparationIssue::Execution(ExecutionError::ExternalWorkflow { reason, .. })
                if reason.contains("was not supplied")
        )),
        "{error}"
    );
    // Without a registry the same reference names what is missing.
    let error = execute(
        &description,
        &Options::new().workflow("onlyExternal"),
        &mut Fake::new(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("source `shared` was not supplied as an Arazzo document"),
        "{error}"
    );

    let mistyped = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "mistyped", "steps": [{
            "stepId": "call", "workflowId": "$sourceDescriptions.api.child" }] }),
    );
    let description: Description = serde_json::from_value(mistyped).unwrap();
    let error = prepare(&description, &options("mistyped")).unwrap_err();
    assert!(
        error.to_string().contains("not an Arazzo document"),
        "{error}"
    );
    assert!(required_sources(&description, &options("mistyped")).is_err());
}

#[test]
fn a_workflow_missing_from_a_linked_document_is_named_with_that_document() {
    let mut documents = standard();
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "typo", "steps": [{
            "stepId": "call", "workflowId": "$sourceDescriptions.shared.chlid" }] }),
    );
    let graph = Graph::new(&documents);
    let options = graph.options("typo");
    let expected = format!("document {SHARED} has no workflow `chlid`");
    let error = prepare(&graph.description, &options).unwrap_err();
    assert!(error.to_string().contains(&expected), "{error}");
    let error = execute(&graph.description, &options, &mut Fake::new()).unwrap_err();
    assert!(error.to_string().contains(&expected), "{error}");
}

#[test]
fn a_local_target_missing_inside_an_external_document_keeps_the_id_bare() {
    let mut documents = standard();
    documents[2].1["workflows"][0]["steps"][0]["onSuccess"] =
        json!([{ "name": "away", "type": "goto", "workflowId": "missing" }]);
    let graph = Graph::new(&documents);
    let error = execute(
        &graph.description,
        &graph.options("main"),
        &mut Fake::new()
            .reply(200, &ok("root"))
            .reply(200, &ok("local"))
            .reply(200, &ok("external")),
    )
    .unwrap_err();
    assert!(
        matches!(&error, ExecutionError::InDocument { document, error }
            if document == SHARED
                && matches!(&**error, ExecutionError::ExternalWorkflow { reference, reason }
                    if reference == "missing"
                        && reason == "this document has no workflow `missing`")),
        "{error}"
    );
    let error = prepare(&graph.description, &graph.options("main")).unwrap_err();
    let diagnostic = error
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path.ends_with("onSuccess[0].workflowId"))
        .expect("a located diagnostic");
    assert_eq!(diagnostic.document.as_deref(), Some(SHARED));
}

#[test]
fn explicit_sources_win_over_registry_links_for_workflow_calls_too() {
    const OTHER: &str = "https://example.test/other/shared.arazzo.json";
    let mut other = fixture("shared.arazzo.json");
    other["$self"] = json!(OTHER);
    other["workflows"][0]["steps"][0]["parameters"][1]["value"] = json!("other");
    let mut documents = standard();
    documents.push((OTHER, other));
    documents.push((
        "https://example.test/other/api.openapi.json",
        fixture("shared-api.openapi.json"),
    ));
    let graph = Graph::new(&documents);
    let other_id = graph.id(OTHER);

    // Adapt once, then retarget the alias in the registry: the explicit
    // entry from the first adaptation wins for metadata and for execution.
    let first = graph.options("main");
    let mut registry = graph.registry.clone();
    registry
        .override_source(graph.root, "shared", other_id)
        .unwrap();
    // Link the new target's own sources too.
    registry
        .load_sources(
            graph.root,
            &mut Loader::new(),
            &SourceLoadOptions::default(),
        )
        .unwrap();
    let options = first.source_registry(&registry, graph.root).unwrap();
    assert_eq!(
        options
            .source_document("shared")
            .unwrap()
            .identity()
            .as_str(),
        SHARED
    );
    let mut client = Fake::new()
        .reply(200, &ok("root"))
        .reply(200, &ok("local"))
        .reply(200, &ok("external"));
    let report = prepare(&graph.description, &options)
        .unwrap()
        .execute(&mut client)
        .unwrap();
    assert_eq!(
        urls(&client)[2],
        "https://shared.example.test/v1/things?id=root"
    );
    assert_eq!(report.steps[3].document.as_deref(), Some(SHARED));

    // A fresh adaptation follows the override.
    let options = Options::new()
        .workflow("main")
        .input("id", "seed")
        .source_registry(&registry, graph.root)
        .unwrap();
    assert_eq!(
        options
            .source_document("shared")
            .unwrap()
            .identity()
            .as_str(),
        OTHER
    );
    let mut client = Fake::new()
        .reply(200, &ok("root"))
        .reply(200, &ok("local"))
        .reply(200, &ok("external"));
    prepare(&graph.description, &options)
        .unwrap()
        .execute(&mut client)
        .unwrap();
    assert_eq!(
        urls(&client)[2],
        "https://shared.example.test/v1/things?id=other"
    );

    // An owned value cannot stand for an Arazzo document.
    let options = Options::new()
        .workflow("main")
        .input("id", "seed")
        .source("shared", SHARED, documents[2].1.clone())
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    let error = prepare(&graph.description, &options).unwrap_err();
    assert!(
        error.to_string().contains("given with `Options::source`"),
        "{error}"
    );
    // The same override after adaptation must not keep the old link.
    let options = graph
        .options("main")
        .source("shared", SHARED, documents[2].1.clone());
    let error = prepare(&graph.description, &options).unwrap_err();
    assert!(
        error.to_string().contains("given with `Options::source`"),
        "{error}"
    );
    let error = execute(
        &graph.description,
        &options,
        &mut Fake::new().reply(200, &ok("root")).reply(200, &ok("local")),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("given with `Options::source`"),
        "{error}"
    );
}

#[test]
fn a_linked_source_that_is_not_arazzo_says_so() {
    let mut documents = standard();
    // Untyped, so only the loaded document says what it is.
    documents[0].1["sourceDescriptions"][1]
        .as_object_mut()
        .unwrap()
        .remove("type");
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "wrong", "steps": [{
            "stepId": "call", "workflowId": "$sourceDescriptions.api.child" }] }),
    );
    let graph = Graph::new(&documents);
    let error = prepare(&graph.description, &graph.options("wrong")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("source `api` is not an Arazzo document"),
        "{error}"
    );
}

#[test]
fn a_called_external_workflows_own_dependencies_run_first_in_its_document() {
    let mut documents = standard();
    documents[2].1["workflows"][0]["dependsOn"] = json!(["other"]);
    documents[2].1 = with_workflow(
        documents[2].1.clone(),
        json!({ "workflowId": "other", "steps": [{
            "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
            "parameters": [{ "name": "id", "in": "query", "value": "$inputs.id" }] }] }),
    );
    let graph = Graph::new(&documents);
    let replies = || {
        Fake::new()
            .reply(200, &ok("root"))
            .reply(200, &ok("local"))
            .reply(200, &ok("other"))
            .reply(200, &ok("external"))
    };
    let options = graph.options("main");
    let mut client = replies();
    let lazy = execute(&graph.description, &options, &mut client).unwrap();
    // `other` is the shared document's, run with the run's own inputs
    // (`id=seed`) just before the external child that needs it.
    let ran: Vec<_> = lazy
        .steps
        .iter()
        .map(|step| (step.document.as_deref(), step.workflow_id.as_str()))
        .collect();
    assert_eq!(
        ran,
        [
            (None, "main"),
            (None, "child"),
            (None, "main"),
            (Some(SHARED), "other"),
            (Some(SHARED), "child"),
            (None, "main"),
        ]
    );
    assert!(
        client.sent()[2].url.ends_with("id=seed"),
        "{}",
        client.sent()[2].url
    );
    let plan = prepare(&graph.description, &options).unwrap();
    let checked = plan.execute(&mut replies()).unwrap();
    assert_eq!(trace(&checked), trace(&lazy));
}

#[test]
fn a_cycle_through_another_document_at_run_time_names_the_dependencys_document() {
    // `main` calls the shared `child`; `child` calls the entry's `helper`,
    // which depends on that running `child`: the cycle is the shared
    // document's.
    let mut documents = standard();
    let shared = &mut documents[2].1;
    shared["sourceDescriptions"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "root", "url": ROOT, "type": "arazzo" }));
    shared["workflows"][0]["steps"] = json!([{ "stepId": "back",
        "workflowId": "$sourceDescriptions.root.helper" }]);
    shared["workflows"][0]["outputs"] = json!({});
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "helper", "dependsOn": ["$sourceDescriptions.shared.child"],
            "steps": [{ "stepId": "get", "operationId": "$sourceDescriptions.api.getThing" }] }),
    );
    let graph = Graph::new(&documents);
    let options = graph.options("main");
    let replies = || Fake::new().reply(200, &ok("root")).reply(200, &ok("local"));
    let lazy = execute(&graph.description, &options, &mut replies()).unwrap_err();
    assert!(
        matches!(&lazy, ExecutionError::InDocument { document, error }
            if document == SHARED
                && matches!(error.as_ref(), ExecutionError::Circular(id) if id == "child")),
        "{lazy:?}"
    );
    let plan = prepare(&graph.description, &options).unwrap();
    let failure = plan.execute(&mut replies()).unwrap_err();
    assert_eq!(failure.error.to_string(), lazy.to_string());
}

#[test]
fn a_run_time_cycle_on_an_entry_dependency_stays_bare() {
    // `main` calls the shared `child`, which depends on the entry's running
    // `main`: the cycle is the entry document's, so it is not wrapped.
    let mut documents = standard();
    let shared = &mut documents[2].1;
    shared["sourceDescriptions"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "root", "url": ROOT, "type": "arazzo" }));
    shared["workflows"][0]["dependsOn"] = json!(["$sourceDescriptions.root.main"]);
    let graph = Graph::new(&documents);
    let options = graph.options("main");
    let replies = || Fake::new().reply(200, &ok("root")).reply(200, &ok("local"));
    let mut client = replies();
    let lazy = execute(&graph.description, &options, &mut client).unwrap_err();
    assert!(
        matches!(&lazy, ExecutionError::Circular(id) if id == "main"),
        "{lazy:?}"
    );
    assert_eq!(client.sent().len(), 2);
    let plan = prepare(&graph.description, &options).unwrap();
    let failure = plan.execute(&mut replies()).unwrap_err();
    assert_eq!(failure.error.to_string(), lazy.to_string());
}

#[cfg(feature = "input-validation")]
mod input_validation {
    use super::*;
    use roas_arazzo_executor::{InputError, InputValidation};

    fn graph() -> Graph {
        let mut documents = standard();
        let shared = &mut documents[2].1;
        shared["components"]["inputs"] = json!({ "lookup": {
            "type": "object", "required": ["id"],
            "properties": { "id": { "type": "string" } } } });
        // Resolved against the shared document, which alone declares it.
        shared["workflows"][0]["inputs"] = json!({ "$ref": "#/components/inputs/lookup" });
        Graph::new(&documents)
    }

    #[test]
    fn an_external_child_validates_bound_inputs_against_its_own_document() {
        let graph = graph();
        let options = graph
            .options("main")
            .input_validation(InputValidation::Draft202012);
        let plan = prepare(&graph.description, &options).unwrap();
        let mut client = Fake::new()
            .reply(200, &ok("root"))
            .reply(200, &ok("local"))
            .reply(200, &ok("external"));
        assert!(plan.execute(&mut client).unwrap().is_success());

        // A number where the child's schema wants a string: the child sends
        // nothing, and the caller's history is kept.
        let mut client = Fake::new()
            .reply(200, &json!({ "value": 7 }))
            .reply(200, &ok("local"));
        let failure = plan.execute(&mut client).unwrap_err();
        assert!(
            matches!(
                &failure.error,
                ExecutionError::Input(InputError::Invalid { workflow, document, .. })
                    if workflow == "child" && document.as_deref() == Some(SHARED)
            ),
            "{}",
            failure.error
        );
        assert!(
            failure
                .error
                .to_string()
                .starts_with(&format!("workflow `child` of {SHARED} has invalid inputs")),
            "{}",
            failure.error
        );
        assert_eq!(client.sent().len(), 2);
        let report = failure.report.unwrap();
        assert!(report.steps.iter().all(|step| step.document.is_none()));
        assert_eq!(report.steps.len(), 3);
    }

    #[test]
    fn an_external_input_schema_diagnostic_names_its_document_once() {
        let mut documents = standard();
        documents[2].1["workflows"][0]["inputs"] = json!({ "$ref": "#/components/inputs/missing" });
        let graph = Graph::new(&documents);
        let options = graph
            .options("main")
            .input_validation(InputValidation::Draft202012);
        let error = prepare(&graph.description, &options).unwrap_err();
        let diagnostic = error
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.path.ends_with(".inputs"))
            .expect("an input-schema diagnostic");
        assert_eq!(diagnostic.document.as_deref(), Some(SHARED));
        let line = diagnostic.to_string();
        assert_eq!(line.matches(SHARED).count(), 1, "{line}");
        assert!(
            line.contains("workflow `child` has an invalid input schema"),
            "{line}"
        );
    }
}

fn in_shared(error: &ExecutionError) -> &ExecutionError {
    match error {
        ExecutionError::InDocument { document, error } if document == SHARED => error,
        other => panic!("expected an error located in {SHARED}, got {other:?}"),
    }
}

#[test]
fn a_lazy_run_names_the_external_document_an_operation_error_comes_from() {
    let mut documents = standard();
    documents[2].1["workflows"][0]["steps"][0]["operationId"] =
        json!("$sourceDescriptions.api.missingOp");
    let graph = Graph::new(&documents);
    let error = execute(
        &graph.description,
        &graph.options("main"),
        &mut Fake::new().reply(200, &ok("root")).reply(200, &ok("local")),
    )
    .unwrap_err();
    assert!(
        matches!(in_shared(&error), ExecutionError::Operation(_)),
        "{error}"
    );
    assert!(
        error.to_string().starts_with(&format!("in {SHARED}: ")),
        "{error}"
    );
}

#[test]
fn runtime_errors_of_external_workflows_name_their_document_on_every_path() {
    let mut documents = standard();
    // Not a static error: the input is only found missing when the
    // workflow's outputs are named, after its frame is left.
    documents[2].1["workflows"][0]["outputs"]["late"] = json!("$inputs.missing");
    let graph = Graph::new(&documents);
    let options = graph.options("main");
    let client = || {
        Fake::new()
            .reply(200, &ok("root"))
            .reply(200, &ok("local"))
            .reply(200, &ok("external"))
    };
    let lazy = execute(&graph.description, &options, &mut client()).unwrap_err();
    let checked = prepare(&graph.description, &options)
        .unwrap()
        .execute(&mut client())
        .unwrap_err();
    assert!(!matches!(
        in_shared(&lazy),
        ExecutionError::InDocument { .. }
    ));
    assert_eq!(lazy.to_string(), checked.to_string());
    // The message carries the inner error once; it is not repeated as a source.
    assert!(std::error::Error::source(&lazy).is_none());
    assert_eq!(
        lazy.to_string(),
        format!("in {SHARED}: {}", in_shared(&lazy))
    );
}

#[test]
fn a_call_back_into_the_entry_reports_its_errors_unwrapped() {
    let mut documents = standard();
    let root = &mut documents[0].1;
    root["$self"] = json!(ROOT);
    root["workflows"].as_array_mut().unwrap().push(json!({
        "workflowId": "helper",
        "steps": [{
            "stepId": "get",
            "operationId": "$sourceDescriptions.api.getThing",
            "parameters": [{ "name": "id", "in": "query", "value": "helper" }]
        }],
        "outputs": { "late": "$steps.nope.outputs.value" }
    }));
    let shared = &mut documents[2].1;
    shared["sourceDescriptions"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "root", "url": "../root.arazzo.json", "type": "arazzo" }));
    shared["workflows"][0]["steps"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({ "stepId": "back", "workflowId": "$sourceDescriptions.root.helper" }),
        );
    let graph = Graph::new(&documents);
    let error = execute(
        &graph.description,
        &graph.options("main"),
        &mut Fake::new()
            .reply(200, &ok("root"))
            .reply(200, &ok("local"))
            .reply(200, &ok("helper")),
    )
    .unwrap_err();
    assert!(
        !matches!(error, ExecutionError::InDocument { .. }) && error.to_string().contains("nope"),
        "the entry's own workflow failed, unwrapped: {error}"
    );
}

/// Entry `A` and external `B`, each with its own `ping` API.
fn pair(a_workflows: Value, b_workflows: Value) -> Graph {
    Graph::new(&[
        (
            A,
            arazzo(
                "1.1.0",
                json!([{ "name": "b", "url": "b.arazzo.json", "type": "arazzo" },
                       { "name": "api", "url": "a.openapi.json", "type": "openapi" }]),
                a_workflows,
            ),
        ),
        (
            B,
            arazzo(
                "1.1.0",
                json!([{ "name": "a", "url": "a.arazzo.json", "type": "arazzo" },
                       { "name": "api", "url": "b.openapi.json", "type": "openapi" }]),
                b_workflows,
            ),
        ),
        (
            "https://example.test/cycle/a.openapi.json",
            api("https://a.example.test"),
        ),
        (
            "https://example.test/cycle/b.openapi.json",
            api("https://b.example.test"),
        ),
    ])
}

fn tangled(workflow_id: &str) -> Value {
    json!({ "workflowId": workflow_id, "steps": [
        { "stepId": "s1", "operationId": "$sourceDescriptions.api.ping", "dependsOn": ["s2"] },
        { "stepId": "s2", "operationId": "$sourceDescriptions.api.ping", "dependsOn": ["s1"] }] })
}

fn lazily(graph: &Graph, workflow: &str, replies: usize) -> ExecutionError {
    let mut client = Fake::new();
    for _ in 0..replies {
        client = client.reply(200, &json!({}));
    }
    let options = Options::new()
        .workflow(workflow)
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    execute(&graph.description, &options, &mut client).unwrap_err()
}

fn located_in<'e>(error: &'e ExecutionError, document: &str) -> &'e ExecutionError {
    match error {
        ExecutionError::InDocument {
            document: found,
            error,
        } if found == document => error,
        other => panic!("expected an error located in {document}, got {other:?}"),
    }
}

#[test]
fn a_callees_step_order_error_names_the_callee_document() {
    let graph = pair(
        json!([{ "workflowId": "main", "steps": [
            { "stepId": "across", "workflowId": "$sourceDescriptions.b.w" }] }]),
        json!([tangled("w")]),
    );
    let error = lazily(&graph, "main", 0);
    assert!(
        matches!(located_in(&error, B), ExecutionError::Circular(_)),
        "{error}"
    );
}

#[test]
fn a_call_back_into_the_entry_keeps_its_step_order_error_unwrapped() {
    let graph = pair(
        json!([
            { "workflowId": "main", "steps": [
                { "stepId": "across", "workflowId": "$sourceDescriptions.b.w" }] },
            tangled("helper")
        ]),
        json!([{ "workflowId": "w", "steps": [
            { "stepId": "back", "workflowId": "$sourceDescriptions.a.helper" }] }]),
    );
    let error = lazily(&graph, "main", 0);
    assert!(matches!(error, ExecutionError::Circular(_)), "{error}");
}

#[test]
fn a_workflow_steps_completion_error_names_the_calling_document() {
    let graph = pair(
        json!([{ "workflowId": "main", "steps": [
            { "stepId": "across", "workflowId": "$sourceDescriptions.b.w" }] }]),
        json!([
            { "workflowId": "w", "steps": [
                { "stepId": "c", "workflowId": "inner",
                  "outputs": { "x": "$inputs.missing" } }] },
            { "workflowId": "inner", "steps": [
                { "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }
        ]),
    );
    let lazy = lazily(&graph, "main", 1);
    assert!(
        !matches!(located_in(&lazy, B), ExecutionError::InDocument { .. }),
        "{lazy}"
    );
    let options = Options::new()
        .workflow("main")
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    let checked = prepare(&graph.description, &options)
        .unwrap()
        .execute(&mut Fake::new().reply(200, &json!({})))
        .unwrap_err();
    assert_eq!(lazy.to_string(), checked.to_string());
}

#[test]
fn a_queued_external_dependencys_step_order_error_names_its_document() {
    let graph = pair(
        json!([{ "workflowId": "main",
            "dependsOn": ["$sourceDescriptions.b.w1", "$sourceDescriptions.b.w2"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }]),
        json!([
            { "workflowId": "w1", "steps": [
                { "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] },
            tangled("w2")
        ]),
    );
    let error = lazily(&graph, "main", 1);
    assert!(
        matches!(located_in(&error, B), ExecutionError::Circular(_)),
        "{error}"
    );
}

#[test]
fn a_dependency_cycle_inside_another_document_names_it_once() {
    let graph = pair(
        json!([{ "workflowId": "main", "dependsOn": ["$sourceDescriptions.b.w"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }]),
        json!([
            { "workflowId": "w", "dependsOn": ["v"],
              "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] },
            { "workflowId": "v", "dependsOn": ["w"],
              "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }
        ]),
    );
    let error = lazily(&graph, "main", 0);
    assert!(
        matches!(located_in(&error, B), ExecutionError::Circular(at) if at == "w"),
        "{error}"
    );
    let options = Options::new()
        .workflow("main")
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    let error = prepare(&graph.description, &options).unwrap_err();
    let cycles: Vec<_> = error
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.issue,
                PreparationIssue::Execution(ExecutionError::Circular(_))
            )
        })
        .collect();
    assert!(!cycles.is_empty(), "{error}");
    for diagnostic in cycles {
        assert_eq!(diagnostic.document.as_deref(), Some(B));
        let line = diagnostic.to_string();
        assert_eq!(line.matches(B).count(), 1, "{line}");
    }
}

#[test]
fn an_unresolvable_dependency_of_an_external_workflow_names_its_document() {
    let graph = pair(
        json!([{ "workflowId": "main", "dependsOn": ["$sourceDescriptions.b.w"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }]),
        json!([{ "workflowId": "w", "dependsOn": ["missing"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }]),
    );
    let error = lazily(&graph, "main", 0);
    assert!(
        matches!(located_in(&error, B), ExecutionError::ExternalWorkflow { reference, reason }
            if reference == "missing" && reason == "this document has no workflow `missing`"),
        "{error}"
    );
    let options = Options::new()
        .workflow("main")
        .source_registry(&graph.registry, graph.root)
        .unwrap();
    let error = prepare(&graph.description, &options).unwrap_err();
    assert!(
        error
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.document.as_deref() == Some(B)),
        "only the external document is at fault: {error}"
    );
    assert!(
        error
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path.ends_with("dependsOn[0]")),
        "{error}"
    );
}

#[test]
fn an_external_operation_path_by_canonical_url_waits_for_its_sources() {
    const CANONICAL: &str = "https://canonical.example.test/shared-api.json";
    let mut documents = standard();
    let step = &mut documents[2].1["workflows"][0]["steps"][0];
    step.as_object_mut().unwrap().remove("operationId");
    step["operationPath"] = json!(format!("{CANONICAL}#/paths/~1things/get"));
    let api = &mut documents[3].1;
    api["openapi"] = json!("3.2.0");
    api["$self"] = json!(CANONICAL);

    // Before the external document's API is loaded, the URL names nothing yet.
    let mut registry = SourceRegistry::new();
    let root = registry.insert(ROOT, documents[0].1.clone()).unwrap();
    registry.insert(SHARED, documents[2].1.clone()).unwrap();
    registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    let description = registry.document(root).unwrap().arazzo().unwrap().clone();
    let options = Options::new()
        .workflow("main")
        .source_registry(&registry, root)
        .unwrap();
    let required = required_sources(&description, &options).unwrap();
    assert_eq!(required, BTreeSet::from(["api".into(), "shared".into()]));

    // Once it is, preparation resolves it by the API's `$self`.
    let graph = Graph::new(&documents);
    prepare(&graph.description, &graph.options("main")).unwrap();
}

#[test]
fn an_entry_dependency_error_reached_from_another_document_stays_with_the_entry() {
    for x in [
        json!({ "workflowId": "x", "dependsOn": ["missing"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }),
        json!({ "workflowId": "x", "dependsOn": ["y"],
            "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }),
    ] {
        let graph = pair(
            json!([
                { "workflowId": "main", "dependsOn": ["$sourceDescriptions.b.w"],
                  "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] },
                x,
                { "workflowId": "y", "dependsOn": ["x"],
                  "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }
            ]),
            json!([{ "workflowId": "w", "dependsOn": ["$sourceDescriptions.a.x"],
                "steps": [{ "stepId": "ping", "operationId": "$sourceDescriptions.api.ping" }] }]),
        );
        let lazy = lazily(&graph, "main", 0);
        assert!(!matches!(lazy, ExecutionError::InDocument { .. }), "{lazy}");
        let options = Options::new()
            .workflow("main")
            .source_registry(&graph.registry, graph.root)
            .unwrap();
        let error = prepare(&graph.description, &options).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.document.is_none()),
            "the external document is valid: {error}"
        );
        assert!(!error.diagnostics.is_empty());
    }
}

/// A workflow whose one request sends `value`.
fn reading(workflow: &str, depends_on: Value, value: &str) -> Value {
    json!({ "workflowId": workflow,
        "dependsOn": depends_on,
        "steps": [{ "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
            "parameters": [{ "name": "id", "in": "query", "value": value }] }] })
}

#[test]
fn a_workflow_reads_what_an_external_dependency_produced() {
    let mut documents = standard();
    let mut dependent = reading(
        "dependent",
        json!(["$sourceDescriptions.shared.child"]),
        "$sourceDescriptions.shared.child.outputs.value#/a",
    );
    dependent["outputs"] = json!({
        "whole": "$sourceDescriptions.shared.child.outputs.value",
        "input": "$sourceDescriptions.shared.child.inputs.id",
        "all": "$sourceDescriptions.shared.child.outputs",
        "text": "was {$sourceDescriptions.shared.child.outputs.value#/a}",
    });
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    let options = graph.options("dependent");
    let plan = prepare(&graph.description, &options).unwrap();
    let replies = || {
        Fake::new()
            .reply(200, &json!({ "value": { "a": "deep" } }))
            .reply(200, &ok("root"))
    };
    let mut lazy = replies();
    let mut checked = replies();
    for (report, client) in [
        (plan.execute(&mut checked).unwrap(), &checked),
        (
            execute(&graph.description, &options, &mut lazy).unwrap(),
            &lazy,
        ),
    ] {
        assert!(report.is_success(), "{report}");
        assert_eq!(report.outputs["whole"], json!({ "a": "deep" }));
        assert_eq!(report.outputs["input"], "seed");
        assert_eq!(report.outputs["all"]["value"], json!({ "a": "deep" }));
        assert_eq!(report.outputs["text"], "was deep");
        assert!(
            client.sent()[1].url.ends_with("?id=deep"),
            "{:?}",
            client.sent()[1].url
        );
    }
}

#[test]
fn the_same_workflow_id_in_each_document_is_read_apart() {
    let mut documents = standard();
    let mut dependent = reading(
        "dependent",
        json!(["child", "$sourceDescriptions.shared.child"]),
        "$workflows.child.outputs.value",
    );
    dependent["outputs"] = json!({
        "local": "$workflows.child.outputs.value",
        "external": "$sourceDescriptions.shared.child.outputs.value",
    });
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    let report = execute(
        &graph.description,
        &graph.options("dependent"),
        &mut Fake::new()
            .reply(200, &ok("local"))
            .reply(200, &ok("external"))
            .reply(200, &ok("root")),
    )
    .unwrap();
    assert_eq!(report.outputs["local"], "local");
    assert_eq!(report.outputs["external"], "external");
}

#[test]
fn a_read_of_an_external_workflow_that_did_not_run_says_so() {
    let mut documents = standard();
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        reading(
            "dependent",
            json!([]),
            "$sourceDescriptions.shared.child.outputs.value",
        ),
    );
    let graph = Graph::new(&documents);
    let options = graph.options("dependent");
    prepare(&graph.description, &options).unwrap();
    let error = execute(&graph.description, &options, &mut Fake::new()).unwrap_err();
    assert!(error.to_string().contains("has not run"), "got: {error}");
}

#[test]
fn a_read_of_a_workflow_the_source_has_not_got_is_refused() {
    let mut documents = standard();
    let reference = "$sourceDescriptions.shared.nope.outputs.value";
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        reading("dependent", json!([]), reference),
    );
    let graph = Graph::new(&documents);
    let options = graph.options("dependent");
    let error = prepare(&graph.description, &options).unwrap_err();
    assert!(error.to_string().contains("nope"), "got: {error}");
    let error = execute(&graph.description, &options, &mut Fake::new()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("workflow `nope` of source `shared`"),
        "got: {error}"
    );
}

#[test]
fn a_read_names_its_source_as_required() {
    let description: Description = serde_json::from_value(with_workflow(
        fixture("root.arazzo.json"),
        reading(
            "dependent",
            json!([]),
            "$sourceDescriptions.shared.child.outputs.value",
        ),
    ))
    .unwrap();
    let options = Options::new().workflow("dependent");
    let required = required_sources(&description, &options).unwrap();
    assert!(required.contains("shared"), "{required:?}");
}

#[test]
fn an_api_source_has_no_workflows_to_read() {
    let mut documents = standard();
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        reading(
            "dependent",
            json!([]),
            "$sourceDescriptions.api.w.outputs.x",
        ),
    );
    let graph = Graph::new(&documents);
    let error = execute(
        &graph.description,
        &graph.options("dependent"),
        &mut Fake::new(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("no supplied document"),
        "got: {error}"
    );
}

#[test]
fn a_dependency_that_reads_an_external_one_waits_for_it_under_parallel() {
    let together = |value: &str| {
        let mut documents = standard();
        for workflow in [
            reading("reader", json!([]), value),
            reading(
                "target",
                json!(["$sourceDescriptions.shared.child", "reader"]),
                "plain",
            ),
        ] {
            documents[0].1 = with_workflow(documents[0].1.clone(), workflow);
        }
        let graph = Graph::new(&documents);
        let options = graph.options("target").parallel(4);
        let mut run = Run::start(&graph.description, &options).unwrap();
        assert!(matches!(run.advance(), Ok(Progress::Send(_))));
        matches!(run.advance(), Ok(Progress::Send(_)))
    };
    assert!(together("plain"));
    assert!(!together("$sourceDescriptions.shared.child.outputs.value"));
}

#[test]
fn a_condition_reads_an_external_workflow_with_navigation() {
    let mut documents = standard();
    let mut dependent = reading(
        "dependent",
        json!(["$sourceDescriptions.shared.child"]),
        "plain",
    );
    dependent["steps"][0]["successCriteria"] =
        json!([{ "condition": "$sourceDescriptions.shared.child.outputs.value[0] == 7" }]);
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    let replies = |first| {
        Fake::new()
            .reply(200, &json!({ "value": [first] }))
            .reply(200, &ok("root"))
    };
    let options = graph.options("dependent");
    prepare(&graph.description, &options).unwrap();
    let passed = execute(&graph.description, &options, &mut replies(7)).unwrap();
    assert!(passed.is_success(), "{passed}");
    let failed = execute(&graph.description, &options, &mut replies(8)).unwrap();
    assert!(!failed.is_success(), "{failed}");
}

#[test]
fn an_empty_pointer_reads_the_whole_field() {
    let mut documents = standard();
    let mut dependent = reading(
        "dependent",
        json!(["$sourceDescriptions.shared.child"]),
        "plain",
    );
    dependent["outputs"] = json!({ "all": "$sourceDescriptions.shared.child.outputs#" });
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    let report = execute(
        &graph.description,
        &graph.options("dependent"),
        &mut Fake::new().reply(200, &ok("v")).reply(200, &ok("root")),
    )
    .unwrap();
    assert_eq!(report.outputs["all"]["value"], "v");
}

#[test]
fn a_workflow_that_is_only_read_is_not_compiled() {
    let mut documents = standard();
    documents[2].1 = with_workflow(
        documents[2].1.clone(),
        json!({ "workflowId": "broken", "steps": [{ "stepId": "get",
            "operationId": "$sourceDescriptions.api.getThing",
            "parameters": [{ "name": "id", "in": "query", "value": "$nonsense.x" }] }] }),
    );
    let mut dependent = reading("dependent", json!([]), "plain");
    dependent["steps"][0]["successCriteria"] = json!([{
        "condition": "$statusCode == 200 || $sourceDescriptions.shared.broken.outputs.x == 1"
    }]);
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    let options = graph.options("dependent");
    let plan = prepare(&graph.description, &options).unwrap();
    let report = plan
        .execute(&mut Fake::new().reply(200, &ok("root")))
        .unwrap();
    assert!(report.is_success(), "{report}");
}

#[test]
fn an_untyped_api_source_is_no_preparation_error() {
    let mut documents = standard();
    let sources = documents[0].1["sourceDescriptions"].as_array_mut().unwrap();
    for source in sources.iter_mut() {
        if source["name"] == "api" {
            source.as_object_mut().unwrap().remove("type");
        }
    }
    let mut dependent = reading("dependent", json!([]), "plain");
    dependent["steps"][0]["successCriteria"] = json!([{
        "condition": "$statusCode == 200 || $sourceDescriptions.api.w.outputs.x == 1"
    }]);
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    prepare(&graph.description, &graph.options("dependent")).unwrap();
}

#[test]
fn a_bad_read_is_located_by_its_offset() {
    let mut documents = standard();
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        reading(
            "dependent",
            json!([]),
            "aaaa {$sourceDescriptions.shared.nope.outputs.x}",
        ),
    );
    let graph = Graph::new(&documents);
    let error = prepare(&graph.description, &graph.options("dependent")).unwrap_err();
    assert_eq!(error.diagnostics[0].offset, Some(6), "{error}");
}

#[test]
fn a_read_names_the_document_it_is_written_in() {
    // `shared` reads the entry's `child` through its link back; the
    // entry has its own, same-named one to tell apart from `shared`'s.
    let mut documents = standard();
    documents[2].1["sourceDescriptions"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "back", "url": ROOT, "type": "arazzo" }));
    let mut reader = reading("reader", json!(["$sourceDescriptions.back.child"]), "plain");
    reader["outputs"] = json!({ "value": "$sourceDescriptions.back.child.outputs.value" });
    documents[2].1 = with_workflow(documents[2].1.clone(), reader);
    documents[0].1 = with_workflow(
        documents[0].1.clone(),
        json!({ "workflowId": "entry",
            "steps": [{ "stepId": "call", "workflowId": "$sourceDescriptions.shared.reader" }],
            "outputs": { "value": "$steps.call.outputs.value" } }),
    );
    let graph = Graph::new(&documents);
    let report = execute(
        &graph.description,
        &graph.options("entry"),
        &mut Fake::new()
            .reply(200, &ok("from the entry"))
            .reply(200, &ok("reader")),
    )
    .unwrap();
    assert_eq!(report.outputs["value"], "from the entry");
}

#[test]
fn a_step_depending_on_a_step_of_an_external_workflow_runs_it_first() {
    let mut documents = standard();
    let mut dependent = reading("dependent", json!([]), "plain");
    dependent["steps"][0]["dependsOn"] = json!(["$sourceDescriptions.shared.child.steps.get"]);
    documents[0].1 = with_workflow(documents[0].1.clone(), dependent);
    let graph = Graph::new(&documents);
    let options = graph.options("dependent");
    assert!(
        required_sources(&graph.description, &Options::new().workflow("dependent"))
            .unwrap()
            .contains("shared")
    );
    let plan = prepare(&graph.description, &options).unwrap();
    for report in [
        plan.execute(&mut Fake::new().reply(200, &ok("child")).reply(200, &ok("root")))
            .unwrap(),
        execute(
            &graph.description,
            &options,
            &mut Fake::new().reply(200, &ok("child")).reply(200, &ok("root")),
        )
        .unwrap(),
    ] {
        assert!(report.is_success(), "{report}");
        let order: Vec<_> = report
            .steps
            .iter()
            .map(|step| (step.document.is_some(), step.workflow_id.as_str()))
            .collect();
        assert_eq!(order, [(true, "child"), (false, "dependent")]);
    }
}
