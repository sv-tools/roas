# roas-cli

Command-line front-end for [`roas`](https://crates.io/crates/roas): validate and convert OpenAPI specs across versions 2.0 / 3.0.x / 3.1.x / 3.2.x, and validate / convert / apply [OpenAPI Overlay](https://spec.openapis.org/overlay/v1.0.0.html) documents (v1.0 / v1.1) via [`roas-overlay`](https://crates.io/crates/roas-overlay).

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
roas validate [FILE]                       # parse + validate an OpenAPI spec
roas convert --to v3_2 [FILE]              # upconvert across versions
roas overlay validate [FILE]               # validate an OpenAPI Overlay document
roas overlay convert --to v1_1 [FILE]      # upconvert an overlay
roas overlay apply --overlay O.yaml [SPEC] # apply overlay(s) to a spec
roas preview [FILE]                        # open the spec in a browser via Redoc
```

The root `validate` and `convert` commands operate on OpenAPI specs; the `overlay` subcommand group operates on OpenAPI Overlay documents.

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

`--merge <FILE>` (repeatable) layers additional specs on top after conversion. Each merge source is loaded with the same format-detection rules as the base, upconverted to the target version, then merged in incoming-order via `roas::merge`. The merge runs *after* the version conversion and *before* `--collapse`. By default the merge is incoming-wins on scalar conflicts, base keeps its `info` / `openapi`, refs replace silently, and schemas are leaf-replaced. `--merge-option` (repeatable) tunes that:

```shell
roas convert --to v3_2 --merge errors.yaml --merge auth.yaml base.json
roas convert --to v3_2 --merge layer.yaml --merge-option base-wins spec.json
roas convert --to v3_2 --merge layer.yaml --merge-option error-on-conflict spec.json
roas convert --to v3_2 --merge layer.yaml --merge-option deep-merge-object-schemas spec.json
roas convert --to v3_2 --merge layer.yaml --merge-option merge-info spec.json
```

Supported `--merge-option` values: `base-wins`, `error-on-conflict`, `deep-merge-object-schemas`, `merge-info`, `replace-lists-when-empty`. Under `error-on-conflict` the first real collision aborts the merge and `roas` exits non-zero with the conflicting path; the base spec is untouched on error.

`--apply <FILE>` (repeatable) applies OpenAPI Overlay documents to the converted spec. Each overlay is loaded with extension-based format detection, its version detected from the `overlay` field, and applied via [`roas-overlay`](https://crates.io/crates/roas-overlay). The full convert pipeline is **convert → `--merge` → `--apply` → `--collapse`** — overlays apply before collapse so overlay-introduced inline components are lifted into `$ref`s too. (When `--apply` and `--collapse` are combined, the overlaid spec is re-parsed at the target version before collapsing, so it must still be a valid OpenAPI document.) `--apply-option` (repeatable) tunes the apply (`error-on-zero-match`, `error-on-mixed-kind-match`):

```shell
roas convert --to v3_2 --apply patch.yaml spec.json
roas convert --to v3_2 --merge layer.yaml --apply patch.yaml --collapse spec.json
roas convert --to v3_2 --apply patch.yaml --apply-option error-on-zero-match spec.json
```

### `overlay`

Work with [OpenAPI Overlay](https://spec.openapis.org/overlay/v1.0.0.html) documents (v1.0 and v1.1). The overlay version is auto-detected from the `overlay` field.

```shell
roas overlay validate overlay.yaml                          # parse + validate
roas overlay convert --to v1_1 overlay.json                 # upconvert v1.0 → v1.1
roas overlay apply --overlay patch.yaml spec.json           # apply to a spec
cat spec.json | roas overlay apply --overlay patch.yaml     # spec on stdin
roas overlay apply --overlay a.yaml --overlay b.yaml spec.json | roas validate
```

- **`overlay validate`** — checks structure: the `overlay` version, non-empty `actions`, valid RFC 9535 JSONPath in every `target` (and `copy`), and the mutual-exclusivity rules. `--ignore <CHECK>` skips a check (`empty-info-title`, `empty-info-version`); `--print` echoes the parsed overlay.
- **`overlay convert --to <v1_0|v1_1>`** — upconverts an overlay. Only upconversion is supported (v1.0 → v1.1 adds the `copy` action and `info.description`); downconversion errors.
- **`overlay apply`** — applies overlay(s) to a target spec. The spec is the positional argument (or stdin); `--overlay <FILE>` (repeatable, at least one required) names the overlay(s), applied in order. The spec is treated as untyped JSON, so this works for any OpenAPI version. `--apply-option` (repeatable) accepts `error-on-zero-match` and `error-on-mixed-kind-match`. On any apply error the spec is left untouched and `roas` exits non-zero.

### `preview`

Starts a local HTTP server on `127.0.0.1:<random>` that serves the spec, embedded inside an HTML page rendered with either [Redoc](https://redocly.com/redoc) (default) or [Swagger UI](https://swagger.io/tools/swagger-ui/), and opens the default browser pointed at it. Pass `--no-open` to skip the browser launch (the URL is printed to stderr in either case). Ctrl+C tears the server down.

```shell
roas preview spec.yaml                               # Redoc (default)
roas preview --renderer swagger-ui spec.yaml         # Swagger UI
roas preview --watch spec.yaml                       # live-reload on file change
roas preview --no-open --from v3_1 spec.json
```

`--watch` watches the spec file and pushes a Server-Sent-Events reload to the browser on every change; the page reloads itself and re-fetches `/spec`. If a write produces a parse error the previous good JSON is kept and the error is logged to stderr. `--watch` requires a real file — stdin input is rejected. Both renderers target OpenAPI 3.0 / 3.1 today — v3.2-specific fields are skipped silently. To preview an older spec under a v3.0+ renderer, upconvert it once with `roas convert --to v3_1 spec.json` and serve the result.

### `completions`

Prints a shell completion script to stdout. Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`. Pipe the output into the location your shell expects:

```shell
roas completions bash       > /etc/bash_completion.d/roas
roas completions zsh        > "${fpath[1]}/_roas"
roas completions fish       > ~/.config/fish/completions/roas.fish
```

The Homebrew formula auto-installs completions for bash/zsh/fish; the Docker image carries the same `completions` subcommand if you need to extract scripts in containerised builds.

### `manpages`

Generates troff manpages — top-level `roas.1` plus one per subcommand (`roas-validate.1`, `roas-convert.1`, `roas-preview.1`, …) — into an output directory:

```shell
roas manpages --out /tmp/man
man /tmp/man/roas-validate.1
```

For a system-wide install: `roas manpages --out "$(brew --prefix)/share/man/man1"` (Homebrew), or `roas manpages --out ~/.local/share/man/man1` for a per-user install. The Homebrew formula installs these automatically.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
