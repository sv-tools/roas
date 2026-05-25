# roas-arazzo

Rust implementation of the [OpenAPI Arazzo Specification](https://spec.openapis.org/arazzo/v1.0.1.html): parse and validate Arazzo workflow descriptions.

[![crates.io](https://img.shields.io/crates/v/roas-arazzo.svg)](https://crates.io/crates/roas-arazzo)

An *Arazzo description* declares deterministic sequences of API calls — *workflows* of *steps*, each invoking an OpenAPI operation or another workflow — together with their inputs, success/failure criteria, and outputs. It complements an OpenAPI description by capturing *how* to use an API, not just *what* it exposes.

This crate is a sibling of [`roas`](https://crates.io/crates/roas) (the typed parser / validator / merger for OpenAPI 2.0–3.2) and [`roas-overlay`](https://crates.io/crates/roas-overlay). It provides the typed document model plus a `Validate` framework that collects every diagnostic in one pass.

## Versions

| Arazzo version | Feature flag     | Status         |
|----------------|------------------|----------------|
| 1.0            | `v1_0` (default) | ✅ implemented  |
| 1.1            | `v1_1`           | 🔜 planned      |

Authoritative JSON Schema for v1.0: <https://spec.openapis.org/arazzo/1.0/schema/2025-10-15>.

## Quick start

```rust
use enumset::EnumSet;
use roas_arazzo::v1_0::Description;
use roas_arazzo::validation::Validate;

let doc: Description = serde_json::from_str(r#"{
    "arazzo": "1.0.1",
    "info": { "title": "Example", "version": "1.0.0" },
    "sourceDescriptions": [
        { "name": "petStore", "url": "https://api.example.com/openapi.json", "type": "openapi" }
    ],
    "workflows": [
        {
            "workflowId": "getPet",
            "steps": [
                {
                    "stepId": "findPet",
                    "operationId": "getPetById",
                    "parameters": [ { "name": "petId", "in": "path", "value": "$inputs.petId" } ],
                    "successCriteria": [ { "condition": "$statusCode == 200" } ]
                }
            ]
        }
    ]
}"#).unwrap();

doc.validate(EnumSet::empty()).expect("description is well-formed");
assert_eq!(doc.workflows[0].workflow_id, "getPet");
```

YAML descriptions work the same way — parse with `serde_yaml_ng` (or any other YAML crate) into `Description`.

## Validation

`Validate::validate` returns every diagnostic it finds rather than failing on the first one. Diagnostics carry a JSONPath-flavor `path` (e.g. `#.workflows[0].steps[1].stepId`). Checks include:

- required / non-empty fields, and the source-name (`^[A-Za-z0-9_\-]+$`) and component/output-key (`^[a-zA-Z0-9\.\-_]+$`) patterns;
- uniqueness of source names, workflow ids, and step ids;
- a step setting exactly one of `operationId` / `operationPath` / `workflowId`, with `in` required on operation-step parameters;
- criterion `type` → `context` dependency and expression-type `version` constants;
- `goto` actions requiring exactly one of `workflowId` / `stepId`.

`ValidationOptions` (EnumSet): `IgnoreEmptyInfoTitle`, `IgnoreEmptyInfoVersion`. Behind the `clap` feature, the enum implements `clap::ValueEnum` so downstream CLIs can surface it directly.

Runtime-expression grammar, `sourceDescriptions` / `$ref` resolution, and deep JSON Schema semantics for `inputs` are out of scope for this release.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
