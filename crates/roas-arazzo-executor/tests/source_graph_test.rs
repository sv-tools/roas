#![cfg(feature = "source-graph")]

use roas::loader::{AsyncResourceFetcher, FetchFuture, Loader, LoaderError, ResourceFetcher};
use roas_arazzo_executor::{
    Options, SourceError, SourceLoadOptions, SourceRegistry, SourceVersion, prepare, testing::Fake,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use url::Url;

fn workflow(sources: Value) -> Value {
    json!({ "arazzo": "1.1.0", "info": { "title": "Graph", "version": "1" },
        "sourceDescriptions": sources,
        "workflows": [{ "workflowId": "w", "steps": [{ "stepId": "get", "operationId": "$sourceDescriptions.api.check" }] }] })
}

fn api() -> Value {
    json!({ "openapi": "3.1.0", "info": { "title": "API", "version": "1" },
        "servers": [{ "url": "https://api.example.test" }],
        "paths": { "/check": { "get": { "operationId": "check" } } } })
}

#[derive(Clone)]
struct Memory {
    documents: BTreeMap<String, Value>,
    reads: Rc<RefCell<Vec<String>>>,
}

impl Memory {
    fn new(documents: impl IntoIterator<Item = (&'static str, Value)>) -> Self {
        Self {
            documents: documents
                .into_iter()
                .map(|(uri, value)| (uri.into(), value))
                .collect(),
            reads: Rc::default(),
        }
    }
    fn loader(&self) -> Loader {
        let mut loader = Loader::new();
        loader.register_fetcher("https://", self.clone());
        loader
    }
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
impl AsyncResourceFetcher for Memory {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a> {
        Box::pin(async move { ResourceFetcher::fetch(self, uri) })
    }
}

#[test]
fn relative_self_and_equivalent_references_reuse_one_document() {
    let mut value = workflow(json!([
        { "name": "api", "url": "./nested/../api.json" },
        { "name": "same", "url": "https://EXAMPLE.test:443/ids/./api.json#/paths" }
    ]));
    value["$self"] = json!("../ids/root.json");
    value["components"] =
        json!({ "inputs": { "schema": { "$id": "nested/", "$ref": "../other.json" } } });
    let mut registry = SourceRegistry::new();
    assert!(registry.is_empty());
    let root = registry
        .insert("https://example.test/cache/root.json", value.clone())
        .unwrap();
    let memory = Memory::new([("https://example.test/ids/api.json", api())]);
    let report = registry
        .load_sources(root, &mut memory.loader(), &SourceLoadOptions::default())
        .unwrap();
    assert!(report.diagnostics.is_empty());
    assert!(report.cycles.is_empty());
    assert_eq!(report.fetch_attempts, 1);
    assert_eq!(registry.len(), 2);
    assert_eq!(memory.reads.borrow().len(), 1);
    assert_eq!(registry.document(root).unwrap().value(), &value);
    assert_eq!(
        registry.document(root).unwrap().identity().as_str(),
        "https://example.test/ids/root.json"
    );
    assert_eq!(
        registry.document(root).unwrap().retrieval_uri().as_str(),
        "https://example.test/cache/root.json"
    );
    assert_eq!(
        registry.source(root, "api").unwrap().target,
        registry.source(root, "same").unwrap().target
    );
    let options = Options::new().source_registry(&registry, root).unwrap();
    assert_eq!(options.source_document("api").unwrap().version(), "3.1.0");
    assert_eq!(
        options.source_document("api").unwrap().model(),
        SourceVersion::OpenApi3_1
    );
    assert!(options.source_document("absent").is_none());
}

#[tokio::test]
async fn graph_and_cloned_options_share_the_loaders_document_value() {
    let memory = Memory::new([("https://graph.test/api.json", api())]);
    let mut loader = Loader::new();
    loader.register_async_fetcher("https://", memory.clone());
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                {"name":"api", "url":"api.json"}, {"name":"same", "url":"./api.json"}
            ])),
        )
        .unwrap();
    registry
        .load_sources_async(root, &mut loader, &SourceLoadOptions::default())
        .await
        .unwrap();
    let loaded = loader
        .load_document_shared("https://graph.test/api.json")
        .unwrap();
    let options = Options::new().source_registry(&registry, root).unwrap();
    let cloned = options.clone();
    let api_id = registry.source(root, "api").unwrap().target.unwrap();
    assert!(std::ptr::eq(
        registry.document(api_id).unwrap().value(),
        &loaded.document
    ));
    for entry in [&options, &cloned] {
        for alias in ["api", "same"] {
            assert!(std::ptr::eq(
                entry.source_document(alias).unwrap().value(),
                &loaded.document
            ));
        }
    }
    let description = registry.document(root).unwrap().arazzo().unwrap().clone();
    drop(registry);
    drop(loader);
    let mut client = Fake::new().reply(200, &json!({}));
    assert!(
        prepare(&description, &cloned)
            .unwrap()
            .execute(&mut client)
            .unwrap()
            .is_success()
    );
    assert_eq!(memory.reads.borrow().len(), 1);
}

