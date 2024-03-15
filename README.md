# roas

Rust OpenAPI Specification (v2.0, v3.0.X and v3.1.X)

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

Parsing and generating OpenAPI Specification:

* [x] OpenAPI Specification v2.0
* [x] OpenAPI Specification v3.0.X
* [ ] OpenAPI Specification v3.0.0

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
roas = { version = "0.2", features = ["v3_0"] } 
```

## Examples

```rust
use roas::v3_0::spec::Spec;
use roas::validation::{Options, Validate};

...

let spec = serde_json::from_str::<Spec>(raw_json).unwrap();
spec.validate(Options::IgnoreMissingTags | Options::IgnoreExternalReferences).unwrap();

...

```
