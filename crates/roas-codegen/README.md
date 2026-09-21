# roas-codegen

Generates Rust and Go types from OpenAPI descriptions — every version from 2.0 to 3.2 — on top of the [`roas`](https://crates.io/crates/roas) models.

[![crates.io](https://img.shields.io/crates/v/roas-codegen.svg)](https://crates.io/crates/roas-codegen)
[![docs.rs](https://docs.rs/roas-codegen/badge.svg)](https://docs.rs/roas-codegen)

A description goes in, is version-detected, parsed, normalized to OpenAPI 3.2 and validated; every schema is lowered to a language-neutral representation where the hard decisions are already made; and a backend renders it. Rust is generated today, Go is next.

## Quick start

```rust
use roas_codegen::{ConfigFile, Input, SourceDocument, Target, generate, validate};
use url::Url;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let bytes = std::fs::read("openapi.json")?;
let document = SourceDocument::parse(Input::Json(bytes), Url::parse("file:///openapi.json")?)?;
let document = validate(document, Default::default())?;

let config = ConfigFile { target: Some(Target::Rust), ..Default::default() }.build()?;
let generation = generate(&document, &config)?;
for file in &generation.files {
    std::fs::write(file.path.file_name().unwrap(), &file.contents)?;
}
for diagnostic in &generation.diagnostics {
    eprintln!("{diagnostic}");
}
# Ok(()) }
```

`generate` returns files in memory, never touching disk, together with every diagnostic and the dependency list the generated code needs — `serde` and `serde_json` with `raw_value`, plus whatever a type substitution imports. Nothing generated depends on `roas`.

## What the Rust output looks like

```yaml
Pet:
  type: object
  additionalProperties: false
  required: [name]
  properties:
    name:   { type: string, minLength: 1 }
    tag:    { type: [string, "null"] }
    status: { type: string, enum: [available, sold] }
```

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pet {
    /// `minLength`: 1.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "reject_null::deserialize")]
    pub status: Option<PetStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "double_option::deserialize")]
    pub tag: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PetStatus {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "sold")]
    Sold,
}
```

The decisions behind that, each one the generated code enforces rather than merely represents:

- **Nullability is two axes.** `required` and nullable are independent, so there are four cases, and plain `Option<T>` enforces none of them: serde accepts `null` into any `Option` and treats a missing member as `None`. Each case gets the helper that rejects the state the schema forbids — `reject_null`, `require_present`, `double_option` — so `{}` and `{"tag": null}` stay distinct on the way in and the way out.
- **Integers read the token.** JSON Schema's `integer` accepts `1.0` and `1e0` and rejects `1.5`; both languages' stock decoders disagree. Generated `Int64` / `Int32` newtypes decode from the raw token exactly, which is why the output depends on `serde_json` with `raw_value` and why the generated types are JSON-only: deserialize them from text or a reader, not through `serde_json::from_value`.
- **Unions are never first-match-wins.** A `oneOf` becomes a tagged enum only when the discriminator provably selects the branch validation would: every branch is a component object that requires the discriminator property and pins it with `const` or a single-value `enum`. Otherwise, and for every `anyOf`, the value is kept raw with a typed accessor per branch. An `allOf` flattens into one struct only when every branch is an object with disjoint, self-declared properties; otherwise it too is a raw holder with a projection per part.
- **Cycles are boxed, lists are not.** A field holding its own type directly gets a `Box`; through a `Vec` or a map it does not need one.
- **Nothing is silently skipped.** `not`, `false`, an unknown dialect, a reference the first release does not follow — each is an `Error` diagnostic that suppresses the affected type and whatever holds it, while the rest is still generated. Unsupported keywords, lossy integer mappings, open compositions and typeless object schemas are warnings, each naming the JSON Pointer.

Names are suggested by the schema — a component name, a `title`, or the position such as `Pet` plus `status` — and made final by the backend after casing, keyword handling (`type` becomes `r#type`) and collision suffixes, so output is byte-for-byte deterministic.

## Configuration

`ConfigFile` is what a TOML file may say: everything optional, unknown keys rejected, relative paths resolved against the file's own directory. `Config` is what generation consumes, and `ConfigFile::build` is the one place a missing `target` is refused.

```toml
target = "rust"
header = "// SPDX-License-Identifier: MIT"
extra_imports = ["use my_crate::prelude::*;"]

[rust]
derives = ["Eq"]

[substitutions.date-time.rust]
name = "chrono::DateTime<chrono::Utc>"
import = "chrono"

[types.Pet]
rename = "Animal"
attrs = ['#[derive(Hash)]']

[types.Pet.fields.tag]
attrs = ['#[validate(length(min = 1))]']
```

Types and fields are addressed by the names in the description, never by the generated identifiers. User attributes are parsed as Rust, and the serde keys that carry the wire contract — `default`, `skip_serializing_if`, `deserialize_with`, `rename`, `flatten` and the container-level equivalents — are refused by name rather than silently doubled. `x-rust-attrs` in a description is code from whoever wrote the description, so it is applied only under `allow_codegen_extensions = true`.

Generated Rust is validated with `syn` before it is returned and formatted with `rustfmt` when one is on the path.

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
