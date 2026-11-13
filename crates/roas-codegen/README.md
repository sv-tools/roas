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

`generate` returns files in memory, writing no output files, together with every diagnostic and the dependency list the generated code needs — for Rust, `serde` and `serde_json` with `raw_value`, `regex` when a pattern is checked (at decode beside `additionalProperties: false`, or by validation), plus whatever a type substitution imports; for Go, only the modules a substitution imports, since the output uses the standard library. Nothing generated depends on `roas`.

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
#[serde(remote = "Self")]
pub struct Pet {
    /// `minLength`: 1.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "reject_null::deserialize")]
    pub status: Option<PetStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "double_option::deserialize")]
    pub tag: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(remote = "Self")]
pub enum PetStatus {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "sold")]
    Sold,
}
```

The decisions behind that, each one the generated code enforces rather than merely represents:

- **Nullability is two axes.** `required` and nullable are independent, so there are four cases, and plain `Option<T>` enforces none of them: serde accepts `null` into any `Option` and treats a missing member as `None`. Each case gets the helper that rejects the state the schema forbids — `reject_null`, `require_present`, `double_option` — so `{}` and `{"tag": null}` stay distinct on the way in and the way out.
- **Closed objects read objects only.** serde's derive also reads a struct from a JSON array, field by field; a closed struct has no catch-all to stop that. Its derived code is made inherent (`remote = "Self"`), and the `Serialize` / `Deserialize` impls beside it delegate to that code, handing it the same deserializer asked for a map — so anything but an object is refused, and the decoder's depth limit still holds. A string enum is read the same way, asked for a string: serde's derive would otherwise also read `{"available": null}` as `available`.
- **Integers read the token.** JSON Schema's `integer` accepts `1.0` and `1e0` and rejects `1.5`; both languages' stock decoders disagree. Generated `Int64` / `Int32` newtypes decode from the raw token exactly, which is why the output depends on `serde_json` with `raw_value` and why the generated types are JSON-only: deserialize them from text or a reader, not through `serde_json::from_value`.
- **Numbers held exactly, on request.** By default a `number` is an `f64` (`f32` for `format: float`), which loses digits. `exact_numbers = true` holds every `number`, whatever its `format`, as a generated `Decimal` instead: an `i128` mantissa times a power of ten, read from the raw token like the integer types and written back without trailing zeros as JavaScript writes numbers (`1.50` as `1.5`, `0.0000001` as `1e-7`), with `FromStr`, `Display`, `Ord`, `new`, `mantissa`, `exponent`, `to_f64` and `is_multiple_of`. It holds up to 38 significant digits and an `i32` exponent; a value past that is refused at decode rather than rounded. Integers stay `Int64` / `Int32`. It needs the crate's `exact-numbers` feature and a JSON description (see [Input and fidelity](#input-and-fidelity)), and nothing beyond `raw_value` in the generated code: serde_json's `arbitrary_precision` is not involved, so going through `serde_json::Value` (`to_value`, `from_value`) reads a `Decimal` as a float and rounds it.
- **A parent's discriminator selects among the schemas building on it.** An object schema with a `discriminator` whose components extend it with `allOf` (OpenAPI's parent-schema polymorphism, `Pet` with `Cat` and `Dog`) gets a tagged union, `PetUnion`, and every value position that refers to the parent with a `$ref` holds the union instead: a field, an item, a map value, a union branch. (A position written as an `allOf` of the parent alone, as 3.0 descriptions do to annotate a `$ref`, keeps a copy of the parent.) The tag selects the variant by `mapping`, else by component name (a schema `mapping` gives another tag is still selected by its name, unless `mapping` uses the name for a tag of its own); any other or missing tag reads the value as the parent itself, or as the `defaultMapping`. The parent stays a struct of its own, which the schemas building on it are made of; a parent with a configured substitution is left alone, held as the substituted type. This form of discriminator is a hint JSON Schema validation does not use, and the union trusts it: a value tagged `Cat` is read and validated as a `Cat`. A tag that is not a string property of the parent is warned about and no union is made. A schema building on the parent that is not generated (an error in it) is left out of the union, with a warning, rather than taking every holder of the parent with it; in Rust, a schema gated by a `cfg` attribute gates only its variant, whose tag is then read as an unknown tag is (the parent, when the `defaultMapping` target is itself compiled out), and the union carries the parent's gates.
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
- **Unknown members.** `additionalProperties: false` is refused at decode, as in Rust; otherwise the extra members land in an `AdditionalProperties` map embedded with `json:",embed"` and are written back out. Beside `patternProperties`, a member matching a pattern lands there too, and with `additionalProperties: false` one matching neither a property nor a pattern is refused at decode in both languages — unless a pattern cannot be translated, when every member is let in. Checking a pattern at decode puts a regex engine into the generated code (`regex` in Rust, `regexp` in Go) even with validation off.
- **Enums check membership.** A named string type accepts any string, so its decoder accepts only the listed values.
- **Integers read the token** into `Int64` / `Int32`, exactly as in Rust.
- **Numbers held exactly** under `exact_numbers`, as in Rust: a `Decimal` comparable with `==` and usable as a map key, with `ParseDecimal`, `String`, `Cmp`, `Float64` and `IsMultipleOf`, adding `math/big` and `strconv` to the imports.
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

Off by default. `validation = true` (or `roas openapi generate --validate`) gives every generated type a `validate()` — the `Validate` trait in Rust, the `Validator` interface in Go — that checks the constraints its type cannot express: `minLength` / `maxLength` (in characters), `minimum` / `maximum` and their exclusive forms, `multipleOf`, `minItems` / `maxItems` / `uniqueItems`, `minProperties` / `maxProperties`, `pattern`, `propertyNames`, `patternProperties`, and a number's `enum` and `const`. What the type does express — presence, nullability, string enum membership, unknown members, integers — is the decoder's job and is not re-checked. Nested types are validated in place, and `oneOf` / `anyOf` / `allOf` holders count the branches that decode and validate, which the raw JSON they keep makes possible.

```rust
let report = pet.validate();
for finding in report.violations() {
    eprintln!("{}: {}", finding.pointer, finding.message); // "/name: is 0 characters long, shorter than minLength 1"
}
```

Findings collect rather than stop at the first, each at an RFC 6901 JSON Pointer into the value, and are of two kinds. A **violation** is a constraint the value breaks. An **unevaluated** finding is one the generated code could not decide, and says why; `is_valid()` / `IsValid()` is true only when there are neither, so an unchecked constraint is never mistaken for a passing one. Integers are compared exactly against each bound and `multipleOf` as the description states it; a fractional bound or multiple is exact only as far as the description was read exactly (JSON with the `exact-numbers` feature), and otherwise as far as an `f64` holds it. A `float` (f32) value at either float32 around its bound is unevaluated against that bound: literals on either side of the bound decode to it. A number's `enum` and `const` are checked too, though the type stays a plain integer or float: exactly for an `integer` — though a listed value written as a float is exact only as far as the description was read exactly (JSON with `exact-numbers`), and one past 2^53 whose text says no more than its float leaves a value that rounds to it unevaluated; for a `double` as far as an `f64` holds it, with a value equal to no listed value's float but to the float of one no `f64` holds as written (more digits than a float keeps, or past its range) unevaluated — which a listed value is known to be only when the description was read exactly (`exact-numbers` with JSON, or an integer token past 2^53 that fits a `u64`), and is otherwise compared as the `f64` it was read as; and a `float` equal to the float32 nearest a listed value is unevaluated. A decoded float has lost its literal, so `multipleOf` on a `number` decides only a fraction against a whole multiple and is otherwise unevaluated, and equal floats under `uniqueItems` are unevaluated too. Under `exact_numbers` none of that applies: a `Decimal` is compared with every bound, `multipleOf`, `enum` and `const` exactly, and each is decided, as is `uniqueItems` over `number` items written inline (items of a named or nullable type are not compared yet, as for integers). A bound in the description with more digits, or a larger or smaller exponent, than a `Decimal` holds still decides every value (it is rounded towards the values meeting it, with none in between), and a listed value no `Decimal` holds is no value's; only a `multipleOf` no `Decimal` holds leaves values unevaluated, with a warning. A `propertyNames` schema saying more than `minLength`, `maxLength` and `pattern`, a `const` the type does not pin (other than a number's), a `required` member with no property of its own, and keywords such as `minLength` on a schema with no `type` (held as raw JSON) or on a substituted type (reported where it is held, or by the non-empty array or map holding it) are reported as unevaluated until they are checked, and so is a keyword the generator does not handle at all, which is also warned about at generation time — including, for an OpenAPI 2.0 or 3.0 source, a keyword its model drops on the way to 3.2 (`maxLength` beside an `allOf`, say). `propertyNames` is checked on each member name, reported at that member; a declared property's name is decided at generation, so only its presence is checked at run time. Constraints on an inline array item or map value are checked on each item or member, reported at its own pointer (`/tags/2`). Beside `patternProperties`, a member no property names is held as raw JSON (`RawJson` in Rust, `jsontext.Value` in Go) and checked against the schema of every pattern it matches, reported at its own pointer, or against `additionalProperties` when it matches none; a pattern that cannot be translated leaves the members it might match unevaluated, unless its schema lets anything be and so does `additionalProperties`. A declared property whose name matches a pattern is checked against that pattern's schema too: the match is decided at generation, and the field's value is written out and read again as the pattern's schema — unless the field holds a substitution or a floating-point number (in Rust also a `serde_json::Value`, or a type read by other names than the schema), whose decoded value may not be what was sent, when it is unevaluated. A `false` pattern schema makes a matching member a violation; a `not` one leaves it unevaluated, with a warning. A schema with `patternProperties` is always generated as a struct, never as a plain map. The walk is bounded: it goes at most 256 generated types deep, and the `oneOf` / `anyOf` / `allOf` branches it tries for one value decode at most 64 MiB between them (each try counted as at least 640 bytes) — an open holder decodes each branch afresh from its raw value, and nested unions would otherwise multiply the work — and past either limit the rest is reported as unevaluated. `types.<Name>.validation = false` opts one type out: it still implements `validate`, with no checks, so the types holding it compile unchanged. `pattern` is read as the ECMAScript regular expression JSON Schema names (in Unicode mode, or in the legacy grammar when nothing in it could match half of a character) and rewritten so Rust's `regex` and Go's `regexp` decide it the same way: `\d` and `\w` are ASCII, `\s` is ECMAScript's whitespace set, `.` excludes the four line terminators, and every non-ASCII character is spelled `\x{…}`. A pattern that cannot mean the same in both — lookaround, backreferences, `\b`, Unicode properties, inline flags, a count above 1000 — is warned about at generation time and reported as unevaluated, as is one too large for the engine to compile. Checked patterns add `regex` to the generated Rust code's dependencies. With validation on, the generated Rust code also needs serde_json's `float_roundtrip` feature, listed with its dependencies, so a number is decoded as exactly as its bound was read. In Rust, a field with a `serde(alias)` is unevaluated when present if it is required or the schema says anything of it or of other members that depends on the name, since a decoded field does not record which name it was read by.

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
| `rust/object_only.rs.jinja` (a closed struct) | `object_only` |
| `rust/catch_all.rs.jinja` (a struct with a catch-all) | `catch_all` |
| `rust/string_only.rs.jinja` (a string enum) | `string_only` |

The generated-code marker is written first in every file by the renderer, whatever the templates do, and a `header` override with `{{ super() }}` keeps the configuration's `header` rather than replacing it.

Templates lay out what the backend decided; they never decide a type. The context is a view model in which every type is already spelled for the target, every name is final and every literal quoted: a file with `template_api`, `source`, `header`, `imports`, `extra_imports`, `support` and `types`, each type with its `kind` (`struct`, `enum`, `union_tagged`, `union_open`, `composed_open`, `newtype`, `alias`), `name`, `raw_name`, `doc` and the parts its kind has (`fields`, `variants`, `branches`, `inner`, `tag`). Inside a type template the type is `t`; inside a field or variant template the member is `f` or `v`. `--print-context` prints the exact context for a description, which is the reference; `template_context` returns it from the library. The backend-owned parts — Rust's serde attributes, Go's `json` struct tag — arrive already in the context, so an override that replaces a field's `attrs` or `decl` should keep them, or it changes the wire format. The same holds for a type's `derives` and `attrs`: a closed struct, a struct with a catch-all and a string enum carry `#[serde(remote = "Self")]` and derive `Serialize` / `Deserialize`, which the impls in `rust/object_only.rs.jinja`, `rust/catch_all.rs.jinja` and `rust/string_only.rs.jinja` delegate to — an override that drops them leaves those impls calling themselves, and a `rust/module.rs.jinja` body must keep including those three templates. The catch-all impl routes members to the derived code by name, so a field alias must be a plain `#[serde(alias = "…")]` with a string literal. The output is still checked — with `syn` for Rust, `gofmt` for Go — and when a template produces code that does not parse, `render` (or `--dump-rendered DIR`) gives the text before the check, to see what the template wrote.

## Input and fidelity

`Input::Json(bytes)` is the only way to obtain exact numbers: the crate parses the bytes itself. `Input::Yaml(bytes)` is read with `serde-saphyr`, which keeps an integer within the 64-bit range exactly but reads a fraction, an exponent or a wider decimal integer through `f64`, and `Input::Value(serde_json::Value)` has unknown provenance by construction. `SourceDocument` keeps the fidelity it was given, so the `exact_numbers` setting refuses a source that cannot honour it, and, without the crate's `exact-numbers` feature, which reads the description's numbers as written, refuses to run at all.

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
