#![cfg(feature = "source-graph")]

use roas::{
    AsyncResourceFetcher, DocumentFetchFuture, FetchFuture, LoadedDocument, Loader, LoaderError,
    ResourceFetcher,
};
use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    Options, SourceLoadOptions, SourceRegistry, execute, execute_async, prepare, required_sources,
    testing::Fake,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use url::Url;

fn workflow() -> Value {
    json!({"arazzo":"1.1.0", "info":{"title":"References", "version":"1"},
        "sourceDescriptions":[{"name":"api","url":"https://api.test/openapi.json","type":"openapi"}],
        "workflows":[{"workflowId":"w","steps":[{"stepId":"s","operationId":"$sourceDescriptions.api.check"}]}]})
}

#[derive(Clone, Default)]
struct Memory {
    values: BTreeMap<String, (String, Value)>,
    reads: Rc<RefCell<Vec<String>>>,
}
impl ResourceFetcher for Memory {
    fn fetch(&mut self, _: &Url) -> Result<Value, LoaderError> {
        panic!("metadata API expected")
    }
    fn fetch_document(&mut self, uri: &Url) -> Result<LoadedDocument, LoaderError> {
        self.reads.borrow_mut().push(uri.to_string());
        self.values
            .get(uri.as_str())
            .map(|(retrieval, value)| {
                LoadedDocument::new(value.clone(), Url::parse(retrieval).unwrap())
            })
            .ok_or_else(|| LoaderError::NoFetcherRegistered {
                uri: uri.to_string(),
            })
    }
}
impl AsyncResourceFetcher for Memory {
    fn fetch<'a>(&'a mut self, _: &'a Url) -> FetchFuture<'a> {
        panic!("metadata API expected")
    }
    fn fetch_document<'a>(&'a mut self, uri: &'a Url) -> DocumentFetchFuture<'a> {
        Box::pin(async move { ResourceFetcher::fetch_document(self, uri) })
    }
}

#[tokio::test]
async fn external_fragments_keep_mounts_and_server_origins_across_redirects() {
    for asynchronous in [false, true] {
        for level in ["operation", "path", "root"] {
            let api: Value =
                serde_yaml_ng::from_str(include_str!("data/operation-resolution/openapi.yaml"))
                    .unwrap();
            let mut parts: Value =
                serde_yaml_ng::from_str(include_str!("data/operation-resolution/parts.yaml"))
                    .unwrap();
            let mut next: Value =
                serde_json::from_str(include_str!("data/operation-resolution/next.json")).unwrap();
            if level != "operation" {
                next["pets"]["get"]
                    .as_object_mut()
                    .unwrap()
                    .remove("servers");
            }
            if level == "root" {
                parts["pets"].as_object_mut().unwrap().remove("servers");
            }
            let memory = Memory {
                values: BTreeMap::from([
                    (
                        "https://api.test/openapi.json".into(),
                        ("https://api.test/openapi.json".into(), api),
                    ),
                    (
                        "https://identity.test/parts.json".into(),
                        ("https://cdn.test/dir/parts.json".into(), parts),
                    ),
                    (
                        "https://cdn.test/dir/next.json".into(),
                        ("https://target.test/v/next.json".into(), next),
                    ),
                ]),
                ..Memory::default()
            };
            let mut loader = Loader::new();
            loader.register_fetcher("https://", memory.clone());
            loader.register_async_fetcher("https://", memory.clone());
            let mut registry = SourceRegistry::new();
            let root = registry
                .insert("https://workflows.test/root.json", workflow())
                .unwrap();
            let report = if asynchronous {
                registry
                    .load_sources_async(root, &mut loader, &SourceLoadOptions::default())
                    .await
                    .unwrap()
            } else {
                registry
                    .load_sources(root, &mut loader, &SourceLoadOptions::default())
                    .unwrap()
            };
            assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
            assert!(
                report.reference_diagnostics.is_empty(),
                "{:?}",
                report.reference_diagnostics
            );
            assert_eq!(report.fetch_attempts, 3);
            assert_eq!(registry.len(), 4);
            assert_eq!(memory.reads.borrow().len(), 3);
            let description: Description = serde_json::from_value(workflow()).unwrap();
            let options = Options::new().source_registry(&registry, root).unwrap();
            drop(registry);
            drop(loader);
            let mut lazy = Fake::new().reply(200, &json!({}));
            let mut checked = lazy.clone();
            let plan = prepare(&description, &options).unwrap();
            let (mut lazy_report, mut checked_report) = if asynchronous {
                (
                    execute_async(&description, &options, &mut lazy)
                        .await
                        .unwrap(),
                    plan.execute_async(&mut checked).await.unwrap(),
                )
            } else {
                (
                    execute(&description, &options, &mut lazy).unwrap(),
                    plan.execute(&mut checked).unwrap(),
                )
            };
            for step in lazy_report
                .steps
                .iter_mut()
                .chain(&mut checked_report.steps)
            {
                step.elapsed = std::time::Duration::ZERO;
            }
            assert_eq!(lazy_report, checked_report);
            assert_eq!(lazy.sent(), checked.sent());
            let base = match level {
                "operation" => "https://target.test/v/operation",
                "path" => "https://cdn.test/path",
                _ => "https://api.test/root",
            };
            assert_eq!(checked.sent()[0].url, format!("{base}/pets"));
            assert_eq!(memory.reads.borrow().len(), 3); // preparation/execution never fetch
        }
    }
}

