# roas

**Rust libraries and a command-line tool for the API description formats that
sit around an API**: [OpenAPI](https://spec.openapis.org/oas/latest.html) for
the requests, [AsyncAPI](https://www.asyncapi.com/docs/reference/specification/v3.1.0)
for the messages, [Overlay](https://spec.openapis.org/overlay/latest.html) for
the edits you keep re-applying to them, and
[Arazzo](https://spec.openapis.org/arazzo/latest.html) for the workflows that
string their operations together — parsed into typed Rust, validated against
the specification, converted between versions, and, in Arazzo's case, actually
run.

Everything is available two ways: as a crate you build on, or as the `roas`
binary you use from a shell or a CI job.

## What it covers

| Specification       | Versions                           | What you can do                                                                                                                                                                                              | Crate                                                 |
|---------------------|------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------|
| **OpenAPI**         | 2.0 (Swagger), 3.0.x, 3.1.x, 3.2.x | parse · validate the description · validate every Schema Object · upconvert 2.0 → 3.0 → 3.1 → 3.2 · merge · collapse inline components · render docs                                                         | [`roas`](crates/roas)                                 |
| **AsyncAPI**        | 2.6, 3.0, 3.1                      | parse · validate, following every `$ref` to what it names and judging it against the kind of object that position holds · upconvert 3.0 → 3.1 · convert 2.6 → 3.0 with a report of what could not be carried | [`roas-asyncapi`](crates/roas-asyncapi)               |
| **Overlay**         | 1.0, 1.1                           | parse · validate · apply to a target document, with a report of what each action matched · upconvert 1.0 → 1.1                                                                                               | [`roas-overlay`](crates/roas-overlay)                 |
| **Arazzo**          | 1.0, 1.1                           | parse · validate · upconvert 1.0 → 1.1                                                                                                                                                                       | [`roas-arazzo`](crates/roas-arazzo)                   |
| **Arazzo, running** | 1.0, 1.1                           | perform every step's request, judge its criteria, follow its `retry` / `goto` / `end` actions and the workflows it calls, and report what happened                                                           | [`roas-arazzo-executor`](crates/roas-arazzo-executor) |

Where a specification and its published JSON Schema disagree, these crates
follow the **prose**; the schemas do not track the specifications one for one.
Such cases are called out in the crate that makes the choice.

## From the shell

[`roas-cli`](crates/roas-cli) ships a single `roas` binary:

```shell
cargo install roas-cli                                     # any platform with a Rust toolchain
brew install sv-tools/apps/roas                            # macOS arm64, Linux arm64 / x86_64
docker run --rm -v "$PWD:/specs" -w /specs ghcr.io/sv-tools/roas:latest openapi validate openapi.yaml
```

| Command                                                | What it does                                                                                                      |
|--------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------|
| `roas openapi validate\|convert\|preview`              | parse and validate an OpenAPI spec, upconvert it (merging, applying overlays and collapsing on the way), or render it in a browser |
| `roas overlay validate\|convert\|apply`                | work with Overlay documents; `apply` edits a spec of any version                                                  |
| `roas arazzo validate\|convert\|list\|run`             | work with Arazzo descriptions — `list` says what workflows one offers, `run` performs them                        |
| `roas asyncapi validate\|convert`                      | work with AsyncAPI documents; `convert --to v3_0` reports what 2.6 → 3.x could not carry                          |
| `roas completions <SHELL>` · `roas manpages --out DIR` | shell completions and troff manpages                                                                              |

Every command that reads a document takes JSON or YAML, from a file or stdin,
and writes in the format it read (`--output-format` overrides). Diagnostics go
to stderr and the document to stdout, so the commands pipe into one another —
and into `jq`:

```shell
roas openapi convert --to v3_2 spec.json | roas openapi validate --print | roas openapi preview
roas arazzo run buy.arazzo.yaml --load file --input petId=7 --output-format json | jq .orderId
```

The [`roas-cli` README](crates/roas-cli/README.md) has the full reference.

## From Rust

### [`roas`](crates/roas) — OpenAPI

One typed tree per version behind a feature flag (`v2`, `v3_0`, `v3_1`,
`v3_2`), round-tripping JSON and YAML without losing what it did not model.

- **Description validation** collects every diagnostic in one pass rather than
  stopping at the first: required and non-empty fields, `$ref` resolution, tag
  and `operationId` uniqueness, unused components, URL shapes. Each check is
  a flag in `validation::Options`.
- **Schema validation** covers the JSON Schema dialect each OAS version
  carries — `$ref` resolution, discriminator and mapping correctness, the
  per-keyword rules — and is usable on its own or as part of the description
  validator.
- **Conversion** is a chain of `From<v_X::Spec> for v_Y::Spec` migrations in
  pure Rust, so an upconvert is a total function rather than a script.
- **Merging and collapsing** combine several specs into one, and lift inline
  components into `components` / `definitions` with strict dedup.

### [`roas-asyncapi`](crates/roas-asyncapi) — AsyncAPI

The event-driven counterpart: channels, operations, messages and bindings for
2.6, 3.0 and 3.1. Reference resolution is model-driven — each type says what
kind of object it is, and a `$ref` is judged by where it sits, so a message
reference that names a schema is reported rather than followed. `v3_0::from_v2_6`
converts a 2.6 document to 3.0; v3 reshaped the document rather than extending
it, so the conversion names what it had to invent and what had nowhere to go.

### [`roas-overlay`](crates/roas-overlay) — Overlay

Parse and validate Overlay documents, and apply them to any JSON or YAML
target — the target is untyped, so an overlay written for one OpenAPI version
applies to another. `ApplyReport` says what each action matched, and an apply
that fails leaves the target untouched.

### [`roas-arazzo`](crates/roas-arazzo) — Arazzo

Workflow descriptions: sequences of API calls, their inputs and outputs, the
criteria that decide whether a step worked, and the actions that decide what
follows. v1.1 adds `$self`, AsyncAPI steps, selectors and expression types;
with both features on, a v1.0 description upconverts to v1.1.

### [`roas-arazzo-executor`](crates/roas-arazzo-executor) — Arazzo, running

Takes a description and performs it. The engine does no IO of its own: it
hands out a request and is handed a response, so `execute`, `execute_async`
and a scripted fake share one interpreter — and a retry's delay can be
asserted rather than spent. Runtime expressions, `simple` / `regex` /
`jsonpath` criteria, request bodies with replacements, sub-workflows and
`dependsOn` ordering are all covered; AsyncAPI steps, XPath, input-schema
validation and parallelism are reported where they are met rather than passed
over.

### [`roas-file-fetcher`](crates/roas-file-fetcher) · [`roas-http-fetcher`](crates/roas-http-fetcher) — loading

`roas` resolves external `$ref`s through `ResourceFetcher` /
`AsyncResourceFetcher`, and fetches nothing unless a fetcher is registered for
the scheme. These two cover the filesystem and HTTP(S), in blocking and async
forms, with optional YAML parsing — or implement the trait yourself for a
registry, a cache, or an embedded bundle.

## Crates

| Crate                                                 | Docs                                                                                               | crates.io                                                                                                               |
|-------------------------------------------------------|----------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| [`roas`](crates/roas)                                 | [![docs.rs](https://docs.rs/roas/badge.svg)](https://docs.rs/roas)                                 | [![crates.io](https://img.shields.io/crates/v/roas.svg)](https://crates.io/crates/roas)                                 |
| [`roas-asyncapi`](crates/roas-asyncapi)               | [![docs.rs](https://docs.rs/roas-asyncapi/badge.svg)](https://docs.rs/roas-asyncapi)               | [![crates.io](https://img.shields.io/crates/v/roas-asyncapi.svg)](https://crates.io/crates/roas-asyncapi)               |
| [`roas-overlay`](crates/roas-overlay)                 | [![docs.rs](https://docs.rs/roas-overlay/badge.svg)](https://docs.rs/roas-overlay)                 | [![crates.io](https://img.shields.io/crates/v/roas-overlay.svg)](https://crates.io/crates/roas-overlay)                 |
| [`roas-arazzo`](crates/roas-arazzo)                   | [![docs.rs](https://docs.rs/roas-arazzo/badge.svg)](https://docs.rs/roas-arazzo)                   | [![crates.io](https://img.shields.io/crates/v/roas-arazzo.svg)](https://crates.io/crates/roas-arazzo)                   |
| [`roas-arazzo-executor`](crates/roas-arazzo-executor) | [![docs.rs](https://docs.rs/roas-arazzo-executor/badge.svg)](https://docs.rs/roas-arazzo-executor) | [![crates.io](https://img.shields.io/crates/v/roas-arazzo-executor.svg)](https://crates.io/crates/roas-arazzo-executor) |
| [`roas-file-fetcher`](crates/roas-file-fetcher)       | [![docs.rs](https://docs.rs/roas-file-fetcher/badge.svg)](https://docs.rs/roas-file-fetcher)       | [![crates.io](https://img.shields.io/crates/v/roas-file-fetcher.svg)](https://crates.io/crates/roas-file-fetcher)       |
| [`roas-http-fetcher`](crates/roas-http-fetcher)       | [![docs.rs](https://docs.rs/roas-http-fetcher/badge.svg)](https://docs.rs/roas-http-fetcher)       | [![crates.io](https://img.shields.io/crates/v/roas-http-fetcher.svg)](https://crates.io/crates/roas-http-fetcher)       |
| [`roas-cli`](crates/roas-cli)                         | —                                                                                                  | [![crates.io](https://img.shields.io/crates/v/roas-cli.svg)](https://crates.io/crates/roas-cli)                         |

Each crate keeps its versions behind feature flags and pulls in only what it
needs: enable `v3_1` alone and no other OpenAPI version is compiled. Shared
dependency versions live in the root `Cargo.toml`.

## Contributing

See each crate's `README.md` for usage examples, and [`AGENTS.md`](AGENTS.md)
for the repository's conventions: crate layout, the build and test commands CI
runs, coding style, and how commits and pull requests are expected to look.

> [!CAUTION]
> The project is in early development; treat any `0.x.x` version as unstable
> and subject to breaking changes.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
