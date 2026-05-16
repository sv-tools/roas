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

`--ignore <CHECK>` skips a specific validation check; repeat the flag to skip more than one. Available checks:

```
missing-tags, external-references, invalid-urls, non-uniq-operation-ids,
unused-path-items, unused-tags, unused-schemas, unused-parameters,
unused-responses, unused-server-variables
```

`--lenient-tags` is a shorthand for `--ignore missing-tags --ignore unused-tags`.

### `convert`

Upconverts a spec to a target version by chaining the existing `From<v_X::Spec> for v_Y::Spec` migrations. Downconversion is not supported.

```shell
roas convert --to v3_2 spec.json
roas convert --to v3_1 --from v2 spec.yaml
```

Output is JSON on stdout.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
