# roas

`roas` is a Rust **SDK and command-line tool** for the OpenAPI Specification:
parse, validate, convert, and round-trip OpenAPI / Swagger documents from
Rust code *or* from the shell. Every released OpenAPI version is supported:
v2.0 (Swagger), v3.0.x, v3.1.x, and v3.2.x.

## Use it as a CLI

[`roas-cli`](crates/roas-cli) ships a `roas` binary with `validate`,
`convert`, and `preview` subcommands. Install via Cargo, Homebrew, or
Docker — pick whichever fits the host:

```shell
cargo install roas-cli                                                # any platform with a Rust toolchain
brew install sv-tools/apps/roas                                       # macOS arm64, Linux
docker run --rm -v "$PWD:/specs" -w /specs ghcr.io/sv-tools/roas:latest validate openapi.yaml
```

See the [`roas-cli` README](crates/roas-cli/README.md) for the full
subcommand reference, piping examples, and the live-reload preview server.

## Use it as a Rust SDK

- **Parsers and serialisers** — deserialise OpenAPI documents from JSON or YAML
  into strongly-typed Rust structs (one type tree per spec version) and
  serialise them back with full round-trip fidelity.
- **Description validators** — validate that an OpenAPI description conforms
  to its specification version: required fields, `$ref` resolution, tag /
  `operationId` uniqueness, unused-component detection, and more. Each check
  is independently togglable via the `validation::Options` enum.
- **Schema validators** — every `Schema Object` (the JSON Schema dialect for
  the matching OAS version) is structurally validated, including `$ref`
  resolution, discriminator / mapping correctness, and the per-keyword rules
  the spec mandates. The schema validator is exercised as part of the larger
  description validator, and also reusable on its own.
- **Version converters** — upconvert OpenAPI descriptions across major
  versions: v2.0 → v3.0.x → v3.1.x → v3.2.x. A chain of `From<v_X::Spec> for
  v_Y::Spec` migrations performs the conversion in pure Rust; the same
  converters are exposed as a CLI sub-command via [`roas-cli`](crates/roas-cli).
- **Pluggable loader** — `ResourceFetcher` / `AsyncResourceFetcher` traits
  for resolving external `$ref`s, with first-party fetcher crates for
  [filesystem](crates/roas-file-fetcher) and [HTTP](crates/roas-http-fetcher)
  sources (JSON or YAML bodies, optional async).

## Crates

| Crate                                           | Docs                                                                                         | crates.io                                                                                                         |
|-------------------------------------------------|----------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------|
| [`roas`](crates/roas)                           | [![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)                           | [![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)                           |
| [`roas-file-fetcher`](crates/roas-file-fetcher) | [![docs.rs](https://docs.rs/roas-file-fetcher/badge.svg)](https://docs.rs/roas-file-fetcher) | [![crates.io](https://img.shields.io/crates/v/roas-file-fetcher.svg)](https://crates.io/crates/roas-file-fetcher) |
| [`roas-http-fetcher`](crates/roas-http-fetcher) | [![docs.rs](https://docs.rs/roas-http-fetcher/badge.svg)](https://docs.rs/roas-http-fetcher) | [![crates.io](https://img.shields.io/crates/v/roas-http-fetcher.svg)](https://crates.io/crates/roas-http-fetcher) |
| [`roas-cli`](crates/roas-cli)                   | —                                                                                            | [![crates.io](https://img.shields.io/crates/v/roas-cli.svg)](https://crates.io/crates/roas-cli)                   |

## OpenAPI versions

| Spec      | Status |
|-----------|--------|
| OpenAPI [v2.0](https://spec.openapis.org/oas/v2.0.html) (Swagger) | parser, description validator, schema validator, converter to v3, documentation rendering via `roas preview` |
| OpenAPI [v3.0.x](https://spec.openapis.org/oas/v3.0.4.html)       | parser, description validator, schema validator, converter to v3.1 / v3.2, documentation rendering via `roas preview` |
| OpenAPI [v3.1.x](https://spec.openapis.org/oas/v3.1.2.html)       | parser, description validator, schema validator, converter to v3.2, documentation rendering via `roas preview` |
| OpenAPI [v3.2.x](https://spec.openapis.org/oas/v3.2.0.html)       | parser, description validator, schema validator, documentation rendering via `roas preview` (target of all upconverters) |

See each crate's `README.md` for usage examples, and `AGENTS.md` at the
repository root for contributor guidelines.

> [!CAUTION]
> The project is in early development; treat any `0.x.x` version as unstable
> and subject to breaking changes.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
