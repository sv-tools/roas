//! A `discriminator` on a plain object schema survives every upgrade.
//!
//! Needs every model at once; one-version behaviour is tested beside
//! each model.
#![cfg(all(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2"))]

use roas::validation::{Options, Validate};
use roas::{v2, v3_0, v3_1, v3_2};
use serde_json::{Value, json};

/// Upgrades a 2.0 document, validating each result, and returns the
/// serialized 3.0, 3.1 and 3.2 forms.
fn upgrade(raw: Value) -> [Value; 3] {
    let spec: v2::spec::Spec = serde_json::from_value(raw).unwrap();
    let v30: v3_0::spec::Spec = spec.into();
    from_v30(v30)
}

fn from_v30(v30: v3_0::spec::Spec) -> [Value; 3] {
    v30.validate(Options::new(), None).unwrap();
    let as_30 = serde_json::to_value(&v30).unwrap();
    let v31: v3_1::spec::Spec = v30.into();
    v31.validate(Options::new(), None).unwrap();
    let as_31 = serde_json::to_value(&v31).unwrap();
    let v32: v3_2::spec::Spec = v31.into();
    v32.validate(Options::new(), None).unwrap();
    let as_32 = serde_json::to_value(&v32).unwrap();
    [as_30, as_31, as_32]
}

fn v2_doc(definitions: Value, paths: Value) -> Value {
    json!({
        "swagger": "2.0",
        "info": {"title": "t", "version": "1"},
        "paths": paths,
        "definitions": definitions,
    })
}

fn pet_paths() -> Value {
    json!({"/pets": {"get": {"responses": {"200": {
        "description": "ok",
        "schema": {"$ref": "#/definitions/Pet"}
    }}}}})
}

fn pet(extra: Value) -> Value {
    let mut pet = json!({
        "type": "object",
        "discriminator": "petType",
        "properties": {"petType": {"type": "string"}},
        "required": ["petType"]
    });
    pet.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    pet
}

#[test]
fn v2_base_discriminator_is_kept_through_every_upgrade() {
    let raw = v2_doc(
        json!({
            "Pet": pet(json!({})),
            "Cat": {"allOf": [
                {"$ref": "#/definitions/Pet"},
                {"type": "object", "properties": {"huntingSkill": {"type": "string"}}}
            ]}
        }),
        json!({"/pets": {"get": {"responses": {"200": {
            "description": "ok",
            "schema": {"type": "array", "items": {"$ref": "#/definitions/Cat"}}
        }}}}}),
    );
    for doc in upgrade(raw) {
        assert_eq!(
            doc["components"]["schemas"]["Pet"]["discriminator"],
            json!({"propertyName": "petType"}),
            "{doc}"
        );
    }
}

/// A string left behind anywhere a schema can sit would no longer be
/// dropped: it would fail the typed parse the conversion expects to
/// succeed. Every position in a 2.0 document is converted.
#[test]
fn v2_discriminator_is_rewritten_in_every_schema_position() {
    let inner = pet(json!({}));
    let raw = v2_doc(
        json!({
            "Holder": {
                "type": "object",
                "properties": {"pet": inner},
                "additionalProperties": inner
            },
            "Pets": {"type": "array", "items": inner},
            "Composed": {"allOf": [inner]}
        }),
        json!({"/pets": {"post": {
            "consumes": ["application/json"],
            "parameters": [{"name": "b", "in": "body", "schema": inner}],
            "responses": {"200": {"description": "ok", "schema": inner}}
        }}}),
    );
    let expected = json!({"propertyName": "petType"});
    let spec: v2::spec::Spec = serde_json::from_value(raw).unwrap();
    let v30: v3_0::spec::Spec = spec.into();
    let v31: v3_1::spec::Spec = v30.clone().into();
    let v32: v3_2::spec::Spec = v31.clone().into();
    for doc in [
        serde_json::to_value(&v30).unwrap(),
        serde_json::to_value(&v31).unwrap(),
        serde_json::to_value(&v32).unwrap(),
    ] {
        let schemas = &doc["components"]["schemas"];
        assert_eq!(
            schemas["Holder"]["properties"]["pet"]["discriminator"],
            expected
        );
        assert_eq!(
            schemas["Holder"]["additionalProperties"]["discriminator"],
            expected
        );
        assert_eq!(schemas["Pets"]["items"]["discriminator"], expected);
        assert_eq!(schemas["Composed"]["allOf"][0]["discriminator"], expected);
        let post = &doc["paths"]["/pets"]["post"];
        let body = &post["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(body["discriminator"], expected, "{doc}");
    }
}

/// `x-nullable` turns the parent into `type: [object, null]` from 3.1
/// on, which is a different model type.
#[test]
fn nullable_parent_keeps_a_typed_discriminator() {
    let raw = v2_doc(
        json!({"Pet": pet(json!({"x-nullable": true}))}),
        pet_paths(),
    );
    let spec: v2::spec::Spec = serde_json::from_value(raw).unwrap();
    let v30: v3_0::spec::Spec = spec.into();
    let v31: v3_1::spec::Spec = v30.into();
    let schemas = v31.components.as_ref().unwrap().schemas.as_ref().unwrap();
    let roas::common::reference::RefOr::Item(v3_1::schema::Schema::Multi(multi)) = &schemas["Pet"]
    else {
        panic!("expected a multi-type schema: {:?}", schemas["Pet"]);
    };
    assert_eq!(
        multi.discriminator.as_ref().unwrap().property_name,
        "petType"
    );

    let v32: v3_2::spec::Spec = v31.into();
    let schemas = v32.components.as_ref().unwrap().schemas.as_ref().unwrap();
    let roas::common::reference::RefOr::Item(v3_2::schema::Schema::Multi(multi)) = &schemas["Pet"]
    else {
        panic!("expected a multi-type schema: {:?}", schemas["Pet"]);
    };
    assert_eq!(
        multi.discriminator.as_ref().unwrap().property_name,
        "petType"
    );
}

#[test]
fn v3_0_mapping_and_extensions_are_kept_through_every_upgrade() {
    let discriminator = json!({
        "propertyName": "petType",
        "mapping": {"cat": "Cat", "dog": "#/components/schemas/Dog"},
        "x-note": {"kept": true}
    });
    let branch = json!({"allOf": [{"$ref": "#/components/schemas/Pet"}]});
    let spec: v3_0::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.0.4",
        "info": {"title": "t", "version": "1"},
        "paths": {"/pets": {"get": {"responses": {"200": {
            "description": "ok",
            "content": {"application/json": {
                "schema": {"$ref": "#/components/schemas/Pet"}
            }}
        }}}}},
        "components": {"schemas": {
            "Pet": {
                "type": "object",
                "discriminator": discriminator,
                "properties": {"petType": {"type": "string"}},
                "required": ["petType"]
            },
            "Cat": branch,
            "Dog": branch
        }}
    }))
    .unwrap();
    for doc in from_v30(spec) {
        assert_eq!(
            doc["components"]["schemas"]["Pet"]["discriminator"], discriminator,
            "{doc}"
        );
    }
}
