# roas

Rust OpenAPI Specification (v2.0, v3.0.X, v3.1.X, v3.2.X) parser, validator, and loader, plus a command-line
front-end.

This repository is a Cargo workspace with three published crates:

| Crate | Description | crates.io |
| --- | --- | --- |
| [`roas`](crates/roas) | Library. Parses, validates, and loads OpenAPI specs. Version-gated via `v2` / `v3_0` / `v3_1` / `v3_2` features. | [![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas) |
| [`roas-cli`](crates/roas-cli) | Command-line front-end. Installs the `roas` binary. Provides `validate` and `convert` subcommands. | [![crates.io](https://img.shields.io/crates/v/roas-cli.svg)](https://crates.io/crates/roas-cli) |
| [`roas-http-fetcher`](crates/roas-http-fetcher) | `http://` / `https://` fetcher for the loader. Wraps `reqwest::blocking`. | [![crates.io](https://img.shields.io/crates/v/roas-http-fetcher.svg)](https://crates.io/crates/roas-http-fetcher) |

Supported specifications:

* [x] OpenAPI Specification [v2.0](https://spec.openapis.org/oas/v2.0.html) (feature `v2`)
* [x] OpenAPI Specification [v3.0.x](https://spec.openapis.org/oas/v3.0.4.html) (feature `v3_0`)
* [x] OpenAPI Specification [v3.1.x](https://spec.openapis.org/oas/v3.1.2.html) (feature `v3_1`)
* [x] OpenAPI Specification [v3.2.x](https://spec.openapis.org/oas/v3.2.0.html) (feature `v3_2`, **default**)

> [!CAUTION]
> The project is in early development stage, so the API may change in the future.
> Consider any 0.x.x version as unstable and subject to breaking changes.

## Library quick start

```shell
cargo add roas
```

```rust
use roas::v3_2::spec::Spec;
use roas::validation::{Options, Validate};

let raw_json = r#"{ "openapi": "3.2.0", "info": { "title": "demo", "version": "1" }, "paths": {} }"#;
let spec: Spec = serde_json::from_str(raw_json).unwrap();
spec.validate(Options::IgnoreMissingTags | Options::IgnoreExternalReferences, None).unwrap();
```

The second argument to `validate` is an optional
[`Loader`](https://docs.rs/roas/latest/roas/loader/struct.Loader.html) that resolves external `$ref`s. See
[crates/roas/README.md](crates/roas/README.md) for more.

## CLI quick start

```shell
cargo install roas-cli

roas validate spec.json
roas convert --to v3.2 swagger.json
```

See [crates/roas-cli/README.md](crates/roas-cli/README.md) for the full command surface.

## Development

This workspace uses Cargo's standard layout:

```shell
cargo install-tools                            # one-time: cargo-nextest, cargo-deny, cargo-llvm-cov, cargo-machete
cargo build                                    # build the whole workspace
cargo nextest run --workspace --all-features   # run the test suite
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo fmt --all
```

Contributor guidelines and conventions are in [`AGENTS.md`](AGENTS.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