#[test]
fn supplied_reference_documents_and_canonical_operation_paths_need_no_fetcher() {
    let mut registry = SourceRegistry::new();
    let mut root_value = workflow();
    root_value["$self"] = json!("https://workflows.test/ids/root.json");
    root_value["sourceDescriptions"][0]["url"] = json!("../../api.json");
    root_value["workflows"][0]["steps"][0]
        .as_object_mut()
        .unwrap()
        .remove("operationId");
    root_value["workflows"][0]["steps"][0]["operationPath"] =
        json!("https://WORKFLOWS.test:443/./api.json#/paths/~1pets/get");
    let root = registry
        .insert("file:///root.json", root_value.clone())
        .unwrap();
    let description: Description = serde_json::from_value(root_value).unwrap();
    assert_eq!(
        required_sources(
            &description,
            &Options::new().source_registry(&registry, root).unwrap()
        )
        .unwrap(),
        BTreeSet::from(["api".into()])
    );
    registry
        .insert(
            "https://workflows.test/api.json",
            json!({"openapi":"3.1.0", "paths":{"/pets":{"$ref":"parts.json#/item"}}}),
        )
        .unwrap();
    let part = json!({"item":{"get":{"operationId":"check"}}});
    registry
        .insert_reference_document("https://workflows.test/parts.json", part.clone())
        .unwrap();
    registry
        .insert_reference_document("https://WORKFLOWS.test:443/./parts.json", part)
        .unwrap();
    assert_eq!(registry.len(), 3);
    assert!(matches!(
        registry.add_retrieval_alias(root, "https://workflows.test/parts.json"),
        Err(roas_arazzo_executor::SourceError::Conflict(_))
    ));
    let report = registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(report.fetch_attempts, 0);
    assert!(report.reference_diagnostics.is_empty());
    let options = Options::new().source_registry(&registry, root).unwrap();
    let mut fake = Fake::new().reply(200, &json!({}));
    prepare(&description, &options)
        .unwrap()
        .execute(&mut fake)
        .unwrap();
    assert_eq!(fake.sent()[0].url, "https://workflows.test/pets");
    assert!(
        registry
            .insert_reference_document("https://workflows.test/parts.json", json!({}))
            .is_err()
    );
    assert!(
        registry
            .insert_reference_document("not a URL", json!({}))
            .is_err()
    );
}

