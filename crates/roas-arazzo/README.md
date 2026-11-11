# roas-arazzo

Rust implementation of the OpenAPI Arazzo Specification ([v1.0](https://spec.openapis.org/arazzo/v1.0.1.html) / [v1.1](https://spec.openapis.org/arazzo/v1.1.0.html)): parse and validate Arazzo workflow descriptions.

[![crates.io](https://img.shields.io/crates/v/roas-arazzo.svg)](https://crates.io/crates/roas-arazzo)
[![docs.rs](https://docs.rs/roas-arazzo/badge.svg)](https://docs.rs/roas-arazzo)

An *Arazzo description* declares deterministic sequences of API calls — *workflows* of *steps*, each invoking an OpenAPI operation or another workflow — together with their inputs, success/failure criteria, and outputs. It complements an OpenAPI description by capturing *how* to use an API, not just *what* it exposes.

This crate is a sibling of [`roas`](https://crates.io/crates/roas) (the typed parser / validator / merger for OpenAPI 2.0–3.2) and [`roas-overlay`](https://crates.io/crates/roas-overlay). It provides the typed document model plus a `Validate` framework that collects every diagnostic in one pass.

## Versions

| Arazzo version | Feature flag     | Status         | Adds over the previous version                                              |
|----------------|------------------|----------------|-----------------------------------------------------------------------------|
| 1.0            | `v1_0`           | ✅ implemented  | —                                                                           |
| 1.1            | `v1_1` (default) | ✅ implemented  | `$self`, AsyncAPI steps, selectors, expression types, action `parameters`  |

`v1_0` and `v1_1` are independent — enable whichever you need. With both enabled, an `impl From<v1_0::Description> for v1_1::Description` upconverts an existing v1.0 description.

JSON Schemas: [v1.0](https://spec.openapis.org/arazzo/1.0/schema/2025-10-15) · [v1.1](https://spec.openapis.org/arazzo/1.1/schema/2026-04-15). Where a schema and the prose specification disagree, this crate follows the prose — the schemas do not track the specification one for one. `PayloadReplacement.value` is such a case: both versions' prose say `Any`, the v1.0 schema says `string`, and this crate takes any JSON value.

## Quick start

```rust
use enumset::EnumSet;
use roas_arazzo::v1_1::Description;
use roas_arazzo::validation::Validate;

let doc: Description = serde_json::from_str(r#"{
    "arazzo": "1.1.0",
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

YAML descriptions work the same way — parse with `serde-saphyr` (or any other YAML crate) into `Description`.

## Validation

`Validate::validate` returns every diagnostic it finds rather than failing on the first one. Diagnostics carry a JSONPath-flavor `path` (e.g. `#.workflows[0].steps[1].stepId`). Checks include:

- required / non-empty fields, and the source-name (`^[A-Za-z0-9_\-]+$`) and component/output-key (`^[a-zA-Z0-9\.\-_]+$`) patterns;
- uniqueness of source names, workflow ids, and step ids;
- a step setting exactly one of `operationId` / `operationPath` / `workflowId`, with `in` required on operation-step parameters;
- criterion `type` → `context` dependency and expression-type `version` constants;
- `goto` actions requiring exactly one of `workflowId` / `stepId`.

`ValidationOptions` (EnumSet): `IgnoreEmptyInfoTitle`, `IgnoreEmptyInfoVersion`. Behind the `clap` feature, the enum implements `clap::ValueEnum` so downstream CLIs can surface it directly.

## Reusable objects

A parameter or action list holds `ReusableOr<T>`: the object itself, or a `Reusable` naming one in `components` (`$components.parameters.page`). `ReusableOr::get_item(&description)` returns the object either way, and takes a `Components` just as well; a reference of another kind and a name the components have not got are separate `ResolveError`s. A `Reusable`'s `value` is not applied — read it with `value_override()`.

## Unknown members

An Arazzo object holds the fields its specification defines and, except for a Reusable Object, `x-` extensions; deserializing drops anything else without a word, so a misspelt `sucessCriteria` simply vanishes. `Description::unknown_members(&raw)` lists what was dropped from the value the description was deserialized from, each with a model-style `path` (`#.workflows[0].steps[1].sucessCriteria`) that a `SourceMap` locates with `locate_key`. A member whose value is `null`, `[]` or `{}` is not listed: a known optional field left empty is dropped the same way, so the two cannot be told apart. List a v1.0 description's members before upconverting it; `UnknownMembers::version` keeps the version it declared.

```rust
let raw = serde_json::json!({
    "arazzo": "1.1.0",
    "info": { "title": "T", "version": "1", "summery": "typo" },
    "sourceDescriptions": [], "workflows": []
});
let description: roas_arazzo::v1_1::Description = serde_json::from_value(raw.clone()).unwrap();
let unknown = description.unknown_members(&raw).unwrap();
assert_eq!(unknown.members[0].path, "#.info.summery");
```

## Source locations

Behind the `source-map` feature, `source_map::SourceMap::parse(text)` reads the JSON or YAML text a description came from and answers where a diagnostic's `path` is written: `locate(path)` for the value, `locate_key(path)` for the member name (for a name an alias or `<<` merge brings into an object, the `*name` that brings it, not the anchor), and `locate_in(path, offset)` for a byte offset inside a string value, mapped back through quotes, escapes (`\u00e9`, surrogate pairs), YAML line folding and block-scalar indentation. Positions are UTF-8 byte offsets with one-based lines and columns counted in characters; an editor speaking UTF-16 converts at its own boundary, with `utf16_column(position)`. Member names may contain dots or `[n]`; a path two members could spell (`a.b` beside `a` → `b`) locates nothing. When the raw scalar does not decode back to exactly the parsed value, `locate_in` returns `None` rather than guess. The map is a sidecar: the typed models are unchanged and stay usable without it.

```rust
use roas_arazzo::source_map::SourceMap;

let map = SourceMap::parse("info:\n  title: \"caf\\u00e9\"\n").unwrap();
let title = map.locate("#.info.title").unwrap();
assert_eq!((title.start.line, title.start.column), (2, 10));
```

## Editor mode

Behind the `editor` feature, `editor::Document::read(text, options)` reads text that is being typed and is wrong most of the time. It never fails: it returns an `Outline` of what the text still says — sources, workflows and their steps, components, each with its range — and every `Diagnostic` it finds, with a severity, a model-style path and a range in the text.

- Text that is not JSON or YAML gets a located syntax error, and the text above the error is read again, up to 16 lines back, so a YAML description usually keeps the outline of everything before it. The outline is then marked incomplete (`is_complete()`), and nothing further is checked.
- Text that reads in full is checked part by part — `arazzo`, `$self`, `info`, each source description, each workflow, `components` — so one broken workflow does not hide another. When every part fits the model, the description is validated and its unknown members are listed as warnings.
- `read` tells JSON from YAML by the first character, and warns when text that looks like JSON is only YAML; `read_as(text, Format::Json | Format::Yaml, options)` reads one format as strictly as the roas CLI reads a file of it.
- `utf16_column(position)` gives the one-based column in UTF-16 code units; an LSP client counts from zero, so subtract one.

Nothing a `Document` returns can be run: it never hands out a typed `Description`, and its outline is plain strings and ranges. To execute a description, deserialize it the strict way.

```rust
use roas_arazzo::editor::Document;

let document = Document::read("arazzo: 1.1.0\ninfo: {title: [T]}\n", Default::default());
assert_eq!(document.diagnostics()[0].path.as_deref(), Some("#.info.title"));
```

Runtime-expression grammar, `sourceDescriptions` / `$ref` resolution, and deep JSON Schema semantics for `inputs` are out of scope for this release.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
