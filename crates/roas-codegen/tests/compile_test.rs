//! The one test that catches a whole class of emission bug: the
//! generated code must compile, and then it must behave. A temporary
//! crate is built around the generated module and run with assertions
//! for every nullability case, the integer wire type, the tagged union
//! and the open holders.
//!
//! The Go output gets the same assertions in a Go module of its own; it
//! needs Go 1.27 or later and is skipped, saying so, without one —
//! unless `ROAS_CODEGEN_REQUIRE_GO` is set, as CI does, which makes that
//! a failure.
//!
//! Set `ROAS_CODEGEN_SKIP_COMPILE=1` to skip both where no toolchain or
//! registry is available.

#![recursion_limit = "1024"]

use roas::validation::{IGNORE_UNUSED, Options};
use roas_codegen::{ConfigFile, Input, SourceDocument, Target, generate, validate};
use std::path::Path;
use std::process::Command;
use url::Url;

const MAIN: &str = r##"
// The binary exercises only part of the generated public API.
#[allow(dead_code)]
mod types;
use types::*;

fn must_fail<T: serde::de::DeserializeOwned + std::fmt::Debug>(json: &str, why: &str) {
    if let Ok(value) = serde_json::from_str::<T>(json) {
        panic!("{json} should have been rejected ({why}) but gave {value:?}");
    }
}

fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(json: &str) -> String {
    let value: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    serde_json::to_string(&value).unwrap()
}

