# roas

Rust OpenAPI Specification (v2.0, v3.0.X and v3.1.X)

[![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)
[![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)

Parsing and generating OpenAPI Specification:

* [x] OpenAPI Specification v2.0 (**v2**: old specification, disabled by default)
* [x] OpenAPI Specification v3.0.x (**v3_0**: default feature)
* [x] OpenAPI Specification v3.1.x (**v3_1**:; experimental and disabled by default)

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
roas = "0.4"  
```

## Examples

```rust
use roas::v3_1::spec::Spec;
use roas::validation::{Options, Validate};

...

let spec = serde_json::from_str::<Spec>(raw_json).unwrap();
spec.validate(Options::IgnoreMissingTags | Options::IgnoreExternalReferences).unwrap();

...

```