#[tokio::test]
async fn reference_discovery_repairs_a_pending_source_identity_without_refetching() {
    for asynchronous in [false, true] {
        let mut value = workflow();
        value["sourceDescriptions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "name":"discovered", "url":"https://identity.test/discovered.json", "type":"openapi"
            }));
        let memory = Memory {
            values: BTreeMap::from([
                (
                    "https://api.test/openapi.json".into(),
                    (
                        "https://api.test/openapi.json".into(),
                        json!({"openapi":"3.2.0","paths":{"/pets":{
                            "$ref":"https://cdn.test/discovered.json#/components/pathItems/pet"
                        }}}),
                    ),
                ),
                (
                    "https://cdn.test/discovered.json".into(),
                    (
                        "https://cdn.test/discovered.json".into(),
                        json!({"openapi":"3.2.0","$self":"https://identity.test/discovered.json",
                        "components":{"pathItems":{"pet":{"get":{"operationId":"check"}}}}}),
                    ),
                ),
            ]),
            ..Memory::default()
        };
        let mut registry = SourceRegistry::new();
        let root = registry
            .insert("https://workflows.test/root.json", value.clone())
            .unwrap();
        let mut loader = Loader::new();
        loader.register_fetcher("https://", memory.clone());
        loader.register_async_fetcher("https://", memory.clone());
        let mut limits = SourceLoadOptions::default();
        limits.max_documents = 4; // root + two successful loads + one failed identity lookup
        let report = if asynchronous {
            registry
                .load_sources_async(root, &mut loader, &limits)
                .await
                .unwrap()
        } else {
            registry.load_sources(root, &mut loader, &limits).unwrap()
        };
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert!(
            report.reference_diagnostics.is_empty(),
            "{:?}",
            report.reference_diagnostics
        );
        assert_eq!(report.fetch_attempts, 3);
        assert_eq!(memory.reads.borrow().len(), 3);
        assert!(
            registry
                .source(root, "discovered")
                .unwrap()
                .target
                .is_some()
        );
        let description = serde_json::from_value(value).unwrap();
        let options = Options::new().source_registry(&registry, root).unwrap();
        let mut fake = Fake::new().reply(200, &json!({}));
        prepare(&description, &options)
            .unwrap()
            .execute(&mut fake)
            .unwrap();
        assert_eq!(fake.sent()[0].url, "https://api.test/pets");
        assert_eq!(memory.reads.borrow().len(), 3);
    }
}

#[test]
fn reference_load_failures_and_budgets_remain_located_and_prevent_required_execution() {
    for budget in ["fetch", "document", "depth"] {
        let mut registry = SourceRegistry::new();
        let root = registry
            .insert("https://workflows.test/root.json", workflow())
            .unwrap();
        registry
            .insert(
                "https://api.test/openapi.json",
                json!({"openapi":"3.1.0", "paths":{"/pets":{"$ref":"missing.json#/item"}}}),
            )
            .unwrap();
        let mut load = SourceLoadOptions::default();
        if budget == "document" {
            load.max_documents = 2;
        }
        if budget == "depth" {
            load.max_depth = 1;
        }
        let report = registry
            .load_sources(root, &mut Loader::new(), &load)
            .unwrap();
        assert!(report.diagnostics.is_empty());
        assert_eq!(report.reference_diagnostics.len(), 1);
        let diagnostic = &report.reference_diagnostics[0];
        assert_eq!(diagnostic.document, "https://api.test/openapi.json");
        assert_eq!(diagnostic.pointer, "/paths/~1pets/$ref");
        assert_eq!(diagnostic.reference, "missing.json#/item");
        assert!(diagnostic.to_string().contains(if budget == "fetch" {
            "no fetcher"
        } else {
            "limit"
        }));
        assert_eq!(report.fetch_attempts, usize::from(budget == "fetch"));
        let options = Options::new().source_registry(&registry, root).unwrap();
        let description: Description = serde_json::from_value(workflow()).unwrap();
        assert!(prepare(&description, &options).is_err());
        let mut fake = Fake::new();
        assert!(execute(&description, &options, &mut fake).is_err());
        assert!(fake.sent().is_empty());
    }
}

