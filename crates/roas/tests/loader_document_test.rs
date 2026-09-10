use roas::loader::{
    AsyncResourceFetcher, FetchFuture, LoadedDocument, Loader, LoaderError, ResourceFetcher,
};
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use url::Url;

struct Fetcher(Rc<Cell<usize>>);
impl ResourceFetcher for Fetcher {
    fn fetch(&mut self, _: &Url) -> Result<Value, LoaderError> {
        self.0.set(self.0.get() + 1);
        Ok(json!({ "$self": "canonical.json", "nested": { "$ref": "other.json#/thing" } }))
    }
}
impl AsyncResourceFetcher for Fetcher {
    fn fetch<'a>(&'a mut self, uri: &'a Url) -> FetchFuture<'a> {
        Box::pin(async move { ResourceFetcher::fetch(self, uri) })
    }
}

#[test]
fn raw_documents_and_legacy_rewritten_resources_share_one_fetch() {
    for raw_first in [true, false] {
        let count = Rc::new(Cell::new(0));
        let mut loader = Loader::new();
        loader.register_fetcher("https://", Fetcher(count.clone()));
        let uri = "https://example.test/folder/root.json";
        if raw_first {
            loader.load_document(uri).unwrap();
        } else {
            loader.load_resource(uri).unwrap();
        }
        assert_eq!(
            loader.load_resource(uri).unwrap()["nested"]["$ref"],
            "https://example.test/folder/other.json#/thing"
        );
        let raw = loader.load_document(uri).unwrap();
        assert_eq!(raw.document["nested"]["$ref"], "other.json#/thing");
        assert_eq!(raw.document["$self"], "canonical.json");
        assert_eq!(raw.retrieval_uri.as_str(), uri);
        assert_eq!(count.get(), 1);
        loader
            .preload_resource(uri, json!({ "$ref": "replacement.json" }))
            .unwrap();
        assert_eq!(
            loader.load_document(uri).unwrap().document["$ref"],
            "replacement.json"
        );
        assert_eq!(
            loader.load_resource(uri).unwrap()["$ref"],
            "https://example.test/folder/replacement.json"
        );
        assert_eq!(count.get(), 1);
    }
}

struct Redirect;
impl ResourceFetcher for Redirect {
    fn fetch(&mut self, _: &Url) -> Result<Value, LoaderError> {
        panic!("metadata method is used")
    }
    fn fetch_document(&mut self, _: &Url) -> Result<LoadedDocument, LoaderError> {
        Ok(LoadedDocument::new(
            json!({ "$ref": "relative.json" }),
            Url::parse("https://final.test/path/doc.json#discard").unwrap(),
        ))
    }
}

#[test]
fn raw_metadata_retains_redirects_without_changing_legacy_rewrite_behavior() {
    let mut loader = Loader::new();
    loader.register_fetcher("https://", Redirect);
    let uri = "https://original.test/doc.json";
    assert_eq!(
        loader.load_document(uri).unwrap().retrieval_uri.as_str(),
        "https://final.test/path/doc.json"
    );
    assert_eq!(
        loader.load_resource(uri).unwrap()["$ref"],
        "https://original.test/relative.json"
    );
    assert!(Loader::new().load_document(uri).is_err());
    assert!(loader.load_document("http://[invalid").is_err());
}

// The crate has no async runtime dependency: these futures complete immediately.
fn ready<T>(future: impl std::future::Future<Output = T>) -> T {
    use std::task::{Context, Poll, Waker};
    match std::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("memory fetch must complete immediately"),
    }
}

#[test]
fn asynchronous_raw_fetching_and_legacy_loading_share_cache_and_policy() {
    let count = Rc::new(Cell::new(0));
    let mut loader = Loader::new();
    loader.register_async_fetcher("https://", Fetcher(count.clone()));
    let uri = "https://example.test/doc.json";
    assert!(loader.load_document(uri).is_err());
    assert_eq!(
        ready(loader.load_document_async(uri)).unwrap().document["nested"]["$ref"],
        "other.json#/thing"
    );
    assert_eq!(
        ready(loader.load_resource_async(uri)).unwrap()["nested"]["$ref"],
        "https://example.test/other.json#/thing"
    );
    assert!(loader.load_document(uri).is_ok());
    assert_eq!(count.get(), 1);
    assert!(ready(Loader::new().load_document_async(uri)).is_err());
}