#[test]
fn opaque_self_errors_explain_both_remedies_without_rejecting_absolute_sources() {
    let mut document = workflow(json!([
        {"name":"relative", "url":"api.json"},
        {"name":"absolute", "url":"https://graph.test/api.json"},
        {"name":"malformed", "url":"http://["}
    ]));
    document["$self"] = json!("urn:example:root");
    let mut registry = SourceRegistry::new();
    let root = registry.insert("file:///root.json", document).unwrap();
    let api = registry
        .insert("https://graph.test/api.json", api())
        .unwrap();
    let report = registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(report.diagnostics.len(), 2);
    let message = report.diagnostics[0].error.to_string();
    assert!(message.contains("absolute URL"), "{message}");
    assert!(message.contains("hierarchical `$self`"), "{message}");
    assert!(message.contains("urn:example:root"), "{message}");
    assert!(
        !report.diagnostics[1]
            .error
            .to_string()
            .contains("hierarchical `$self`")
    );
    assert_eq!(registry.source(root, "absolute").unwrap().target, Some(api));
}

#[test]
fn failed_aliases_share_an_attempt_but_queries_remain_distinct() {
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                {"name": "first", "url": "missing.json?revision=1"},
                {"name": "same", "url": "./missing.json?revision=1#/ignored"},
                {"name": "other", "url": "missing.json?revision=2"}
            ])),
        )
        .unwrap();
    let memory = Memory::new([]);
    let mut options = SourceLoadOptions::default();
    options.max_documents = 2; // root plus one failed attempt
    let report = registry
        .load_sources(root, &mut memory.loader(), &options)
        .unwrap();
    assert_eq!(report.fetch_attempts, 1);
    assert_eq!(memory.reads.borrow().len(), 1);
    assert_eq!(report.diagnostics.len(), 3);
    assert!(std::sync::Arc::ptr_eq(
        &report.diagnostics[0].error,
        &report.diagnostics[1].error
    ));
    assert!(matches!(
        *report.diagnostics[2].error,
        SourceError::Limit {
            kind: "document",
            limit: 2
        }
    ));
    assert!(report.diagnostics[0].path.ends_with("[0].url"));
    assert!(report.diagnostics[1].path.ends_with("[1].url"));
    assert!(report.diagnostics[2].path.ends_with("[2].url"));
}

#[test]
fn wrong_kinds_invalid_bases_and_alias_collisions_are_typed_and_preserve_documents() {
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                {"name": "wrong", "url": "api.json", "type": "arazzo"},
                {"name": "invalid", "url": "http://["},
                {"name": "valid", "url": "api.json", "type": "openapi"}
            ])),
        )
        .unwrap();
    let api_id = registry
        .insert("https://graph.test/api.json", api())
        .unwrap();
    assert!(matches!(
        registry.override_source(root, "wrong", api_id),
        Err(SourceError::Kind { .. })
    ));
    assert!(matches!(
        registry.add_retrieval_alias(api_id, "https://graph.test/root.json"),
        Err(SourceError::Conflict(_))
    ));
    let report = registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(report.diagnostics.len(), 2);
    assert!(matches!(
        *report.diagnostics[0].error,
        SourceError::Kind { .. }
    ));
    assert!(matches!(
        *report.diagnostics[1].error,
        SourceError::InvalidUri { .. }
    ));
    assert_eq!(registry.source(root, "valid").unwrap().target, Some(api_id));
    assert_eq!(registry.len(), 2);

    let mut value = workflow(json!([{"name": "relative", "url": "api.json"}]));
    value["$self"] = json!("urn:example:opaque");
    let opaque = registry.insert("file:///opaque.json", value).unwrap();
    let report = registry
        .load_sources(opaque, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    assert!(matches!(
        *report.diagnostics[0].error,
        SourceError::InvalidUri { .. }
    ));
    assert_eq!(report.fetch_attempts, 0);
    assert!(matches!(
        registry.resolve(opaque, "api.json", false),
        Err(SourceError::InvalidUri { .. })
    ));

    let mut collision = workflow(json!([]));
    collision["$self"] = json!("https://graph.test/api.json");
    assert!(matches!(
        registry.insert("file:///collision.json", collision),
        Err(SourceError::Conflict(_))
    ));
    assert_eq!(registry.len(), 3);
    let foreign = SourceRegistry::new();
    assert!(matches!(
        Options::new().source_registry(&foreign, root),
        Err(SourceError::UnknownDocument(_))
    ));
    assert!(matches!(
        registry.override_source(root, "valid", opaque),
        Err(SourceError::Kind { .. })
    ));
}

