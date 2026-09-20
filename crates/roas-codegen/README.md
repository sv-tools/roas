# roas-codegen

Generates Rust and Go types from OpenAPI descriptions — every version from 2.0 to 3.2 — on top of the [`roas`](https://crates.io/crates/roas) models.

[![crates.io](https://img.shields.io/crates/v/roas-codegen.svg)](https://crates.io/crates/roas-codegen)
[![docs.rs](https://docs.rs/roas-codegen/badge.svg)](https://docs.rs/roas-codegen)

This release is the front half of the pipeline: a description goes in, is version-detected, parsed, normalized to OpenAPI 3.2 and validated, and comes out as a document the generator can trust — with a report of everything the normalization changed and everything the first release will not generate. Emission of Rust and Go is the next slice.

## What it does today

```rust
use roas_codegen::{Input, SourceDocument, validate};
use url::Url;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let bytes = std::fs::read("openapi.json")?;
let uri = Url::parse("file:///openapi.json")?;

// Detects the version, parses it, records what upconverting to 3.2 loses.
let document = SourceDocument::parse(Input::Json(bytes), uri)?;
println!("{} document, {:?} numbers", document.version(), document.fidelity());

// Runs the `roas` validator with external references skipped; the
// document comes back inside the error, so a failed run loses nothing.
let validated = validate(document, Default::default())?;
for diagnostic in validated.diagnostics() {
    println!("{diagnostic}");
}
# Ok(()) }
```

The report names, with a JSON Pointer into the source as it was given:

- **normalization losses** — a 2.0 `discriminator` on a plain object schema, which 3.0 cannot carry, or a `collectionFormat: tsv`;
- **references the first release refuses** — anything outside the document, a plain-name anchor such as `#Pet`, and a schema whose meaning depends on an `$id` re-basing its subtree.

Nothing is silently skipped, and nothing external is resolved: a reference that cannot yet be followed correctly is reported rather than guessed at.

## Input and fidelity

`Input::Json(bytes)` is the only way to obtain exact numbers: the crate parses the bytes itself. `Input::Yaml(bytes)` goes through a reader that rounds through `f64`, and `Input::Value(serde_json::Value)` has unknown provenance by construction. `SourceDocument` keeps the fidelity it was given, so the `exact_numbers` setting can refuse a source that cannot honour it.

The URI is required. Every schema identity is the document URI plus a JSON Pointer, so input with no natural URI — stdin, say — takes a synthetic one such as `stdin:///`.

## Configuration

`ConfigFile` is what a TOML file may say: everything optional, unknown keys rejected, relative paths resolved against the file's own directory. `Config` is what generation consumes, and `ConfigFile::build` is the one place a missing `target` is refused.

```toml
target = "rust"
validation = true

[types.Pet.fields.tag]
attrs = ['#[validate(length(min = 1))]']
```

## License

MIT OR Apache-2.0
