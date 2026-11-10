# roas-codegen

Generates Rust and Go types from OpenAPI descriptions — every version from 2.0 to 3.2 — on top of the [`roas`](https://crates.io/crates/roas) models.

[![crates.io](https://img.shields.io/crates/v/roas-codegen.svg)](https://crates.io/crates/roas-codegen)
[![docs.rs](https://docs.rs/roas-codegen/badge.svg)](https://docs.rs/roas-codegen)

A description goes in, is version-detected, parsed, normalized to OpenAPI 3.2 and validated; every schema is lowered to a language-neutral representation where the hard decisions are already made; and a backend renders it, in Rust or in Go.

## Quick start

```rust
use roas_codegen::{
    ConfigFile, Input, SourceDocument, Target, generate, generation_options, validate,
};
use url::Url;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let bytes = std::fs::read("openapi.json")?;
let document = SourceDocument::parse(Input::Json(bytes), Url::parse("file:///openapi.json")?)?;
let document = validate(document, generation_options())?;

let mut file = ConfigFile::default();
file.target = Some(Target::Rust);
let config = file.build()?;
let generation = generate(&document, &config)?;
for file in &generation.files {
    std::fs::write(file.path.file_name().unwrap(), &file.contents)?;
}
for diagnostic in &generation.diagnostics {
    eprintln!("{diagnostic}");
}
# Ok(()) }
```

`generate` returns files in memory, writing no output files, together with every diagnostic and the dependency list the generated code needs — for Rust, `serde` and `serde_json` with `raw_value`, plus whatever a type substitution imports; for Go, only the modules a substitution imports, since the output uses the standard library. Nothing generated depends on `roas`.

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

## What the Go output looks like

The same schema, with the same decisions:

```go
type Pet struct {
	// `minLength`: 1.
	Name   string           `json:"name"`
	Status *PetStatus       `json:"status,omitzero"`
	Tag    Nullable[string] `json:"tag,omitzero"`
}

type PetStatus string

const (
	PetStatusAvailable PetStatus = "available"
	PetStatusSold      PetStatus = "sold"
)
```

The output uses `encoding/json/v2` from the standard library, so it needs Go 1.27 or later and nothing else. Struct tags cannot say what the schema says, so every generated type has its own `UnmarshalJSONFrom`:

- **Presence and null are checked.** Go leaves a missing member at its zero value and stores a zero value for a JSON `null`. A struct's decoder refuses a missing required member and a `null` in a member that is not nullable, and a `null` anywhere else a schema forbids it — a list item, say — is refused too. `Nullable[T]` keeps the three states of an optional, nullable member: `omitzero` leaves out only the absent one.
- **Unknown members.** `additionalProperties: false` is refused at decode, as in Rust; otherwise the extra members land in an `AdditionalProperties` map embedded with `json:",embed"` and are written back out.
- **Enums check membership.** A named string type accepts any string, so its decoder accepts only the listed values.
- **Integers read the token** into `Int64` / `Int32`, exactly as in Rust.
- **The decoders own their options.** Each generated type decodes its members with its own null rules, so unmarshalers a caller passes with `json.WithUnmarshalers` do not reach inside generated types; a substituted type decodes by its own rules alone, as in Rust.
- **Unions.** A tagged union is a struct holding one variant, with `As<Variant>` accessors, `<Union>From<Variant>` constructors, `Tag()` and `Value()`; it reads the tag first and then decodes the whole value as that variant. Open unions and compositions keep the raw JSON with an accessor and a constructor per branch.

Go identifiers are exported and ASCII. A property name a struct tag cannot carry — one holding `,`, `\`, `'`, `"` or a backtick, or the empty name — gets `json:"-"`, and its struct writes itself with a generated `MarshalJSONTo` instead; decoding goes by name anyway. Generating Go needs `gofmt`, which formats the output and is its only syntax check, so without it `generate` fails with `GenerateError::Toolchain` rather than returning unchecked code.

## Configuration

The Rust output is one module, `<rust.module>.rs`, `types.rs` by default; the Go output is one file, `<go.file>.go`, also `types.go` by default, in package `go.package`, `api` by default. Targets sharing a directory need different names. The output is types for a package in your own module; no `go.mod` is generated.

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

# Off by default: `format: uuid` is a `string`. Go 1.27 has `uuid` in the
# standard library, so this adds no dependency.
[substitutions.uuid.go]
name = "uuid.UUID"
import = "uuid"

[types.Pet]
rename = "Animal"
attrs = ['#[derive(Hash)]']

[types.Pet.fields.tag]
attrs = ['#[validate(length(min = 1))]']
go_tags = { db = "tag" }
```

Types and fields are addressed by the names in the description, never by the generated identifiers. User attributes are parsed as Rust, and the serde keys that carry the wire contract — `default`, `skip_serializing_if`, `deserialize_with`, `rename`, `flatten` and the container-level equivalents — are refused by name rather than silently doubled. `x-rust-attrs` in a description is code from whoever wrote the description, so it is applied only under `allow_codegen_extensions = true`. For Go, each `extra_imports` entry is an import spec placed inside the generated `import ( ... )`, and `go_tags` adds keys to a field's struct tag; `json` is the generator's and is refused, and a Go rename must be an exported identifier.

Generated Rust is validated with `syn` before it is returned and formatted with `rustfmt` when one is on the path. `rustfmt` always runs with an empty configuration, so a `rustfmt.toml` nearby never changes the output; when it cannot run, the output is unformatted and a warning says so.

## Generated validation

Off by default. `validation = true` (or `roas openapi generate --validate`) gives every generated type a `validate()` — the `Validate` trait in Rust, the `Validator` interface in Go — that checks the constraints its type cannot express: `minLength` / `maxLength` (in characters), `minimum` / `maximum` and their exclusive forms, `multipleOf`, `minItems` / `maxItems` / `uniqueItems`, and `minProperties` / `maxProperties`. What the type does express — presence, nullability, enum membership, unknown members, integers — is the decoder's job and is not re-checked. Nested types are validated in place, and `oneOf` / `anyOf` / `allOf` holders count the branches that decode and validate, which the raw JSON they keep makes possible.

```rust
let report = pet.validate();
for finding in report.violations() {
    eprintln!("{}: {}", finding.pointer, finding.message); // "/name: is 0 characters long, shorter than minLength 1"
}
```

Findings collect rather than stop at the first, each at an RFC 6901 JSON Pointer into the value, and are of two kinds. A **violation** is a constraint the value breaks. An **unevaluated** finding is one the generated code could not decide, and says why; `is_valid()` / `IsValid()` is true only when there are neither, so an unchecked constraint is never mistaken for a passing one. Integers are compared exactly against each bound and `multipleOf` as the description states it; a fractional bound or multiple is exact only as far as the description was read exactly (JSON with the `exact-numbers` feature), and otherwise as far as an `f64` holds it. A decoded float has lost its literal, so `multipleOf` on a `number` decides only a fraction against a whole multiple and is otherwise unevaluated, and equal floats under `uniqueItems` are unevaluated too. `pattern` and `propertyNames` are reported as unevaluated until they are checked. A constraint on an inline array item or map value has no generated type to carry it and is reported at generation time; give that schema a name of its own. `types.<Name>.validation = false` opts one type out: it still implements `validate`, with no checks, so the types holding it compile unchanged. The generated code needs nothing beyond what it already does.

## Templates

Every file is rendered through [minijinja](https://docs.rs/minijinja) templates, embedded in the crate. `template_dir` in the configuration (or `--template-dir`) points at a directory of your own, which overrides them by name: `rust/struct.rs.jinja`, `go/field.go.jinja` and so on, with the same paths as [`templates/`](templates). The directory holds a `roas-templates.toml` declaring the contract it was written against:

```toml
template_api = 1
```

A mismatch is an error, not a mystery. An override usually extends the default of the same name, which stays reachable under the reserved `builtin/` prefix, and fills only the blocks it changes; `{{ super() }}` keeps the default's content:

```jinja
{# rust/struct.rs.jinja #}
{% extends "builtin/rust/struct.rs.jinja" %}
{% block derives %}#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
{% endblock %}
```

The defaults include one another by plain name, so an override of an included template (`rust/field.rs.jinja`) applies wherever it is used, and an override can include new partials from the same directory. A template that extends another may only fill blocks the templates it extends declare — minijinja would ignore any other block without a word, so a typo or a renamed block is an error instead. The check reads templates with minijinja's own parser; for it, a template has at most one `extends`, naming its template as one string literal; a block nested inside a filled block is a new declaration a further override can fill.

| Template | Blocks |
| --- | --- |
| `rust/module.rs.jinja`, `go/file.go.jinja` | `header`, `imports`, `body`, `footer` |
| `rust/struct.rs.jinja` | `doc`, `derives`, `attrs`, `fields`, `trailing` |
| `rust/enum.rs.jinja`, `rust/union_tagged.rs.jinja` | `doc`, `derives`, `attrs`, `variants`, `trailing` |
| `rust/union_open.rs.jinja`, `rust/composed_open.rs.jinja` | `doc`, `derives`, `attrs`, `accessors`, `trailing` |
| `rust/alias.rs.jinja` | `doc`, `derives`, `attrs`, `trailing` |
| `rust/field.rs.jinja`, `rust/variant.rs.jinja` | `doc`, `attrs`, `decl` |
| `go/struct.go.jinja` | `doc`, `fields`, `trailing` |
| `go/enum.go.jinja`, `go/union_tagged.go.jinja` | `doc`, `variants`, `trailing` |
| `go/union_open.go.jinja`, `go/composed_open.go.jinja` | `doc`, `accessors`, `trailing` |
| `go/alias.go.jinja` | `doc`, `trailing` |
| `go/field.go.jinja` | `doc`, `decl` |
| `rust/validate.rs.jinja`, `go/validate.go.jinja` (with validation on) | `validate` |

The generated-code marker is written first in every file by the renderer, whatever the templates do, and a `header` override with `{{ super() }}` keeps the configuration's `header` rather than replacing it.

Templates lay out what the backend decided; they never decide a type. The context is a view model in which every type is already spelled for the target, every name is final and every literal quoted: a file with `template_api`, `source`, `header`, `imports`, `extra_imports`, `support` and `types`, each type with its `kind` (`struct`, `enum`, `union_tagged`, `union_open`, `composed_open`, `newtype`, `alias`), `name`, `raw_name`, `doc` and the parts its kind has (`fields`, `variants`, `branches`, `inner`, `tag`). Inside a type template the type is `t`; inside a field or variant template the member is `f` or `v`. `--print-context` prints the exact context for a description, which is the reference; `template_context` returns it from the library. The backend-owned parts — Rust's serde attributes, Go's `json` struct tag — arrive already in the context, so an override that replaces a field's `attrs` or `decl` should keep them, or it changes the wire format. The output is still checked — with `syn` for Rust, `gofmt` for Go — and when a template produces code that does not parse, `render` (or `--dump-rendered DIR`) gives the text before the check, to see what the template wrote.

## Input and fidelity

`Input::Json(bytes)` is the only way to obtain exact numbers: the crate parses the bytes itself. `Input::Yaml(bytes)` is read with `serde-saphyr`, which keeps an integer within the 64-bit range exactly but reads a fraction, an exponent or a wider decimal integer through `f64`, and `Input::Value(serde_json::Value)` has unknown provenance by construction. `SourceDocument` keeps the fidelity it was given, so the `exact_numbers` setting can refuse a source that cannot honour it.

The URI is required. Every schema identity is the document URI plus a JSON Pointer, so input with no natural URI — stdin, say — takes a synthetic one such as `stdin:///`.

## Writing files

`generate` writes no output files (it only uses a temporary directory to run `rustfmt`; `gofmt` reads and writes pipes). `write` puts a `Generation` into a directory under a manifest — `.roas-codegen-<label>-<language>.json` — that records each file it owns with a content hash, and `check` compares the directory with a generation without writing anything.

```rust
use roas_codegen::{Generation, WriteOptions, check, write};

fn save(generation: &Generation) -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new("src/api");
    let report = write(generation, out, &WriteOptions::new("petstore"))?;
    println!("{} written, {} removed", report.written.len(), report.removed.len());
    assert!(check(generation, out, &WriteOptions::new("petstore"))?.is_clean());
    Ok(())
}
```

- **Only owned files change.** A path another target's manifest owns is refused, and so is an existing file no manifest owns unless `overwrite_unowned` is set or it already holds exactly the generated content, in which case it is adopted. Two targets with different labels can share a directory, except two Go targets: Go files in one directory are one package, where both would declare the same helpers, so a second Go manifest there is refused.
- **Ownership is claimed before anything is written.** Every check runs first, including that no path goes through a symbolic link. A pending manifest owning the previous and the new files is written, each file is written to a temporary sibling and renamed into place, and the final manifest replaces the pending one, so an interrupted run is completed by the next one.
- **Errors stop the write.** A generation carrying `Error` diagnostics is refused unless `allow_partial` is set, and even then files the previous run wrote are kept rather than removed.
- **Labels belong to one description.** When `WriteOptions::source_id` is set, the manifest records it and a different source under the same label is refused. The `roas` command line sets it to the description's path relative to the output directory without a `.json`/`.yaml`/`.yml` extension, so two `openapi.yaml` files from different folders cannot overwrite each other, while converting a description from JSON to YAML keeps its label.
- **One run at a time.** An operating-system lock on the output directory's `.roas-codegen.lock` spans the run, exclusive. Other output roots the run can reach, enclosing or nested, are locked shared, so siblings under one root run side by side while overlapping roots exclude each other. Enclosing roots are looked for up to the first shared, sticky directory such as `/tmp`, so other users' files there cannot block a run. Locks end with the process, so a killed run does not block the next; the lock files stay, as they mark the roots. Ownership is compared case-insensitively, and nothing below the output directory is reached through a symbolic link.
- **Stale files are removed only if unedited.** A file the previous run wrote and this one does not is deleted when it still holds the generated content, and kept and reported otherwise.

The `roas` command line wraps all of this as `roas openapi generate`, with `--check` for CI.

## License

MIT OR Apache-2.0