fn main() {
    // required, not nullable: missing is rejected.
    must_fail::<Pet>(r#"{}"#, "name is required");
    // optional + nullable: three states, all exact.
    assert_eq!(round_trip::<Pet>(r#"{"name":"a"}"#), r#"{"name":"a"}"#);
    assert_eq!(round_trip::<Pet>(r#"{"name":"a","tag":null}"#), r#"{"name":"a","tag":null}"#);
    assert_eq!(round_trip::<Pet>(r#"{"name":"a","tag":"x"}"#), r#"{"name":"a","tag":"x"}"#);
    let pet: Pet = serde_json::from_str(r#"{"name":"a","tag":null}"#).unwrap();
    assert_eq!(pet.tag, Some(None));
    // optional, not nullable: explicit null is rejected.
    must_fail::<Pet>(r#"{"name":"a","status":null}"#, "status is not nullable");
    let pet: Pet = serde_json::from_str(r#"{"name":"a","status":"sold"}"#).unwrap();
    assert_eq!(pet.status, Some(PetStatus::Sold));
    must_fail::<Pet>(r#"{"name":"a","status":"lost"}"#, "not a member of the enum");
    // additionalProperties: false
    must_fail::<Pet>(r#"{"name":"a","colour":"red"}"#, "unknown member");
    // required + nullable: null accepted, missing rejected.
    must_fail::<Owner>(r#"{}"#, "name is required even though nullable");
    let owner: Owner = serde_json::from_str(r#"{"name":null}"#).unwrap();
    assert_eq!(owner.name, None);
    assert_eq!(round_trip::<Owner>(r#"{"name":null,"extra":1}"#), r#"{"name":null,"extra":1}"#);
    // integers read the token exactly.
    let c: Counter = serde_json::from_str(r#"{"n":1.0,"m":1e0}"#).unwrap();
    assert_eq!((c.n, c.m), (Some(Int64(1)), Some(Int32(1))));
    must_fail::<Counter>(r#"{"n":1.5}"#, "not an integer");
    must_fail::<Counter>(r#"{"n":1.00000000000000000001}"#, "not an integer, however close");
    must_fail::<Counter>(r#"{"m":3000000000}"#, "outside int32");
    // Bounded work and exact answers at the edges of the token grammar.
    let c: Counter = serde_json::from_str(r#"{"n":0e2147483647,"m":0e-5}"#).unwrap();
    assert_eq!((c.n, c.m), (Some(Int64(0)), Some(Int32(0))));
    must_fail::<Counter>(r#"{"n":1e2147483647}"#, "far beyond int64");
    must_fail::<Counter>(r#"{"n":1.5e-2147483648}"#, "exponent at the i32 edge");
    let c: Counter = serde_json::from_str(r#"{"n":0e99999999999,"m":0.0e-99999999999}"#).unwrap();
    assert_eq!((c.n, c.m), (Some(Int64(0)), Some(Int32(0))));
    must_fail::<Counter>(r#"{"n":1e99999999999}"#, "far beyond int64");
    // The `const` pin lives in the branch type too.
    must_fail::<Cat>(r#"{"petType":"Dog"}"#, "Cat pins petType to Cat");
    let c: Counter = serde_json::from_str(r#"{"n":1.20e1}"#).unwrap();
    assert_eq!(c.n, Some(Int64(12)));
    let c: Counter = serde_json::from_str(r#"{"n":9007199254740993}"#).unwrap();
    assert_eq!(c.n, Some(Int64(9007199254740993)));
    assert_eq!(round_trip::<Counter>(r#"{"n":-7}"#), r#"{"n":-7}"#);
    // tagged union: the tag selects, the whole value decodes, the tag survives.
    let animal: Animal = serde_json::from_str(r#"{"petType":"Cat","lives":9}"#).unwrap();
    let Animal::Cat(cat) = &animal else { panic!("expected Cat, got {animal:?}") };
    assert_eq!(cat.lives, Some(Int64(9)));
    assert_eq!(serde_json::to_string(&animal).unwrap(), r#"{"lives":9,"petType":"Cat"}"#);
    must_fail::<Animal>(r#"{"petType":"Fish"}"#, "unknown tag");
    must_fail::<Animal>(r#"{"lives":9}"#, "missing tag");
    // open union: raw kept, accessors typed, nothing selected.
    let loose: Loose = serde_json::from_str(r#""hello""#).unwrap();
    assert_eq!(loose.as_loose_variant_1(), Some("hello".to_owned()));
    assert_eq!(loose.as_loose_variant_2(), None);
    assert_eq!(serde_json::to_string(&loose).unwrap(), r#""hello""#);
    let built = Loose::from_loose_variant_2(&true).unwrap();
    assert_eq!(built.as_loose_variant_2(), Some(true));
    // composed open: projections over the same raw value.
    let composed: Composed = serde_json::from_str(r#"{"petType":"Cat","n":1}"#).unwrap();
    assert!(composed.as_cat().is_some());
    let cat: Cat = serde_json::from_str(r#"{"petType":"Cat"}"#).unwrap();
    assert!(Composed::from_cat(&cat).unwrap().as_cat().is_some());
    assert_eq!(composed, composed.clone());
    // cycles through Box.
    let node: Node = serde_json::from_str(r#"{"next":{"done":null}}"#).unwrap();
    let inner = node.next.unwrap();
    assert!(inner.next.is_none());
    assert_eq!(inner.done, Some(None));
    must_fail::<Node>(r#"{"next":null}"#, "next is optional but not nullable");
    // a newtype over a list, and a type alias.
    must_fail::<Pet>(r#"{"name":null}"#, "name is not nullable");
    must_fail::<Ids>(r#"["a",null]"#, "items are not nullable");
    must_fail::<Ids>(r#"null"#, "the list is not nullable");
    let ids: Ids = serde_json::from_str(r#"["a","b"]"#).unwrap();
    assert_eq!(ids.0.len(), 2);
    let _: Alias = serde_json::from_str(r#"{"petType":"Cat"}"#).unwrap();
    // A parent schema's discriminator selects among the schemas building on
    // it: by mapping, by component name, and anything else is the parent.
    let shelter: Shelter = serde_json::from_str(
        r#"{"first":{"kind":"meow","lives":9},"pets":[{"kind":"Doggo","bark":"woof"},{"kind":"Base","name":"b"},{"kind":"other","name":"o"}]}"#,
    )
    .unwrap();
    assert!(matches!(&shelter.first, Some(BaseUnion::Kitty(k)) if k.lives.map(|n| n.0) == Some(9)));
    let pets = shelter.pets.as_ref().unwrap();
    assert!(matches!(&pets[0], BaseUnion::Doggo(d) if d.bark.as_deref() == Some("woof")));
    assert!(matches!(&pets[1], BaseUnion::Base(b) if b.name.as_deref() == Some("b")));
    assert!(matches!(&pets[2], BaseUnion::Base(b) if b.kind == "other"));
    // `mapping` overrides a value, not a schema: `Kitty` is still selected
    // by its name.
    let shelter: Shelter = serde_json::from_str(r#"{"first":{"kind":"Kitty","lives":3}}"#).unwrap();
    assert!(matches!(&shelter.first, Some(BaseUnion::Kitty(_))));
    // `mapping` gives the parent's name to `MDog`: the parent is read only
    // as the fallback, its made-up tag selecting nothing.
    let held: MHolder = serde_json::from_str(r#"{"m":{"kind":"MPet","bark":"b"}}"#).unwrap();
    assert!(matches!(held.m, Some(MPetUnion::MDog(_))));
    let held: MHolder = serde_json::from_str(r#"{"m":{"kind":"MPet~"}}"#).unwrap();
    assert!(matches!(held.m, Some(MPetUnion::MPet(_))));
    // A null tag is read as a missing one.
    let held: NHolder = serde_json::from_str(r#"{"n":{"kind":null}}"#).unwrap();
    assert!(matches!(held.n, Some(NBaseUnion::NBase(_))));
    let held: NHolder = serde_json::from_str(r#"{"n":{"kind":"NCat","lives":1}}"#).unwrap();
    assert!(matches!(held.n, Some(NBaseUnion::NCat(_))));
    assert_eq!(
        round_trip::<Shelter>(r#"{"first":{"kind":"meow","lives":9}}"#),
        r#"{"first":{"kind":"meow","lives":9}}"#
    );
    // A mapping key wins over a component name; a schema building on one
    // building on the parent is a variant too; the tag may be a `$ref`.
    let held: TagHolder = serde_json::from_str(r#"{"t":{"kind":"Doggy","c":"x"}}"#).unwrap();
    assert!(matches!(held.t, Some(TaggedUnion::Catty(_))));
    let held: TagHolder = serde_json::from_str(r#"{"t":{"kind":"Lion","l":"y"}}"#).unwrap();
    assert!(matches!(held.t, Some(TaggedUnion::Lion(_))));
    let held: TagHolder = serde_json::from_str(r#"{"t":{"kind":"other"}}"#).unwrap();
    assert!(matches!(held.t, Some(TaggedUnion::Tagged(_))));
    // A null is no variant, even of a parent boxed in a cycle.
    assert!(serde_json::from_str::<Forest>(r#"{"trees":[null]}"#).is_err());
    assert!(serde_json::from_str::<Forest>(r#"{"trees":[[]]}"#).is_err());
    assert!(serde_json::from_str::<Forest>(r#"{"trees":[{"kind":5}]}"#).is_err());
    assert!(serde_json::from_str::<Forest>(r#"{"trees":[{"kind":"x","child":{"kind":"Leaf"}}]}"#).is_ok());
    // A `Kitty` on its own is still a `Kitty`.
    let _: Kitty = serde_json::from_str(r#"{"kind":"meow","lives":1}"#).unwrap();
    // Beside `additionalProperties: false`, a member must be a property or
    // match a pattern; pattern members land in the catch-all.
    assert_eq!(
        round_trip::<Extensible>(r#"{"id":"a","x-k":"v","n12":5}"#),
        r#"{"id":"a","n12":5,"x-k":"v"}"#
    );
    assert!(serde_json::from_str::<Extensible>(r#"{"id":"a","zz":1}"#).is_err());
    assert!(serde_json::from_str::<Extensible>(r#"{"n1x":1}"#).is_err());
    // A pattern the generator cannot translate lets every member in.
    assert!(serde_json::from_str::<Untranslated>(r#"{"zz":1}"#).is_ok());
    assert_eq!(round_trip::<Holder>(r#"{"needed":null}"#), r#"{"needed":null}"#);
    assert_eq!(
        round_trip::<Holder>(r#"{"any":null,"loose":null,"needed":1,"nothing":null,"ref":null}"#),
        r#"{"any":null,"loose":null,"needed":1,"nothing":null,"ref":null}"#
    );
    must_fail::<Holder>(r#"{}"#, "needed is required");
    assert_eq!(round_trip::<Dash>(r#"{"-":"x"}"#), r#"{"-":"x"}"#);
    assert_eq!(
        round_trip::<Odd>(r#"{"":1,"a,b":"x","q\"t":null,"x`y":true,"zz":2,"aa":3}"#),
        r#"{"":1,"a,b":"x","q\"t":null,"x`y":true,"aa":3,"zz":2}"#
    );
    assert_eq!(round_trip::<Odd>(r#"{"a,b":"x"}"#), r#"{"a,b":"x"}"#);
    must_fail::<Odd>(r#"{"":1}"#, "a,b is required");
    assert_eq!(
        round_trip::<Wrapper>(r#"{"owner":{"name":null,"x":1}}"#),
        r#"{"owner":{"name":null,"x":1}}"#
    );
    must_fail::<Wrapper>(r#"{"zz":1}"#, "Wrapper is closed");
    println!("generated code behaves");
}
"##;

/// One description exercising every case both backends must get right.
fn description() -> serde_json::Value {
    let raw = serde_json::json!({
        "openapi": "3.2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {
            "Pet": {
                "type": "object", "additionalProperties": false, "required": ["name"],
                "properties": {
                    "name": {"type": "string", "minLength": 1},
                    "tag": {"type": ["string", "null"]},
                    "status": {"type": "string", "enum": ["available", "sold"]}
                }
            },
            "Owner": {"type": "object", "required": ["name"], "properties": {"name": {"type": ["string", "null"]}}},
            "Counter": {"type": "object", "properties": {"n": {"type": "integer"}, "m": {"type": "integer", "format": "int32"}}},
            "Cat": {"type": "object", "required": ["petType"], "properties": {"petType": {"type": "string", "const": "Cat"}, "lives": {"type": "integer"}}},
            "Dog": {"type": "object", "required": ["petType"], "properties": {"petType": {"type": "string", "enum": ["Dog"]}}},
            "Animal": {"oneOf": [{"$ref": "#/components/schemas/Cat"}, {"$ref": "#/components/schemas/Dog"}], "discriminator": {"propertyName": "petType"}},
            "Loose": {"oneOf": [{"type": "string"}, {"type": "boolean"}]},
            "Composed": {"allOf": [{"$ref": "#/components/schemas/Cat"}, {"type": "object", "additionalProperties": false}]},
            "Node": {"type": "object", "properties": {"next": {"type": ["object", "null"], "$comment": "x"}}},
            "Ids": {"type": "array", "items": {"type": "string"}},
            "Alias": {"$ref": "#/components/schemas/Cat"},
            "Base": {"type": "object", "required": ["kind"],
                "properties": {"kind": {"type": "string"}, "name": {"type": "string"}},
                "discriminator": {"propertyName": "kind", "mapping": {"meow": "#/components/schemas/Kitty"}}},
            "Kitty": {"allOf": [{"$ref": "#/components/schemas/Base"},
                {"type": "object", "properties": {"lives": {"type": "integer"}}}]},
            "Doggo": {"allOf": [{"$ref": "#/components/schemas/Base"},
                {"type": "object", "properties": {"bark": {"type": "string"}}}]},
            "NBase": {"type": "object", "properties": {"kind": {"type": ["string", "null"]}},
                "discriminator": {"propertyName": "kind"}},
            "NCat": {"allOf": [{"$ref": "#/components/schemas/NBase"},
                {"type": "object", "properties": {"lives": {"type": "integer"}}}]},
            "NHolder": {"type": "object", "properties": {"n": {"$ref": "#/components/schemas/NBase"}}},
            "MPet": {"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string"}},
                "discriminator": {"propertyName": "kind", "mapping": {"MPet": "#/components/schemas/MDog"}}},
            "MDog": {"allOf": [{"$ref": "#/components/schemas/MPet"},
                {"type": "object", "properties": {"bark": {"type": "string"}}}]},
            "MHolder": {"type": "object", "properties": {"m": {"$ref": "#/components/schemas/MPet"}}},
            "Shelter": {"type": "object", "properties": {
                "pets": {"type": "array", "items": {"$ref": "#/components/schemas/Base"}},
                "first": {"$ref": "#/components/schemas/Base"}}},
            "TagKind": {"type": "string", "enum": ["Doggy", "Catty", "Lion", "Tagged", "other"]},
            "Tagged": {"type": "object", "required": ["kind"],
                "properties": {"kind": {"$ref": "#/components/schemas/TagKind"}},
                "discriminator": {"propertyName": "kind", "mapping": {"Doggy": "#/components/schemas/Catty"}}},
            "Catty": {"allOf": [{"$ref": "#/components/schemas/Tagged"},
                {"type": "object", "properties": {"c": {"type": "string"}}}]},
            "Doggy": {"allOf": [{"$ref": "#/components/schemas/Tagged"},
                {"type": "object", "properties": {"d": {"type": "string"}}}]},
            "Lion": {"allOf": [{"$ref": "#/components/schemas/Catty"},
                {"type": "object", "properties": {"l": {"type": "string"}}}]},
            "TagHolder": {"type": "object", "properties": {"t": {"$ref": "#/components/schemas/Tagged"}}},
            "Tree": {"type": "object", "required": ["kind"],
                "properties": {"kind": {"type": "string"}, "child": {"$ref": "#/components/schemas/Tree"}},
                "discriminator": {"propertyName": "kind"}},
            "Leaf": {"allOf": [{"$ref": "#/components/schemas/Tree"}, {"type": "object"}]},
            "Forest": {"type": "object", "properties": {
                "trees": {"type": "array", "items": {"$ref": "#/components/schemas/Tree"}}}},
            "Extensible": {"type": "object", "additionalProperties": false,
                "properties": {"id": {"type": "string"}},
                "patternProperties": {"^x-": {"type": "string"}, "^n\\d+$": {"type": "integer"}}},
            "Untranslated": {"type": "object", "additionalProperties": false,
                "properties": {"id": {"type": "string"}},
                "patternProperties": {"\\bx": {}}}
        }}
    });
    // `Node.next` must reference Node; write it as a reference.
    let mut raw = raw;
    raw["components"]["schemas"]["Node"]["properties"]["next"] =
        serde_json::json!({"oneOf": [{"$ref": "#/components/schemas/Node"}, {"type": "null"}]});
    // A nullable reference is spelled `oneOf [ref, null]` in 3.1; the
    // generator reads that as an open union. Use the plain form instead.
    raw["components"]["schemas"]["Node"]["properties"]["next"] =
        serde_json::json!({"$ref": "#/components/schemas/Node"});
    raw["components"]["schemas"]["Node"]["properties"]["done"] =
        serde_json::json!({"type": ["boolean", "null"]});
    // Types whose values include `null`, directly and through a reference.
    raw["components"]["schemas"]["Anything"] = serde_json::json!({});
    raw["components"]["schemas"]["Holder"] = serde_json::json!({
        "type": "object", "required": ["needed"],
        "properties": {
            "any": {}, "nothing": {"type": "null"},
            "ref": {"$ref": "#/components/schemas/Anything"},
            "needed": {"$ref": "#/components/schemas/Anything"},
            "loose": {"$ref": "#/components/schemas/Loose"}
        }
    });
    // A closed struct holding an open one: the open one keeps its extras.
    raw["components"]["schemas"]["Wrapper"] = serde_json::json!({
        "type": "object", "additionalProperties": false,
        "properties": {"owner": {"$ref": "#/components/schemas/Owner"}}
    });
    // Member names no Go struct tag can carry, beside a catch-all.
    raw["components"]["schemas"]["Odd"] = serde_json::json!({
        "type": "object", "required": ["a,b"],
        "properties": {
            "": {"type": "integer"}, "a,b": {"type": "string"},
            "q\"t": {"type": ["string", "null"]}, "x`y": {"type": "boolean"}
        }
    });
    // A member named `-`, which a bare `json:"-"` tag would skip.
    raw["components"]["schemas"]["Dash"] = serde_json::json!({
        "type": "object", "required": ["-"], "properties": {"-": {"type": "string"}}
    });
    raw
}

fn generation(target: Target) -> roas_codegen::Generation {
    let bytes = serde_json::to_vec(&description()).unwrap();
    let doc =
        SourceDocument::parse(Input::Json(bytes), Url::parse("file:///t.json").unwrap()).unwrap();
    let doc = validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).unwrap();
    let mut config = ConfigFile::default();
    config.target = Some(target);
    let config = config.build().unwrap();
    let generation = generate(&doc, &config).expect("generates");
    assert!(!generation.has_errors(), "{:?}", generation.diagnostics);
    generation
}

#[test]
fn generated_rust_compiles_and_behaves() {
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
    let generation = generation(Target::Rust);

    let dir = std::env::temp_dir().join(format!("roas-codegen-compile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let deps: String = generation
        .dependencies
        .rust
        .iter()
        .map(|d| {
            format!(
                "{} = {{ version = \"1\", features = {:?} }}\n",
                d.name, d.features
            )
        })
        .collect();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"generated\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\n{deps}"),
    )
    .unwrap();
    for file in &generation.files {
        std::fs::write(dir.join("src").join(&file.path), &file.contents).unwrap();
    }
    std::fs::write(dir.join("src/main.rs"), MAIN).unwrap();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["run", "--quiet"])
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .output()
        .expect("cargo runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("generated code behaves"),
        "generated crate failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}\n--- types.rs\n{}",
        generation.files[0].contents
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = Path::new("");
}

const GO_MAIN: &str = r##"package main

import (
	jsonv1 "encoding/json"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"

	"generated/api"
)

func mustFail[T any](js, why string) {
	var v T
	if err := json.Unmarshal([]byte(js), &v); err == nil {
		panic(fmt.Sprintf("%s should have been rejected (%s) but gave %+v", js, why, v))
	}
}

func decode[T any](js string) T {
	var v T
	if err := json.Unmarshal([]byte(js), &v); err != nil {
		panic(fmt.Sprintf("%s: %v", js, err))
	}
	return v
}

func encode(v any) string {
	out, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	return string(out)
}

func roundTrip[T any](js string) string {
	return encode(decode[T](js))
}

func check(got, want any) {
	if got != want {
		panic(fmt.Sprintf("got %v, want %v", got, want))
	}
}

func main() {
	// required, not nullable: missing and null are rejected.
	mustFail[api.Pet](`{}`, "name is required")
	mustFail[api.Pet](`{"name":null}`, "name is not nullable")
	// optional + nullable: three states, all exact.
	check(roundTrip[api.Pet](`{"name":"a"}`), `{"name":"a"}`)
	check(roundTrip[api.Pet](`{"name":"a","tag":null}`), `{"name":"a","tag":null}`)
	check(roundTrip[api.Pet](`{"name":"a","tag":"x"}`), `{"name":"a","tag":"x"}`)
	pet := decode[api.Pet](`{"name":"a","tag":null}`)
	check(pet.Tag.Set && pet.Tag.Value == nil, true)
	// optional, not nullable: explicit null is rejected.
	mustFail[api.Pet](`{"name":"a","status":null}`, "status is not nullable")
	pet = decode[api.Pet](`{"name":"a","status":"sold"}`)
	check(*pet.Status, api.PetStatusSold)
	mustFail[api.Pet](`{"name":"a","status":"lost"}`, "not a member of the enum")
	// additionalProperties: false
	mustFail[api.Pet](`{"name":"a","colour":"red"}`, "unknown member")
	// required + nullable: null accepted, missing rejected.
	mustFail[api.Owner](`{}`, "name is required even though nullable")
	owner := decode[api.Owner](`{"name":null}`)
	check(owner.Name.Set && owner.Name.Value == nil, true)
	check(roundTrip[api.Owner](`{"name":null,"extra":1}`), `{"name":null,"extra":1}`)
	// integers read the token exactly.
	c := decode[api.Counter](`{"n":1.0,"m":1e0}`)
	check(*c.N, api.Int64(1))
	check(*c.M, api.Int32(1))
	mustFail[api.Counter](`{"n":1.5}`, "not an integer")
	mustFail[api.Counter](`{"n":1.00000000000000000001}`, "not an integer, however close")
	mustFail[api.Counter](`{"m":3000000000}`, "outside int32")
	mustFail[api.Counter](`{"n":null}`, "n is not nullable")
	// Bounded work and exact answers at the edges of the token grammar.
	c = decode[api.Counter](`{"n":0e2147483647,"m":0e-5}`)
	check(*c.N, api.Int64(0))
	check(*c.M, api.Int32(0))
	mustFail[api.Counter](`{"n":1e2147483647}`, "far beyond int64")
	mustFail[api.Counter](`{"n":1.5e-2147483648}`, "exponent at the i32 edge")
	c = decode[api.Counter](`{"n":0e99999999999,"m":0.0e-99999999999}`)
	check(*c.N, api.Int64(0))
	mustFail[api.Counter](`{"n":1e99999999999}`, "far beyond int64")
	c = decode[api.Counter](`{"n":1.20e1}`)
	check(*c.N, api.Int64(12))
	c = decode[api.Counter](`{"n":9007199254740993}`)
	check(*c.N, api.Int64(9007199254740993))
	c = decode[api.Counter](`{"n":-9223372036854775808,"m":-2147483648}`)
	check(*c.N, api.Int64(-9223372036854775808))
	check(*c.M, api.Int32(-2147483648))
	mustFail[api.Counter](`{"n":9223372036854775808}`, "just past int64")
	mustFail[api.Counter](`{"m":2147483648}`, "just past int32")
	check(roundTrip[api.Counter](`{"n":-7}`), `{"n":-7}`)
	// The `const` pin lives in the branch type too.
	mustFail[api.Cat](`{"petType":"Dog"}`, "Cat pins petType to Cat")
	// tagged union: the tag selects, the whole value decodes, the tag survives.
	animal := decode[api.Animal](`{"lives":9,"petType":"Cat"}`)
	cat, ok := animal.AsCat()
	check(ok, true)
	check(*cat.Lives, api.Int64(9))
	_, ok = animal.AsDog()
	check(ok, false)
	check(animal.Tag(), "Cat")
	check(encode(animal), `{"lives":9,"petType":"Cat"}`)
	check(encode(api.AnimalFromCat(cat)), `{"lives":9,"petType":"Cat"}`)
	mustFail[api.Animal](`{"petType":"Fish"}`, "unknown tag")
	mustFail[api.Animal](`{"lives":9}`, "missing tag")
	mustFail[api.Animal](`null`, "not an object")
	if _, err := json.Marshal(api.Animal{}); err == nil {
		panic("the zero Animal should not marshal")
	}
	// open union: raw kept, accessors typed, nothing selected.
	loose := decode[api.Loose](`"hello"`)
	s, ok := loose.AsLooseVariant1()
	check(ok, true)
	check(s, "hello")
	_, ok = loose.AsLooseVariant2()
	check(ok, false)
	check(encode(loose), `"hello"`)
	built, err := api.LooseFromLooseVariant2(true)
	check(err, nil)
	b, ok := built.AsLooseVariant2()
	check(ok && b, true)
	// composed open: projections over the same raw value.
	composed := decode[api.Composed](`{"petType":"Cat","n":1}`)
	_, ok = composed.AsCat()
	check(ok, true)
	fromCat, err := api.ComposedFromCat(decode[api.Cat](`{"petType":"Cat"}`))
	check(err, nil)
	_, ok = fromCat.AsCat()
	check(ok, true)
	check(encode(api.ComposedFromRaw(composed.Raw())), `{"petType":"Cat","n":1}`)
	// cycles through a pointer.
	node := decode[api.Node](`{"next":{"done":null}}`)
	check(node.Next.Next == nil, true)
	check(node.Next.Done.Set && node.Next.Done.Value == nil, true)
	mustFail[api.Node](`{"next":null}`, "next is optional but not nullable")
	// a newtype over a list, and a type alias.
	mustFail[api.Ids](`["a",null]`, "items are not nullable")
	mustFail[api.Ids](`null`, "the list is not nullable")
	check(len(decode[api.Ids](`["a","b"]`)), 2)
	check(roundTrip[api.Ids](`["a","b"]`), `["a","b"]`)
	var _ api.Cat = decode[api.Alias](`{"petType":"Cat"}`)
	shelter := decode[api.Shelter](`{"first":{"kind":"meow","lives":9},"pets":[{"kind":"Doggo","bark":"woof"},{"kind":"Base","name":"b"},{"kind":"other","name":"o"}]}`)
	if k, ok := shelter.First.AsKitty(); !ok || int64(*k.Lives) != 9 {
		panic(fmt.Sprintf("first: %+v", shelter.First))
	}
	pets := *shelter.Pets
	if d, ok := pets[0].AsDoggo(); !ok || *d.Bark != "woof" {
		panic(fmt.Sprintf("pets[0]: %+v", pets[0]))
	}
	if b, ok := pets[1].AsBase(); !ok || *b.Name != "b" {
		panic(fmt.Sprintf("pets[1]: %+v", pets[1]))
	}
	if b, ok := pets[2].AsBase(); !ok || b.Kind != "other" {
		panic(fmt.Sprintf("pets[2]: %+v", pets[2]))
	}
	// mapping overrides a value, not a schema: Kitty is still selected by
	// its name.
	if _, ok := decode[api.Shelter](`{"first":{"kind":"Kitty","lives":3}}`).First.AsKitty(); !ok {
		panic("Kitty is selected by its name too")
	}
	check(roundTrip[api.Shelter](`{"first":{"kind":"meow","lives":9}}`), `{"first":{"kind":"meow","lives":9}}`)
	// `mapping` gives the parent's name to MDog: the parent is read only as
	// the fallback, its made-up tag selecting nothing.
	if _, ok := decode[api.MHolder](`{"m":{"kind":"MPet","bark":"b"}}`).M.AsMDog(); !ok {
		panic("MPet is mapped to MDog")
	}
	if _, ok := decode[api.MHolder](`{"m":{"kind":"MPet~"}}`).M.AsMPet(); !ok {
		panic("a made-up tag is read as the fallback")
	}
	// A null tag is read as a missing one.
	if _, ok := decode[api.NHolder](`{"n":{"kind":null}}`).N.AsNBase(); !ok {
		panic("a null tag is read as the parent")
	}
	if _, ok := decode[api.NHolder](`{"n":{"kind":"NCat","lives":1}}`).N.AsNCat(); !ok {
		panic("NCat is selected by name")
	}
	if _, ok := decode[api.TagHolder](`{"t":{"kind":"Doggy","c":"x"}}`).T.AsCatty(); !ok {
		panic("Doggy is mapped to Catty")
	}
	if _, ok := decode[api.TagHolder](`{"t":{"kind":"Lion","l":"y"}}`).T.AsLion(); !ok {
		panic("Lion builds on Catty")
	}
	mustFail[api.Forest](`{"trees":[null]}`, "a null is no Tree")
	mustFail[api.Forest](`{"trees":[[]]}`, "an array is no Tree")
	mustFail[api.Forest](`{"trees":[{"kind":5}]}`, "a tag is a string")
	check(len(*decode[api.Forest](`{"trees":[{"kind":"x","child":{"kind":"Leaf"}}]}`).Trees), 1)
	if _, ok := decode[api.TagHolder](`{"t":{"kind":"other"}}`).T.AsTagged(); !ok {
		panic("other is read as the parent")
	}
	check(len(decode[api.Extensible](`{"id":"a","x-k":"v","n12":5}`).AdditionalProperties), 2)
	mustFail[api.Extensible](`{"id":"a","zz":1}`, "zz matches no pattern")
	mustFail[api.Extensible](`{"n1x":1}`, "n1x matches no pattern")
	check(len(decode[api.Untranslated](`{"zz":1}`).AdditionalProperties), 1)
	// null is a value of `{}`, `type: null` and an open holder: kept, not lost.
	check(roundTrip[api.Holder](`{"needed":null}`), `{"needed":null}`)
	check(roundTrip[api.Holder](`{"any":null,"loose":null,"needed":1,"nothing":null,"ref":null}`),
		`{"any":null,"loose":null,"needed":1,"nothing":null,"ref":null}`)
	check(roundTrip[api.Holder](`{"needed":{"a":[1]}}`), `{"needed":{"a":[1]}}`)
	mustFail[api.Holder](`{}`, "needed is required")
	check(roundTrip[api.Dash](`{"-":"x"}`), `{"-":"x"}`)
	odd := "{\"\":1,\"a,b\":\"x\",\"q\\\"t\":null,\"x`y\":true,\"zz\":2,\"aa\":3}"
	check(roundTrip[api.Odd](odd), "{\"\":1,\"a,b\":\"x\",\"q\\\"t\":null,\"x`y\":true,\"aa\":3,\"zz\":2}")
	check(roundTrip[api.Odd](`{"a,b":"x"}`), `{"a,b":"x"}`)
	// encoding/json v1 calls the encoder on a value it cannot address, too.
	var oddValue api.Odd
	if err := json.Unmarshal([]byte(odd), &oddValue); err != nil {
		panic(err)
	}
	v1, err := jsonv1.Marshal(map[string]api.Odd{"k": oddValue})
	check(err, nil)
	check(string(v1), "{\"k\":{\"\":1,\"a,b\":\"x\",\"q\\\"t\":null,\"x`y\":true,\"aa\":3,\"zz\":2}}")
	mustFail[api.Odd](`{"":1}`, "a,b is required")
	check(roundTrip[api.Wrapper](`{"owner":{"name":null,"x":1}}`), `{"owner":{"name":null,"x":1}}`)
	mustFail[api.Wrapper](`{"zz":1}`, "Wrapper is closed")
	// A malformed literal is a syntax error, not a null.
	for _, in := range []string{`{"name":nope}`, `{"name":"a","status":nUll}`} {
		var pet api.Pet
		var syntax *jsontext.SyntacticError
		if err := json.Unmarshal([]byte(in), &pet); !errors.As(err, &syntax) {
			panic(fmt.Sprintf("%s: want a syntax error, got %v", in, err))
		}
	}
	var ids api.Ids
	var syntax *jsontext.SyntacticError
	if err := json.Unmarshal([]byte(`["a",nope]`), &ids); !errors.As(err, &syntax) {
		panic(fmt.Sprintf("want a syntax error, got %v", err))
	}
	mustFail[api.Dash](`{}`, "- is required")
	fmt.Println("generated code behaves")
}
"##;

/// Go 1.27 or later, which `encoding/json/v2` without an experiment
/// needs, or why not.
fn go_toolchain() -> Result<(), String> {
    let output = Command::new("go")
        .args(["env", "GOVERSION"])
        .env("GOTOOLCHAIN", "local")
        .output()
        .map_err(|e| format!("go could not be started: {e}"))?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let minor = version
        .strip_prefix("go1.")
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|minor| minor.parse::<u32>().ok());
    match minor {
        Some(minor) if minor >= 27 => Ok(()),
        _ => Err(format!("{version:?} is older than go1.27 or unrecognised")),
    }
}

#[test]
fn generated_go_compiles_and_behaves() {
    let required = std::env::var_os("ROAS_CODEGEN_REQUIRE_GO").is_some();
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() && !required {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
    if let Err(reason) = go_toolchain() {
        assert!(!required, "ROAS_CODEGEN_REQUIRE_GO is set: {reason}");
        eprintln!("skipped: {reason}");
        return;
    }
    let generation = generation(Target::Go);
    assert!(generation.dependencies.go.is_empty());

    let dir = std::env::temp_dir().join(format!("roas-codegen-go-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("api")).unwrap();
    std::fs::write(dir.join("go.mod"), "module generated\n\ngo 1.27\n").unwrap();
    for file in &generation.files {
        std::fs::write(dir.join("api").join(&file.path), &file.contents).unwrap();
    }
    std::fs::write(dir.join("main.go"), GO_MAIN).unwrap();
    let output = Command::new("go")
        .args(["run", "."])
        .current_dir(&dir)
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-mod=mod")
        .env("GOWORK", "off")
        .output()
        .expect("go runs");
    let vet = Command::new("go")
        .args(["vet", "./..."])
        .current_dir(&dir)
        .env("GOTOOLCHAIN", "local")
        .env("GOWORK", "off")
        .output()
        .expect("go vet runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("generated code behaves"),
        "generated Go failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}\n--- {}\n{}",
        generation.files[0].path.display(),
        generation.files[0].contents
    );
    assert!(
        vet.status.success(),
        "go vet\n{}",
        String::from_utf8_lossy(&vet.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A description with a constraint of every checked kind, for the
/// validation tests of both languages.
fn validated_description() -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {
            "Pet": {
                "type": "object", "required": ["name"], "minProperties": 2, "maxProperties": 4,
                "properties": {
                    "name": {"type": "string", "minLength": 1, "maxLength": 5},
                    "age": {"type": "integer", "minimum": 0, "exclusiveMaximum": 30, "multipleOf": 2},
                    "weight": {"type": "number", "minimum": 0.5, "exclusiveMaximum": 100},
                    "ratio": {"type": "number", "multipleOf": 1},
                    "tags": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 3, "uniqueItems": true},
                    "code": {"type": "string", "pattern": "^a"},
                    "owner": {"$ref": "#/components/schemas/Owner"},
                    "friends": {"type": "array", "items": {"$ref": "#/components/schemas/Owner"}},
                    "extra": {"type": "object", "additionalProperties": {"type": "integer"}, "maxProperties": 1}
                }
            },
            "Owner": {"type": "object", "properties": {"nick": {"type": ["string", "null"], "maxLength": 3}}},
            "Short": {"type": "string", "maxLength": 2},
            "Patterned": {"type": "object", "properties": {
                "digits": {"type": "string", "pattern": "^\\d+$"},
                "space": {"type": "string", "pattern": "^\\s$"},
                "dot": {"type": "string", "pattern": "^.$"},
                "word": {"type": "string", "pattern": "^\\w+$"},
                "legacy": {"type": "string", "pattern": "^\\d{3}\\-\\d{4}$"},
                "amp": {"type": "string", "pattern": "^[a&&b]+$"},
                "astral": {"type": "string", "pattern": "^😀{2}$"},
                "boundary": {"type": "string", "pattern": "\\bx"},
                "maybe": {"type": ["string", "null"], "pattern": "a"},
                "none": {"type": "string", "pattern": "[]"},
                "any": {"type": "string", "pattern": "^[^]$"}
            }},
            "Coded": {"type": "string", "enum": ["ab", "x1"], "pattern": "^[a-z]+$"},
            "Either": {"oneOf": [{"type": "string", "pattern": "^a"}, {"type": "string", "pattern": "z$"}]},
            "Code": {"type": "string", "pattern": "^[A-Z]{2}$"},
            "Each": {"type": "object", "properties": {
                "grid": {"type": "array", "items": {"type": "array", "maxItems": 2, "items": {"type": "integer", "minimum": 0}}},
                "scores": {"type": "object", "additionalProperties": {"type": "number", "maximum": 1}},
                "maybe": {"type": "array", "items": {"type": ["string", "null"], "minLength": 2}},
                "codes": {"type": "array", "items": {"type": "string", "pattern": "^[a-z]+$"}},
                "odd": {"type": "array", "items": {"type": "string", "x-foo": 1, "if": {"type": "string"}, "then": true}}
            }},
            "Pf": {"type": "object", "properties": {"p": {"type": "array", "prefixItems": [{"type": "string"}], "items": {"type": "string", "maxLength": 1}}}},
            "Pu": {"oneOf": [
                {"type": "array", "prefixItems": [{"type": "string"}], "items": {"type": "string", "maxLength": 1}},
                {"type": "array", "items": {"type": "integer"}}
            ]},
            "Pr": {"type": "array", "prefixItems": [{"type": "integer"}], "items": {"$ref": "#/components/schemas/Short"}},
            "Pn": {"type": "array", "prefixItems": [{"type": "integer"}], "items": false},
            "GatedRest": {"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": {"type": "string", "maxLength": 1}},
            "Names": {"type": "object", "propertyNames": {"type": "string", "pattern": "^[a-z]+$", "maxLength": 2},
                "properties": {"ok": {"type": "string"}, "Bad": {"type": "string"}},
                "additionalProperties": {"type": "string"}},
            "NamesLook": {"type": "object", "additionalProperties": {"type": "integer"}, "propertyNames": {"pattern": "a(?=b)"}},
            "NamesEnum": {"type": "object", "additionalProperties": {"type": "integer"}, "propertyNames": {"enum": ["a"]}},
            "NamesGated": {"type": "object", "additionalProperties": false, "propertyNames": {"maxLength": 1},
                "properties": {"ab": {"type": "string"}, "c": {"type": "string"}}},
            "NamesTrue": {"type": "object", "additionalProperties": {"type": "integer"}, "propertyNames": true},
            "EachList": {"type": "array", "items": {"type": "string", "maxLength": 1}},
            "Rest": {"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": {"type": "integer", "maximum": 5}},
            "Shorts": {"type": "array", "items": {"$ref": "#/components/schemas/Short"}},
            "Choice": {"oneOf": [{"$ref": "#/components/schemas/Short"}, {"$ref": "#/components/schemas/Owner"}]},
            "Numeric": {"type": ["integer", "number"]},
            "Shaped": {"properties": {"x": {"type": "string"}}},
            "Inline": {"oneOf": [{"type": "string", "maxLength": 2}, {"type": "integer"}]},
            "Letters": {"type": "string", "enum": ["a", "abcd"], "maxLength": 2},
            "Fine": {"type": "object", "properties": {
                "f": {"type": "number", "format": "float", "maximum": 0.1},
                "choice": {"oneOf": [{"type": "string"}, {"type": "integer"}]}
            }},
            "Named": {"type": "object", "additionalProperties": {"type": "integer"}, "propertyNames": {"maxLength": 3}},
            "Edge": {"type": "object", "properties": {
                "big": {"type": "string", "maxLength": 9223372036854775808u64, "minLength": 0},
                "m": {"type": "integer", "multipleOf": 9223372036854775808u64},
                "wide": {"type": "number", "format": "float", "maximum": 1e300, "minimum": -1e300},
                "many": {"type": "array", "items": {"type": "integer"}, "maxItems": 18446744073709551615u64}
            }},
            "Closed": {"type": "object", "additionalProperties": false, "propertyNames": {"maxLength": 1}, "properties": {"a": {"type": "string"}}},
            "TypeList": {"type": ["string", "integer"], "maxLength": 3, "minimum": 5},
            "MaybeShort": {"oneOf": [{"type": ["string", "null"], "maxLength": 2}, {"type": "integer"}]},
            "Picks": {"oneOf": [{"type": "integer", "enum": [1]}, {"type": "integer", "enum": [2]}]},
            "Itself": {"anyOf": [{"$ref": "#/components/schemas/Itself"}, {"type": "string"}]},
            "ChoiceAlias": {"$ref": "#/components/schemas/Choice"},
            "Holds": {"type": "object", "properties": {"c": {"$ref": "#/components/schemas/ChoiceAlias"}}},
            "OrNull": {"type": ["string", "integer", "null"], "maxLength": 1},
            "Base": {"type": "object", "properties": {"q": {"type": "string"}}},
            "Extended": {"$ref": "#/components/schemas/Base", "properties": {"z": {"type": "string", "maxLength": 1}}},
            "Tiny": {"type": "object", "properties": {"f": {"type": "number", "format": "float", "maximum": 3.5e38}}},
            "One": {"type": "integer", "enum": [1]},
            "Two": {"type": "integer", "enum": [2]},
            "PicksRef": {"oneOf": [{"$ref": "#/components/schemas/One"}, {"$ref": "#/components/schemas/Two"}]},
            "Old": {"type": "object", "properties": {"name": {"type": "string", "deprecated": true, "minLength": 1}}},
            "MaybeText": {"type": ["string", "null"]},
            "Both": {"$ref": "#/components/schemas/MaybeText", "type": "string"},
            "Sib": {"$ref": "#/components/schemas/Short", "minLength": 2},
            "SibC": {"$ref": "#/components/schemas/Short", "const": "zz"},
            "Ni": {"type": "integer", "enum": [1, 2]},
            "VBase": {"type": "object", "required": ["kind"],
                "properties": {"kind": {"type": "string"}, "name": {"type": "string", "maxLength": 3}},
                "discriminator": {"propertyName": "kind"}},
            "VKitty": {"allOf": [{"$ref": "#/components/schemas/VBase"},
                {"type": "object", "properties": {"lives": {"type": "integer", "maximum": 9}}}]},
            "VShelter": {"type": "object", "properties": {"first": {"$ref": "#/components/schemas/VBase"}}},
            "GBase": {"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string"}},
                "discriminator": {"propertyName": "kind", "defaultMapping": "#/components/schemas/GCat"}},
            "GCat": {"allOf": [{"$ref": "#/components/schemas/GBase"},
                {"type": "object", "properties": {"lives": {"type": "integer", "maximum": 9}}}]},
            "GHolder": {"type": "object", "properties": {"g": {"$ref": "#/components/schemas/GBase"}}},
            "FloatEnum": {"type": "number", "format": "float", "enum": [0.5, 1.25]},
            "DoubleEnum": {"type": "number", "enum": [0.1, 2]},
            "IntConst": {"type": "integer", "const": 7},
            "NoInt": {"type": "integer", "enum": []},
            "BigEnum": {"type": "number", "enum": [9007199254740993u64]},
            "MixedEnum": {"type": "number", "enum": [9007199254740992u64, 9007199254740993u64]},
            "FloatTenth": {"type": "number", "format": "float", "enum": [0.1]},
            "FloatEdge": {"type": "number", "format": "float", "enum": [3.4028235677973366e38]},
            "FloatHalf": {"type": "number", "format": "float", "enum": [1.0000000596046448]},
            "FloatHuge": {"type": "number", "format": "float", "enum": [1e39]},
            "IntConstFloat": {"type": "integer", "const": 9007199254740992.0},
            "IntOrNull": {"type": "object", "properties": {"n": {"type": ["integer", "null"], "enum": [1, null]}}},
            "Narrow": {"type": "integer", "format": "int32", "enum": [-5, 3000000000i64, 5]},
            "NumConst": {"type": "object", "properties": {"c": {"type": "number", "const": 1.5}}},
            "Pins": {"type": "object", "properties": {
                "n": {"type": "integer", "enum": [1, 2]},
                "k": {"type": "boolean", "const": true}
            }},
            "EnumO": {"type": ["string", "integer"], "enum": ["ab", 5]},
            "Def": {"type": ["integer", "string"], "default": "x", "minimum": 1},
            "EnumN": {"type": ["string", "null"], "enum": ["ab", "cde", null], "maxLength": 2},
            "F32X": {"type": "number", "format": "float", "exclusiveMaximum": 1},
            "HolderC": {"anyOf": [{"type": "string"}, {"type": "integer"}], "const": "only"},
            "NullC": {"type": "object", "required": ["n"], "properties": {"n": {"const": 3}}},
            "HoldsBoth": {"type": "object", "properties": {"b": {"$ref": "#/components/schemas/Both"}}},
            "NullSib": {"$ref": "#/components/schemas/MaybeText", "type": ["string", "null"]},
            "EdgeBranch": {"oneOf": [{"type": ["integer", "null"], "format": "int32", "minimum": -2147483648i64}, {"type": "string"}]},
            "FInf": {"type": "object", "properties": {
                "f": {"type": "number", "format": "float", "maximum": 1e40},
                "m": {"type": "number", "format": "float", "multipleOf": 1}
            }},
            "Mid": {"type": "object", "properties": {"f": {"type": "number", "format": "float", "maximum": 1.0000000596046448}}},
            "NoNull": {"oneOf": [{"type": ["string", "null"], "enum": ["a"]}, {"type": "integer"}]},
            "NoNullList": {"type": ["string", "boolean", "null"], "enum": ["a", true]},
            "PartNull": {"allOf": [
                {"type": ["string", "null"], "pattern": "^a", "if": {"type": "null"}, "then": false},
                {"type": ["string", "null"]}
            ]},
            "Items": {"oneOf": [
                {"type": "array", "items": {"type": "integer", "maximum": 3}},
                {"type": "array", "items": {"type": "integer", "minimum": 10}}
            ]},
            "NullOnly": {"type": ["string", "null"], "enum": [null]},
            "StrItems": {"oneOf": [
                {"type": "array", "items": {"type": "string", "minLength": 2}},
                {"type": "array", "items": {"type": "string", "maxLength": 1}}
            ]},
            "PatMap": {"oneOf": [
                {"type": "object", "patternProperties": {"^a": {"type": "integer"}}},
                {"type": "object", "patternProperties": {"^a": {"type": "string"}}}
            ]},
            "Scalar": {"const": 3},
            "HoldsScalar": {"type": "object", "properties": {"n": {"$ref": "#/components/schemas/Scalar"}}},
            "KLow": {"type": "object", "required": ["k"], "properties": {"k": {"type": "array", "items": {"type": "integer", "maximum": 3}}}},
            "KHigh": {"type": "object", "required": ["k"], "properties": {"k": {"type": "array", "items": {"type": "integer", "minimum": 10}}}},
            "PickK": {"oneOf": [{"$ref": "#/components/schemas/KLow"}, {"$ref": "#/components/schemas/KHigh"}]},
            "KHas": {"type": "object", "required": ["k"], "properties": {"k": {"type": "array", "contains": {"type": "integer"}}}},
            "KStr": {"type": "object", "required": ["k"], "properties": {"k": {"type": "array", "items": {"type": "string"}}}},
            "PickC": {"oneOf": [{"$ref": "#/components/schemas/KHas"}, {"$ref": "#/components/schemas/KStr"}]},
            "FmtList": {"type": ["string", "integer"], "format": "date-time"},
            "EnumF": {"type": ["integer", "string"], "enum": [1.5, "a"]},
            "ObjOrArr": {"oneOf": [
                {"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}},
                {"type": "array", "items": {"type": "string"}}
            ]},
            "ClosedOrInt": {"anyOf": [{"$ref": "#/components/schemas/Closed"}, {"type": "integer"}]},
            "RawMin": {"minProperties": 1},
            "ReqOnly": {"type": "object", "required": ["k"]},
            "ClosedAlias": {"$ref": "#/components/schemas/Closed"},
            "AliasOrArr": {"oneOf": [
                {"$ref": "#/components/schemas/ClosedAlias"},
                {"type": "array", "items": {"type": "string"}}
            ]},
            "NIf": {"type": ["string", "null"], "if": {"type": "null"}, "then": false},
            "HoldsNIf": {"type": "object", "properties": {"v": {"$ref": "#/components/schemas/NIf"}}},
            "ListIf": {"type": ["string", "integer", "null"], "if": {"type": "null"}, "then": false},
            "ScalarAlias": {"$ref": "#/components/schemas/Scalar"},
            "HoldsScalarAlias": {"type": "object", "properties": {"n": {"$ref": "#/components/schemas/ScalarAlias"}}},
            "EnumIf": {"type": "string", "enum": ["a"], "if": {"const": "a"}, "then": false},
            "Mid2": {"type": "object", "properties": {"f": {"type": "number", "format": "float", "minimum": 1.0000000596046448}}},
            "DMin": {"type": "number", "minimum": 8.143539365241406e270},
            "SibEnum": {"$ref": "#/components/schemas/MaybeText", "enum": ["a", "b"]},
            "HoldsSibEnum": {"type": "object", "properties": {"e": {"$ref": "#/components/schemas/SibEnum"}}},
            "HoldsListIf": {"type": "object", "properties": {"value": {"$ref": "#/components/schemas/ListIf"}}},
            "NIfOff": {"type": ["string", "null"], "if": {"type": "null"}, "then": false},
            "HoldsNIfOff": {"type": "object", "properties": {"v": {"$ref": "#/components/schemas/NIfOff"}}},
            "Dated": {"type": "object", "properties": {"d": {"type": "string", "format": "date", "minLength": 5}}},
            "SmallMaps": {"type": "array", "items": {"additionalProperties": {"type": "string"}, "maxProperties": 2}},
            "Money": {"type": "string", "maxLength": 3},
            "Base2": {"type": "string", "maxLength": 2},
            "ShortOff": {"type": "string", "maxLength": 1},
            "LongOn": {"type": "string", "minLength": 2},
            "OffOrOn": {"oneOf": [{"$ref": "#/components/schemas/ShortOff"}, {"$ref": "#/components/schemas/LongOn"}]},
            "Currency": {"type": "string", "enum": ["USD", "EUR"]},
            "HoldsCurrency": {"type": "object", "properties": {"c": {"$ref": "#/components/schemas/Currency"}}},
            "Point": {"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}}},
            "Sub2": {"type": "object", "properties": {"x": false}},
            "Sub3": {"type": "object", "properties": {"p": {"not": {}}}},
            "HoldsSub3": {"type": "object", "properties": {"s3": {"$ref": "#/components/schemas/Sub3"}}},
            "HoldsSub2": {"type": "object", "properties": {"s2": {"$ref": "#/components/schemas/Sub2"}}},
            "Mixed": {"type": ["string", "integer"], "enum": ["a", 1]},
            "Strict": {"type": "object", "required": ["a"]},
            "HoldsMixed": {"type": "object", "properties": {
                "m": {"$ref": "#/components/schemas/Mixed"},
                "s": {"$ref": "#/components/schemas/Strict"}
            }},
            "HoldsPoint": {"type": "object", "properties": {"s": {"$ref": "#/components/schemas/Point"}}},
            "Bag": {"type": "object", "maxProperties": 1},
            "HoldsBag": {"type": "object", "properties": {"b": {"$ref": "#/components/schemas/Bag"}, "l": {"type": "array", "items": {"$ref": "#/components/schemas/Bag"}}}},
            "GatedU": {"anyOf": [{"type": "string", "maxLength": 1}, {"type": "integer"}]},
            "Gated2": {"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}},
            "GatedField": {"type": "object", "minProperties": 1, "properties": {
                "a": {"type": "string", "minLength": 1}, "b": {"type": "string"}
            }},
            "AllGated": {"type": "object", "properties": {"a": {"type": "string", "minLength": 1}}},
            "IntOrNum": {"oneOf": [{"type": "integer"}, {"type": "number"}]},
            "DeepNode": {"anyOf": [{"type": "array", "items": {"$ref": "#/components/schemas/DeepNode"}}, {"type": "string"}]},
            "ChainH": {"oneOf": [{"$ref": "#/components/schemas/ChainS"}, {"type": "string"}]},
            "ChainS": {"type": "object", "additionalProperties": false, "properties": {
                "s": {"$ref": "#/components/schemas/ChainS"},
                "h": {"$ref": "#/components/schemas/ChainH"},
                "n": {"type": "integer", "minimum": 0}
            }},
            "Twice": {"anyOf": [{"$ref": "#/components/schemas/TwiceP"}, {"$ref": "#/components/schemas/TwiceQ"}]},
            "TwiceP": {"type": "object", "properties": {"h": {"$ref": "#/components/schemas/Twice"}}},
            "TwiceQ": {"type": "object", "properties": {"h": {"$ref": "#/components/schemas/Twice"}}},
            "EnumItems": {"oneOf": [
                {"type": "array", "items": {"type": "string", "enum": ["a"]}},
                {"type": "array", "items": {"type": "object"}}
            ]},
            "PatOrStr": {"anyOf": [
                {"type": "object", "patternProperties": {"^x": {"type": "integer"}}, "additionalProperties": false},
                {"type": "string"}
            ]},
            "Long3": {"type": "string", "minLength": 3},
            "PatAdd": {"type": "object", "properties": {"k": {"type": "string"}},
                "patternProperties": {"^x": {"type": "string"}},
                "additionalProperties": {"$ref": "#/components/schemas/Long3"}},
            "PatBoth": {"type": "object", "patternProperties": {
                "^a": {"type": "string", "maxLength": 2},
                "b$": {"type": "string", "minLength": 2}}},
            "PatOwners": {"type": "object", "properties": {"id": {"type": "string"}},
                "patternProperties": {"^o": {"$ref": "#/components/schemas/Owner"}}},
            "PatUntranslated": {"type": "object",
                "patternProperties": {"\\bx": {"type": "integer"}},
                "additionalProperties": {"type": "string"}},
            "PatTypeless": {"patternProperties": {"^a": {"type": "integer"}}},
            "PatDecl": {"type": "object", "required": ["ar"],
                "properties": {"ab": {"type": "string"}, "ar": {"type": "string"}, "an": {"type": ["string", "null"]}},
                "patternProperties": {"^a": {"type": "string", "maxLength": 1}}},
            "PatDeclDenied": {"type": "object", "additionalProperties": false,
                "properties": {"ab": {"type": "string"}},
                "patternProperties": {"^a": {"type": "string", "maxLength": 1}}},
            "PatF32": {"type": "object", "properties": {"xb": {"type": "number", "format": "float"}},
                "patternProperties": {"^x": {"type": "number", "exclusiveMaximum": 1}}},
            "PatSub": {"type": "object", "properties": {"xd": {"type": "string", "format": "date"}},
                "patternProperties": {"^x": {"type": "string", "maxLength": 1}}},
            "PatAnything": {"type": "object", "properties": {"id": {"type": "string"}},
                "patternProperties": {"\\bq": {}}},
            "PatF64Max": {"type": "object", "properties": {"xa": {"type": "number"}, "xv": {}},
                "patternProperties": {"^x": {"type": "integer", "maximum": 9007199254740992u64}}},
            "Whatever": {},
            "PatRefAnything": {"type": "object", "properties": {"id": {"type": "string"}},
                "patternProperties": {"\\bq": {"$ref": "#/components/schemas/Whatever"}}},
            "PatAliasRef": {"type": "object", "properties": {"xa": {"$ref": "#/components/schemas/ShortAliased"}},
                "patternProperties": {"^x": {"type": "object", "properties": {"aa": {"type": "string", "maxLength": 1}}}}},
            "PatHoldsPatterned": {"type": "object",
                "properties": {"xInner": {"$ref": "#/components/schemas/PatBoth"}},
                "patternProperties": {"^x": {"type": "object", "maxProperties": 0}}},
            "PatAnyTranslated": {"type": "object", "patternProperties": {"^x": {}}},
            "PatAnyBeside": {"type": "object", "additionalProperties": {},
                "patternProperties": {"\\bx": {}}},
            "PatAnyUntranslatedClosed": {"type": "object", "additionalProperties": false,
                "patternProperties": {"\\bx": {}}},
            "PatAnyUntranslatedTyped": {"type": "object", "additionalProperties": {"type": "string"},
                "patternProperties": {"\\bx": {}}},
            "PatAnyOthers": {"type": "object", "additionalProperties": {"type": "string"},
                "patternProperties": {"^x": {}}},
            "PatClosedFalse": {"type": "object", "additionalProperties": false,
                "patternProperties": {"^x-": false}},
            "PatFalse": {"type": "object",
                "properties": {"id": {"type": "string"}, "x-id": {"type": "string"}},
                "patternProperties": {"^x-": false, "^n": {"not": {"type": "string"}}}},
            "PatMapAdd": {"type": "object",
                "patternProperties": {"^x": {"type": "integer"}},
                "additionalProperties": {"$ref": "#/components/schemas/Long3"}},
            "IntCatch": {"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": {"type": "integer"}},
            "ZooCat": {"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string", "const": "ZooCat"}}},
            "ZooDog": {"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string", "enum": ["ZooDog"]}}},
            "ZooAnimal": {"oneOf": [{"$ref": "#/components/schemas/ZooCat"}, {"$ref": "#/components/schemas/ZooDog"}],
                "discriminator": {"propertyName": "kind"}},
            "Zoo": {"type": "object", "properties": {"title": {"type": "string"}}, "additionalProperties": {"$ref": "#/components/schemas/ZooAnimal"}},
            "ZooOrStr": {"oneOf": [{"$ref": "#/components/schemas/Zoo"}, {"type": "string"}]},
            "IntCatchOrStr": {"oneOf": [{"$ref": "#/components/schemas/IntCatch"}, {"type": "string"}]},
            "AliasU": {"anyOf": [{"$ref": "#/components/schemas/IntCatch"}, {"type": "string"}]},
            "GatedInt": {"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": {"type": "integer"}},
            "GatedIntU": {"oneOf": [{"$ref": "#/components/schemas/GatedInt"}, {"type": "string"}]},
            "GatedIntAll": {"allOf": [{"$ref": "#/components/schemas/GatedInt"}, {"type": "object", "properties": {"q": {"type": "integer"}}}]},
            "GatedClosed": {"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}, "b": {"type": "string"}}},
            "GatedClosedU": {"oneOf": [{"$ref": "#/components/schemas/GatedClosed"}, {"type": "integer"}]},
            "AliasB": {"type": "object", "required": ["aa"], "properties": {"aa": {"type": "string"}}},
            "OneAliased": {"oneOf": [{"$ref": "#/components/schemas/IntCatch"}, {"$ref": "#/components/schemas/AliasB"}]},
            "IntA": {"type": "object", "properties": {"a": {"type": "integer"}}},
            "OneGated": {"oneOf": [{"$ref": "#/components/schemas/GatedInt"}, {"$ref": "#/components/schemas/IntA"}]},
            "SubAliased": {"type": "object", "properties": {"a": {"type": "string"}, "d": {"$ref": "#/components/schemas/Money"}}, "additionalProperties": {"type": "integer"}},
            "OneSubAliased": {"oneOf": [{"$ref": "#/components/schemas/SubAliased"}, {"$ref": "#/components/schemas/AliasB"}]},
            "AllSubAliased": {"allOf": [{"$ref": "#/components/schemas/SubAliased"}, {"type": "object", "properties": {"q": {"type": "integer"}}}]},
            "ShortAliased": {"type": "object", "properties": {"a": {"type": "string", "maxLength": 2}}},
            "OneShortAliased": {"oneOf": [{"$ref": "#/components/schemas/ShortAliased"}, {"type": "integer"}]},
            "HoldsShortAliased": {"type": "object", "properties": {"inner": {"$ref": "#/components/schemas/ShortAliased"}}},
            "OneHoldsShortAliased": {"oneOf": [{"$ref": "#/components/schemas/HoldsShortAliased"}, {"type": "integer"}]},
            "ManyShortAliased": {"oneOf": [{"type": "array", "items": {"$ref": "#/components/schemas/ShortAliased"}}, {"type": "string"}]},
            "FreeAliased": {"type": "object", "required": ["r"], "properties": {"a": {"type": "string"}, "r": {"type": "string"}}},
            "ClosedAliased": {"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}},
            "RequiredAliased": {"type": "object", "required": ["a"], "properties": {"a": {"type": "string", "maxLength": 2}}},
            "NamesAliased": {"type": "object", "propertyNames": {"maxLength": 1}, "properties": {"ab": {"type": "string"}}},
            "ReqFreeAliased": {"type": "object", "required": ["a"], "properties": {"a": {"type": "string"}}},
            "NamesTrueAliased": {"type": "object", "propertyNames": true, "properties": {"a": {"type": "string"}}},
            "NamesEmptyAliased": {"type": "object", "propertyNames": {}, "properties": {"a": {"type": "string"}}},
            "Ip": {"oneOf": [{"type": "string", "format": "ipv4"}, {"type": "string", "maxLength": 3}]},
            "StringOnly": {"type": "string", "enum": ["a", "b"]},
            "ObjectOnly": {"type": "object", "additionalProperties": false, "properties": {"s": {"$ref": "#/components/schemas/StringOnly"}}},
            "InlineItems": {"type": "array", "items": {"type": "string", "minLength": 2}},
            "PadNode": {"anyOf": [{"$ref": "#/components/schemas/PadA"}, {"$ref": "#/components/schemas/PadB"}]},
            "PadA": {"type": "object", "properties": {"next": {"$ref": "#/components/schemas/PadNode"}, "pad": {"type": "string"}}},
            "PadB": {"type": "object", "properties": {"next": {"$ref": "#/components/schemas/PadNode"}, "pad": {"type": "string", "maxLength": 100000000}}},
            "EnumOrObj": {"oneOf": [{"type": "string", "enum": ["a"]}, {"type": "object"}]},
            "EmptyEnum": {"type": "string", "enum": [], "minLength": 1},
            "IntOrStr": {"oneOf": [{"type": "integer"}, {"type": "string"}]},
            "AllInt": {"allOf": [{"type": "integer"}, {"minimum": 0}]},
            "ArrOrStr": {"oneOf": [{"type": "array", "items": {"type": "integer"}}, {"type": "string"}]},
            "ObjOrStr": {"oneOf": [{"type": "object", "properties": {"n": {"type": "integer"}}}, {"type": "string"}]},
            "NullOnlyN": {"type": "null", "if": {}, "then": false},
            "HoldsNullOnly": {"type": "object", "properties": {"n": {"$ref": "#/components/schemas/NullOnlyN"}}},
            "Gated": {"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string", "maxLength": 1}}},
            "NullType": {"type": ["null"], "if": {}, "then": false},
            "WrapShort": {"type": "object", "required": ["s"], "properties": {"s": {"$ref": "#/components/schemas/ShortOff"}}},
            "WrapLong": {"type": "object", "required": ["s"], "properties": {"s": {"$ref": "#/components/schemas/LongOn"}}},
            "PickWrap": {"oneOf": [{"$ref": "#/components/schemas/WrapShort"}, {"$ref": "#/components/schemas/WrapLong"}]},
            "F32Edge": {"type": "number", "format": "float", "exclusiveMaximum": 3.4028235677973366e38},
            "AliasSub": {"$ref": "#/components/schemas/Base2"},
            "HoldsAliasSub": {"type": "object", "properties": {"v": {"$ref": "#/components/schemas/AliasSub"}}},
            "ListNullOnly": {"type": ["string", "integer", "null"], "enum": [null], "if": {}, "then": false},
            "EnumObj": {"type": ["object", "string"], "enum": ["a", {"x": 1}]},
            "NullIf": {"type": ["string", "null"], "enum": [null], "if": {}, "then": false},
            "NIfAlias": {"$ref": "#/components/schemas/NIf"},
            "HoldsNIfAlias": {"type": "object", "properties": {"v": {"$ref": "#/components/schemas/NIfAlias"}}},
            "Monies": {"type": "array", "items": {"$ref": "#/components/schemas/Money"}},
            "MoneyMap": {"type": "object", "properties": {"x": {"type": "string"}}, "additionalProperties": {"$ref": "#/components/schemas/Money"}},
            "DateAlias": {"type": "string", "format": "date", "maxLength": 2},
            "HoldsDate": {"type": "object", "properties": {"d": {"$ref": "#/components/schemas/DateAlias"}}},
            "Node": {"type": "object", "additionalProperties": false, "properties": {
                "next": {"$ref": "#/components/schemas/Node"}, "n": {"type": "integer"}
            }},
            "Leaf": {"type": "object", "additionalProperties": false, "properties": {"s": {"type": "string"}}},
            "OldClosed": {"type": "object", "additionalProperties": false, "deprecated": true, "properties": {"a": {"type": "string"}}},
            "OldCatch": {"type": "object", "deprecated": true, "additionalProperties": {"type": "integer"}},
            "OldEnum": {"type": "string", "deprecated": true, "enum": ["a"]},
            "OldNew": {"type": "string", "deprecated": true, "maxLength": 3},
            "OldAlias": {"$ref": "#/components/schemas/OldClosed", "deprecated": true},
            "OldTagged": {"deprecated": true, "oneOf": [{"$ref": "#/components/schemas/ZooCat"}, {"$ref": "#/components/schemas/ZooDog"}], "discriminator": {"propertyName": "kind"}},
            "OldOpen": {"deprecated": true, "description": "Old.", "anyOf": [{"type": "string"}, {"$ref": "#/components/schemas/OldClosed"}]},
            "OldAll": {"deprecated": true, "title": "Old all", "allOf": [{"type": "string"}, {"maxLength": 3}]},
            "OldList": {"type": "array", "deprecated": true, "items": {"$ref": "#/components/schemas/OldClosed"}},
            "UsesOld": {"type": "object", "properties": {
                "c": {"$ref": "#/components/schemas/OldCatch"}, "e": {"$ref": "#/components/schemas/OldEnum"},
                "n": {"$ref": "#/components/schemas/OldNew"}, "a": {"$ref": "#/components/schemas/OldAlias"},
                "t": {"$ref": "#/components/schemas/OldTagged"}, "o": {"$ref": "#/components/schemas/OldOpen"},
                "l": {"$ref": "#/components/schemas/OldAll"}, "s": {"$ref": "#/components/schemas/OldList"}}},
            "NullName": {"type": "object", "propertyNames": {"maxLength": 1}, "properties": {"ab": {}, "c": {}}},
            "Leaves": {"type": "object", "additionalProperties": {"$ref": "#/components/schemas/Leaf"}},
            "S": {"type": "object", "additionalProperties": false, "properties": {"x": {"type": "string"}}},
            "D": {"type": "object", "additionalProperties": false, "properties": {"x": {"type": "string"}}},
            "ListObj": {"type": ["string", "object", "null"], "properties": {"p": {"type": "string"}}, "if": {}, "then": {"type": "string"}},
            "HoldsMoney": {"type": "object", "properties": {"m": {"$ref": "#/components/schemas/Money"}}},
            "NestedClosed": {"oneOf": [
                {"type": "array", "items": {"$ref": "#/components/schemas/Closed"}},
                {"type": "array", "items": {"type": "array"}}
            ]}
        }}
    })
}

fn validated_generation(target: Target) -> roas_codegen::Generation {
    let bytes = serde_json::to_vec(&validated_description()).unwrap();
    let doc =
        SourceDocument::parse(Input::Json(bytes), Url::parse("file:///v.json").unwrap()).unwrap();
    let doc = validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).unwrap();
    // One type opted out, and a format substituted by a plain string.
    let mut config: ConfigFile = serde_json::from_value(serde_json::json!({
        "types": {"NIfOff": {"validation": false}, "ShortOff": {"validation": false}, "Gated": {"attrs": ["#[cfg(any())]"]}, "Gated2": {"attrs": ["#[cfg_attr(all(), cfg(any()))]"]}, "GatedU": {"attrs": ["#[cfg(any())]"]}, "GatedField": {"fields": {"a": {"attrs": ["#[cfg(any())]"]}}}, "GatedRest": {"fields": {"a": {"attrs": ["#[cfg(any())]"]}}}, "GCat": {"attrs": ["#[cfg(any())]"]}, "GatedInt": {"fields": {"a": {"attrs": ["#[cfg(any())]"]}}}, "GatedClosed": {"fields": {"a": {"attrs": ["#[cfg(any())]"]}}}, "SubAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "ShortAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "FreeAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "ClosedAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "RequiredAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "NamesAliased": {"fields": {"ab": {"attrs": ["#[serde(alias = \"b\")]"]}}}, "ReqFreeAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "NamesTrueAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "NamesEmptyAliased": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "IntCatch": {"fields": {"a": {"attrs": ["#[serde(alias = \"aa\")]"]}}}, "NamesGated": {"fields": {"ab": {"attrs": ["#[cfg(any())]"]}}}, "AllGated": {"fields": {"a": {"attrs": ["#[cfg(any())]"]}}}, "NIfAlias": {"attrs": ["#[allow(dead_code)]"]}},
        "substitutions": {
            "date": {"rust": {"name": "String"}, "go": {"name": "string"}},
            "Money": {"rust": {"name": "String"}, "go": {"name": "string"}},
            "AliasSub": {"rust": {"name": "String"}, "go": {"name": "string"}},
            "Currency": {"rust": {"name": "String"}, "go": {"name": "string"}},
            "Point": {"rust": {"name": "serde_json::Value"}, "go": {"name": "any"}},
            "Bag": {"rust": {"name": "std::collections::BTreeMap<String, serde_json::Value>"}, "go": {"name": "map[string]any"}},
            "Sub2": {"rust": {"name": "serde_json::Value"}, "go": {"name": "jsontext.Value"}},
            "ipv4": {"rust": {"name": "std::net::Ipv4Addr"}, "go": {"name": "netip.Addr", "import": "net/netip"}},
            "Sub3": {"rust": {"name": "serde_json::Value"}, "go": {"name": "jsontext.Value"}},
            "Mixed": {"rust": {"name": "serde_json::Value"}, "go": {"name": "jsontext.Value"}},
            "Strict": {"rust": {"name": "serde_json::Value"}, "go": {"name": "jsontext.Value"}}
        }
    }))
    .unwrap();
    config.target = Some(target);
    config.validation = Some(true);
    let generation = generate(&doc, &config.build().unwrap()).expect("generates");
    assert!(!generation.has_errors(), "{:?}", generation.diagnostics);
    generation
}

const VALIDATE_MAIN: &str = r##"
#[allow(dead_code)]
mod types;
use types::*;

fn report<T: serde::de::DeserializeOwned + Validate>(json: &str) -> Vec<(String, FindingKind)> {
    let value: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    let mut found: Vec<_> = value.validate().findings.into_iter().map(|f| (f.pointer, f.kind)).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn v(pointer: &str) -> (String, FindingKind) {
    (pointer.to_owned(), FindingKind::Violation)
}

fn u(pointer: &str) -> (String, FindingKind) {
    (pointer.to_owned(), FindingKind::Unevaluated)
}

fn main() {
    // Within every bound: nothing to report.
    assert_eq!(report::<Pet>(r#"{"name":"rex","age":4,"tags":["a","b"]}"#), vec![]);
    let pet: Pet = serde_json::from_str(r#"{"name":"rex","age":4}"#).unwrap();
    assert!(pet.validate().is_valid());
    // Lengths count characters, not bytes.
    assert_eq!(report::<Pet>(r#"{"name":"ééééé","age":2}"#), vec![]);
    assert_eq!(report::<Pet>(r#"{"name":"","age":2}"#), vec![v("/name")]);
    assert_eq!(report::<Pet>(r#"{"name":"toolong","age":2}"#), vec![v("/name")]);
    // Integers: bounds, exclusive bounds and multiples, exactly.
    assert_eq!(report::<Pet>(r#"{"name":"a","age":-2}"#), vec![v("/age")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","age":30}"#), vec![v("/age")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","age":3}"#), vec![v("/age")]);
    // Floats.
    assert_eq!(report::<Pet>(r#"{"name":"a","weight":0.25}"#), vec![v("/weight")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","weight":100}"#), vec![v("/weight")]);
    // A fraction is no multiple of a whole number; a whole float may have
    // been rounded, so it is not decided.
    assert_eq!(report::<Pet>(r#"{"name":"a","ratio":1.5}"#), vec![v("/ratio")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","ratio":2}"#), vec![u("/ratio")]);
    // Arrays.
    assert_eq!(report::<Pet>(r#"{"name":"a","tags":[]}"#), vec![v("/tags")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","tags":["a","b","c","d"]}"#), vec![v("/tags")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","tags":["a","b","a"]}"#), vec![v("/tags/2")]);
    // Patterns are not checked yet, and say so.
    assert_eq!(report::<Pet>(r#"{"name":"a","code":"b"}"#), vec![v("/code")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","code":"ab"}"#), vec![]);
    // Patterns mean what ECMAScript says, in either language.
    for (json, want) in [
        (r#"{"digits":"123"}"#, vec![]),
        (r#"{"digits":"١٢٣"}"#, vec![v("/digits")]),
        (r#"{"space":"\u00a0"}"#, vec![]),
        (r#"{"space":"\u3000"}"#, vec![]),
        (r#"{"space":"x"}"#, vec![v("/space")]),
        (r#"{"dot":"\u2028"}"#, vec![v("/dot")]),
        (r#"{"dot":"😀"}"#, vec![]),
        (r#"{"word":"é"}"#, vec![v("/word")]),
        (r#"{"legacy":"123-4567"}"#, vec![]),
        (r#"{"legacy":"1234567"}"#, vec![v("/legacy")]),
        (r#"{"amp":"a&b"}"#, vec![]),
        (r#"{"amp":"c"}"#, vec![v("/amp")]),
        (r#"{"astral":"😀😀"}"#, vec![]),
        (r#"{"astral":"😀"}"#, vec![v("/astral")]),
        (r#"{"boundary":"x"}"#, vec![u("/boundary")]),
        (r#"{"maybe":null}"#, vec![]),
        (r#"{"maybe":"xax"}"#, vec![]),
        (r#"{"maybe":"x"}"#, vec![v("/maybe")]),
        (r#"{"none":""}"#, vec![v("/none")]),
        (r#"{"any":"😀"}"#, vec![]),
        (r#"{"any":"ab"}"#, vec![v("/any")]),
    ] {
        assert_eq!(report::<Patterned>(json), want, "{json}");
    }
    assert_eq!(report::<Code>(r#""AB""#), vec![]);
    // Inline items and member values are checked one by one.
    assert_eq!(report::<Each>(r#"{"grid":[[0,1],[2]]}"#), vec![]);
    assert_eq!(report::<Each>(r#"{"grid":[[0,-1],[1,2,3]]}"#), vec![v("/grid/0/1"), v("/grid/1")]);
    assert_eq!(report::<Each>(r#"{"scores":{"a":0.5,"b":2}}"#), vec![v("/scores/b")]);
    assert_eq!(report::<Each>(r#"{"maybe":[null,"ab","a"]}"#), vec![v("/maybe/2")]);
    assert_eq!(report::<Each>(r#"{"codes":["ab","A1"]}"#), vec![v("/codes/1")]);
    assert_eq!(report::<Each>(r#"{"odd":[]}"#), vec![]);
    assert_eq!(report::<Each>(r#"{"odd":["x","y"]}"#), vec![u("/odd/0"), u("/odd/1")]);
    assert_eq!(report::<EachList>(r#"["a","bc"]"#), vec![v("/1")]);
    // A gated property lands among the other members: not checked as one.
    assert_eq!(report::<GatedRest>(r#"{"a":"xyz"}"#), vec![u("")]);
    // Beside `prefixItems`, `items` is not applied to the prefix.
    assert_eq!(report::<Pf>(r#"{"p":["abc"]}"#), vec![u("/p")]);
    assert_eq!(report::<Pu>(r#"["abc"]"#), vec![u("")]);
    // A prefix item is neither decoded nor checked as one of `items`.
    assert_eq!(report::<Pr>(r#"[5,"abc"]"#), vec![u("")]);
    assert_eq!(report::<Pn>("[1]"), vec![u("")]);
    assert_eq!(report::<Rest>(r#"{"a":"x","b":3,"c":9}"#), vec![v("/c")]);
    assert_eq!(report::<Code>(r#""ABC""#), vec![v("")]);
    assert_eq!(report::<Coded>(r#""ab""#), vec![]);
    assert_eq!(report::<Coded>(r#""x1""#), vec![v("")]);
    // Patterns decide which branches match.
    assert_eq!(report::<Either>(r#""ab""#), vec![]);
    assert_eq!(report::<Either>(r#""xz""#), vec![]);
    assert_eq!(report::<Either>(r#""az""#), vec![v("")]);
    assert_eq!(report::<Either>(r#""x""#), vec![v("")]);
    // Members: counted on the object, and on a map.
    assert_eq!(report::<Pet>(r#"{"name":"a"}"#), vec![v("")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","age":2,"weight":1,"ratio":1.5,"tags":["x"]}"#), vec![v(""), v("/ratio")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","extra":{"x":1,"y":2}}"#), vec![v("/extra")]);
    // Nested types report at their own pointer, nulls skipped.
    assert_eq!(report::<Pet>(r#"{"name":"a","owner":{"nick":"long"}}"#), vec![v("/owner/nick")]);
    assert_eq!(report::<Pet>(r#"{"name":"a","owner":{"nick":null}}"#), vec![]);
    assert_eq!(
        report::<Pet>(r#"{"name":"a","friends":[{"nick":"ok"},{"nick":"long"}]}"#),
        vec![v("/friends/1/nick")]
    );
    // A component newtype checks itself, wherever it is used.
    assert_eq!(report::<Shorts>(r#"["ab","abc"]"#), vec![v("/1")]);
    // oneOf counts the branches that decode and validate.
    assert_eq!(report::<Choice>(r#""ab""#), vec![]);
    assert_eq!(report::<Choice>(r#""abc""#), vec![v("")]);
    assert_eq!(report::<Choice>(r#"{"nick":"ok"}"#), vec![]);
    // A type list accepts what any member does, overlaps included.
    assert_eq!(report::<Numeric>("1"), vec![]);
    assert_eq!(report::<Numeric>("1.5"), vec![]);
    // A typeless schema's object keywords say nothing about non-objects.
    assert_eq!(report::<Shaped>("42"), vec![]);
    assert_eq!(report::<Shaped>(r#"{"x":1}"#), vec![v("")]);
    // An inline branch keeps its constraints.
    assert_eq!(report::<Inline>(r#""ab""#), vec![]);
    assert_eq!(report::<Inline>(r#""abcd""#), vec![v("")]);
    assert_eq!(report::<Inline>("5"), vec![]);
    // Enum members still meet the other string constraints.
    assert_eq!(report::<Letters>(r#""a""#), vec![]);
    assert_eq!(report::<Letters>(r#""abcd""#), vec![v("")]);
    // A float equal to its bound may have come from either side of it.
    assert_eq!(report::<Fine>(r#"{"f":0.1}"#), vec![u("/f")]);
    assert_eq!(report::<Fine>(r#"{"f":0.2}"#), vec![v("/f")]);
    // A null in an open holder is one of its values, and matches no branch.
    assert_eq!(report::<Fine>(r#"{"choice":null}"#), vec![v("/choice")]);
    // Member names are checked.
    assert_eq!(report::<Named>(r#"{"abcd":1}"#), vec![v("/abcd")]);
    assert_eq!(report::<Named>(r#"{"abc":1}"#), vec![]);
    // Member names: declared ones decided when generating, the others read.
    assert_eq!(report::<Names>(r#"{"ok":"x"}"#), vec![]);
    assert_eq!(report::<Names>(r#"{"Bad":"x","ok":"y"}"#), vec![v("/Bad")]);
    assert_eq!(report::<Names>(r#"{"zz":"x","Q":"y"}"#), vec![v("/Q")]);
    assert_eq!(report::<NamesLook>(r#"{}"#), vec![]);
    assert_eq!(report::<NamesLook>(r#"{"a":1}"#), vec![u("/a")]);
    assert_eq!(report::<NamesEnum>(r#"{"a":1}"#), vec![u("")]);
    assert_eq!(report::<NamesTrue>(r#"{"a":1}"#), vec![]);
    // The only failing name compiled out: nothing left to report.
    assert_eq!(report::<NamesGated>(r#"{"c":"x"}"#), vec![]);
    // Bounds beyond every value, and the one multiple of 2^63 an i64 holds.
    assert_eq!(report::<Edge>(r#"{"big":"x","wide":1.5,"many":[1],"m":-9223372036854775808}"#), vec![]);
    assert_eq!(report::<Edge>(r#"{"m":0}"#), vec![]);
    assert_eq!(report::<Edge>(r#"{"m":5}"#), vec![v("/m")]);
    // A declared name within propertyNames, decided when generating.
    assert_eq!(report::<Closed>(r#"{"a":"x"}"#), vec![]);
    // A type list's constraints apply to the member they fit.
    assert_eq!(report::<TypeList>(r#""ab""#), vec![]);
    assert_eq!(report::<TypeList>("7"), vec![]);
    assert_eq!(report::<TypeList>(r#""abcdef""#), vec![v("")]);
    assert_eq!(report::<TypeList>("1"), vec![v("")]);
    // A nullable inline branch: checked behind its null.
    assert_eq!(report::<MaybeShort>(r#""ab""#), vec![]);
    assert_eq!(report::<MaybeShort>(r#""abc""#), vec![v("")]);
    assert_eq!(report::<MaybeShort>("null"), vec![]);
    // A numeric enum is checked, so a branch with one decides.
    assert_eq!(report::<Picks>("1"), vec![]);
    assert_eq!(report::<Picks>("3"), vec![v("")]);
    // A composition naming itself is decoded, not validated forever.
    assert_eq!(report::<Itself>(r#""a""#), vec![]);
    // A null through an alias of an open union is still checked.
    assert_eq!(report::<Holds>(r#"{"c":null}"#), vec![v("/c")]);
    // A type list with null holds a raw null validly.
    assert_eq!(report::<OrNull>("null"), vec![]);
    assert_eq!(report::<OrNull>(r#""ab""#), vec![v("")]);
    // `$ref` with sibling properties: each part checked once, no loop.
    assert_eq!(report::<Extended>(r#"{"q":"a","z":"b"}"#), vec![]);
    assert_eq!(report::<Extended>(r#"{"q":"a","z":"bc"}"#), vec![v("/z")]);
    // serde_json reads an f32 past its range as an infinity; the bound,
    // past every f32 but not every f64, still catches it.
    assert!(serde_json::from_str::<Tiny>(r#"{"f":1e39}"#).is_err());
    assert_eq!(report::<Tiny>(r#"{"f":1.5}"#), vec![]);
    // A referenced numeric enum is unchecked too: no certain double match.
    assert_eq!(report::<PicksRef>("1"), vec![]);
    assert_eq!(report::<PicksRef>("3"), vec![v("")]);
    // A deprecated field is still checked, without a warning.
    assert_eq!(report::<Old>(r#"{"name":""}"#), vec![v("/name")]);
    // An intersection allows null only when every side does.
    assert_eq!(report::<Both>(r#""a""#), vec![]);
    assert!(report::<Both>("null").contains(&v("")));
    // `$ref` siblings nothing checks prove no match.
    assert_eq!(report::<Sib>(r#""a""#), vec![u("")]);
    assert_eq!(report::<SibC>(r#""a""#), vec![u("")]);
    // A number's enum and const are checked: integers exactly, a double
    // as far as an f64 holds it, a float equal to a value is undecided.
    assert_eq!(report::<Ni>("3"), vec![v("")]);
    // A union read by a parent's discriminator checks the variant it holds.
    assert_eq!(report::<VShelter>(r#"{"first":{"kind":"VKitty","lives":10}}"#), vec![v("/first/lives")]);
    assert_eq!(report::<VShelter>(r#"{"first":{"kind":"x","name":"long"}}"#), vec![v("/first/name")]);
    assert_eq!(report::<VShelter>(r#"{"first":{"kind":"VKitty","lives":9,"name":"ab"}}"#), vec![]);
    // A subtype compiled out leaves its tag to the parent: the holder stays.
    assert_eq!(report::<GHolder>(r#"{"g":{"kind":"GCat","lives":10}}"#), vec![]);
    // ...and so does a `defaultMapping` to it: unknown tags are the parent's.
    assert_eq!(report::<GHolder>(r#"{"g":{"kind":"other","lives":10}}"#), vec![]);
    assert_eq!(report::<Ni>("2"), vec![]);
    assert_eq!(report::<Ni>("2.0"), vec![]);
    assert_eq!(report::<Pins>(r#"{"n":3}"#), vec![v("/n")]);
    assert_eq!(report::<FloatEnum>("0.5"), vec![u("")]);
    assert_eq!(report::<FloatEnum>("0.75"), vec![v("")]);
    assert_eq!(report::<DoubleEnum>("0.1"), vec![]);
    assert_eq!(report::<DoubleEnum>("2"), vec![]);
    assert_eq!(report::<DoubleEnum>("3"), vec![v("")]);
    assert_eq!(report::<IntConst>("7"), vec![]);
    assert_eq!(report::<IntConst>("8"), vec![v("")]);
    assert_eq!(report::<NoInt>("1"), vec![v("")]);
    // A listed double no float holds leaves its float undecided, after
    // the values that are held.
    assert_eq!(report::<BigEnum>("9007199254740992"), vec![u("")]);
    assert_eq!(report::<BigEnum>("9007199254740994"), vec![v("")]);
    assert_eq!(report::<MixedEnum>("9007199254740992"), vec![]);
    // Only the float32 nearest a listed value is undecided.
    assert_eq!(report::<FloatTenth>("0.1"), vec![u("")]);
    assert_eq!(report::<FloatTenth>("0.099999994"), vec![v("")]);
    // Past the largest float32, a listed value decodes to it.
    assert_eq!(report::<FloatEdge>("3.4028235e38"), vec![u("")]);
    // Exactly halfway, a listed value may decode to either neighbour.
    assert_eq!(report::<FloatHalf>("1"), vec![u("")]);
    assert_eq!(report::<FloatHalf>("1.0000001"), vec![u("")]);
    assert_eq!(report::<FloatHalf>("1.0000002"), vec![v("")]);
    // Past the halfway point, a listed value decodes to nothing.
    assert_eq!(report::<FloatHuge>("3.4028235e38"), vec![v("")]);
    // An integer const written as a float past 2^53: a value rounding to
    // another float is a violation however the description was read.
    assert_eq!(report::<IntConstFloat>("9007199254740996"), vec![v("")]);
    assert_eq!(report::<IntOrNull>(r#"{"n":null}"#), vec![]);
    assert_eq!(report::<IntOrNull>(r#"{"n":1}"#), vec![]);
    assert_eq!(report::<IntOrNull>(r#"{"n":2}"#), vec![v("/n")]);
    assert_eq!(report::<Narrow>("-5"), vec![]);
    assert_eq!(report::<Narrow>("6"), vec![v("")]);
    assert_eq!(report::<NumConst>(r#"{"c":1.5}"#), vec![]);
    assert_eq!(report::<NumConst>(r#"{"c":2}"#), vec![v("/c")]);
    // A const that is not a number is still not checked.
    assert_eq!(report::<Pins>(r#"{"k":false}"#), vec![u("/k")]);
    // A type list's enum and default are split among its members.
    assert_eq!(report::<EnumO>(r#""ab""#), vec![]);
    assert_eq!(report::<EnumO>("5"), vec![]);
    assert_eq!(report::<EnumO>("6"), vec![v("")]);
    assert_eq!(report::<Def>("5"), vec![]);
    assert_eq!(report::<Def>("0"), vec![v("")]);
    assert_eq!(report::<EnumN>(r#""cde""#), vec![v("")]);
    assert_eq!(report::<F32X>("0.99999999"), vec![u("")]);
    assert_eq!(report::<F32X>("0.5"), vec![]);
    // A holder's own const, and a const on a present null.
    assert_eq!(report::<HolderC>(r#""wrong""#), vec![u("")]);
    assert_eq!(report::<NullC>(r#"{"n":null}"#), vec![u("/n")]);
    // An intersection's null is checked behind a field, and allowed when
    // both sides allow it.
    assert!(report::<HoldsBoth>(r#"{"b":null}"#).contains(&v("/b")));
    assert_eq!(report::<NullSib>("null"), vec![]);
    assert_eq!(report::<EdgeBranch>(r#""a""#), vec![]);
    // serde_json reads a float past its range as an infinity, from some
    // literal on either side of the bound.
    assert!(serde_json::from_str::<FInf>(r#"{"f":1e39}"#).is_err());
    assert_eq!(report::<FInf>(r#"{"f":1e38}"#), vec![]);
    assert_eq!(report::<Mid>(r#"{"f":1.0000000596046448}"#), vec![u("/f")]);
    // An enum without null leaves null out of a type list.
    assert_eq!(report::<NoNullList>(r#""a""#), vec![]);
    assert_eq!(report::<NoNull>(r#""a""#), vec![]);
    assert_eq!(report::<PartNull>("null"), vec![u("")]);
    // Constraints on inline items decide a branch.
    assert_eq!(report::<Items>("[1]"), vec![]);
    assert_eq!(report::<Items>("[5]"), vec![v("")]);
    assert_eq!(report::<StrItems>(r#"["a"]"#), vec![]);
    // Member patterns are not checked yet, and say so.
    // Pattern members are checked against their patterns' schemas.
    assert_eq!(report::<PatMap>(r#"{"a":"x"}"#), vec![]);
    assert_eq!(report::<PatMap>(r#"{"a":1}"#), vec![]);
    assert_eq!(report::<PatMap>(r#"{"a":true}"#), vec![v("")]);
    // A type list left with only its null.
    assert_eq!(report::<NullOnly>("null"), vec![]);
    // A present null reaches a JSON-value wrapper.
    assert_eq!(report::<HoldsScalar>(r#"{"n":null}"#), vec![u("/n")]);
    // What a branch's members leave unchecked proves no match.
    assert_eq!(report::<PickK>(r#"{"k":[1]}"#), vec![]);
    assert_eq!(report::<PickC>(r#"{"k":["x"]}"#), vec![u("")]);
    // A type-list member is never dropped.
    assert_eq!(report::<FmtList>("5"), vec![]);
    assert_eq!(report::<EnumF>(r#""a""#), vec![]);
    // A struct is decoded from an object only.
    assert_eq!(report::<ObjOrArr>(r#"["x"]"#), vec![]);
    assert_eq!(report::<ClosedOrInt>("[]"), vec![v("")]);
    assert_eq!(report::<AliasOrArr>(r#"["x"]"#), vec![]);
    // Raw JSON is not checked against kind-specific keywords yet.
    assert_eq!(report::<RawMin>("{}"), vec![u("")]);
    // A required member decoding does not ask for.
    assert_eq!(report::<ReqOnly>("{}"), vec![u("")]);
    // A null held outside its type is checked for it.
    assert_eq!(report::<HoldsNIf>(r#"{"v":null}"#), vec![u("/v")]);
    assert_eq!(report::<ListIf>("null"), vec![u("")]);
    assert_eq!(report::<HoldsScalarAlias>(r#"{"n":null}"#), vec![u("/n")]);
    assert_eq!(report::<EnumIf>(r#""a""#), vec![u("")]);
    // A float at either float around its bound may have come from either side.
    assert_eq!(report::<Mid2>(r#"{"f":1.000000059604644775390624}"#), vec![u("/f")]);
    // A double read exactly, as its bound was.
    assert_eq!(report::<DMin>("81435393652414055e254"), vec![]);
    // A null the siblings' enum leaves out is not passed.
    assert!(!report::<HoldsSibEnum>(r#"{"e":null}"#).is_empty());
    // A null held outside a type list is checked as the list would be.
    assert_eq!(report::<HoldsListIf>(r#"{"value":null}"#), vec![u("/value")]);
    // An opted-out type is not asked about its null.
    assert_eq!(report::<HoldsNIfOff>(r#"{"v":null}"#), vec![]);
    // A substituted type is not checked against what applies to it.
    assert_eq!(report::<Dated>(r#"{"d":"x"}"#), vec![u("/d")]);
    // A hoisted inline item checks itself.
    assert_eq!(report::<SmallMaps>(r#"[{"k":"v"}]"#), vec![]);
    assert_eq!(report::<SmallMaps>(r#"[{"a":"1","b":"2","c":"3"}]"#), vec![v("/0")]);
    // A closed struct is read from an object only, wherever it is.
    assert_eq!(report::<NestedClosed>(r#"[["x"]]"#), vec![]);
    // A substituted component is not checked against what it declares.
    assert_eq!(report::<HoldsMoney>(r#"{"m":"abcdef"}"#), vec![u("/m")]);
    assert_eq!(report::<Monies>(r#"["abcdef"]"#), vec![u("")]);
    assert_eq!(report::<Monies>("[]"), vec![]);
    assert_eq!(report::<MoneyMap>(r#"{"k":"abcdef"}"#), vec![u("")]);
    assert_eq!(report::<HoldsDate>(r#"{"d":"long"}"#), vec![u("/d")]);
    // A closed struct keeps the decoder's depth limit, and decodes inside
    // a flattened map.
    let deep = |depth: usize| format!("{}{{}}{}", r#"{"next":"#.repeat(depth), "}".repeat(depth));
    assert!(serde_json::from_str::<Node>(&deep(200)).is_err());
    assert!(serde_json::from_str::<Node>(&deep(100_000)).is_err());
    assert!(serde_json::from_str::<Node>(&deep(10)).is_ok());
    assert_eq!(report::<Leaves>(r#"{"a":{"s":"b"}}"#), vec![]);
    assert!(serde_json::from_str::<S>(r#"{"x":"y"}"#).is_ok() && serde_json::from_str::<D>("[]").is_err());
    // A null held by a type list with a hoisted member keeps the shared keywords.
    assert_eq!(report::<ListObj>("null"), vec![u("")]);
    // A type list's enum asserts on its object member too.
    assert_eq!(report::<EnumObj>(r#""a""#), vec![]);
    assert_eq!(report::<EnumObj>(r#"{"x":2}"#), vec![u("")]);
    // A list left with its null keeps what else it says.
    assert_eq!(report::<NullIf>("null"), vec![u("")]);
    assert_eq!(report::<ListNullOnly>("null"), vec![u("")]);
    // An opted-out branch proves no match.
    assert_eq!(report::<OffOrOn>(r#""abc""#), vec![u("")]);
    // A literal at a bound past the float32 range decodes to the largest float32.
    assert_eq!(report::<F32Edge>("3.4028235677973366e38"), vec![u("")]);
    assert_eq!(report::<F32Edge>("1.5"), vec![]);
    // A substituted enum is not checked against its members.
    assert_eq!(report::<HoldsCurrency>(r#"{"c":"BAD"}"#), vec![u("/c")]);
    // A list of only null keeps what else it says.
    assert_eq!(report::<NullType>("null"), vec![u("")]);
    // A substituted object is not checked against what its members declare.
    assert_eq!(report::<HoldsPoint>(r#"{"s":{"n":-1}}"#), vec![u("/s")]);
    assert_eq!(report::<HoldsBag>(r#"{"b":{"a":[1]},"l":[{"x":{}}]}"#), vec![u("/b"), u("/l")]);
    assert_eq!(report::<HoldsSub2>(r#"{"s2":{"x":1}}"#), vec![u("/s2")]);
    assert_eq!(report::<HoldsSub3>(r#"{"s3":{"p":1}}"#), vec![u("/s3")]);
    // A field compiled out is neither read nor counted.
    assert_eq!(report::<GatedField>(r#"{"b":"x"}"#), vec![]);
    // A null-only reference keeps what it says.
    assert_eq!(report::<HoldsNullOnly>(r#"{"n":null}"#), vec![u("/n")]);
    assert_eq!(report::<HoldsNullOnly>("{}"), vec![]);
    // A whole number past int64 may match the integer branch too.
    assert_eq!(report::<IntOrNum>("9223372036854775808"), vec![u("")]);
    // So is one past f64, or one nested anywhere in the branch.
    assert_eq!(report::<IntOrStr>("1e400"), vec![u("")]);
    assert!(report::<AllInt>("1e400").iter().all(|f| f.1 == FindingKind::Unevaluated));
    assert_eq!(report::<ArrOrStr>("[1e19]"), vec![u("")]);
    assert_eq!(report::<ObjOrStr>(r#"{"n":1e19}"#), vec![u("")]);
    // A recursive holder is walked only so deep, then reported.
    let deep = format!("{}\"x\"{}", "[".repeat(12_000), "]".repeat(12_000));
    let node: DeepNode = serde_json::from_str(&deep).expect("decodes");
    assert!(node.validate().unchecked().next().is_some());
    // So is a chain of structs between holders, each decoded afresh.
    let mut chain = String::new();
    for _ in 0..130 {
        chain.push_str(&r#"{"s":"#.repeat(120));
        chain.push_str(r#"{"h":"#);
    }
    chain.push_str("\"x\"");
    chain.push_str(&"}".repeat(130 * 121));
    let h: ChainH = serde_json::from_str(&chain).expect("decodes");
    assert!(h.validate().unchecked().next().is_some());
    // Branches that each lead back to the holder: the work is bounded.
    let twice = format!("{}{{}}{}", r#"{"h":"#.repeat(40), "}".repeat(40));
    let started = std::time::Instant::now();
    assert!(report::<Twice>(&twice).iter().any(|f| f.1 == FindingKind::Unevaluated));
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    // A large value under nested holders: the work is bounded by bytes.
    let padded = format!(
        "{}{{\"pad\":\"{}\"}}{}",
        r#"{"next":"#.repeat(30),
        "x".repeat(1 << 20),
        "}".repeat(30)
    );
    let started = std::time::Instant::now();
    let _ = report::<PadNode>(&padded);
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    // No items, nothing unchecked about them.
    // Pattern members are not judged by `additionalProperties`.
    assert_eq!(report::<PatAdd>(r#"{"x":""}"#), vec![]);
    assert_eq!(report::<PatAdd>(r#"{"x":5}"#), vec![v("/x")]);
    assert_eq!(report::<PatAdd>(r#"{"z":"ab"}"#), vec![v("/z")]);
    assert_eq!(report::<PatAdd>(r#"{"z":"abc","k":"q"}"#), vec![]);
    // A member matching several patterns satisfies every one.
    assert_eq!(report::<PatBoth>(r#"{"ab":"xy"}"#), vec![]);
    assert_eq!(report::<PatBoth>(r#"{"ab":"xyz"}"#), vec![v("/ab")]);
    assert_eq!(report::<PatBoth>(r#"{"ab":"x"}"#), vec![v("/ab")]);
    assert_eq!(report::<PatBoth>(r#"{"zz":5}"#), vec![]);
    // A pattern's schema is walked into.
    assert_eq!(report::<PatOwners>(r#"{"id":"1","o1":{"nick":"abcd"}}"#), vec![v("/o1/nick")]);
    assert_eq!(report::<PatOwners>(r#"{"o1":{"nick":"ab"},"z":[]}"#), vec![]);
    // A pattern the generator cannot translate leaves its members undecided.
    assert_eq!(report::<PatUntranslated>(r#"{"x":"s"}"#), vec![u("/x")]);
    // A declared property matching a pattern is checked against it too.
    assert_eq!(report::<PatDecl>(r#"{"ar":"x","ab":"xyz"}"#), vec![v("/ab")]);
    assert_eq!(report::<PatDecl>(r#"{"ar":"xy"}"#), vec![v("/ar")]);
    assert_eq!(report::<PatDecl>(r#"{"ar":"x","an":null}"#), vec![v("/an")]);
    assert_eq!(report::<PatDecl>(r#"{"ar":"x","ab":"y","an":"z","ac":"w"}"#), vec![]);
    assert_eq!(report::<PatDecl>(r#"{"ar":"x","an":"zz"}"#), vec![v("/an")]);
    assert_eq!(report::<PatDeclDenied>(r#"{"ab":"xyz"}"#), vec![v("/ab")]);
    // A field that does not keep the value as sent is not judged by a
    // pattern: a 32-bit float, a substitution.
    assert_eq!(report::<PatF32>(r#"{"xb":0.99999999}"#), vec![u("/xb")]);
    assert_eq!(report::<PatF32>(r#"{"xc":0.5}"#), vec![]);
    assert_eq!(report::<PatSub>(r#"{"xd":"2024-01-01"}"#), vec![u("/xd")]);
    // A float, and in Rust a `serde_json::Value`, may not keep its digits.
    assert_eq!(report::<PatF64Max>(r#"{"xa":9007199254740993}"#), vec![u("/xa")]);
    assert_eq!(report::<PatF64Max>(r#"{"xv":1.0000000000000000001}"#), vec![u("/xv")]);
    assert_eq!(report::<PatF64Max>(r#"{"xb":9007199254740993}"#), vec![v("/xb")]);
    // A type read by other names writes the member back under another.
    assert_eq!(report::<PatAliasRef>(r#"{"xa":{"aa":"long"}}"#), vec![u("/xa"), u("/xa/a")]);
    // A pattern that lets anything be decides nothing, translated or not.
    assert_eq!(report::<PatAnything>(r#"{"id":"a","z":1}"#), vec![]);
    assert_eq!(report::<PatRefAnything>(r#"{"id":"a","z":1}"#), vec![]);
    assert_eq!(report::<PatAnyTranslated>(r#"{"x":1e400}"#), vec![]);
    assert_eq!(report::<PatAnyBeside>(r#"{"x":1,"y":1}"#), vec![]);
    // Matching a pattern that lets anything be still says the member is
    // not one of the others.
    assert_eq!(report::<PatAnyOthers>(r#"{"x":5,"y":"s"}"#), vec![]);
    assert_eq!(report::<PatAnyOthers>(r#"{"y":5}"#), vec![v("/y")]);
    // Untranslated, whether it matches is not known, so neither is whether
    // `additionalProperties` applies.
    assert_eq!(report::<PatAnyUntranslatedClosed>(r#"{"zzz":1}"#), vec![u("/zzz")]);
    assert_eq!(report::<PatAnyUntranslatedTyped>(r#"{"zzz":1}"#), vec![u("/zzz")]);
    // A struct holding its pattern members as text keeps them exactly.
    assert_eq!(report::<PatHoldsPatterned>(r#"{"xInner":{"ab":"xy"}}"#), vec![v("/xInner")]);
    assert_eq!(report::<PatHoldsPatterned>(r#"{"xInner":{}}"#), vec![]);
    // Let in at decode by its pattern, refused by the pattern's `false`.
    assert_eq!(report::<PatClosedFalse>(r#"{"x-a":1}"#), vec![v("/x-a")]);
    assert!(serde_json::from_str::<PatClosedFalse>(r#"{"y":1}"#).is_err());
    // `false` refuses a matching member, declared or not; `not` is unchecked.
    assert_eq!(report::<PatFalse>(r#"{"id":"q"}"#), vec![]);
    assert_eq!(report::<PatFalse>(r#"{"x-a":1}"#), vec![v("/x-a")]);
    assert_eq!(report::<PatFalse>(r#"{"x-id":"a"}"#), vec![v("/x-id")]);
    assert_eq!(report::<PatFalse>(r#"{"n1":1}"#), vec![u("/n1")]);
    // With no `type`, the patterns speak of objects only.
    assert_eq!(report::<PatTypeless>("5"), vec![]);
    assert_eq!(report::<PatTypeless>(r#"{"a":1}"#), vec![]);
    assert_eq!(report::<PatTypeless>(r#"{"a":"x"}"#), vec![v("/a")]);
    // Deprecated types of every kind are used without a warning, and a
    // deprecated composition's annotations are not unevaluated keywords
    // (the typeless `allOf` part's `maxLength` is).
    assert_eq!(report::<UsesOld>(r#"{"o":"x","l":"abc","t":{"kind":"ZooCat"},"c":{"k":1}}"#), vec![u("/l")]);
    // A typeless member takes null.
    assert_eq!(report::<NullName>(r#"{"ab":null,"c":null}"#), vec![v("/ab")]);
    assert_eq!(report::<PatMapAdd>(r#"{"x":1}"#), vec![]);
    assert_eq!(report::<PatMapAdd>(r#"{"x":"s"}"#), vec![v("/x")]);
    // Integers, raw holders and tagged unions decode inside a catch-all
    // (`ZooOrStr` below); `IntCatch` has an alias, so it proves nothing.
    assert_eq!(report::<IntCatchOrStr>(r#"{"x":1}"#), vec![u("")]);
    // A member Rust reads under another name than the schema (a field
    // compiled out, an alias) may fail to decode though valid: undecided.
    assert_eq!(report::<AliasU>(r#"{"aa":5}"#), vec![u("")]);
    assert_eq!(report::<GatedIntU>(r#"{"a":"xyz"}"#), vec![u("")]);
    assert_eq!(report::<GatedIntAll>(r#"{"a":"xyz"}"#), vec![u("")]);
    assert_eq!(report::<GatedClosedU>(r#"{"a":"x"}"#), vec![u("")]);
    // A value of another JSON kind cannot match, whatever the names.
    assert_eq!(report::<GatedIntU>("5"), vec![v("")]);
    assert_eq!(report::<GatedClosedU>(r#""s""#), vec![v("")]);
    assert!(report::<GatedIntAll>("5").contains(&v("")));
    // Decoded under other names, a branch is no proof of a match either.
    assert_eq!(report::<OneAliased>(r#"{"aa":"x"}"#), vec![u("")]);
    assert_eq!(report::<OneGated>(r#"{"a":5}"#), vec![u("")]);
    assert_eq!(report::<GatedIntAll>(r#"{"a":5}"#), vec![u("")]);
    // ...also beside a substitution, and its checks prove nothing either.
    assert_eq!(report::<OneSubAliased>(r#"{"aa":"x"}"#), vec![u("")]);
    assert_eq!(report::<AllSubAliased>(r#"{"aa":"x"}"#), vec![u("")]);
    assert_eq!(report::<OneShortAliased>(r#"{"aa":"xyz"}"#), vec![u("")]);
    assert_eq!(report::<OneHoldsShortAliased>(r#"{"inner":{"aa":"xyz"}}"#), vec![u("")]);
    assert_eq!(report::<ManyShortAliased>(r#"[{"aa":"xyz"}]"#), vec![u("")]);
    assert_eq!(report::<ManyShortAliased>(r#"{"aa":"xyz"}"#), vec![v("")]);
    // On its own, an aliased field does not say which name it was read by.
    assert_eq!(report::<ShortAliased>(r#"{"aa":"xyz"}"#), vec![u("/a")]);
    assert_eq!(report::<ShortAliased>(r#"{"a":"xyz"}"#), vec![u("/a")]);
    assert_eq!(report::<ShortAliased>("{}"), vec![]);
    assert_eq!(report::<RequiredAliased>(r#"{"aa":"xyz"}"#), vec![u("/a")]);
    assert_eq!(report::<ClosedAliased>(r#"{"aa":"x"}"#), vec![u("/a")]);
    assert_eq!(report::<NamesAliased>(r#"{"b":"x"}"#), vec![u("/ab")]);
    assert_eq!(report::<IntCatch>(r#"{"aa":"x","b":1}"#), vec![u("/a")]);
    // Nothing would be decided differently: nothing to report.
    assert_eq!(report::<FreeAliased>(r#"{"aa":"x","r":"y"}"#), vec![]);
    assert_eq!(report::<NamesTrueAliased>(r#"{"aa":"x"}"#), vec![]);
    assert_eq!(report::<NamesEmptyAliased>(r#"{"aa":"x"}"#), vec![]);
    // A required member under an alias is really missing.
    assert_eq!(report::<ReqFreeAliased>(r#"{"aa":"x"}"#), vec![u("/a")]);
    let caught: IntCatch = serde_json::from_str(r#"{"aa":"x","b":7,"c":1.0}"#).unwrap();
    assert_eq!(caught.a.as_deref(), Some("x"));
    assert_eq!(serde_json::to_string(&caught).unwrap(), r#"{"a":"x","b":7,"c":1}"#);
    assert!(serde_json::from_str::<IntCatch>(r#"{"b":1.5}"#).is_err());
    assert!(serde_json::from_str::<IntCatch>(r#"{"a":"x","a":"y"}"#).is_err());
    assert!(serde_json::from_str::<IntCatch>("[]").is_err());
    let zoo: Zoo = serde_json::from_str(r#"{"title":"t","a":{"kind":"ZooCat"},"b":{"kind":"ZooDog"}}"#).unwrap();
    assert_eq!(zoo.additional_properties.len(), 2);
    assert!(serde_json::from_str::<Zoo>(r#"{"a":{"kind":"Cow"}}"#).is_err());
    // A surrogate escape serde_json will not read as a string: undecided.
    assert_eq!(report::<IntOrStr>(r#""\ud800""#), vec![u("")]);
    assert_eq!(report::<ZooOrStr>(r#"{"a":{"kind":"ZooCat"}}"#), vec![]);
    // A substitution that fails to decode proves no mismatch.
    assert_eq!(report::<Ip>(r#""x""#), vec![u("")]);
    // Schemas named like the decoding helpers keep their names.
    let _: ObjectOnly = serde_json::from_str(r#"{"s":"a"}"#).unwrap();
    assert_eq!(report::<InlineItems>("[]"), vec![]);
    assert_eq!(report::<InlineItems>(r#"["a","bc","d"]"#), vec![v("/0"), v("/2")]);
    // A string enum branch reads a JSON string only.
    assert_eq!(report::<EnumOrObj>(r#"{"a":null}"#), vec![]);
    assert_eq!(report::<EnumOrObj>(r#""a""#), vec![]);
    assert_eq!(report::<EnumItems>(r#"[{"a":null}]"#), vec![]);
    assert_eq!(report::<EnumItems>(r#"["a"]"#), vec![]);
    // Members a pattern allows are not refused as unknown.
    assert_eq!(report::<PatOrStr>(r#"{"x":1}"#), vec![]);
    assert_eq!(report::<PatOrStr>(r#"{"x":"s"}"#), vec![v("")]);
    // A member matching no pattern is refused at decode: no branch matches.
    assert_eq!(report::<PatOrStr>(r#"{"y":1}"#), vec![v("")]);
    // A branch that misses for its value is still a miss.
    assert_eq!(report::<IntOrStr>("true"), vec![v("")]);
    assert_eq!(report::<HoldsMixed>(r#"{"m":"zzz","s":{"b":1}}"#), vec![u("/m"), u("/s"), u("/s")]);
    // An opted-out type nested in a branch proves no match.
    assert_eq!(report::<PickWrap>(r#"{"s":"ab"}"#), vec![u("")]);
    // A substituted reference keeps what its target declares.
    assert_eq!(report::<HoldsAliasSub>(r#"{"v":"long"}"#), vec![u("/v")]);
    // An alias spelled as a newtype has its target's null.
    assert_eq!(report::<HoldsNIfAlias>(r#"{"v":null}"#), vec![u("/v")]);
    println!("validation behaves");
}
"##;

#[test]
fn generated_rust_validation_compiles_and_behaves() {
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
    let generation = validated_generation(Target::Rust);
    let dir = std::env::temp_dir().join(format!("roas-codegen-validate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let deps: String = generation
        .dependencies
        .rust
        .iter()
        .map(|d| {
            format!(
                "{} = {{ version = \"1\", features = {:?} }}\n",
                d.name, d.features
            )
        })
        .collect();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"generated\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\n{deps}"),
    )
    .unwrap();
    for file in &generation.files {
        std::fs::write(dir.join("src").join(&file.path), &file.contents).unwrap();
    }
    std::fs::write(dir.join("src/main.rs"), VALIDATE_MAIN).unwrap();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["run", "--quiet"])
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env("RUSTFLAGS", "-D warnings")
        .output()
        .expect("cargo runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("validation behaves"),
        "generated crate failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}\n--- types.rs\n{}",
        generation.files[0].contents
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const GO_VALIDATE_MAIN: &str = r##"package main

import (
	"encoding/json/v2"
	"fmt"
	"reflect"
	"sort"
	"strings"

	"generated/api"
)

type found struct {
	Pointer string
	Kind    api.FindingKind
}

func report[T api.Validator](js string) []found {
	var value T
	if err := json.Unmarshal([]byte(js), &value); err != nil {
		panic(fmt.Sprintf("%s: %v", js, err))
	}
	out := []found{}
	for _, f := range value.Validate().Findings {
		out = append(out, found{f.Pointer, f.Kind})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Pointer < out[j].Pointer })
	return out
}

func v(p string) found { return found{p, api.Violation} }
func u(p string) found { return found{p, api.Unevaluated} }

func check(js string, got []found, want ...found) {
	if want == nil {
		want = []found{}
	}
	if !reflect.DeepEqual(got, want) {
		panic(fmt.Sprintf("%s: got %v, want %v", js, got, want))
	}
}

func main() {
	type P = api.Pet
	for _, c := range []struct {
		js   string
		want []found
	}{
		{`{"name":"rex","age":4,"tags":["a","b"]}`, nil},
		{`{"name":"ééééé","age":2}`, nil},
		{`{"name":"","age":2}`, []found{v("/name")}},
		{`{"name":"toolong","age":2}`, []found{v("/name")}},
		{`{"name":"a","age":-2}`, []found{v("/age")}},
		{`{"name":"a","age":30}`, []found{v("/age")}},
		{`{"name":"a","age":3}`, []found{v("/age")}},
		{`{"name":"a","weight":0.25}`, []found{v("/weight")}},
		{`{"name":"a","weight":100}`, []found{v("/weight")}},
		{`{"name":"a","ratio":1.5}`, []found{v("/ratio")}},
		{`{"name":"a","ratio":2}`, []found{u("/ratio")}},
		{`{"name":"a","tags":[]}`, []found{v("/tags")}},
		{`{"name":"a","tags":["a","b","c","d"]}`, []found{v("/tags")}},
		{`{"name":"a","tags":["a","b","a"]}`, []found{v("/tags/2")}},
		{`{"name":"a","code":"b"}`, []found{v("/code")}},
		{`{"name":"a","code":"ab"}`, nil},
		{`{"name":"a"}`, []found{v("")}},
		{`{"name":"a","age":2,"weight":1,"ratio":1.5,"tags":["x"]}`, []found{v(""), v("/ratio")}},
		{`{"name":"a","extra":{"x":1,"y":2}}`, []found{v("/extra")}},
		{`{"name":"a","owner":{"nick":"long"}}`, []found{v("/owner/nick")}},
		{`{"name":"a","owner":{"nick":null}}`, nil},
		{`{"name":"a","friends":[{"nick":"ok"},{"nick":"long"}]}`, []found{v("/friends/1/nick")}},
	} {
		check(c.js, report[P](c.js), c.want...)
	}
	for _, c := range []struct {
		js   string
		want []found
	}{
		{`{"digits":"123"}`, nil},
		{`{"digits":"١٢٣"}`, []found{v("/digits")}},
		{`{"space":"\u00a0"}`, nil},
		{`{"space":"\u3000"}`, nil},
		{`{"space":"x"}`, []found{v("/space")}},
		{`{"dot":"\u2028"}`, []found{v("/dot")}},
		{`{"dot":"😀"}`, nil},
		{`{"word":"é"}`, []found{v("/word")}},
		{`{"legacy":"123-4567"}`, nil},
		{`{"legacy":"1234567"}`, []found{v("/legacy")}},
		{`{"amp":"a&b"}`, nil},
		{`{"amp":"c"}`, []found{v("/amp")}},
		{`{"astral":"😀😀"}`, nil},
		{`{"astral":"😀"}`, []found{v("/astral")}},
		{`{"boundary":"x"}`, []found{u("/boundary")}},
		{`{"maybe":null}`, nil},
		{`{"maybe":"xax"}`, nil},
		{`{"maybe":"x"}`, []found{v("/maybe")}},
		{`{"none":""}`, []found{v("/none")}},
		{`{"any":"😀"}`, nil},
		{`{"any":"ab"}`, []found{v("/any")}},
	} {
		check(c.js, report[api.Patterned](c.js), c.want...)
	}
	check(`"AB"`, report[api.Code](`"AB"`))
	check(`{"grid":[[0,1],[2]]}`, report[api.Each](`{"grid":[[0,1],[2]]}`))
	check(`{"grid":[[0,-1],[1,2,3]]}`, report[api.Each](`{"grid":[[0,-1],[1,2,3]]}`), v("/grid/0/1"), v("/grid/1"))
	check(`{"scores":{"a":0.5,"b":2}}`, report[api.Each](`{"scores":{"a":0.5,"b":2}}`), v("/scores/b"))
	check(`{"maybe":[null,"ab","a"]}`, report[api.Each](`{"maybe":[null,"ab","a"]}`), v("/maybe/2"))
	check(`{"codes":["ab","A1"]}`, report[api.Each](`{"codes":["ab","A1"]}`), v("/codes/1"))
	check(`{"odd":[]}`, report[api.Each](`{"odd":[]}`))
	check(`{"odd":["x","y"]}`, report[api.Each](`{"odd":["x","y"]}`), u("/odd/0"), u("/odd/1"))
	check(`["a","bc"]`, report[api.EachList](`["a","bc"]`), v("/1"))
	check(`{"p":["abc"]}`, report[api.Pf](`{"p":["abc"]}`), u("/p"))
	check(`["abc"]`, report[api.Pu](`["abc"]`), u(""))
	check(`[5,"abc"]`, report[api.Pr](`[5,"abc"]`), u(""))
	check(`[1]`, report[api.Pn](`[1]`), u(""))
	check(`{"a":"x","b":3,"c":9}`, report[api.Rest](`{"a":"x","b":3,"c":9}`), v("/c"))
	check(`"ABC"`, report[api.Code](`"ABC"`), v(""))
	check(`"ab"`, report[api.Coded](`"ab"`))
	check(`"x1"`, report[api.Coded](`"x1"`), v(""))
	check(`"ab"`, report[api.Either](`"ab"`))
	check(`"xz"`, report[api.Either](`"xz"`))
	check(`"az"`, report[api.Either](`"az"`), v(""))
	check(`"x"`, report[api.Either](`"x"`), v(""))
	check(`["ab","abc"]`, report[api.Shorts](`["ab","abc"]`), v("/1"))
	check(`"ab"`, report[api.Choice](`"ab"`))
	check(`"abc"`, report[api.Choice](`"abc"`), v(""))
	check(`{"nick":"ok"}`, report[api.Choice](`{"nick":"ok"}`))
	check(`1`, report[api.Numeric](`1`))
	check(`1.5`, report[api.Numeric](`1.5`))
	check(`42`, report[api.Shaped](`42`))
	check(`{"x":1}`, report[api.Shaped](`{"x":1}`), v(""))
	check(`"ab"`, report[api.Inline](`"ab"`))
	check(`"abcd"`, report[api.Inline](`"abcd"`), v(""))
	check(`5`, report[api.Inline](`5`))
	check(`"a"`, report[api.Letters](`"a"`))
	check(`"abcd"`, report[api.Letters](`"abcd"`), v(""))
	check(`{"f":0.1}`, report[api.Fine](`{"f":0.1}`), u("/f"))
	check(`{"f":0.2}`, report[api.Fine](`{"f":0.2}`), v("/f"))
	check(`{"choice":null}`, report[api.Fine](`{"choice":null}`), v("/choice"))
	check(`{"abcd":1}`, report[api.Named](`{"abcd":1}`), v("/abcd"))
	check(`{"abc":1}`, report[api.Named](`{"abc":1}`))
	check(`{"ok":"x"}`, report[api.Names](`{"ok":"x"}`))
	check(`{"Bad":"x","ok":"y"}`, report[api.Names](`{"Bad":"x","ok":"y"}`), v("/Bad"))
	check(`{"zz":"x","Q":"y"}`, report[api.Names](`{"zz":"x","Q":"y"}`), v("/Q"))
	check(`{}`, report[api.NamesLook](`{}`))
	check(`{"a":1}`, report[api.NamesLook](`{"a":1}`), u("/a"))
	check(`{"a":1}`, report[api.NamesEnum](`{"a":1}`), u(""))
	check(`{"a":1}`, report[api.NamesTrue](`{"a":1}`))
	check(`{"ab":"x"}`, report[api.NamesGated](`{"ab":"x"}`), v("/ab"))
	check(`edge`, report[api.Edge](`{"big":"x","wide":1.5,"many":[1],"m":-9223372036854775808}`))
	check(`{"m":0}`, report[api.Edge](`{"m":0}`))
	check(`{"m":5}`, report[api.Edge](`{"m":5}`), v("/m"))
	check(`{"a":"x"}`, report[api.Closed](`{"a":"x"}`))
	check(`"ab"`, report[api.TypeList](`"ab"`))
	check(`7`, report[api.TypeList](`7`))
	check(`"abcdef"`, report[api.TypeList](`"abcdef"`), v(""))
	check(`1`, report[api.TypeList](`1`), v(""))
	check(`"ab"`, report[api.MaybeShort](`"ab"`))
	check(`"abc"`, report[api.MaybeShort](`"abc"`), v(""))
	check(`null`, report[api.MaybeShort](`null`))
	check(`1`, report[api.Picks](`1`))
	check(`3`, report[api.Picks](`3`), v(""))
	check(`"a"`, report[api.Itself](`"a"`))
	check(`{"c":null}`, report[api.Holds](`{"c":null}`), v("/c"))
	check(`null`, report[api.OrNull](`null`))
	check(`"ab"`, report[api.OrNull](`"ab"`), v(""))
	check(`{"q":"a","z":"b"}`, report[api.Extended](`{"q":"a","z":"b"}`))
	check(`{"q":"a","z":"bc"}`, report[api.Extended](`{"q":"a","z":"bc"}`), v("/z"))
	check(`1`, report[api.PicksRef](`1`))
	check(`3`, report[api.PicksRef](`3`), v(""))
	check(`{"name":""}`, report[api.Old](`{"name":""}`), v("/name"))
	check(`"a"`, report[api.Both](`"a"`))
	if got := report[api.Both](`null`); len(got) == 0 || got[0] != v("") {
		panic(fmt.Sprintf("null intersection: got %v", got))
	}
	check(`"a"`, report[api.Sib](`"a"`), u(""))
	check(`"a"`, report[api.SibC](`"a"`), u(""))
	check(`3`, report[api.Ni](`3`), v(""))
	check(`{"first":{"kind":"VKitty","lives":10}}`, report[api.VShelter](`{"first":{"kind":"VKitty","lives":10}}`), v("/first/lives"))
	check(`{"first":{"kind":"x","name":"long"}}`, report[api.VShelter](`{"first":{"kind":"x","name":"long"}}`), v("/first/name"))
	check(`{"first":{"kind":"VKitty","lives":9,"name":"ab"}}`, report[api.VShelter](`{"first":{"kind":"VKitty","lives":9,"name":"ab"}}`))
	// Go has no gates: the subtype is read as itself.
	check(`{"g":{"kind":"GCat","lives":10}}`, report[api.GHolder](`{"g":{"kind":"GCat","lives":10}}`), v("/g/lives"))
	check(`{"g":{"kind":"other","lives":10}}`, report[api.GHolder](`{"g":{"kind":"other","lives":10}}`), v("/g/lives"))
	check(`2`, report[api.Ni](`2`))
	check(`2.0`, report[api.Ni](`2.0`))
	check(`{"n":3}`, report[api.Pins](`{"n":3}`), v("/n"))
	check(`0.5`, report[api.FloatEnum](`0.5`), u(""))
	check(`0.75`, report[api.FloatEnum](`0.75`), v(""))
	check(`0.1`, report[api.DoubleEnum](`0.1`))
	check(`2`, report[api.DoubleEnum](`2`))
	check(`3`, report[api.DoubleEnum](`3`), v(""))
	check(`7`, report[api.IntConst](`7`))
	check(`8`, report[api.IntConst](`8`), v(""))
	check(`1`, report[api.NoInt](`1`), v(""))
	check(`9007199254740992`, report[api.BigEnum](`9007199254740992`), u(""))
	check(`9007199254740994`, report[api.BigEnum](`9007199254740994`), v(""))
	check(`9007199254740992`, report[api.MixedEnum](`9007199254740992`))
	check(`0.1`, report[api.FloatTenth](`0.1`), u(""))
	check(`0.099999994`, report[api.FloatTenth](`0.099999994`), v(""))
	check(`3.4028235e38`, report[api.FloatEdge](`3.4028235e38`), u(""))
	check(`1`, report[api.FloatHalf](`1`), u(""))
	check(`1.0000001`, report[api.FloatHalf](`1.0000001`), u(""))
	check(`1.0000002`, report[api.FloatHalf](`1.0000002`), v(""))
	check(`3.4028235e38`, report[api.FloatHuge](`3.4028235e38`), v(""))
	check(`9007199254740996`, report[api.IntConstFloat](`9007199254740996`), v(""))
	check(`{"n":null}`, report[api.IntOrNull](`{"n":null}`))
	check(`{"n":1}`, report[api.IntOrNull](`{"n":1}`))
	check(`{"n":2}`, report[api.IntOrNull](`{"n":2}`), v("/n"))
	check(`-5`, report[api.Narrow](`-5`))
	check(`6`, report[api.Narrow](`6`), v(""))
	check(`{"c":1.5}`, report[api.NumConst](`{"c":1.5}`))
	check(`{"c":2}`, report[api.NumConst](`{"c":2}`), v("/c"))
	check(`{"k":false}`, report[api.Pins](`{"k":false}`), u("/k"))
	check(`"ab"`, report[api.EnumO](`"ab"`))
	check(`5`, report[api.EnumO](`5`))
	check(`6`, report[api.EnumO](`6`), v(""))
	check(`5`, report[api.Def](`5`))
	check(`0`, report[api.Def](`0`), v(""))
	check(`"cde"`, report[api.EnumN](`"cde"`), v(""))
	check(`0.99999999`, report[api.F32X](`0.99999999`), u(""))
	check(`0.5`, report[api.F32X](`0.5`))
	check(`"wrong"`, report[api.HolderC](`"wrong"`), u(""))
	check(`{"n":null}`, report[api.NullC](`{"n":null}`), u("/n"))
	if got := report[api.HoldsBoth](`{"b":null}`); len(got) == 0 || got[0] != v("/b") {
		panic(fmt.Sprintf("null intersection field: got %v", got))
	}
	check(`null`, report[api.NullSib](`null`))
	check(`"a"`, report[api.EdgeBranch](`"a"`))
	check(`{"f":1.0000000596046448}`, report[api.Mid](`{"f":1.0000000596046448}`), u("/f"))
	check(`"a"`, report[api.NoNullList](`"a"`))
	check(`null`, report[api.NoNullList](`null`), v(""))
	check(`"a"`, report[api.NoNull](`"a"`))
	check(`null`, report[api.NoNull](`null`), v(""))
	check(`null`, report[api.PartNull](`null`), u(""))
	check(`[1]`, report[api.Items](`[1]`))
	check(`[5]`, report[api.Items](`[5]`), v(""))
	check(`["a"]`, report[api.StrItems](`["a"]`))
	check(`{"a":"x"}`, report[api.PatMap](`{"a":"x"}`))
	check(`{"a":1}`, report[api.PatMap](`{"a":1}`))
	check(`{"a":true}`, report[api.PatMap](`{"a":true}`), v(""))
	check(`null`, report[api.NullOnly](`null`))
	check(`{"n":null}`, report[api.HoldsScalar](`{"n":null}`), u("/n"))
	check(`{"k":[1]}`, report[api.PickK](`{"k":[1]}`))
	check(`{"k":["x"]}`, report[api.PickC](`{"k":["x"]}`), u(""))
	check(`5`, report[api.FmtList](`5`))
	check(`"a"`, report[api.EnumF](`"a"`))
	check(`["x"]`, report[api.ObjOrArr](`["x"]`))
	check(`[]`, report[api.ClosedOrInt](`[]`), v(""))
	check(`["x"]`, report[api.AliasOrArr](`["x"]`))
	check(`{}`, report[api.RawMin](`{}`), u(""))
	check(`{}`, report[api.ReqOnly](`{}`), u(""))
	check(`{"v":null}`, report[api.HoldsNIf](`{"v":null}`), u("/v"))
	check(`null`, report[api.ListIf](`null`), u(""))
	check(`{"n":null}`, report[api.HoldsScalarAlias](`{"n":null}`), u("/n"))
	check(`"a"`, report[api.EnumIf](`"a"`), u(""))
	check(`{"f":1.000000059604644775390624}`, report[api.Mid2](`{"f":1.000000059604644775390624}`), u("/f"))
	check(`81435393652414055e254`, report[api.DMin](`81435393652414055e254`))
	if got := report[api.HoldsSibEnum](`{"e":null}`); len(got) == 0 {
		panic("null outside the siblings' enum passed")
	}
	check(`{"value":null}`, report[api.HoldsListIf](`{"value":null}`), u("/value"))
	check(`{"v":null}`, report[api.HoldsNIfOff](`{"v":null}`))
	check(`{"d":"x"}`, report[api.Dated](`{"d":"x"}`), u("/d"))
	check(`[{"k":"v"}]`, report[api.SmallMaps](`[{"k":"v"}]`))
	check(`[{"a":"1","b":"2","c":"3"}]`, report[api.SmallMaps](`[{"a":"1","b":"2","c":"3"}]`), v("/0"))
	check(`[["x"]]`, report[api.NestedClosed](`[["x"]]`))
	check(`{"m":"abcdef"}`, report[api.HoldsMoney](`{"m":"abcdef"}`), u("/m"))
	check(`["abcdef"]`, report[api.Monies](`["abcdef"]`), u(""))
	check(`[]`, report[api.Monies](`[]`))
	check(`{"k":"abcdef"}`, report[api.MoneyMap](`{"k":"abcdef"}`), u(""))
	check(`{"d":"long"}`, report[api.HoldsDate](`{"d":"long"}`), u("/d"))
	check(`{"a":{"s":"b"}}`, report[api.Leaves](`{"a":{"s":"b"}}`))
	check(`null`, report[api.ListObj](`null`), u(""))
	check(`"a"`, report[api.EnumObj](`"a"`))
	check(`{"x":2}`, report[api.EnumObj](`{"x":2}`), u(""))
	check(`null`, report[api.NullIf](`null`), u(""))
	check(`null`, report[api.ListNullOnly](`null`), u(""))
	check(`"abc"`, report[api.OffOrOn](`"abc"`), u(""))
	check(`3.4028235677973366e38`, report[api.F32Edge](`3.4028235677973366e38`), u(""))
	check(`1.5`, report[api.F32Edge](`1.5`))
	check(`{"c":"BAD"}`, report[api.HoldsCurrency](`{"c":"BAD"}`), u("/c"))
	check(`null`, report[api.NullType](`null`), u(""))
	check(`{"s":"ab"}`, report[api.PickWrap](`{"s":"ab"}`), u(""))
	check(`{"b":"x"}`, report[api.GatedField](`{"b":"x"}`))
	check(`{"n":null}`, report[api.HoldsNullOnly](`{"n":null}`), u("/n"))
	check(`{}`, report[api.HoldsNullOnly](`{}`))
	check(`9223372036854775808`, report[api.IntOrNum](`9223372036854775808`), u(""))
	check(`1e400`, report[api.IntOrStr](`1e400`), u(""))
	check(`[1e19]`, report[api.ArrOrStr](`[1e19]`), u(""))
	check(`{"n":1e19}`, report[api.ObjOrStr](`{"n":1e19}`), u(""))
	check(`true`, report[api.IntOrStr](`true`), v(""))
	if got := report[api.DeepNode](strings.Repeat("[", 2000) + `"x"` + strings.Repeat("]", 2000)); len(got) == 0 {
		panic("a deep recursive holder passed unwalked")
	}
	check(`{"a":null}`, report[api.EnumOrObj](`{"a":null}`))
	check(`[{"a":null}]`, report[api.EnumItems](`[{"a":null}]`))
	check(`{"x":""}`, report[api.PatAdd](`{"x":""}`))
	check(`{"x":5}`, report[api.PatAdd](`{"x":5}`), v("/x"))
	check(`{"z":"ab"}`, report[api.PatAdd](`{"z":"ab"}`), v("/z"))
	check(`{"z":"abc","k":"q"}`, report[api.PatAdd](`{"z":"abc","k":"q"}`))
	check(`{"ab":"xy"}`, report[api.PatBoth](`{"ab":"xy"}`))
	check(`{"ab":"xyz"}`, report[api.PatBoth](`{"ab":"xyz"}`), v("/ab"))
	check(`{"ab":"x"}`, report[api.PatBoth](`{"ab":"x"}`), v("/ab"))
	check(`{"zz":5}`, report[api.PatBoth](`{"zz":5}`))
	check(`{"id":"1","o1":{"nick":"abcd"}}`, report[api.PatOwners](`{"id":"1","o1":{"nick":"abcd"}}`), v("/o1/nick"))
	check(`{"o1":{"nick":"ab"},"z":[]}`, report[api.PatOwners](`{"o1":{"nick":"ab"},"z":[]}`))
	check(`{"x":"s"}`, report[api.PatUntranslated](`{"x":"s"}`), u("/x"))
	check(`{"ar":"x","ab":"xyz"}`, report[api.PatDecl](`{"ar":"x","ab":"xyz"}`), v("/ab"))
	check(`{"ar":"xy"}`, report[api.PatDecl](`{"ar":"xy"}`), v("/ar"))
	check(`{"ar":"x","an":null}`, report[api.PatDecl](`{"ar":"x","an":null}`), v("/an"))
	check(`{"ar":"x","ab":"y","an":"z","ac":"w"}`, report[api.PatDecl](`{"ar":"x","ab":"y","an":"z","ac":"w"}`))
	check(`{"ar":"x","an":"zz"}`, report[api.PatDecl](`{"ar":"x","an":"zz"}`), v("/an"))
	check(`{"ab":"xyz"}`, report[api.PatDeclDenied](`{"ab":"xyz"}`), v("/ab"))
	check(`{"xb":0.99999999}`, report[api.PatF32](`{"xb":0.99999999}`), u("/xb"))
	check(`{"xc":0.5}`, report[api.PatF32](`{"xc":0.5}`))
	check(`{"xd":"2024-01-01"}`, report[api.PatSub](`{"xd":"2024-01-01"}`), u("/xd"))
	check(`{"xa":9007199254740993}`, report[api.PatF64Max](`{"xa":9007199254740993}`), u("/xa"))
	// Go holds an untyped member as its text: decided.
	check(`{"xv":1.0000000000000000001}`, report[api.PatF64Max](`{"xv":1.0000000000000000001}`), v("/xv"))
	check(`{"xb":9007199254740993}`, report[api.PatF64Max](`{"xb":9007199254740993}`), v("/xb"))
	// Go reads no alias: the member is an undeclared one of the inner type.
	check(`{"xa":{"aa":"long"}}`, report[api.PatAliasRef](`{"xa":{"aa":"long"}}`), v("/xa/aa"))
	check(`{"id":"a","z":1}`, report[api.PatAnything](`{"id":"a","z":1}`))
	check(`{"id":"a","z":1}`, report[api.PatRefAnything](`{"id":"a","z":1}`))
	check(`{"x":1e400}`, report[api.PatAnyTranslated](`{"x":1e400}`))
	check(`{"x":1,"y":1}`, report[api.PatAnyBeside](`{"x":1,"y":1}`))
	check(`{"x":5,"y":"s"}`, report[api.PatAnyOthers](`{"x":5,"y":"s"}`))
	check(`{"y":5}`, report[api.PatAnyOthers](`{"y":5}`), v("/y"))
	check(`{"zzz":1}`, report[api.PatAnyUntranslatedClosed](`{"zzz":1}`), u("/zzz"))
	check(`{"zzz":1}`, report[api.PatAnyUntranslatedTyped](`{"zzz":1}`), u("/zzz"))
	check(`{"xInner":{"ab":"xy"}}`, report[api.PatHoldsPatterned](`{"xInner":{"ab":"xy"}}`), v("/xInner"))
	check(`{"xInner":{}}`, report[api.PatHoldsPatterned](`{"xInner":{}}`))
	check(`{"x-a":1}`, report[api.PatClosedFalse](`{"x-a":1}`), v("/x-a"))
	check(`{"id":"q"}`, report[api.PatFalse](`{"id":"q"}`))
	check(`{"x-a":1}`, report[api.PatFalse](`{"x-a":1}`), v("/x-a"))
	check(`{"x-id":"a"}`, report[api.PatFalse](`{"x-id":"a"}`), v("/x-id"))
	check(`{"n1":1}`, report[api.PatFalse](`{"n1":1}`), u("/n1"))
	check(`5`, report[api.PatTypeless](`5`))
	check(`{"a":1}`, report[api.PatTypeless](`{"a":1}`))
	check(`{"a":"x"}`, report[api.PatTypeless](`{"a":"x"}`), v("/a"))
	check(`{"o":"x","l":"abc","t":{"kind":"ZooCat"},"c":{"k":1}}`, report[api.UsesOld](`{"o":"x","l":"abc","t":{"kind":"ZooCat"},"c":{"k":1}}`), u("/l"))
	check(`{"ab":null,"c":null}`, report[api.NullName](`{"ab":null,"c":null}`), v("/ab"))
	check(`{"x":1}`, report[api.PatMapAdd](`{"x":1}`))
	check(`{"x":"s"}`, report[api.PatMapAdd](`{"x":"s"}`), v("/x"))
	check(`{"x":1}`, report[api.IntCatchOrStr](`{"x":1}`))
	// Go has no cfg gates or aliases: these decode and match (beside a
	// substitution, a failed decode is undecided in Go too).
	check(`{"aa":5}`, report[api.AliasU](`{"aa":5}`))
	check(`{"a":"xyz"}`, report[api.GatedIntU](`{"a":"xyz"}`))
	check(`{"a":"xyz"}`, report[api.GatedIntAll](`{"a":"xyz"}`))
	check(`{"a":"x"}`, report[api.GatedClosedU](`{"a":"x"}`))
	check(`5`, report[api.GatedIntU](`5`), v(""))
	check(`"s"`, report[api.GatedClosedU](`"s"`), v(""))
	check(`{"aa":"x"}`, report[api.OneAliased](`{"aa":"x"}`))
	check(`{"a":5}`, report[api.OneGated](`{"a":5}`))
	check(`{"a":5}`, report[api.GatedIntAll](`{"a":5}`), v(""))
	check(`{"aa":"x"}`, report[api.OneSubAliased](`{"aa":"x"}`), u(""))
	check(`{"aa":"x"}`, report[api.AllSubAliased](`{"aa":"x"}`), u(""))
	check(`{"aa":"xyz"}`, report[api.OneShortAliased](`{"aa":"xyz"}`))
	check(`{"inner":{"aa":"xyz"}}`, report[api.OneHoldsShortAliased](`{"inner":{"aa":"xyz"}}`))
	check(`{"aa":"xyz"}`, report[api.ShortAliased](`{"aa":"xyz"}`))
	check(`{"a":"xyz"}`, report[api.ShortAliased](`{"a":"xyz"}`), v("/a"))
	check(`[{"aa":"xyz"}]`, report[api.ManyShortAliased](`[{"aa":"xyz"}]`))
	check(`{"aa":"xyz"}`, report[api.ManyShortAliased](`{"aa":"xyz"}`), v(""))
	check(`{"a":{"kind":"ZooCat"}}`, report[api.ZooOrStr](`{"a":{"kind":"ZooCat"}}`))
	check(`"x"`, report[api.Ip](`"x"`), u(""))
	check(`[]`, report[api.InlineItems](`[]`))
	check(`["a","bc","d"]`, report[api.InlineItems](`["a","bc","d"]`), v("/0"), v("/2"))
	padded := strings.Repeat(`{"next":`, 30) + `{"pad":"` + strings.Repeat("x", 1<<20) + `"}` + strings.Repeat("}", 30)
	_ = report[api.PadNode](padded)
	check(`{"x":1}`, report[api.PatOrStr](`{"x":1}`))
	check(`{"x":"s"}`, report[api.PatOrStr](`{"x":"s"}`), v(""))
	check(`{"y":1}`, report[api.PatOrStr](`{"y":1}`), v(""))
	twice := strings.Repeat(`{"h":`, 40) + `{}` + strings.Repeat("}", 40)
	if got := report[api.Twice](twice); len(got) == 0 {
		panic("doubling branches passed unchecked")
	}
	// A substitution spelled as an interface or a map decodes by its own rules.
	check(`{"s":{"n":-1}}`, report[api.HoldsPoint](`{"s":{"n":-1}}`), u("/s"))
	check(`{"b":{"a":[1]},"l":[{"x":{}}]}`, report[api.HoldsBag](`{"b":{"a":[1]},"l":[{"x":{}}]}`), u("/b"), u("/l"))
	check(`{"s2":{"x":1}}`, report[api.HoldsSub2](`{"s2":{"x":1}}`), u("/s2"))
	check(`{"s3":{"p":1}}`, report[api.HoldsSub3](`{"s3":{"p":1}}`), u("/s3"))
	check(`{"m":"zzz","s":{"b":1}}`, report[api.HoldsMixed](`{"m":"zzz","s":{"b":1}}`), u("/m"), u("/s"), u("/s"))
	check(`{"v":"long"}`, report[api.HoldsAliasSub](`{"v":"long"}`), u("/v"))
	check(`{"v":null}`, report[api.HoldsNIfAlias](`{"v":null}`), u("/v"))
	fmt.Println("validation behaves")
}
"##;

#[test]
fn generated_go_validation_compiles_and_behaves() {
    let required = std::env::var_os("ROAS_CODEGEN_REQUIRE_GO").is_some();
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() && !required {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
    if let Err(reason) = go_toolchain() {
        assert!(!required, "ROAS_CODEGEN_REQUIRE_GO is set: {reason}");
        eprintln!("skipped: {reason}");
        return;
    }
    let generation = validated_generation(Target::Go);
    let dir = std::env::temp_dir().join(format!("roas-codegen-go-validate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("api")).unwrap();
    std::fs::write(dir.join("go.mod"), "module generated\n\ngo 1.27\n").unwrap();
    for file in &generation.files {
        std::fs::write(dir.join("api").join(&file.path), &file.contents).unwrap();
    }
    std::fs::write(dir.join("main.go"), GO_VALIDATE_MAIN).unwrap();
    let output = Command::new("go")
        .args(["run", "."])
        .current_dir(&dir)
        .env("GOTOOLCHAIN", "local")
        .env("GOWORK", "off")
        .output()
        .expect("go runs");
    let vet = Command::new("go")
        .args(["vet", "./..."])
        .current_dir(&dir)
        .env("GOTOOLCHAIN", "local")
        .env("GOWORK", "off")
        .output()
        .expect("go vet runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("validation behaves"),
        "generated Go failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}\n--- {}\n{}",
        generation.files[0].path.display(),
        generation.files[0].contents
    );
    assert!(
        vet.status.success(),
        "go vet\n{}",
        String::from_utf8_lossy(&vet.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "exact-numbers")]
fn exact_generation(target: Target) -> roas_codegen::Generation {
    let description = serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {
            "Price": {"type": "object", "required": ["amount"], "properties": {
                "amount": {"type": "number", "minimum": 0, "exclusiveMaximum": 1000000, "multipleOf": 0.01},
                "rate": {"type": "number", "format": "float", "minimum": 0.1},
                "big": {"type": "number"},
                "tags": {"type": "array", "items": {"type": "number"}, "uniqueItems": true},
                "code": {"type": "number", "enum": [1.5, 2.50, 1e2]},
                "half": {"type": "number", "multipleOf": 0.5},
                "fixed": {"type": "number", "const": -0.25}
            }},
            "Rates": {"type": "array", "items": {"type": "number", "maximum": 1}},
            "Either": {"oneOf": [{"type": "number"}, {"type": "string"}]},
            "Pat": {"type": "object", "patternProperties": {"^x": {"type": "number"}}}
        }}
    });
    let mut description = description;
    // Literals past what a decimal holds, read from text: `json!` would
    // read them as floats.
    description["components"]["schemas"]["Long"] = serde_json::from_str(
        r#"{"type": "object", "properties": {
            "lo": {"type": "number", "minimum": 0.10000000000000000000000000000000000000001},
            "hi": {"type": "number", "exclusiveMaximum": 0.10000000000000000000000000000000000000001},
            "huge": {"type": "number", "maximum": 1e3000000000},
            "never": {"type": "number", "minimum": 1e3000000000},
            "tiny": {"type": "number", "exclusiveMinimum": -1e-3000000000},
            "pick": {"type": "number", "enum": [1, 1.000000000000000000000000000000000000001]},
            "only": {"type": "number", "const": 1.000000000000000000000000000000000000001},
            "odd": {"type": "number", "multipleOf": 1.000000000000000000000000000000000000001},
            "edge": {"type": "number", "minimum": 1e2147483648}
        }}"#,
    )
    .unwrap();
    let bytes = serde_json::to_vec(&description).unwrap();
    let doc =
        SourceDocument::parse(Input::Json(bytes), Url::parse("file:///x.json").unwrap()).unwrap();
    let doc = validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).unwrap();
    let mut config = ConfigFile::default();
    config.target = Some(target);
    config.validation = Some(true);
    config.exact_numbers = Some(true);
    let generation = generate(&doc, &config.build().unwrap()).expect("generates");
    assert!(!generation.has_errors(), "{:?}", generation.diagnostics);
    generation
}

#[cfg(feature = "exact-numbers")]
const EXACT_MAIN: &str = r##"
#[allow(dead_code)]
mod types;
use types::*;

fn report<T: serde::de::DeserializeOwned + Validate>(json: &str) -> Vec<(String, FindingKind)> {
    let value: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    let mut found: Vec<_> = value.validate().findings.into_iter().map(|f| (f.pointer, f.kind)).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn v(pointer: &str) -> (String, FindingKind) {
    (pointer.to_owned(), FindingKind::Violation)
}

fn u(pointer: &str) -> (String, FindingKind) {
    (pointer.to_owned(), FindingKind::Unevaluated)
}

fn d(text: &str) -> Decimal {
    text.parse().unwrap()
}

fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(json: &str) -> String {
    serde_json::to_string(&serde_json::from_str::<T>(json).unwrap()).unwrap()
}

fn main() {
    // Held exactly, written in the shortest form.
    let price: Price = serde_json::from_str(r#"{"amount":12.30}"#).unwrap();
    assert_eq!(price.amount, d("12.3"));
    assert_eq!(round_trip::<Price>(r#"{"amount":12.30}"#), r#"{"amount":12.3}"#);
    let exact = r#"{"amount":0,"big":1.0000000000000000000000000000000000001}"#;
    assert_eq!(round_trip::<Price>(exact), exact);
    assert_eq!(
        round_trip::<Price>(r#"{"amount":0,"big":123456789012345678901234567890123456e100}"#),
        r#"{"amount":0,"big":1.23456789012345678901234567890123456e135}"#
    );
    assert_eq!(round_trip::<Price>(r#"{"amount":0,"big":0.000001}"#), r#"{"amount":0,"big":0.000001}"#);
    assert_eq!(round_trip::<Price>(r#"{"amount":0,"big":1e-7}"#), r#"{"amount":0,"big":1e-7}"#);
    assert_eq!(round_trip::<Price>(r#"{"amount":0,"big":-12e-8}"#), r#"{"amount":0,"big":-1.2e-7}"#);
    assert_eq!(format!("[{:>6}]", d("-1.5")), "[  -1.5]");
    assert_eq!(round_trip::<Price>(r#"{"amount":0,"big":-1E+21}"#), r#"{"amount":0,"big":-1e21}"#);
    assert_eq!(round_trip::<Price>(r#"{"amount":-0.0}"#), r#"{"amount":0}"#);
    // Past 38 digits or an i32 exponent, and anything but a number, is refused.
    for json in [
        r#"{"amount":1.000000000000000000000000000000000000001}"#,
        r#"{"amount":1e2147483648}"#,
        r#"{"amount":"1.5"}"#,
        r#"{"amount":null}"#,
    ] {
        assert!(serde_json::from_str::<Price>(json).is_err(), "{json}");
    }
    assert_eq!(round_trip::<Price>(r#"{"amount":0e999999999999}"#), r#"{"amount":0}"#);
    // Ordered exactly, and constructed from parts.
    assert!(d("-1.5") < d("-1.25") && d("1e-30") < d("1e30") && d("0.1") > d("0.09999999999999999999999"));
    assert_eq!(Decimal::new(1500, -3), Some(d("1.5")));
    assert_eq!(Decimal::new(1, -1).map(|x| x.to_f64()), Some(0.1));
    assert!(d("1e30").is_multiple_of(&d("1e-30")) && !d("1e-30").is_multiple_of(&d("0.5")));
    // Every check decided, exactly.
    assert_eq!(report::<Price>(r#"{"amount":999999.99}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0.015}"#), vec![v("/amount")]);
    assert_eq!(report::<Price>(r#"{"amount":1000000}"#), vec![v("/amount")]);
    assert_eq!(report::<Price>(r#"{"amount":-0.01}"#), vec![v("/amount")]);
    assert_eq!(report::<Price>(r#"{"amount":0,"rate":0.1}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0,"rate":0.09999999999999999999}"#), vec![v("/rate")]);
    assert_eq!(report::<Price>(r#"{"amount":0,"tags":[1,1.0]}"#), vec![v("/tags/1")]);
    assert_eq!(report::<Price>(r#"{"amount":0,"tags":[0.1,0.10000000000000000001]}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0,"code":2.5}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0,"code":100}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0,"code":2.51}"#), vec![v("/code")]);
    assert_eq!(report::<Price>(r#"{"amount":0,"half":1e30}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0,"half":1e-30}"#), vec![v("/half")]);
    assert_eq!(report::<Price>(r#"{"amount":0,"fixed":-0.250}"#), vec![]);
    assert_eq!(report::<Price>(r#"{"amount":0,"fixed":-0.26}"#), vec![v("/fixed")]);
    assert_eq!(report::<Rates>(r#"[1, 1.0000000000000000000000001]"#), vec![v("/1")]);
    // A bound with more digits than a decimal holds still decides every value.
    let long = r#"{"lo":0.1000000000000000000000000000000000001,"hi":0.1,"huge":1e300,"tiny":0,"pick":1}"#;
    assert_eq!(report::<Long>(long), vec![]);
    assert_eq!(report::<Long>(r#"{"lo":0.1}"#), vec![v("/lo")]);
    assert_eq!(report::<Long>(r#"{"hi":0.1000000000000000000000000000000000001}"#), vec![v("/hi")]);
    assert_eq!(report::<Long>(r#"{"never":1e300}"#), vec![v("/never")]);
    assert_eq!(report::<Long>(r#"{"tiny":-1e-30}"#), vec![v("/tiny")]);
    assert_eq!(report::<Long>(r#"{"pick":2}"#), vec![v("/pick")]);
    assert_eq!(report::<Long>(r#"{"only":1}"#), vec![v("/only")]);
    assert_eq!(report::<Long>(r#"{"odd":3}"#), vec![u("/odd")]);
    assert_eq!(report::<Long>(r#"{"edge":1.1e2147483648}"#), vec![]);
    assert_eq!(report::<Long>(r#"{"edge":9e2147483647}"#), vec![v("/edge")]);
    // A rounded bound is still named by its keyword.
    let found = serde_json::from_str::<Long>(r#"{"hi":0.1000000000000000000000000000000000001}"#).unwrap().validate();
    assert!(found.findings[0].message.contains("exclusiveMaximum"), "{found:?}");
    // A number no decimal holds fails a branch for its representation:
    // nothing is proven.
    let wide = "1.000000000000000000000000000000000000001";
    assert_eq!(report::<Either>(r#""s""#), vec![]);
    assert_eq!(report::<Either>(wide), vec![u("")]);
    assert_eq!(report::<Pat>(&format!(r#"{{"x1":{wide}}}"#)), vec![u("/x1")]);
    println!("exact numbers behave");
}
"##;

#[cfg(feature = "exact-numbers")]
#[test]
fn generated_rust_exact_numbers_compile_and_behave() {
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
    let generation = exact_generation(Target::Rust);
    let dir = std::env::temp_dir().join(format!("roas-codegen-exact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let deps: String = generation
        .dependencies
        .rust
        .iter()
        .map(|d| {
            format!(
                "{} = {{ version = \"1\", features = {:?} }}\n",
                d.name, d.features
            )
        })
        .collect();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"generated\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\n{deps}"),
    )
    .unwrap();
    for file in &generation.files {
        std::fs::write(dir.join("src").join(&file.path), &file.contents).unwrap();
    }
    std::fs::write(dir.join("src/main.rs"), EXACT_MAIN).unwrap();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["run", "--quiet"])
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env("RUSTFLAGS", "-D warnings")
        .output()
        .expect("cargo runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("exact numbers behave"),
        "generated crate failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}\n--- types.rs\n{}",
        generation.files[0].contents
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "exact-numbers")]
const GO_EXACT_MAIN: &str = r##"package main

import (
	"encoding/json/v2"
	"fmt"
	"reflect"
	"sort"
	"strings"

	"generated/api"
)

type found struct {
	Pointer string
	Kind    api.FindingKind
}

func report[T api.Validator](js string) []found {
	var value T
	if err := json.Unmarshal([]byte(js), &value); err != nil {
		panic(fmt.Sprintf("%s: %v", js, err))
	}
	out := []found{}
	for _, f := range value.Validate().Findings {
		out = append(out, found{f.Pointer, f.Kind})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Pointer < out[j].Pointer })
	return out
}

func v(p string) found { return found{p, api.Violation} }
func u(p string) found { return found{p, api.Unevaluated} }

func check(what string, got []found, want ...found) {
	if want == nil {
		want = []found{}
	}
	if !reflect.DeepEqual(got, want) {
		panic(fmt.Sprintf("%s: got %v, want %v", what, got, want))
	}
}

func d(text string) api.Decimal {
	x, err := api.ParseDecimal(text)
	if err != nil {
		panic(err)
	}
	return x
}

func roundTrip(js string) string {
	var p api.Price
	if err := json.Unmarshal([]byte(js), &p); err != nil {
		panic(fmt.Sprintf("%s: %v", js, err))
	}
	out, err := json.Marshal(p)
	if err != nil {
		panic(err)
	}
	return string(out)
}

func same(got, want string) {
	if got != want {
		panic(fmt.Sprintf("got %s, want %s", got, want))
	}
}

func main() {
	var price api.Price
	if err := json.Unmarshal([]byte(`{"amount":12.30}`), &price); err != nil || price.Amount != d("12.3") {
		panic(fmt.Sprint(price, err))
	}
	same(roundTrip(`{"amount":12.30}`), `{"amount":12.3}`)
	exact := `{"amount":0,"big":1.0000000000000000000000000000000000001}`
	same(roundTrip(exact), exact)
	same(roundTrip(`{"amount":0,"big":123456789012345678901234567890123456e100}`), `{"amount":0,"big":1.23456789012345678901234567890123456e135}`)
	same(roundTrip(`{"amount":0,"big":0.000001}`), `{"amount":0,"big":0.000001}`)
	same(roundTrip(`{"amount":0,"big":1e-7}`), `{"amount":0,"big":1e-7}`)
	same(roundTrip(`{"amount":0,"big":-12e-8}`), `{"amount":0,"big":-1.2e-7}`)
	same(roundTrip(`{"amount":0,"big":-1E+21}`), `{"amount":0,"big":-1e21}`)
	same(roundTrip(`{"amount":-0.0}`), `{"amount":0}`)
	for _, js := range []string{
		`{"amount":1.000000000000000000000000000000000000001}`,
		`{"amount":1e2147483648}`,
		`{"amount":"1.5"}`,
		`{"amount":null}`,
	} {
		var p api.Price
		if json.Unmarshal([]byte(js), &p) == nil {
			panic("accepted " + js)
		}
	}
	same(roundTrip(`{"amount":0e999999999999}`), `{"amount":0}`)
	if !(d("-1.5").Cmp(d("-1.25")) < 0 && d("1e-30").Cmp(d("1e30")) < 0 && d("0.1").Cmp(d("0.09999999999999999999999")) > 0) {
		panic("order")
	}
	if d("0.1").Float64() != 0.1 || !d("1e30").IsMultipleOf(d("1e-30")) || d("1e-30").IsMultipleOf(d("0.5")) {
		panic("arithmetic")
	}
	check("in range", report[api.Price](`{"amount":999999.99}`))
	check("multipleOf", report[api.Price](`{"amount":0.015}`), v("/amount"))
	check("exclusiveMaximum", report[api.Price](`{"amount":1000000}`), v("/amount"))
	check("minimum", report[api.Price](`{"amount":-0.01}`), v("/amount"))
	check("rate", report[api.Price](`{"amount":0,"rate":0.1}`))
	check("rate below", report[api.Price](`{"amount":0,"rate":0.09999999999999999999}`), v("/rate"))
	check("unique", report[api.Price](`{"amount":0,"tags":[1,1.0]}`), v("/tags/1"))
	check("distinct", report[api.Price](`{"amount":0,"tags":[0.1,0.10000000000000000001]}`))
	check("enum", report[api.Price](`{"amount":0,"code":2.5}`))
	check("enum 100", report[api.Price](`{"amount":0,"code":100}`))
	check("not enum", report[api.Price](`{"amount":0,"code":2.51}`), v("/code"))
	check("half", report[api.Price](`{"amount":0,"half":1e30}`))
	check("not half", report[api.Price](`{"amount":0,"half":1e-30}`), v("/half"))
	check("const", report[api.Price](`{"amount":0,"fixed":-0.250}`))
	check("not const", report[api.Price](`{"amount":0,"fixed":-0.26}`), v("/fixed"))
	check("items", report[api.Rates](`[1, 1.0000000000000000000000001]`), v("/1"))
	check("long", report[api.Long](`{"lo":0.1000000000000000000000000000000000001,"hi":0.1,"huge":1e300,"tiny":0,"pick":1}`))
	check("lo", report[api.Long](`{"lo":0.1}`), v("/lo"))
	check("hi", report[api.Long](`{"hi":0.1000000000000000000000000000000000001}`), v("/hi"))
	check("never", report[api.Long](`{"never":1e300}`), v("/never"))
	check("tiny", report[api.Long](`{"tiny":-1e-30}`), v("/tiny"))
	check("pick", report[api.Long](`{"pick":2}`), v("/pick"))
	check("only", report[api.Long](`{"only":1}`), v("/only"))
	check("odd", report[api.Long](`{"odd":3}`), u("/odd"))
	check("edge", report[api.Long](`{"edge":1.1e2147483648}`))
	check("below edge", report[api.Long](`{"edge":9e2147483647}`), v("/edge"))
	var hi api.Long
	if err := json.Unmarshal([]byte(`{"hi":0.1000000000000000000000000000000000001}`), &hi); err != nil {
		panic(err)
	}
	if got := hi.Validate().Findings; len(got) != 1 || !strings.Contains(got[0].Message, "exclusiveMaximum") {
		panic(fmt.Sprint(got))
	}
	wide := "1.000000000000000000000000000000000000001"
	check("either", report[api.Either](`"s"`))
	check("either wide", report[api.Either](wide), u(""))
	check("pattern wide", report[api.Pat](`{"x1":`+wide+`}`), u("/x1"))
	fmt.Println("exact numbers behave")
}
"##;

#[cfg(feature = "exact-numbers")]
#[test]
fn generated_go_exact_numbers_compile_and_behave() {
    let required = std::env::var_os("ROAS_CODEGEN_REQUIRE_GO").is_some();
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() && !required {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
    if let Err(reason) = go_toolchain() {
        assert!(!required, "ROAS_CODEGEN_REQUIRE_GO is set: {reason}");
        eprintln!("skipped: {reason}");
        return;
    }
    let generation = exact_generation(Target::Go);
    let dir = std::env::temp_dir().join(format!("roas-codegen-go-exact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("api")).unwrap();
    std::fs::write(dir.join("go.mod"), "module generated\n\ngo 1.27\n").unwrap();
    for file in &generation.files {
        std::fs::write(dir.join("api").join(&file.path), &file.contents).unwrap();
    }
    std::fs::write(dir.join("main.go"), GO_EXACT_MAIN).unwrap();
    let run = |args: &[&str]| {
        Command::new("go")
            .args(args)
            .current_dir(&dir)
            .env("GOTOOLCHAIN", "local")
            .env("GOWORK", "off")
            .output()
            .expect("go runs")
    };
    let output = run(&["run", "."]);
    let vet = run(&["vet", "./..."]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("exact numbers behave"),
        "generated Go failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}\n--- {}\n{}",
        generation.files[0].path.display(),
        generation.files[0].contents
    );
    assert!(
        vet.status.success(),
        "go vet\n{}",
        String::from_utf8_lossy(&vet.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
