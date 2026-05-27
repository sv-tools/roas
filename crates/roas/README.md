# roas

Rust OpenAPI Specification — parser, validator, and loader for v2.0 / v3.0.x / v3.1.x / v3.2.x.

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

A typed, [`serde`](https://serde.rs)-based model of every supported OpenAPI version, with a collecting validator,
cross-version conversion, document merging / collapsing, and optional external-`$ref` loading. JSON and YAML round-trip
through the same model.

## Versions

| OpenAPI version                                    | Feature flag       | Status      |
|----------------------------------------------------|--------------------|-------------|
| [2.0](https://spec.openapis.org/oas/v2.0.html)     | `v2`               | ✅ supported |
| [3.0.x](https://spec.openapis.org/oas/v3.0.4.html) | `v3_0`             | ✅ supported |
| [3.1.x](https://spec.openapis.org/oas/v3.1.2.html) | `v3_1`             | ✅ supported |
| [3.2.x](https://spec.openapis.org/oas/v3.2.0.html) | `v3_2` *(default)* | ✅ supported |

Each version lives behind its own feature, so you compile only what you need. With two adjacent versions enabled, a
`From` impl upconverts a spec to the newer one.

## Install

```shell
cargo add roas
```

## Quick start

```shell
cargo add roas serde_json
```

```rust
use roas::v3_2::spec::Spec;
use roas::validation::{Options, Validate};

let raw_json = r#"{ "openapi": "3.2.0", "info": { "title": "demo", "version": "1" }, "paths": {} }"#;
let spec: Spec = serde_json::from_str(raw_json).unwrap();
spec.validate(
    Options::IgnoreMissingTags | Options::IgnoreExternalReferences,
    None,
).unwrap();
```

## Resolving external references

By default, external `$ref`s are reported rather than followed. Register a
fetcher on a `Loader` and pass it to `validate` to resolve them. Fetchers
ship as their own crates — [`roas-file-fetcher`](https://crates.io/crates/roas-file-fetcher)
for `file://` and [`roas-http-fetcher`](https://crates.io/crates/roas-http-fetcher)
for `http(s)://`:

```shell
cargo add roas roas-file-fetcher serde_json
```

```rust
use roas::loader::Loader;
use roas::v3_2::spec::Spec;
use roas::validation::{Options, Validate};
use roas_file_fetcher::FileFetcher;

let raw = std::fs::read_to_string("openapi.json").unwrap();
let spec: Spec = serde_json::from_str(&raw).unwrap();

// Register a fetcher so `file://` $refs resolve during validation.
let mut loader = Loader::new();
loader.register_fetcher("file://", FileFetcher::new());

// Without `IgnoreExternalReferences`, external refs are now followed
// through the loader instead of being reported as unresolved.
spec.validate(Options::IgnoreMissingTags.into(), Some(&mut loader)).unwrap();
```

For `http(s)://` refs, register a `roas_http_fetcher::HttpFetcher` on the
`http://` and `https://` prefixes the same way (it's `Clone`, so one client
can serve both).

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your
option.
