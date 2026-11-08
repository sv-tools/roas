//! Stage 7: workflows of external Arazzo documents run in their own document
//! scope, on the same engine as local calls.
#![cfg(feature = "source-graph")]

use roas::loader::{Loader, LoaderError, ResourceFetcher};
use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    DocumentId, ExecutionError, ExecutionReport, Options, Outcome, Performed, PreparationIssue,
    SourceLoadOptions, SourceRegistry, SourceVersion, execute, prepare, required_sources,
    testing::Fake,
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

fn urls(client: &Fake) -> Vec<&str> {
    client
        .sent()
        .iter()
        .map(|request| request.url.as_str())
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
        matches!(&error, ExecutionError::Circular(at) if at == "w" || at.ends_with(B)),
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
    // Without a registry the same reference explains how to supply it.
    let error = execute(
        &description,
        &Options::new().workflow("onlyExternal"),
        &mut Fake::new(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("Options::source_registry"),
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
fn a_called_external_workflow_with_its_own_dependencies_is_rejected_by_preparation() {
    let mut documents = standard();
    documents[2].1["workflows"][0]["dependsOn"] = json!(["other"]);
    documents[2].1 = with_workflow(
        documents[2].1.clone(),
        json!({ "workflowId": "other", "steps": [{
            "stepId": "get", "operationId": "$sourceDescriptions.api.getThing" }] }),
    );
    let graph = Graph::new(&documents);
    let error = prepare(&graph.description, &graph.options("main")).unwrap_err();
    assert!(error.to_string().contains("declares dependsOn"), "{error}");
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
                ExecutionError::Input(InputError::Invalid { workflow, .. }) if workflow == "child"
            ),
            "{}",
            failure.error
        );
        assert_eq!(client.sent().len(), 2);
        let report = failure.report.unwrap();
        assert!(report.steps.iter().all(|step| step.document.is_none()));
        assert_eq!(report.steps.len(), 3);
    }
}