#[test]
fn redirect_metadata_controls_relative_self_and_child_references() {
    struct Redirect;
    impl ResourceFetcher for Redirect {
        fn fetch(&mut self, _: &Url) -> Result<Value, LoaderError> {
            panic!("metadata-aware path required")
        }
        fn fetch_document(&mut self, _: &Url) -> Result<roas::LoadedDocument, LoaderError> {
            let mut value = workflow(json!([{"name":"api", "url":"api.json"}]));
            value["$self"] = json!("../identity/child.json");
            Ok(roas::LoadedDocument::new(
                value,
                Url::parse("https://graph.test/redirected/child.json").unwrap(),
            ))
        }
    }
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                {"name":"child", "url":"https://graph.test/identity/child.json", "type":"arazzo"}
            ])),
        )
        .unwrap();
    let api_id = registry
        .insert("https://graph.test/identity/api.json", api())
        .unwrap();
    let mut loader = Loader::new();
    loader.register_fetcher("https://", Redirect);
    let report = registry
        .load_sources(root, &mut loader, &SourceLoadOptions::default())
        .unwrap();
    assert!(report.diagnostics.is_empty(), "{report:?}");
    let child = registry.source(root, "child").unwrap().target.unwrap();
    assert_eq!(
        registry.document(child).unwrap().retrieval_uri().as_str(),
        "https://graph.test/redirected/child.json"
    );
    assert_eq!(
        registry.document(child).unwrap().identity().as_str(),
        "https://graph.test/identity/child.json"
    );
    assert_eq!(registry.source(child, "api").unwrap().target, Some(api_id));
    assert_eq!(report.fetch_attempts, 1);
}

#[test]
fn deferred_shorter_identity_path_revisits_depth_limited_descendants() {
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                {"name":"short", "url":"identity.json", "type":"arazzo"},
                {"name":"long", "url":"a.json", "type":"arazzo"}
            ])),
        )
        .unwrap();
    let mut identified = workflow(json!([{"name":"api", "url":"api.json"}]));
    identified["$self"] = json!("https://graph.test/identity.json");
    let memory = Memory::new([
        (
            "https://graph.test/a.json",
            workflow(json!([{"name":"b", "url":"b.json", "type":"arazzo"}])),
        ),
        (
            "https://graph.test/b.json",
            workflow(json!([{"name":"mirror", "url":"mirror.json", "type":"arazzo"}])),
        ),
        ("https://graph.test/mirror.json", identified),
        ("https://graph.test/api.json", api()),
    ]);
    let mut options = SourceLoadOptions::default();
    options.retrieval_aliases = true;
    options.max_depth = 3;
    let report = registry
        .load_sources(root, &mut memory.loader(), &options)
        .unwrap();
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.fetch_attempts, 5); // one failed identity lookup, four readable documents
    let child = registry.source(root, "short").unwrap().target.unwrap();
    assert!(registry.source(child, "api").unwrap().target.is_some());
    assert_eq!(registry.len(), 5);
}

