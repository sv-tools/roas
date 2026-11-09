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
            "Alias": {"$ref": "#/components/schemas/Cat"}
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
