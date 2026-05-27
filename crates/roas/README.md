# roas

Rust OpenAPI Specification — parser, validator, and loader for v2.0 / v3.0.x / v3.1.x / v3.2.x.

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

A typed, [`serde`](https://serde.rs)-based model of every supported OpenAPI version, with a collecting validator, cross-version conversion, document merging / collapsing, and optional external-`$ref` loading. JSON and YAML round-trip through the same model.

## Versions

| OpenAPI version                                      | Feature flag       | Status       |
|------------------------------------------------------|--------------------|--------------|
| [2.0](https://spec.openapis.org/oas/v2.0.html)       | `v2`               | ✅ supported |
| [3.0.x](https://spec.openapis.org/oas/v3.0.4.html)   | `v3_0`             | ✅ supported |
| [3.1.x](https://spec.openapis.org/oas/v3.1.2.html)   | `v3_1`             | ✅ supported |
| [3.2.x](https://spec.openapis.org/oas/v3.2.0.html)   | `v3_2` *(default)* | ✅ supported |

Each version lives behind its own feature, so you compile only what you need. With two adjacent versions enabled, a `From` impl upconverts a spec to the newer one.

> [!CAUTION]
> The project is in an early development stage, so the API may change.
> Treat any `0.x` release as unstable and subject to breaking changes.

## Install

```shell
cargo add roas
```

The default feature is `v3_2`. For another version, disable defaults and enable the one you need:

```toml
[dependencies]
roas = { version = "0.17", default-features = false, features = ["v3_0"] }
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

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
