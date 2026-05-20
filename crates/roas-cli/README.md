# roas-cli

Command-line front-end for [`roas`](https://crates.io/crates/roas): validate and convert OpenAPI specs across versions 2.0 / 3.0.x / 3.1.x / 3.2.x.

[![crates.io](https://img.shields.io/crates/v/roas-cli.svg)](https://crates.io/crates/roas-cli)

## Install

The installed binary is named `roas` (the crate is `roas-cli`).

### Cargo

```shell
cargo install roas-cli
```

### Homebrew

```shell
brew install sv-tools/apps/roas
```

The tap is [`sv-tools/homebrew-apps`](https://github.com/sv-tools/homebrew-apps); the formula tracks the latest published release. macOS arm64 and Linux (arm64 / x86_64) only — Intel macOS users should `cargo install` or use Docker.

### Docker

Multi-arch image (`linux/amd64`, `linux/arm64`):

```shell
docker run --rm -v "$PWD:/specs" -w /specs ghcr.io/sv-tools/roas:latest validate openapi.yaml
```

Pinned versions: `ghcr.io/sv-tools/roas:<version>` — see the [GitHub Releases](https://github.com/sv-tools/roas/releases). The image's entrypoint is the `roas` binary, so any subcommand and flags follow `docker run ... ghcr.io/sv-tools/roas:<tag>`.

## Usage

```shell
roas validate [FILE]            # parse + validate
roas convert --to v3_2 [FILE]   # upconvert across versions
roas preview [FILE]             # open the spec in a browser via Redoc
```

Input can be JSON or YAML. With a file path, the parser is selected by extension (`.yaml` / `.yml` → YAML, everything else → JSON). Pass `-` as the file path, or omit it entirely and pipe the spec, to read from stdin; stdin defaults to JSON. `--format json|yaml` overrides everything.

### Piping specs

Every subcommand accepts the spec on stdin, so they chain naturally. `validate` is silent on stdout by default — pass `--print` to echo the parsed spec downstream:

```shell
cat spec.json | roas validate                           # auto: piped stdin
cat spec.yaml | roas validate --format yaml             # stdin defaults to JSON; override
roas convert --to v3_2 spec.json | roas validate --print | roas preview
```

`preview --watch` requires a real file and is rejected for stdin input.

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

Pass `--print` to echo the parsed spec on stdout (diagnostics stay on stderr), so `validate` can sit in the middle of a pipeline. The output format matches the input: YAML in → YAML out, JSON in → JSON out.

### `convert`

Upconverts a spec to a target version by chaining the existing `From<v_X::Spec> for v_Y::Spec` migrations. Downconversion is not supported.

```shell
roas convert --to v3_2 spec.json                          # JSON in → JSON out
roas convert --to v3_2 spec.yaml                          # YAML in → YAML out
roas convert --to v3_2 --output-format yaml spec.json     # switch format
roas convert --to v3_1 --from v2 spec.yaml
```

Output goes to stdout. The format matches the input by default (YAML in → YAML out, JSON in → JSON out); pass `--output-format json|yaml` to override.

### `preview`

Starts a local HTTP server on `127.0.0.1:<random>` that serves the spec, embedded inside an HTML page rendered with either [Redoc](https://redocly.com/redoc) (default) or [Swagger UI](https://swagger.io/tools/swagger-ui/), and opens the default browser pointed at it. Pass `--no-open` to skip the browser launch (the URL is printed to stderr in either case). Ctrl+C tears the server down.

```shell
roas preview spec.yaml                               # Redoc (default)
roas preview --renderer swagger-ui spec.yaml         # Swagger UI
roas preview --watch spec.yaml                       # live-reload on file change
roas preview --no-open --from v3_1 spec.json
```

`--watch` watches the spec file and pushes a Server-Sent-Events reload to the browser on every change; the page reloads itself and re-fetches `/spec`. If a write produces a parse error the previous good JSON is kept and the error is logged to stderr. `--watch` requires a real file — stdin input is rejected. Both renderers target OpenAPI 3.0 / 3.1 today — v3.2-specific fields are skipped silently. To preview an older spec under a v3.0+ renderer, upconvert it once with `roas convert --to v3_1 spec.json` and serve the result.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
