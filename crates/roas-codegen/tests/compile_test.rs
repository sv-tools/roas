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

#![recursion_limit = "512"]

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
        "types": {"NIfOff": {"validation": false}, "ShortOff": {"validation": false}, "NIfAlias": {"attrs": ["#[allow(dead_code)]"]}},
        "substitutions": {
            "date": {"rust": {"name": "String"}, "go": {"name": "string"}},
            "Money": {"rust": {"name": "String"}, "go": {"name": "string"}},
            "AliasSub": {"rust": {"name": "String"}, "go": {"name": "string"}}
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
    assert_eq!(report::<Pet>(r#"{"name":"a","code":"b"}"#), vec![u("/code")]);
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
    // Member names are not checked yet, and say so.
    assert_eq!(report::<Named>(r#"{"abcd":1}"#), vec![u("")]);
    // Bounds beyond every value, and the one multiple of 2^63 an i64 holds.
    assert_eq!(report::<Edge>(r#"{"big":"x","wide":1.5,"many":[1],"m":-9223372036854775808}"#), vec![]);
    assert_eq!(report::<Edge>(r#"{"m":0}"#), vec![]);
    assert_eq!(report::<Edge>(r#"{"m":5}"#), vec![v("/m")]);
    assert_eq!(report::<Closed>(r#"{"a":"x"}"#), vec![u("")]);
    // A type list's constraints apply to the member they fit.
    assert_eq!(report::<TypeList>(r#""ab""#), vec![]);
    assert_eq!(report::<TypeList>("7"), vec![]);
    assert_eq!(report::<TypeList>(r#""abcdef""#), vec![v("")]);
    assert_eq!(report::<TypeList>("1"), vec![v("")]);
    // A nullable inline branch: checked behind its null.
    assert_eq!(report::<MaybeShort>(r#""ab""#), vec![]);
    assert_eq!(report::<MaybeShort>(r#""abc""#), vec![v("")]);
    assert_eq!(report::<MaybeShort>("null"), vec![]);
    // Decoding proves no match where an enum goes unchecked.
    assert_eq!(report::<Picks>("1"), vec![u("")]);
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
    assert_eq!(report::<PicksRef>("1"), vec![u("")]);
    // A deprecated field is still checked, without a warning.
    assert_eq!(report::<Old>(r#"{"name":""}"#), vec![v("/name")]);
    // An intersection allows null only when every side does.
    assert_eq!(report::<Both>(r#""a""#), vec![]);
    assert!(report::<Both>("null").contains(&v("")));
    // `$ref` siblings nothing checks prove no match.
    assert_eq!(report::<Sib>(r#""a""#), vec![u("")]);
    assert_eq!(report::<SibC>(r#""a""#), vec![u("")]);
    // A numeric enum or a const is never taken as satisfied.
    assert_eq!(report::<Ni>("3"), vec![u("")]);
    assert_eq!(report::<Pins>(r#"{"n":3}"#), vec![u("/n")]);
    assert_eq!(report::<Pins>(r#"{"k":false}"#), vec![u("/k")]);
    // A type list's enum and default are split among its members.
    assert_eq!(report::<EnumO>(r#""ab""#), vec![]);
    assert_eq!(report::<EnumO>("5"), vec![u("")]);
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
    // Constraints on inline items prove no branch.
    assert_eq!(report::<Items>("[1]"), vec![u("")]);
    assert_eq!(report::<StrItems>(r#"["a"]"#), vec![u("")]);
    // Member patterns are not checked yet, and say so.
    assert_eq!(report::<PatMap>(r#"{"a":"x"}"#), vec![u("")]);
    // A type list left with only its null.
    assert_eq!(report::<NullOnly>("null"), vec![]);
    // A present null reaches a JSON-value wrapper.
    assert_eq!(report::<HoldsScalar>(r#"{"n":null}"#), vec![u("/n")]);
    // What a branch's members leave unchecked proves no match.
    assert_eq!(report::<PickK>(r#"{"k":[1]}"#), vec![u("")]);
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
		{`{"name":"a","code":"b"}`, []found{u("/code")}},
		{`{"name":"a"}`, []found{v("")}},
		{`{"name":"a","age":2,"weight":1,"ratio":1.5,"tags":["x"]}`, []found{v(""), v("/ratio")}},
		{`{"name":"a","extra":{"x":1,"y":2}}`, []found{v("/extra")}},
		{`{"name":"a","owner":{"nick":"long"}}`, []found{v("/owner/nick")}},
		{`{"name":"a","owner":{"nick":null}}`, nil},
		{`{"name":"a","friends":[{"nick":"ok"},{"nick":"long"}]}`, []found{v("/friends/1/nick")}},
	} {
		check(c.js, report[P](c.js), c.want...)
	}
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
	check(`{"abcd":1}`, report[api.Named](`{"abcd":1}`), u(""))
	check(`edge`, report[api.Edge](`{"big":"x","wide":1.5,"many":[1],"m":-9223372036854775808}`))
	check(`{"m":0}`, report[api.Edge](`{"m":0}`))
	check(`{"m":5}`, report[api.Edge](`{"m":5}`), v("/m"))
	check(`{"a":"x"}`, report[api.Closed](`{"a":"x"}`), u(""))
	check(`"ab"`, report[api.TypeList](`"ab"`))
	check(`7`, report[api.TypeList](`7`))
	check(`"abcdef"`, report[api.TypeList](`"abcdef"`), v(""))
	check(`1`, report[api.TypeList](`1`), v(""))
	check(`"ab"`, report[api.MaybeShort](`"ab"`))
	check(`"abc"`, report[api.MaybeShort](`"abc"`), v(""))
	check(`null`, report[api.MaybeShort](`null`))
	check(`1`, report[api.Picks](`1`), u(""))
	check(`"a"`, report[api.Itself](`"a"`))
	check(`{"c":null}`, report[api.Holds](`{"c":null}`), v("/c"))
	check(`null`, report[api.OrNull](`null`))
	check(`"ab"`, report[api.OrNull](`"ab"`), v(""))
	check(`{"q":"a","z":"b"}`, report[api.Extended](`{"q":"a","z":"b"}`))
	check(`{"q":"a","z":"bc"}`, report[api.Extended](`{"q":"a","z":"bc"}`), v("/z"))
	check(`1`, report[api.PicksRef](`1`), u(""))
	check(`{"name":""}`, report[api.Old](`{"name":""}`), v("/name"))
	check(`"a"`, report[api.Both](`"a"`))
	if got := report[api.Both](`null`); len(got) == 0 || got[0] != v("") {
		panic(fmt.Sprintf("null intersection: got %v", got))
	}
	check(`"a"`, report[api.Sib](`"a"`), u(""))
	check(`"a"`, report[api.SibC](`"a"`), u(""))
	check(`3`, report[api.Ni](`3`), u(""))
	check(`{"n":3}`, report[api.Pins](`{"n":3}`), u("/n"))
	check(`{"k":false}`, report[api.Pins](`{"k":false}`), u("/k"))
	check(`"ab"`, report[api.EnumO](`"ab"`))
	check(`5`, report[api.EnumO](`5`), u(""))
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
	check(`[1]`, report[api.Items](`[1]`), u(""))
	check(`["a"]`, report[api.StrItems](`["a"]`), u(""))
	check(`{"a":"x"}`, report[api.PatMap](`{"a":"x"}`), u(""))
	check(`null`, report[api.NullOnly](`null`))
	check(`{"n":null}`, report[api.HoldsScalar](`{"n":null}`), u("/n"))
	check(`{"k":[1]}`, report[api.PickK](`{"k":[1]}`), u(""))
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
