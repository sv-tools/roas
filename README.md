# roas

Rust OpenAPI Specification (v2.0, v3.0.X and v3.1.X)

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

Parsing and generating OpenAPI Specification:

* [x] OpenAPI Specification v2.0
* [ ] OpenAPI Specification v3.0.X
* [ ] OpenAPI Specification v3.0.0

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
roas = { version = "0.1.0", features = ["v2"] } 
```

## Examples

### v2.0

```rust
use roas::v2::spec::Spec;
use roas::validation::{Options, Validate};

...
let spec = serde_json::from_str::<Spec>(raw_json).unwrap();
spec.validate(Options::IgnoreMissingTags | Options::IgnoreExternalReferences).unwrap();
...

```
