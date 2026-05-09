# roas

Rust OpenAPI Specification (v2.0, v3.0.X and v3.1.X)

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

Parsing and generating OpenAPI Specification:

* [x] OpenAPI Specification [v2.0](https://spec.openapis.org/oas/v2.0.html) (**v2**: old specification, disabled by default)
* [x] OpenAPI Specification [v3.0.x](https://spec.openapis.org/oas/v3.0.4.html) (**v3_0**: disable by default)
* [x] OpenAPI Specification [v3.1.x](https://spec.openapis.org/oas/v3.1.2.html) (**v3_1**:; default feature)

> [!CAUTION]
> The project is in early development stage, so the API may change in the future.
> Consider any 0.x.x version as unstable and subject to breaking changes.

## Usage

To use `roas`, add it to your `Cargo.toml`:

```shell
cargo add roas
```

or manually add the following lines:

```toml
[dependencies]
roas = "0.7"
```

The default feature is `v3_1`. To parse v2.0 or v3.0 specs, enable the
corresponding feature:

```toml
[dependencies]
roas = { version = "0.7", default-features = false, features = ["v3_0"] }
```

## Examples

The default feature is `v3_1`. The example below also uses `serde_json`
directly, so add both crates:

```shell
cargo add roas serde_json
```

```rust
use roas::v3_1::spec::Spec;
use roas::validation::{Options, Validate};

let raw_json = r#"{ "openapi": "3.1.2", "info": { "title": "demo", "version": "1" }, "paths": {} }"#;
let spec: Spec = serde_json::from_str(raw_json).unwrap();
spec.validate(Options::IgnoreMissingTags | Options::IgnoreExternalReferences).unwrap();
```
