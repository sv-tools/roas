# roas-overlay

Rust implementation of the [OpenAPI Overlay Specification](https://spec.openapis.org/overlay/v1.0.0.html): parse, validate, and apply Overlay documents to OpenAPI specs.

[![crates.io](https://img.shields.io/crates/v/roas-overlay.svg)](https://crates.io/crates/roas-overlay)

An *Overlay* is a sidecar document whose ordered list of *actions* — each a [RFC 9535 JSONPath](https://www.rfc-editor.org/rfc/rfc9535) `target` plus an `update`, `remove`, or v1.1 `copy` instruction — transforms a target OpenAPI document. Common uses: layering environment-specific changes over a base API, adding vendor extensions without forking, removing internal endpoints from a public bundle.

This crate is a sibling of [`roas`](https://crates.io/crates/roas) (the typed parser / validator / merger for OpenAPI 2.0–3.2). It operates on `serde_json::Value` so a single implementation works across every OpenAPI version, and so overlays that produce intermediate states the typed model would reject (drop a required field then add it back) still apply cleanly.

## Versions

| Overlay version | Feature flag     | Status            |
|-----------------|------------------|-------------------|
| 1.0             | `v1_0` (default) | ✅ implemented     |
| 1.1             | `v1_1`           | planned (next PR) |

## Quick start

```rust
use enumset::EnumSet;
use roas_overlay::apply::Apply;
use roas_overlay::v1_0::Overlay;
use roas_overlay::validation::Validate;

let overlay: Overlay = serde_json::from_str(r#"{
    "overlay": "1.0.0",
    "info": { "title": "Example", "version": "1.0.0" },
    "actions": [
        { "target": "$.info", "update": { "description": "Patched." } },
        { "target": "$.paths['/internal/metrics']", "remove": true }
    ]
}"#).unwrap();

overlay.validate(EnumSet::empty()).expect("overlay is well-formed");

let mut target: serde_json::Value = serde_json::from_str(r#"{
    "openapi": "3.1.0",
    "info": { "title": "API", "version": "1.0.0" },
    "paths": { "/internal/metrics": { "get": {} } }
}"#).unwrap();

let report = overlay.apply(&mut target, EnumSet::empty()).unwrap();
assert_eq!(report.actions.len(), 2);
assert_eq!(target["info"]["description"], "Patched.");
assert!(target["paths"].as_object().unwrap().is_empty());
```

YAML overlays work the same way — parse with `serde_yaml_ng` (or any other YAML crate) into `Overlay`.

## Apply algorithm

For each action in declaration order, against the *current* working copy of the target:

1. Compile the `target` JSONPath. Syntax errors abort the merge with `InvalidJsonPath`.
2. Resolve matching nodes via [`serde_json_path`](https://crates.io/crates/serde_json_path).
3. Zero matches → silent no-op (or `ZeroMatch` error under `ApplyOptions::ErrorOnZeroMatch`).
4. Targets must be objects or arrays for *every* action (spec §4.4); primitives or `null` raise `PrimitiveActionTarget`.
5. `remove: true` → drop each matched node from its container. Sibling array indices are preserved by processing matches in reverse.
6. Otherwise, if `update` is set, the behavior depends on the matched node's kind:
   - **Array target** → `update` is appended as a single new element, regardless of its shape (spec §4.4: *"an entry to append to the array"*).
   - **Object target** → recursive merge per [§4.4.3.1](https://spec.openapis.org/overlay/v1.0.0.html#merging-rules): keys present in both sides recurse (objects), or use the merge rules (primitives replace, arrays concatenate at the property level).

On any error the target document is left **untouched** — `Overlay::apply` operates on a clone and commits only on success.

## Options

`ApplyOptions` (EnumSet):

- `ErrorOnZeroMatch` — fail when an action's `target` selects zero nodes (default: silent no-op).
- `ErrorOnMixedKindMatch` — fail when an `update` selects nodes of mixed kind (some objects, some arrays). The v1.1 spec calls this out normatively; this option lets v1.0 callers opt in.

`ValidationOptions` (EnumSet):

- `IgnoreEmptyInfoTitle`, `IgnoreEmptyInfoVersion` — allow `info.title` / `info.version` to be empty.

Behind the `clap` feature, both enums implement `clap::ValueEnum` so downstream CLIs (such as `roas-cli`) can surface them directly.

## Validation

`Validate::validate` returns every diagnostic it finds rather than failing on the first one. Diagnostics carry a JSONPath-flavor `path` (e.g. `#.actions[3].target`).

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