#[test]
fn local_and_external_cycles_terminate_and_bad_targets_stay_diagnostics_after_retry() {
    for target in [
        json!({"$ref":"https://api.test/openapi.json#/paths/~1pets"}),
        json!(false),
        json!({"$ref":42}),
        json!({"$ref":"#/%GG"}),
    ] {
        let mut registry = SourceRegistry::new();
        let root = registry
            .insert("https://workflows.test/root.json", workflow())
            .unwrap();
        registry
            .insert(
                "https://api.test/openapi.json",
                json!({"openapi":"3.1.0", "paths":{"/pets":{"$ref":"parts.json#/item"}}}),
            )
            .unwrap();
        let memory = Memory {
            values: BTreeMap::from([(
                "https://api.test/parts.json".into(),
                (
                    "https://api.test/parts.json".into(),
                    json!({"item":target.clone()}),
                ),
            )]),
            ..Memory::default()
        };
        let mut loader = Loader::new();
        loader.register_fetcher("https://", memory.clone());
        let report = registry
            .load_sources(root, &mut loader, &SourceLoadOptions::default())
            .unwrap();
        assert_eq!(memory.reads.borrow().len(), 1);
        if target
            .get("$ref")
            .and_then(Value::as_str)
            .is_some_and(|reference| reference.starts_with("https:"))
        {
            assert!(report.reference_diagnostics.is_empty()); // a representable graph; indexing diagnoses the cycle
        } else {
            assert_eq!(
                report.reference_diagnostics.len(),
                1,
                "{:?}",
                report.reference_diagnostics
            );
        }
        let options = Options::new().source_registry(&registry, root).unwrap();
        assert!(prepare(&serde_json::from_value(workflow()).unwrap(), &options).is_err());
    }
}

#[test]
fn openapi_identity_is_version_specific_and_never_replaces_server_retrieval_base() {
    for version in ["3.0.4", "3.1.1", "3.2.0"] {
        let mut registry = SourceRegistry::new();
        let root = registry
            .insert("https://workflows.test/root.json", workflow())
            .unwrap();
        let api = registry.insert("https://api.test/openapi.json", json!({"openapi":version,"$self":"https://identity.test/api.json", "servers":[{"url":"/v1"}], "paths":{"/pets":{"get":{"operationId":"check"}}}})).unwrap();
        assert_eq!(
            registry.document(api).unwrap().identity().as_str(),
            if version == "3.2.0" {
                "https://identity.test/api.json"
            } else {
                "https://api.test/openapi.json"
            }
        );
        registry
            .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
            .unwrap();
        let options = Options::new().source_registry(&registry, root).unwrap();
        let mut fake = Fake::new().reply(200, &json!({}));
        prepare(&serde_json::from_value(workflow()).unwrap(), &options)
            .unwrap()
            .execute(&mut fake)
            .unwrap();
        assert_eq!(fake.sent()[0].url, "https://api.test/v1/pets");
    }
}

#[test]
fn canonical_api_identity_can_select_a_preloaded_source_before_linking() {
    let mut registry = SourceRegistry::new();
    let mut value = workflow();
    value["workflows"][0]["steps"][0]
        .as_object_mut()
        .unwrap()
        .remove("operationId");
    value["workflows"][0]["steps"][0]["operationPath"] =
        json!("https://identity.test/api.json#/paths/~1pets/get");
    let description: Description = serde_json::from_value(value.clone()).unwrap();
    let root = registry
        .insert("https://workflows.test/root.json", value)
        .unwrap();
    registry.insert_reference_document("https://api.test/openapi.json", json!({"openapi":"3.2.0", "$self":"https://identity.test/api.json", "paths":{"/pets":{"get":{}}}})).unwrap();
    let before = Options::new().source_registry(&registry, root).unwrap();
    assert_eq!(
        required_sources(&description, &before).unwrap(),
        BTreeSet::from(["api".into()])
    );
    registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    let after = Options::new().source_registry(&registry, root).unwrap();
    let mut fake = Fake::new().reply(200, &json!({}));
    prepare(&description, &after)
        .unwrap()
        .execute(&mut fake)
        .unwrap();
    assert_eq!(fake.sent()[0].url, "https://api.test/pets");
    assert!(prepare(&description, &before).is_err()); // immutable snapshot, not live links
}