#[test]
fn all_supplied_identities_are_indexed_before_loading_and_aliases_are_scoped() {
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "file:///supplied/root.json",
            workflow(json!([
                { "name": "child", "url": "https://identity.test/child.json", "type": "arazzo" },
                { "name": "api", "url": "https://api.test/root.json" }
            ])),
        )
        .unwrap();
    let mut child = workflow(json!([{ "name": "api", "url": "https://api.test/child.json" }]));
    child["$self"] = json!("https://identity.test/child.json");
    let child = registry
        .insert("file:///offline/mirror.json", child)
        .unwrap();
    let root_api = registry
        .insert("https://api.test/root.json", api())
        .unwrap();
    let child_api = registry
        .insert("https://api.test/child.json", api())
        .unwrap();
    let report = registry
        .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
        .unwrap();
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.fetch_attempts, 0);
    assert_eq!(registry.source(root, "child").unwrap().target, Some(child));
    assert_eq!(registry.source(root, "api").unwrap().target, Some(root_api));
    assert_eq!(
        registry.source(child, "api").unwrap().target,
        Some(child_api)
    );
    assert!(matches!(
        registry.resolve(root, "file:///offline/mirror.json", false),
        Err(SourceError::Identity { .. })
    ));
    assert_eq!(
        registry
            .resolve(root, "file:///offline/mirror.json", true)
            .unwrap(),
        Some(child)
    );
    assert_eq!(
        registry
            .resolve(root, "https://identity.test/child.json#/workflows/0", false)
            .unwrap(),
        Some(child)
    );
}

#[test]
fn diamonds_are_shared_and_back_edges_are_cycles_not_expanded_documents() {
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                { "name": "a", "url": "a.json", "type": "arazzo" },
                { "name": "b", "url": "b.json", "type": "arazzo" }
            ])),
        )
        .unwrap();
    let branches = workflow(json!([{ "name": "shared", "url": "shared.json", "type": "arazzo" }]));
    let memory = Memory::new([
        ("https://graph.test/a.json", branches.clone()),
        ("https://graph.test/b.json", branches),
        (
            "https://graph.test/shared.json",
            workflow(json!([{ "name": "back", "url": "root.json", "type": "arazzo" }])),
        ),
    ]);
    let report = registry
        .load_sources(root, &mut memory.loader(), &SourceLoadOptions::default())
        .unwrap();
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.fetch_attempts, 3);
    assert_eq!(registry.len(), 4);
    assert_eq!(report.cycles.len(), 1);
    assert_eq!(report.cycles[0].target, root);
    let a = registry.source(root, "a").unwrap().target.unwrap();
    let b = registry.source(root, "b").unwrap().target.unwrap();
    assert_eq!(
        registry.source(a, "shared").unwrap().target,
        registry.source(b, "shared").unwrap().target
    );
}

#[test]
fn later_discovery_repairs_an_earlier_unresolved_identity() {
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert(
            "https://graph.test/root.json",
            workflow(json!([
                { "name": "identity", "url": "https://identity.test/child", "type": "arazzo" },
                { "name": "location", "url": "mirror.json", "type": "arazzo" }
            ])),
        )
        .unwrap();
    let mut child = workflow(json!([{ "name": "api", "url": "https://api.test/openapi.json" }]));
    child["$self"] = json!("https://identity.test/child");
    let memory = Memory::new([
        ("https://graph.test/mirror.json", child),
        ("https://api.test/openapi.json", api()),
    ]);
    let report = registry
        .load_sources(root, &mut memory.loader(), &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].source_name, "location");
    assert!(matches!(
        &*report.diagnostics[0].error,
        SourceError::Identity { .. }
    ));
    assert!(registry.source(root, "identity").unwrap().target.is_some());
    assert!(registry.source(root, "location").unwrap().target.is_none());
    let mut options = SourceLoadOptions::default();
    options.retrieval_aliases = true;
    let report = registry
        .load_sources(root, &mut Loader::new(), &options)
        .unwrap();
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.fetch_attempts, 0);
}

#[test]
fn unavailable_unrelated_source_does_not_make_a_qualified_run_fail_or_a_bare_run_pass() {
    let mut value = workflow(json!([
        { "name": "api", "url": "api.json" }, { "name": "unavailable", "url": "absent.json" }
    ]));
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert("https://graph.test/root.json", value.clone())
        .unwrap();
    let memory = Memory::new([("https://graph.test/api.json", api())]);
    let report = registry
        .load_sources(root, &mut memory.loader(), &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].owner, root);
    assert_eq!(report.diagnostics[0].path, "#.sourceDescriptions[1].url");
    assert!(report.diagnostics[0].to_string().contains("unavailable"));
    let options = Options::new().source_registry(&registry, root).unwrap();
    let description = registry.document(root).unwrap().arazzo().unwrap();
    assert!(
        prepare(description, &options)
            .unwrap()
            .execute(&mut Fake::new().reply(200, &json!({})))
            .unwrap()
            .is_success()
    );
    value["workflows"][0]["steps"][0]["operationId"] = json!("check");
    let error = prepare(&serde_json::from_value(value).unwrap(), &options).unwrap_err();
    assert!(error.to_string().contains("was not supplied"));
}

