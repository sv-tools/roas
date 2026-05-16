# roas-cli

Command-line front-end for [`roas`](https://crates.io/crates/roas): validate and convert OpenAPI specs across versions 2.0 / 3.0.x / 3.1.x / 3.2.x.

[![crates.io](https://img.shields.io/crates/v/roas-cli.svg)](https://crates.io/crates/roas-cli)

## Install

```shell
cargo install roas-cli
```

The installed binary is named `roas`.

## Usage

```shell
roas validate <FILE>            # parse + validate
roas convert --to v3_2 <FILE>   # upconvert across versions
roas preview <FILE>             # open the spec in a browser via Redoc
```

Input can be JSON or YAML; the parser is selected by file extension (`.yaml` / `.yml` → YAML, everything else → JSON).

### `validate`

Auto-detects the spec version from the `openapi` / `swagger` field; pass `--from` to force. External `$ref`s are skipped by default; opt in with `--load`:

```shell
roas validate spec.yaml                   # local refs only
roas validate --load file spec.yaml       # follow `file://` $refs
roas validate --load http spec.yaml       # follow `http(s)://` $refs
roas validate --load file --load http spec.yaml  # both
```

`--ignore <CHECK>` skips a specific validation check; repeat the flag to skip more than one. The list is sourced from `roas::validation::Options` (via roas's `clap` feature), so it stays in sync with the library:

```
missing-tags, external-references, invalid-urls, non-uniq-operation-ids,
unused-path-items, unused-tags, unused-schemas, unused-parameters,
unused-responses, unused-server-variables, unused-examples,
unused-request-bodies, unused-headers, unused-security-schemes,
unused-links, unused-callbacks, unused-media-types,
empty-info-title, empty-info-version, empty-response-description,
empty-external-documentation-url
```

Run `roas validate --help` for descriptions of each check.

### `convert`

Upconverts a spec to a target version by chaining the existing `From<v_X::Spec> for v_Y::Spec` migrations. Downconversion is not supported.

```shell
roas convert --to v3_2 spec.json
roas convert --to v3_1 --from v2 spec.yaml
```

Output is JSON on stdout.

### `preview`

Starts a local HTTP server on `127.0.0.1:<random>` that serves the spec, embedded inside an HTML page rendered with [Redoc](https://redocly.com/redoc), and opens the default browser pointed at it. Pass `--no-open` to skip the browser launch (the URL is printed to stderr in either case). Ctrl+C tears the server down.

```shell
roas preview spec.yaml
roas preview --no-open --from v3_1 spec.json
```

Redoc currently targets OpenAPI 3.0 / 3.1 — v3.2-specific fields are skipped silently by the renderer.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
