//! End-to-end coverage for the loader integration with `Validate::validate`.
//!
//! Loads a Petstore-style v3.2 spec from disk that references an
//! external `error.json` document. The placeholder URL in the spec is
//! patched at runtime to point at the on-disk `error.json` so the test
//! exercises [`JsonFileFetcher`] through the public
//! [`Validate::validate`] entry point — not a synthetic preload.
//!
//! Three states are asserted:
//!
//! 1. With a loader that has `file://` registered, validation succeeds.
//! 2. With no loader, the external `$ref` becomes a "not supported"
//!    validation error.
//! 3. With a loader that has no fetcher for the scheme, the loader
//!    failure surfaces as a `failed to resolve` validation error.

#![cfg(feature = "v3_2")]

use std::fs;

use roas::loader::{JsonFileFetcher, Loader};
use roas::v3_2::spec::Spec;
use roas::validation::{Options, Validate};
use url::Url;

const PETSTORE_SPEC_PATH: &str = "tests/loader_data/petstore.json";
const EXTERNAL_ERROR_PATH: &str = "tests/loader_data/error.json";
const PLACEHOLDER: &str = "__EXTERNAL_ERROR_URL__";

/// Read the on-disk spec, substitute the `__EXTERNAL_ERROR_URL__`
/// placeholder with a real `file://` URL pointing at the sibling
/// `error.json`, and return the parsed spec.
fn load_spec_with_external_error_url() -> Spec {
    let raw = fs::read_to_string(PETSTORE_SPEC_PATH)
        .unwrap_or_else(|e| panic!("read {PETSTORE_SPEC_PATH}: {e}"));
    let error_abs = fs::canonicalize(EXTERNAL_ERROR_PATH)
        .unwrap_or_else(|e| panic!("canonicalize {EXTERNAL_ERROR_PATH}: {e}"));
    let error_url =
        Url::from_file_path(&error_abs).unwrap_or_else(|()| panic!("file URL from {error_abs:?}"));
    let patched = raw.replace(PLACEHOLDER, error_url.as_str());
    serde_json::from_str::<Spec>(&patched).expect("patched spec must parse")
}

#[test]
fn validate_with_json_file_fetcher_resolves_external_schema() {
    let spec = load_spec_with_external_error_url();

    let mut loader = Loader::new();
    loader.register_fetcher("file://", JsonFileFetcher);

    spec.validate(Options::IgnoreMissingTags.only(), Some(&mut loader))
        .expect("validation must succeed when JsonFileFetcher can serve the external ref");
}

#[test]
fn validate_without_loader_errors_on_external_ref() {
    let spec = load_spec_with_external_error_url();

    let err = spec
        .validate(Options::IgnoreMissingTags.only(), None)
        .expect_err("external `$ref` must error when no loader is attached");
    assert!(
        err.errors
            .iter()
            .any(|e| e.contains("error.json#/Error") && e.contains("not supported")),
        "expected `not supported` error referencing error.json, got: {:?}",
        err.errors,
    );
}

#[test]
fn validate_with_empty_loader_surfaces_no_fetcher_error() {
    let spec = load_spec_with_external_error_url();

    // Loader exists but has no fetcher registered for `file://`, so the
    // resolution attempt surfaces as a `failed to resolve` validation
    // error (with a `NoFetcherRegistered` source).
    let mut loader = Loader::new();
    let err = spec
        .validate(Options::IgnoreMissingTags.only(), Some(&mut loader))
        .expect_err("missing fetcher must surface as a validation error");
    assert!(
        err.errors.iter().any(|e| {
            e.contains("error.json#/Error")
                && e.contains("failed to resolve")
                && e.contains("no fetcher registered")
        }),
        "expected `failed to resolve` + `no fetcher registered` error, got: {:?}",
        err.errors,
    );
}

#[test]
fn validate_with_ignore_external_references_short_circuits_loader() {
    // `IgnoreExternalReferences` must skip external `$ref`s entirely —
    // the loader should not even be consulted, and the resolved
    // external value should not be validated. This guards the option's
    // pre-loader-integration semantics: attaching a loader to a spec
    // with broken externals must not start surfacing those breaks when
    // the user explicitly asked to ignore externals.
    //
    // We use an unreachable external URL: if the option weren't
    // respected the loader would surface a `failed to resolve` error.
    let raw = fs::read_to_string(PETSTORE_SPEC_PATH).unwrap();
    let patched = raw.replace(PLACEHOLDER, "file:///does/not/exist/never");
    let spec: Spec = serde_json::from_str(&patched).expect("spec must parse");

    let mut loader = Loader::new();
    loader.register_fetcher("file://", JsonFileFetcher);

    spec.validate(
        Options::IgnoreMissingTags | Options::IgnoreExternalReferences,
        Some(&mut loader),
    )
    .expect("IgnoreExternalReferences must short-circuit before the loader is consulted");
}