#[test]
fn graph_budgets_are_located_and_independent_of_execution_limits() {
    for (max_documents, max_depth, kind) in [(1, 32, "document"), (256, 0, "depth")] {
        let mut registry = SourceRegistry::new();
        let root = registry
            .insert(
                "https://graph.test/root.json",
                workflow(json!([{ "name": "api", "url": "api.json" }])),
            )
            .unwrap();
        let memory = Memory::new([("https://graph.test/api.json", api())]);
        let mut options = SourceLoadOptions::default();
        options.max_documents = max_documents;
        options.max_depth = max_depth;
        let report = registry
            .load_sources(root, &mut memory.loader(), &options)
            .unwrap();
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            matches!(&*report.diagnostics[0].error, SourceError::Limit { kind: actual, .. } if *actual == kind)
        );
        assert_eq!(report.fetch_attempts, 0);
        assert!(memory.reads.borrow().is_empty());
        options.max_documents = 0;
        assert!(matches!(
            registry.load_sources(root, &mut memory.loader(), &options),
            Err(SourceError::Limit { .. })
        ));
        assert_eq!(registry.len(), 1);
    }
}

#[test]
fn selection_and_scoped_overrides_preserve_legacy_precedence() {
    let value = workflow(
        json!([{ "name": "api", "url": "api.json" }, { "name": "unused", "url": "unused.json" }]),
    );
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert("https://graph.test/root.json", value.clone())
        .unwrap();
    let other = registry
        .insert("https://graph.test/other.json", value)
        .unwrap();
    let a = registry.insert("file:///supplied/a.json", api()).unwrap();
    let b = registry.insert("file:///supplied/b.json", api()).unwrap();
    registry.override_source(root, "api", a).unwrap();
    registry.override_source(other, "api", b).unwrap();
    registry
        .override_base_url(root, "api", "https://root.test")
        .unwrap();
    registry
        .override_base_url(other, "api", "https://other.test")
        .unwrap();
    let mut load = SourceLoadOptions::default();
    load.root_sources = Some(["api".into()].into());
    let report = registry
        .load_sources(root, &mut Loader::new(), &load)
        .unwrap();
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.fetch_attempts, 0);
    assert!(registry.source(root, "unused").unwrap().target.is_none());
    for (owner, expected) in [
        (root, "https://root.test/check"),
        (other, "https://other.test/check"),
    ] {
        let options = Options::new().source_registry(&registry, owner).unwrap();
        let mut client = Fake::new().reply(200, &json!({}));
        prepare(
            registry.document(owner).unwrap().arazzo().unwrap(),
            &options,
        )
        .unwrap()
        .execute(&mut client)
        .unwrap();
        assert_eq!(client.sent()[0].url, expected);
    }
    let options = Options::new()
        .source("api", "api.json", api())
        .base_url("api", "https://explicit.test")
        .source_registry(&registry, root)
        .unwrap();
    assert!(options.source_document("api").is_none());
    let mut client = Fake::new().reply(200, &json!({}));
    prepare(registry.document(root).unwrap().arazzo().unwrap(), &options)
        .unwrap()
        .execute(&mut client)
        .unwrap();
    assert_eq!(client.sent()[0].url, "https://explicit.test/check");
    load.root_sources = Some(["typo".into()].into());
    assert!(matches!(
        registry.load_sources(root, &mut Loader::new(), &load),
        Err(SourceError::Alias { .. })
    ));
    assert!(registry.override_source(root, "typo", a).is_err());
    assert!(
        registry
            .override_base_url(root, "typo", "https://example.test")
            .is_err()
    );
}

