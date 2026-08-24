//! A description older than v3.2 is upconverted and then validated by
//! the same interpreter, the way `roas-arazzo-executor` runs a v1.0
//! workflow by upconverting it to v1.1 first.

#![cfg(any(feature = "v3_1", feature = "v3_0", feature = "v2"))]

use roas_http_validator::{Options, RequestView, Validator};
use serde_json::json;

/// Every version must reach the same two verdicts on `/pets/{petId}`:
/// `7` is an integer, `rex` is not.
fn assert_reads_a_path_parameter(validator: &Validator) {
    let report = validator
        .validate(&RequestView::new("GET", "/pets/7"))
        .expect("the description describes GET /pets/{petId}");
    assert!(report.is_valid(), "{report}");
    assert_eq!(report.operation_id.as_deref(), Some("getPet"));

    let report = validator
        .validate(&RequestView::new("GET", "/pets/rex"))
        .expect("the description describes GET /pets/{petId}");
    assert_eq!(report.errors.len(), 1, "{report}");
}

#[cfg(feature = "v3_1")]
#[test]
fn a_v3_1_description_is_accepted() {
    let spec = serde_json::from_value(json!({
        "openapi": "3.1.0",
        "info": { "title": "Pets", "version": "1.0.0" },
        "paths": { "/pets/{petId}": { "get": {
            "operationId": "getPet",
            "parameters": [
                { "name": "petId", "in": "path", "required": true,
                  "schema": { "type": "integer" } }
            ]
        } } }
    }))
    .expect("the v3.1 description must parse");
    assert_reads_a_path_parameter(&Validator::from_v3_1(spec, Options::new()));
}

#[cfg(feature = "v3_0")]
#[test]
fn a_v3_0_description_is_accepted() {
    let spec = serde_json::from_value(json!({
        "openapi": "3.0.3",
        "info": { "title": "Pets", "version": "1.0.0" },
        "paths": { "/pets/{petId}": { "get": {
            "operationId": "getPet",
            "parameters": [
                { "name": "petId", "in": "path", "required": true,
                  "schema": { "type": "integer" } }
            ],
            "responses": { "200": { "description": "ok" } }
        } } }
    }))
    .expect("the v3.0 description must parse");
    assert_reads_a_path_parameter(&Validator::from_v3_0(spec, Options::new()));
}

#[cfg(feature = "v2")]
#[test]
fn a_v2_swagger_description_is_accepted() {
    // v2 spells a parameter's type inline rather than in a `schema`;
    // the upconversion is what turns it into a Schema Object.
    let spec = serde_json::from_value(json!({
        "swagger": "2.0",
        "info": { "title": "Pets", "version": "1.0.0" },
        "paths": { "/pets/{petId}": { "get": {
            "operationId": "getPet",
            "parameters": [
                { "name": "petId", "in": "path", "required": true, "type": "integer" }
            ],
            "responses": { "200": { "description": "ok" } }
        } } }
    }))
    .expect("the v2 description must parse");
    assert_reads_a_path_parameter(&Validator::from_v2(spec, Options::new()));
}

#[cfg(feature = "v2")]
#[test]
fn a_v2_base_path_is_carried_into_the_server_url() {
    let spec = serde_json::from_value(json!({
        "swagger": "2.0",
        "info": { "title": "Pets", "version": "1.0.0" },
        "host": "api.example.com",
        "basePath": "/v1",
        "schemes": ["https"],
        "paths": { "/pets": { "get": {
            "operationId": "listPets",
            "responses": { "200": { "description": "ok" } }
        } } }
    }))
    .expect("the v2 description must parse");
    let validator = Validator::from_v2(spec, Options::new());
    let report = validator
        .validate(&RequestView::new("GET", "/v1/pets"))
        .expect("the base path from `basePath` is stripped");
    assert_eq!(report.operation_id.as_deref(), Some("listPets"));
}
