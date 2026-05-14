# roas-cli

Command-line front-end for the [`roas`](https://crates.io/crates/roas) OpenAPI library.

[![crates.io](https://img.shields.io/crates/v/roas-cli.svg)](https://crates.io/crates/roas-cli)

Installs as the `roas` binary.

## Install

```shell
cargo install roas-cli
```

Homebrew (planned, follow-up):

```shell
brew install sv-tools/tap/roas
```

## Usage

### `roas validate`

Parses an OpenAPI document and runs the full validation pass. Version is auto-detected from the document's `openapi`
/ `swagger` field; pass `--from` to force.

```shell
roas validate spec.json
roas validate --from v3.1 spec.json
roas validate --lenient-tags spec.json          # treat missing/unused tags as warnings

# Enable the external-reference loader.
roas validate --load file spec.json             # allow file://
roas validate --load http spec.json             # allow http:// + https://
roas validate --load file --load http spec.json # both
```

### `roas convert`

Chains the existing version migrations to upconvert a spec. Force the input version with `--from` if auto-detection
isn't possible.

```shell
roas convert --to v3.2 spec.json
roas convert --from v2 --to v3.2 swagger.json
```

Downconversions (e.g. v3.2 → v3.0) are not supported.

## See also

- [`roas`](https://crates.io/crates/roas) — the library.
- [`roas-http-fetcher`](https://crates.io/crates/roas-http-fetcher) — `http://` / `https://` fetcher used by
  `--load http`.
