use roas::loader::{Loader, LoaderError, ResourceFetcher};
use roas_arazzo_executor::{SourceLoadOptions, SourceRegistry};
use roas_file_fetcher::FileFetcher;
use serde_json::Value;
use std::{cell::RefCell, path::Path, rc::Rc};
use url::Url;

struct CountingFiles(Rc<RefCell<Vec<Url>>>);

impl ResourceFetcher for CountingFiles {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        self.0.borrow_mut().push(uri.clone());
        FileFetcher::new().fetch(uri)
    }
}

#[test]
fn mixed_json_yaml_file_graph_preserves_cycles_and_fetches_diamonds_once() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/source-graph/root.json");
    let uri = Url::from_file_path(path).unwrap();
    let root_value = serde_json::from_str(include_str!("fixtures/source-graph/root.json")).unwrap();
    let mut registry = SourceRegistry::new();
    let root = registry.insert(uri.as_str(), root_value).unwrap();
    let reads = Rc::default();
    let mut loader = Loader::new();
    loader.register_fetcher("file://", CountingFiles(Rc::clone(&reads)));
    let report = registry
        .load_sources(root, &mut loader, &SourceLoadOptions::default())
        .unwrap();
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.fetch_attempts, 4);
    assert_eq!(reads.borrow().len(), 4);
    assert_eq!(registry.len(), 5);
    assert_eq!(report.cycles.len(), 1);
    assert_eq!(report.cycles[0].target, root);
    let left = registry.source(root, "left").unwrap().target.unwrap();
    let right = registry.source(root, "right").unwrap().target.unwrap();
    for name in ["shared", "api"] {
        assert_eq!(
            registry.source(left, name).unwrap().target,
            registry.source(right, name).unwrap().target
        );
    }
    let again = registry
        .load_sources(root, &mut loader, &SourceLoadOptions::default())
        .unwrap();
    assert_eq!(again.fetch_attempts, 0);
    assert_eq!(again.cycles, report.cycles);
}
