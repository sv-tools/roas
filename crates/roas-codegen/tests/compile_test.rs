//! The one test that catches a whole class of emission bug: the
//! generated code must compile, and then it must behave. A temporary
//! crate is built around the generated module and run with assertions
//! for every nullability case, the integer wire type, the tagged union
//! and the open holders.
//!
//! Set `ROAS_CODEGEN_SKIP_COMPILE=1` to skip it where no toolchain or
//! registry is available.

use roas::validation::{IGNORE_UNUSED, Options};
use roas_codegen::{ConfigFile, Input, SourceDocument, Target, generate, validate};
use std::path::Path;
use std::process::Command;
use url::Url;

const MAIN: &str = r##"
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
    let composed: Composed = serde_json::from_str(r#"{"petType":"Pet","n":1}"#).unwrap();
    assert!(composed.as_cat().is_some());
    assert_eq!(composed, composed.clone());
    // cycles through Box.
    let node: Node = serde_json::from_str(r#"{"next":{"done":null}}"#).unwrap();
    let inner = node.next.unwrap();
    assert!(inner.next.is_none());
    assert_eq!(inner.done, Some(None));
    must_fail::<Node>(r#"{"next":null}"#, "next is optional but not nullable");
    // a newtype over a list, and a type alias.
    let ids: Ids = serde_json::from_str(r#"["a","b"]"#).unwrap();
    assert_eq!(ids.0.len(), 2);
    let _: Alias = serde_json::from_str(r#"{"petType":"Pet"}"#).unwrap();
    println!("generated code behaves");
}
"##;

#[test]
fn generated_rust_compiles_and_behaves() {
    if std::env::var_os("ROAS_CODEGEN_SKIP_COMPILE").is_some() {
        eprintln!("skipped: ROAS_CODEGEN_SKIP_COMPILE is set");
        return;
    }
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
    let bytes = serde_json::to_vec(&raw).unwrap();
    let doc =
        SourceDocument::parse(Input::Json(bytes), Url::parse("file:///t.json").unwrap()).unwrap();
    let doc = validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).unwrap();
    let config = ConfigFile {
        target: Some(Target::Rust),
        ..Default::default()
    }
    .build()
    .unwrap();
    let generation = generate(&doc, &config).expect("generates");
    assert!(!generation.has_errors(), "{:?}", generation.diagnostics);

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
