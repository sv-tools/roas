# roas

Rust OpenAPI Specification (v2.0, v3.0.X, v3.1.X and v3.2.X) parser, validator, and loader.

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

Supported specifications:

* [x] OpenAPI Specification [v2.0](https://spec.openapis.org/oas/v2.0.html) (feature `v2`)
* [x] OpenAPI Specification [v3.0.x](https://spec.openapis.org/oas/v3.0.4.html) (feature `v3_0`)
* [x] OpenAPI Specification [v3.1.x](https://spec.openapis.org/oas/v3.1.2.html) (feature `v3_1`)
* [x] OpenAPI Specification [v3.2.x](https://spec.openapis.org/oas/v3.2.0.html) (feature `v3_2`, **default**)

> [!CAUTION]
> The project is in early development stage, so the API may change in the future.
> Consider any 0.x.x version as unstable and subject to breaking changes.

## Usage

```shell
cargo add roas
```

or manually:

```toml
[dependencies]
roas = "0.11"
```

The default feature is `v3_2`. To parse v2.0, v3.0, or v3.1 specs, enable the corresponding feature:

```toml
[dependencies]
roas = { version = "0.11", default-features = false, features = ["v3_0"] }
```

## Example

```rust
use roas::v3_2::spec::Spec;
use roas::validation::{Options, Validate};

let raw_json = r#"{ "openapi": "3.2.0", "info": { "title": "demo", "version": "1" }, "paths": {} }"#;
let spec: Spec = serde_json::from_str(raw_json).unwrap();
spec.validate(Options::IgnoreMissingTags | Options::IgnoreExternalReferences, None).unwrap();
```

The second argument to `validate` is an optional [`Loader`](https://docs.rs/roas/latest/roas/loader/struct.Loader.html)
that resolves external `$ref`s. `None` keeps the original behaviour (external refs surface as a "not supported"
validation error unless `IgnoreExternalReferences` is set). Pass `Some(&mut loader)` with a registered fetcher to
recursively fetch, deserialize, and validate the referenced documents — see the
[`loader` module docs](https://docs.rs/roas/latest/roas/loader/index.html).

## Companion crates

- [`roas-cli`](https://crates.io/crates/roas-cli) — `roas` command-line front-end (`validate`, `convert`).
- [`roas-http-fetcher`](https://crates.io/crates/roas-http-fetcher) — `http://` / `https://` fetcher for the loader.
