# roas-cli

Command-line front-end for [`roas`](https://crates.io/crates/roas): validate and convert OpenAPI specs across versions 2.0 / 3.0.x / 3.1.x / 3.2.x, validate / convert / apply [OpenAPI Overlay](https://spec.openapis.org/overlay/v1.0.0.html) documents (v1.0 / v1.1) via [`roas-overlay`](https://crates.io/crates/roas-overlay), validate / convert [OpenAPI Arazzo](https://spec.openapis.org/arazzo/v1.0.1.html) workflow descriptions (v1.0 / v1.1) via [`roas-arazzo`](https://crates.io/crates/roas-arazzo), and validate / convert [AsyncAPI](https://www.asyncapi.com/docs/reference/specification/v3.1.0) documents (2.6 / 3.0 / 3.1) via [`roas-asyncapi`](https://crates.io/crates/roas-asyncapi).

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
docker run --rm -v "$PWD:/specs" -w /specs ghcr.io/sv-tools/roas:latest openapi validate openapi.yaml
```

Pinned versions: `ghcr.io/sv-tools/roas:<version>` — see the [GitHub Releases](https://github.com/sv-tools/roas/releases). The image's entrypoint is the `roas` binary, so any subcommand and flags follow `docker run ... ghcr.io/sv-tools/roas:<tag>`.

## Stability

From 1.0 the command-line surface follows semver: the commands, their flags, and
the meaning of exit codes do not change incompatibly without a major bump. A new
subcommand, a new optional flag, or a new value for an existing flag is a minor
release.

Not covered: the wording of diagnostics and reports, which is written for people
and may be reworded in any release, and the versions of the `roas*` libraries the
binary is built against, which are an implementation detail — parse the output
only where a flag documents a machine-readable format.

## Usage

```shell
roas openapi validate [FILE]               # parse + validate an OpenAPI spec
roas openapi convert --to v3_2 [FILE]      # upconvert across versions
roas openapi preview [FILE]                # open the spec in a browser via Redoc
roas overlay validate [FILE]               # validate an OpenAPI Overlay document
roas overlay convert --to v1_1 [FILE]      # upconvert an overlay
roas overlay apply --overlay O.yaml [SPEC] # apply overlay(s) to a spec
roas arazzo validate [FILE]                # validate an OpenAPI Arazzo description
roas arazzo convert --to v1_1 [FILE]       # upconvert an Arazzo description
roas arazzo list [FILE]                    # what workflows a description offers
roas arazzo run --workflow ID [FILE]       # run one against a real API
roas asyncapi validate [FILE]              # validate an AsyncAPI document
roas asyncapi convert --to v3_1 [FILE]     # upconvert an AsyncAPI document
```

One subcommand group per specification: `openapi` for OpenAPI specs, `overlay` for OpenAPI Overlay documents, `arazzo` for OpenAPI Arazzo workflow descriptions, and `asyncapi` for AsyncAPI documents.

Input can be JSON or YAML. With a file path, the parser is selected by extension (`.yaml` / `.yml` → YAML, everything else → JSON). Pass `-` as the file path, or omit it entirely and pipe the spec, to read from stdin; stdin defaults to JSON. `--format json|yaml` overrides everything.

### Piping specs

Every subcommand accepts the spec on stdin, so they chain naturally. `validate` is silent on stdout by default — pass `--print` to echo the parsed spec downstream:

```shell
cat spec.json | roas openapi validate                    # auto: piped stdin
cat spec.yaml | roas openapi validate --format yaml      # stdin defaults to JSON; override
roas openapi convert --to v3_2 spec.json | roas openapi validate --print | roas openapi preview
```

`openapi preview --watch` requires a real file and is rejected for stdin input.

### `openapi validate`

Auto-detects the spec version from the `openapi` / `swagger` field; pass `--from` to force. External `$ref`s are skipped by default; opt in with `--load`:

```shell
roas openapi validate spec.yaml                   # local refs only
roas openapi validate --load file spec.yaml       # follow `file://` $refs
roas openapi validate --load http spec.yaml       # follow `http(s)://` $refs
roas openapi validate --load file --load http spec.yaml  # both
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

Run `roas openapi validate --help` for descriptions of each check.

Pass `--print` to echo the parsed spec on stdout (diagnostics stay on stderr), so `validate` can sit in the middle of a pipeline. The output format matches the input: YAML in → YAML out, JSON in → JSON out.

### `openapi convert`

Upconverts a spec to a target version by chaining the existing `From<v_X::Spec> for v_Y::Spec` migrations. Downconversion is not supported.

```shell
roas openapi convert --to v3_2 spec.json                          # JSON in → JSON out
roas openapi convert --to v3_2 spec.yaml                          # YAML in → YAML out
roas openapi convert --to v3_2 --output-format yaml spec.json     # switch format
roas openapi convert --to v3_1 --from v2 spec.yaml
```

Output goes to stdout. The format matches the input by default (YAML in → YAML out, JSON in → JSON out); pass `--output-format json|yaml` to override.

`--merge <FILE>` (repeatable) layers additional specs on top after conversion. Each merge source is loaded with the same format-detection rules as the base, upconverted to the target version, then merged in incoming-order via `roas::merge`. The merge runs *after* the version conversion and *before* `--collapse`. By default the merge is incoming-wins on scalar conflicts, base keeps its `info` / `openapi`, refs replace silently, and schemas are leaf-replaced. `--merge-option` (repeatable) tunes that:

```shell
roas openapi convert --to v3_2 --merge errors.yaml --merge auth.yaml base.json
roas openapi convert --to v3_2 --merge layer.yaml --merge-option base-wins spec.json
roas openapi convert --to v3_2 --merge layer.yaml --merge-option error-on-conflict spec.json
roas openapi convert --to v3_2 --merge layer.yaml --merge-option deep-merge-object-schemas spec.json
roas openapi convert --to v3_2 --merge layer.yaml --merge-option merge-info spec.json
```

Supported `--merge-option` values: `base-wins`, `error-on-conflict`, `deep-merge-object-schemas`, `merge-info`, `replace-lists-when-empty`. Under `error-on-conflict` the first real collision aborts the merge and `roas` exits non-zero with the conflicting path; the base spec is untouched on error.

`--apply <FILE>` (repeatable) applies OpenAPI Overlay documents to the converted spec. Each overlay is loaded with extension-based format detection, its version detected from the `overlay` field, and applied via [`roas-overlay`](https://crates.io/crates/roas-overlay). The full convert pipeline is **convert → `--merge` → `--apply` → `--collapse`** — overlays apply before collapse so overlay-introduced inline components are lifted into `$ref`s too. (When `--apply` and `--collapse` are combined, the overlaid spec is re-parsed at the target version before collapsing, so it must still be a valid OpenAPI document.) `--apply-option` (repeatable) tunes the apply (`error-on-zero-match`, `error-on-mixed-kind-match`):

```shell
roas openapi convert --to v3_2 --apply patch.yaml spec.json
roas openapi convert --to v3_2 --merge layer.yaml --apply patch.yaml --collapse spec.json
roas openapi convert --to v3_2 --apply patch.yaml --apply-option error-on-zero-match spec.json
```

### `overlay`

Work with [OpenAPI Overlay](https://spec.openapis.org/overlay/v1.0.0.html) documents (v1.0 and v1.1). The overlay version is auto-detected from the `overlay` field.

```shell
roas overlay validate overlay.yaml                          # parse + validate
roas overlay convert --to v1_1 overlay.json                 # upconvert v1.0 → v1.1
roas overlay apply --overlay patch.yaml spec.json           # apply to a spec
cat spec.json | roas overlay apply --overlay patch.yaml     # spec on stdin
roas overlay apply --overlay a.yaml --overlay b.yaml spec.json | roas openapi validate
```

- **`overlay validate`** — checks structure: the `overlay` version, non-empty `actions`, valid RFC 9535 JSONPath in every `target` (and `copy`), and the mutual-exclusivity rules. `--ignore <CHECK>` skips a check (`empty-info-title`, `empty-info-version`); `--print` echoes the parsed overlay.
- **`overlay convert --to <v1_0|v1_1>`** — upconverts an overlay. Only upconversion is supported (v1.0 → v1.1 adds the `copy` action and `info.description`); downconversion errors.
- **`overlay apply`** — applies overlay(s) to a target spec. The spec is the positional argument (or stdin); `--overlay <FILE>` (repeatable, at least one required) names the overlay(s), applied in order. The spec is treated as untyped JSON, so this works for any OpenAPI version. `--apply-option` (repeatable) accepts `error-on-zero-match` and `error-on-mixed-kind-match`. On any apply error the spec is left untouched and `roas` exits non-zero.

### `arazzo`

Work with [OpenAPI Arazzo](https://spec.openapis.org/arazzo/v1.0.1.html) workflow descriptions (v1.0 and v1.1). The Arazzo version is auto-detected from the `arazzo` field. Arazzo *describes* sequences of API calls rather than transforming a spec, so there is no `apply`.

```shell
roas arazzo validate workflow.yaml             # parse + validate
roas arazzo convert --to v1_1 workflow.json    # upconvert v1.0 → v1.1
cat workflow.json | roas arazzo validate       # description on stdin
```

- **`arazzo validate`** — checks structure: required / non-empty fields, source-name and component/output-key patterns, uniqueness of source names / workflow ids / step ids, the step shape rules (OpenAPI / AsyncAPI / Workflow), criterion type / context / version constraints, and `goto` action targets. `--ignore <CHECK>` skips a check (`empty-info-title`, `empty-info-version`); `--print` echoes the parsed description.
- **`arazzo convert --to <v1_0|v1_1>`** — upconverts a description. Only upconversion is supported (v1.0 → v1.1 adds `$self`, AsyncAPI steps, selectors, expression types, and action `parameters`); downconversion errors.
- **`arazzo list`** — prints each workflow's id, summary, `dependsOn`, inputs (with types, and which are required) and steps. What to pass to `arazzo run --workflow`, and what inputs it wants.

```text
buyPet
  summary: Find then order a pet
  depends on: authenticate
  inputs: petId (string, required), token (string)
  steps: 2 (findPet, orderPet)
```

- **`arazzo run`** — runs a workflow via [`roas-arazzo-executor`](https://crates.io/crates/roas-arazzo-executor): every step's request, its criteria, its `retry` / `goto` / `end` actions, and the workflows it calls. This is the one command that talks to an API rather than reading a document.

```shell
roas arazzo run buy.arazzo.yaml --load file --input petId=7
roas arazzo run buy.arazzo.yaml --source petStore=./openapi.yaml --base-url petStore=http://127.0.0.1:8080
roas arazzo run buy.arazzo.yaml --workflow buyPet --inputs inputs.yaml --header 'Authorization: Bearer …'
```

```text
workflow `buyPet` succeeded
- findPet GET http://127.0.0.1:8080/pets/7 → 200
- orderPet POST http://127.0.0.1:8080/orders → 201
  orderId = "o-1"
  petName = "fluffy"
```

`--workflow <ID>` says which workflow to run. A description with one workflow needs no flag; with several the command stops and lists them rather than picking, since running the wrong one means real requests against a real API. A workflow's `dependsOn` still pulls in what it needs, in order, and those steps appear in the report under their own workflow.

It needs the source descriptions the steps point at: name them with `--source <name>=<path>`, or let `--load file` / `--load http` fetch them by the URLs the description gives (a relative URL is read from beside the description). `--input name=value` sets one input — read as JSON where it is JSON, so `--input n=7` is a number and `--input n=seven` a string — and `--inputs <FILE>` takes an object of them. `--base-url <name>=<url>` sends a source's requests elsewhere, which is how a workflow written against production runs against a test server. `--max-steps` bounds a `goto` that loops.

The description is validated before anything is sent — a run makes real requests, and a description that does not hold together should not make them. `--ignore <CHECK>` lets one pass, as `arazzo validate --ignore` does.

The report goes to **stderr** and the workflow's outputs to **stdout**, so the outputs pipe onward; `--quiet` silences the report. The exit status follows the workflow: non-zero when it failed.

### `asyncapi`

Work with [AsyncAPI](https://www.asyncapi.com/docs/reference/specification/v3.1.0) documents (2.6, 3.0 and 3.1). The version is auto-detected from the `asyncapi` field. AsyncAPI describes an event-driven API rather than transforming a spec, so there is no `apply`.

```shell
roas asyncapi validate events.yaml               # parse + validate
roas asyncapi convert --to v3_0 events26.yaml    # upconvert 2.6 → 3.0
roas asyncapi convert --to v3_1 --strict e.json  # fail if anything is lost
cat events.json | roas asyncapi validate         # document on stdin
```

- **`asyncapi validate`** — checks structure and cross-references: required / non-empty fields, server and channel wiring, operation and reply targets, message and schema references (every `$ref` is followed to what it names, and judged against the kind of object that position holds), and channel parameters against the address. `--check <CHECK>` adjusts one check — `empty-info-title`, `empty-info-version` and `unused-channel-parameter` relax one, `external-reference` adds one, requiring the document to be self-contained. `--print` echoes the parsed document.
- **`asyncapi convert --to <v2_6|v3_0|v3_1>`** — upconverts a document; downconversion errors. 3.0 → 3.1 is the object model unchanged. **2.6 → 3.x is lossy**: v3 keys a channel by name and carries the address inside it, moves operations to a map of their own and states them from the application's point of view (`publish` → `receive`, `subscribe` → `send`), and gives a parameter no schema. The conversion invents the names v3 needs and reports every one of them, along with everything it could not carry, to stderr — stdout is the document alone, so a pipeline is unaffected. `--strict` turns any such note into a failure (nothing is written to stdout), and `--quiet` silences the report.

### `openapi preview`

Starts a local HTTP server on `127.0.0.1:<random>` that serves the spec, embedded inside an HTML page rendered with either [Redoc](https://redocly.com/redoc) (default) or [Swagger UI](https://swagger.io/tools/swagger-ui/), and opens the default browser pointed at it. Pass `--no-open` to skip the browser launch (the URL is printed to stderr in either case). Ctrl+C tears the server down.

```shell
roas openapi preview spec.yaml                               # Redoc (default)
roas openapi preview --renderer swagger-ui spec.yaml         # Swagger UI
roas openapi preview --watch spec.yaml                       # live-reload on file change
roas openapi preview --no-open --from v3_1 spec.json
```

`--watch` watches the spec file and pushes a Server-Sent-Events reload to the browser on every change; the page reloads itself and re-fetches `/spec`. If a write produces a parse error the previous good JSON is kept and the error is logged to stderr. `--watch` requires a real file — stdin input is rejected. Both renderers target OpenAPI 3.0 / 3.1 today — v3.2-specific fields are skipped silently. To preview an older spec under a v3.0+ renderer, upconvert it once with `roas openapi convert --to v3_1 spec.json` and serve the result.

### `completions`

Prints a shell completion script to stdout. Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`. Pipe the output into the location your shell expects:

```shell
roas completions bash       > /etc/bash_completion.d/roas
roas completions zsh        > "${fpath[1]}/_roas"
roas completions fish       > ~/.config/fish/completions/roas.fish
```

The Homebrew formula auto-installs completions for bash/zsh/fish; the Docker image carries the same `completions` subcommand if you need to extract scripts in containerised builds.

### `manpages`

Generates troff manpages — top-level `roas.1`, one per group (`roas-openapi.1`, `roas-arazzo.1`, …) and one per command inside a group (`roas-openapi-validate.1`, `roas-arazzo-run.1`, …) — into an output directory:

```shell
roas manpages --out /tmp/man
man /tmp/man/roas-openapi-validate.1
```

For a system-wide install: `roas manpages --out "$(brew --prefix)/share/man/man1"` (Homebrew), or `roas manpages --out ~/.local/share/man/man1` for a per-user install. The Homebrew formula installs these automatically.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