#[test]
fn complete_arazzo_parse_and_identity_conflicts_are_not_silent_overwrites() {
    let mut registry = SourceRegistry::new();
    let mut value = workflow(json!([{ "name": "api", "url": "https://api.test" }]));
    value["$self"] = json!("https://identity.test/w");
    let id = registry
        .insert("file:///first.json", value.clone())
        .unwrap();
    assert_eq!(
        registry
            .insert("file:///second.json", value.clone())
            .unwrap(),
        id
    );
    assert_eq!(
        registry
            .insert("file:///first.json", value.clone())
            .unwrap(),
        id
    );
    value["info"]["title"] = json!("conflict");
    assert!(matches!(
        registry.insert("file:///first.json", value.clone()),
        Err(SourceError::Conflict(_))
    ));
    assert!(matches!(
        registry.insert("file:///third.json", value.clone()),
        Err(SourceError::Conflict(_))
    ));
    value["workflows"] = json!([{ "workflowId": "broken", "steps": "not a list" }]);
    assert!(matches!(
        registry.insert("file:///broken.json", value),
        Err(SourceError::Parse { .. })
    ));
    assert_eq!(registry.len(), 1);
    let mut value = workflow(json!([{ "name": "api", "url": "a" }, { "name": "api", "url": "b" }]));
    assert!(matches!(
        registry.insert("file:///duplicates.json", value.clone()),
        Err(SourceError::Alias { .. })
    ));
    value["sourceDescriptions"] = json!([]);
    value["$self"] = json!("https://identity.test/w#fragment");
    assert!(matches!(
        registry.insert("file:///fragment.json", value),
        Err(SourceError::InvalidUri { .. })
    ));
    assert!(registry.insert("relative.json", api()).is_err());
    assert!(SourceRegistry::new().document(id).is_err());
}

#[test]
fn source_versions_are_checked_without_implying_broker_support() {
    for (field, version, model) in [
        ("swagger", "2.0", SourceVersion::OpenApi2),
        ("openapi", "3.0.4", SourceVersion::OpenApi3_0),
        ("openapi", "3.1.1", SourceVersion::OpenApi3_1),
        ("openapi", "3.2.0", SourceVersion::OpenApi3_2),
        ("asyncapi", "2.6.0", SourceVersion::AsyncApi2_6),
        ("asyncapi", "3.0.0", SourceVersion::AsyncApi3_0),
        ("asyncapi", "3.1.0", SourceVersion::AsyncApi3_1),
    ] {
        let mut registry = SourceRegistry::new();
        let id = registry
            .insert("file:///document.json", json!({ field: version }))
            .unwrap();
        assert_eq!(registry.document(id).unwrap().model(), model);
        assert_eq!(registry.document(id).unwrap().version(), version);
        assert!(registry.document(id).unwrap().arazzo().is_none());
    }
    for version in ["1.0.1", "1.1.0"] {
        let mut value = workflow(json!([{ "name": "api", "url": "api.json" }]));
        value["arazzo"] = json!(version);
        let mut registry = SourceRegistry::new();
        let id = registry.insert("file:///document.json", value).unwrap();
        assert_eq!(registry.document(id).unwrap().version(), version);
        assert_eq!(
            registry
                .document(id)
                .unwrap()
                .arazzo()
                .unwrap()
                .arazzo
                .as_str(),
            "1.1.0"
        );
    }
    for value in [
        json!({}),
        json!({ "openapi": "3.9.0" }),
        json!({ "asyncapi": "3.1.1" }),
        json!({ "arazzo": "2.0.0" }),
        json!({ "arazzo": "1.1.x" }),
        json!({ "openapi": "3.1.0", "arazzo": "1.1.0" }),
    ] {
        assert!(
            SourceRegistry::new()
                .insert("file:///bad.json", value)
                .is_err()
        );
    }
}

#[tokio::test]
async fn async_graph_loading_uses_async_policy_and_shares_cache_with_sync() {
    let value = workflow(json!([{ "name": "api", "url": "api.json" }]));
    let mut registry = SourceRegistry::new();
    let root = registry
        .insert("https://graph.test/root.json", value)
        .unwrap();
    let memory = Memory::new([("https://graph.test/api.json", api())]);
    let mut loader = Loader::new();
    loader.register_async_fetcher("https://", memory.clone());
    let report = registry
        .load_sources(root, &mut loader, &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(report.diagnostics.len(), 1);
    assert!(memory.reads.borrow().is_empty());
    let report = registry
        .load_sources_async(root, &mut loader, &SourceLoadOptions::default())
        .await
        .unwrap();
    assert!(report.diagnostics.is_empty());
    assert_eq!(memory.reads.borrow().len(), 1);
    assert!(
        registry
            .load_sources(root, &mut Loader::new(), &SourceLoadOptions::default())
            .unwrap()
            .diagnostics
            .is_empty()
    );
    fn send_sync<T: Send + Sync>() {}
    send_sync::<SourceRegistry>();
    send_sync::<roas_arazzo_executor::SourceLoadReport>();
}
