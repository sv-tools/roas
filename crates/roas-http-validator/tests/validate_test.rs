//! End-to-end: a description in, a request in, a verdict out.

use roas_http_validator::{ErrorKind, Location, Options, RequestView, RoutingError, Validator};
use serde_json::json;

/// The shared fixture — a templated path, a concrete path beside it,
/// parameters in four locations, and a JSON body.
fn petstore() -> Validator {
    let spec = serde_yaml_ng::from_str(include_str!("data/petstore.openapi.yaml"))
        .expect("the description must parse");
    Validator::new(spec)
}

/// A validator over a description written inline, for the cases that
/// need one keyword and nothing else.
fn validator(paths: serde_json::Value) -> Validator {
    with_options(paths, Options::new())
}

fn with_options(paths: serde_json::Value, options: Options) -> Validator {
    let spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": paths,
    }))
    .expect("the description must parse");
    Validator::with_options(spec, options)
}

/// Every error the request produced, as text.
fn errors(validator: &Validator, request: &RequestView<'_>) -> Vec<String> {
    validator
        .validate(request)
        .expect("the description must describe this request")
        .errors
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// A `GET /pets` that satisfies the fixture, for tests that change one
/// thing about it.
fn listing() -> RequestView<'static> {
    RequestView::new("GET", "/pets").with_header("X-Request-Id", "abc-123")
}

// ── routing ──────────────────────────────────────────────────────────

#[test]
fn a_path_the_description_does_not_name_is_not_a_validation_failure() {
    let error = petstore()
        .validate(&RequestView::new("GET", "/unicorns"))
        .expect_err("the description names no such path");
    assert_eq!(
        error,
        RoutingError::PathNotFound {
            path: "/unicorns".to_owned(),
        },
    );
}

#[test]
fn a_method_the_path_does_not_offer_names_the_ones_it_does() {
    let error = petstore()
        .validate(&RequestView::new("DELETE", "/pets"))
        .expect_err("the path offers no DELETE");
    assert_eq!(
        error,
        RoutingError::MethodNotAllowed {
            template: "/pets".to_owned(),
            method: "DELETE".to_owned(),
            allowed: vec!["GET".to_owned(), "POST".to_owned()],
        },
    );
}

#[test]
fn a_concrete_path_outranks_the_template_beside_it() {
    let report = petstore()
        .validate(&RequestView::new("GET", "/pets/mine"))
        .expect("the description describes GET /pets/mine");
    assert_eq!(report.operation_id.as_deref(), Some("listMyPets"));
    assert_eq!(report.template, "/pets/mine");
}

#[test]
fn a_request_carrying_the_servers_base_path_still_matches() {
    let report = petstore()
        .validate(&listing().with_query("").with_body(b"".as_slice()))
        .expect("the description describes GET /pets");
    assert!(report.is_valid(), "{report}");

    let prefixed = RequestView::new("GET", "/v1/pets").with_header("X-Request-Id", "abc-123");
    let report = petstore()
        .validate(&prefixed)
        .expect("the base path from `servers` is stripped");
    assert!(report.is_valid(), "{report}");
    assert_eq!(report.template, "/pets");
}

#[test]
fn a_report_names_the_operation_and_the_path_parameters_it_read() {
    let report = petstore()
        .validate(&RequestView::new("GET", "/pets/7"))
        .expect("the description describes GET /pets/{petId}");
    assert!(report.is_valid(), "{report}");
    assert_eq!(report.template, "/pets/{petId}");
    assert_eq!(report.method, "GET");
    assert_eq!(report.operation_id.as_deref(), Some("getPet"));
    assert_eq!(
        report.path_parameters,
        [("petId".to_owned(), "7".to_owned())]
    );
}

// ── parameters ───────────────────────────────────────────────────────

#[test]
fn a_path_parameter_is_coerced_before_the_schema_judges_it() {
    let validator = petstore();
    assert!(errors(&validator, &RequestView::new("GET", "/pets/7")).is_empty());
    assert_eq!(
        errors(&validator, &RequestView::new("GET", "/pets/rex")),
        ["path parameter \"petId\": cannot be read: \"rex\" is not an integer"],
    );
    assert_eq!(
        errors(&validator, &RequestView::new("GET", "/pets/0")),
        ["path parameter \"petId\": 0 is below minimum 1"],
    );
}

#[test]
fn a_parameter_declared_on_the_path_item_applies_to_every_operation() {
    // `X-Request-Id` is declared once on `/pets`, not on either
    // operation, and both must require it.
    let validator = petstore();
    assert_eq!(
        errors(&validator, &RequestView::new("GET", "/pets")),
        ["header parameter \"X-Request-Id\": is required and was not sent"],
    );
    assert!(errors(&validator, &listing()).is_empty());
}

#[test]
fn an_operation_parameter_overrides_the_path_items_of_the_same_name() {
    let validator = validator(json!({
        "/x": {
            "parameters": [
                { "name": "n", "in": "query", "required": true, "schema": { "type": "string" } }
            ],
            "get": {
                "parameters": [
                    { "name": "n", "in": "query", "schema": { "type": "integer" } }
                ]
            }
        }
    }));
    // Not required any more, and now an integer.
    assert!(errors(&validator, &RequestView::new("GET", "/x")).is_empty());
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=abc")
        ),
        ["query parameter \"n\": cannot be read: \"abc\" is not an integer"],
    );
}

#[test]
fn a_query_parameter_is_coerced_to_the_type_its_schema_declares() {
    let validator = petstore();
    assert!(errors(&validator, &listing().with_query("limit=10")).is_empty());
    assert_eq!(
        errors(&validator, &listing().with_query("limit=1000")),
        ["query parameter \"limit\": 1000 is above maximum 100"],
    );
    assert_eq!(
        errors(&validator, &listing().with_query("limit=many")),
        ["query parameter \"limit\": cannot be read: \"many\" is not an integer"],
    );
}

#[test]
fn an_exploded_array_is_one_query_pair_per_member() {
    let validator = petstore();
    let request = listing().with_query("tags=cute&tags=small");
    assert!(errors(&validator, &request).is_empty());
}

#[test]
fn a_cookie_is_read_from_the_cookie_header() {
    let validator = petstore();
    assert!(errors(&validator, &listing().with_header("cookie", "session=abcd")).is_empty());
    assert_eq!(
        errors(&validator, &listing().with_header("cookie", "session=ab")),
        ["cookie parameter \"session\": is shorter than minLength 3 (2 characters)"],
    );
}

#[test]
fn a_repeated_header_reads_as_the_list_it_stands_for() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "x-tag", "in": "header",
              "schema": { "type": "array", "items": { "type": "string" } } }
        ] } }
    }));
    let request = RequestView::new("GET", "/x")
        .with_header("x-tag", "a")
        .with_header("x-tag", "b");
    assert!(errors(&validator, &request).is_empty());
}

// ── styles ───────────────────────────────────────────────────────────

/// One query parameter, one style, one array schema.
fn styled(style: &str, explode: bool) -> Validator {
    validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "id", "in": "query", "style": style, "explode": explode,
              "schema": { "type": "array", "items": { "type": "integer" } } }
        ] } }
    }))
}

#[test]
fn a_form_array_that_does_not_explode_is_comma_separated() {
    let validator = styled("form", false);
    assert!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("id=3,4,5")
        )
        .is_empty()
    );
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("id=3,x")
        ),
        ["query parameter \"id\": cannot be read: \"x\" is not an integer"],
    );
}

#[test]
fn space_and_pipe_delimited_arrays_split_on_their_own_character() {
    let space = styled("spaceDelimited", false);
    assert!(
        errors(
            &space,
            &RequestView::new("GET", "/x").with_query("id=3%204%205")
        )
        .is_empty()
    );

    let pipe = styled("pipeDelimited", false);
    assert!(errors(&pipe, &RequestView::new("GET", "/x").with_query("id=3|4|5")).is_empty());
}

#[test]
fn a_deep_object_is_rebuilt_from_bracketed_names() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "id", "in": "query", "style": "deepObject", "explode": true,
              "schema": { "type": "object", "properties": {
                  "role": { "type": "string" }, "age": { "type": "integer" }
              } } }
        ] } }
    }));
    let request = RequestView::new("GET", "/x").with_query("id[role]=admin&id[age]=42");
    assert!(errors(&validator, &request).is_empty());

    let bad = RequestView::new("GET", "/x").with_query("id[role]=admin&id[age]=old");
    assert_eq!(
        errors(&validator, &bad),
        ["query parameter \"id\": cannot be read: \"old\" is not an integer"],
    );
}

#[test]
fn an_exploded_form_object_spreads_over_top_level_pairs() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "id", "in": "query", "style": "form", "explode": true,
              "schema": { "type": "object", "properties": {
                  "role": { "type": "string" }, "age": { "type": "integer" }
              } } }
        ] } }
    }));
    let request = RequestView::new("GET", "/x").with_query("role=admin&age=42");
    assert!(errors(&validator, &request).is_empty());
}

#[test]
fn a_form_object_that_does_not_explode_is_a_flat_pair_list() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "id", "in": "query", "style": "form", "explode": false,
              "schema": { "type": "object", "properties": { "role": { "type": "string" } } } }
        ] } }
    }));
    let request = RequestView::new("GET", "/x").with_query("id=role,admin");
    assert!(errors(&validator, &request).is_empty());
}

#[test]
fn label_and_matrix_paths_carry_their_decoration() {
    for (style, path, exploded) in [
        ("label", "/x/.3.4", false),
        ("matrix", "/x/;id=3,4", false),
        ("matrix", "/x/;id=3;id=4", true),
    ] {
        let validator = validator(json!({
            "/x/{id}": { "get": { "parameters": [
                { "name": "id", "in": "path", "required": true,
                  "style": style, "explode": exploded,
                  "schema": { "type": "array", "items": { "type": "integer" } } }
            ] } }
        }));
        assert!(
            errors(&validator, &RequestView::new("GET", path)).is_empty(),
            "{style} did not read {path}",
        );
    }
}

#[test]
fn a_simple_path_array_is_comma_separated() {
    let validator = validator(json!({
        "/x/{id}": { "get": { "parameters": [
            { "name": "id", "in": "path", "required": true,
              "schema": { "type": "array", "items": { "type": "integer" } } }
        ] } }
    }));
    assert!(errors(&validator, &RequestView::new("GET", "/x/3,4,5")).is_empty());
}

// ── bodies ───────────────────────────────────────────────────────────

fn posting(body: &'static [u8]) -> RequestView<'static> {
    RequestView::new("POST", "/pets")
        .with_header("X-Request-Id", "abc-123")
        .with_header("content-type", "application/json")
        .with_body(body)
}

#[test]
fn a_json_body_is_judged_against_the_media_types_schema() {
    let validator = petstore();
    assert!(errors(&validator, &posting(br#"{"name":"Rex"}"#)).is_empty());
    assert_eq!(
        errors(&validator, &posting(br#"{"tag":"cute"}"#)),
        ["body at /name: is required and was not sent"],
    );
    assert_eq!(
        errors(&validator, &posting(br#"{"name":7}"#)),
        ["body at /name: expected string, got integer"],
    );
}

#[test]
fn a_body_that_is_not_json_says_so_rather_than_failing_the_schema() {
    let found = errors(&petstore(), &posting(b"{not json"));
    assert_eq!(found.len(), 1);
    assert!(
        found[0].starts_with("body: cannot be read: invalid JSON"),
        "{found:?}"
    );
}

#[test]
fn a_required_body_that_did_not_arrive_is_reported() {
    let request = RequestView::new("POST", "/pets").with_header("X-Request-Id", "abc-123");
    assert_eq!(
        errors(&petstore(), &request),
        ["body: is required and was not sent"],
    );
}

#[test]
fn a_media_type_the_operation_does_not_describe_lists_the_ones_it_does() {
    let request = RequestView::new("POST", "/pets")
        .with_header("X-Request-Id", "abc-123")
        .with_header("content-type", "text/plain")
        .with_body(b"Rex".as_slice());
    assert_eq!(
        errors(&petstore(), &request),
        ["body: media type \"text/plain\" is not one of: application/json"],
    );
}

#[test]
fn a_media_type_range_matches_what_falls_under_it() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/*": { "schema": { "type": "object", "required": ["a"] } }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(br#"{"b":1}"#.as_slice());
    assert_eq!(
        errors(&validator, &request),
        ["body at /a: is required and was not sent"]
    );
}

#[test]
fn a_form_body_is_rebuilt_field_by_field() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "required": true, "content": {
            "application/x-www-form-urlencoded": { "schema": {
                "type": "object",
                "required": ["name"],
                "properties": { "name": { "type": "string" }, "age": { "type": "integer" } }
            } }
        } } } }
    }));
    let ok = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"name=Rex&age=4".as_slice());
    assert!(errors(&validator, &ok).is_empty());

    let bad = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"age=four".as_slice());
    assert_eq!(
        errors(&validator, &bad),
        ["body: cannot be read: \"four\" is not an integer"],
    );
}

#[test]
fn a_text_body_is_judged_as_the_string_it_is() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "text/plain": { "schema": { "type": "string", "minLength": 4 } }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "text/plain")
        .with_body(b"Rex".as_slice());
    assert_eq!(
        errors(&validator, &request),
        ["body: is shorter than minLength 4 (3 characters)"],
    );
}

#[test]
fn a_media_type_that_cannot_be_read_is_reported_as_unchecked() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "multipart/form-data": { "schema": { "type": "object" } }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "multipart/form-data")
        .with_body(b"--boundary".as_slice());
    assert_eq!(
        errors(&validator, &request),
        ["body: was NOT checked — a multipart/form-data body is not implemented yet"],
    );
}

#[test]
fn skipping_the_body_leaves_it_unjudged() {
    let spec = serde_yaml_ng::from_str(include_str!("data/petstore.openapi.yaml"))
        .expect("the description must parse");
    let validator = Validator::with_options(spec, Options::new().skip_body());
    let request = RequestView::new("POST", "/pets").with_header("X-Request-Id", "abc-123");
    assert!(errors(&validator, &request).is_empty());
}

// ── options ──────────────────────────────────────────────────────────

#[test]
fn an_undescribed_query_parameter_is_reported_only_when_asked_for() {
    let paths = json!({
        "/x": { "get": { "parameters": [
            { "name": "limit", "in": "query", "schema": { "type": "integer" } }
        ] } }
    });
    let request = RequestView::new("GET", "/x").with_query("limit=1&limti=2");

    assert!(errors(&validator(paths.clone()), &request).is_empty());

    let strict = with_options(paths, Options::new().reject_undescribed_query_parameters());
    assert_eq!(
        errors(&strict, &request),
        ["query parameter \"limti\": is not described by this operation"],
    );
}

#[test]
fn a_base_path_option_overrides_what_the_servers_say() {
    let spec = serde_yaml_ng::from_str(include_str!("data/petstore.openapi.yaml"))
        .expect("the description must parse");
    let validator = Validator::with_options(spec, Options::new().base_path("/api"));
    let request = RequestView::new("GET", "/api/pets/7");
    assert!(
        validator
            .validate(&request)
            .expect("the description describes GET /pets/{petId}")
            .is_valid(),
    );
    assert!(
        validator
            .validate(&RequestView::new("GET", "/v1/pets/7"))
            .is_err()
    );
}

// ── references ───────────────────────────────────────────────────────

#[test]
fn a_referenced_parameter_is_followed_to_what_it_names() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": { "get": { "parameters": [
            { "$ref": "#/components/parameters/Limit" }
        ] } } },
        "components": { "parameters": { "Limit": {
            "name": "limit", "in": "query", "required": true,
            "schema": { "type": "integer" }
        } } }
    }))
    .expect("the description must parse");
    let validator = Validator::new(spec);
    assert_eq!(
        errors(&validator, &RequestView::new("GET", "/x")),
        ["query parameter \"limit\": is required and was not sent"],
    );
}

#[test]
fn a_reference_that_names_nothing_is_reported_rather_than_ignored() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "$ref": "#/components/requestBodies/Gone" } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(b"{}".as_slice());
    let report = validator
        .validate(&request)
        .expect("the description describes POST /x");
    assert_eq!(report.errors.len(), 1);
    assert!(
        matches!(report.errors[0].kind, ErrorKind::UnresolvedReference(_)),
        "{report}",
    );
    assert_eq!(report.errors[0].location, Location::Body);
}

#[test]
fn a_path_item_that_is_a_reference_is_followed() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": { "$ref": "#/components/pathItems/Shared" } },
        "components": { "pathItems": { "Shared": { "get": { "operationId": "shared" } } } }
    }))
    .expect("the description must parse");
    let report = Validator::new(spec)
        .validate(&RequestView::new("GET", "/x"))
        .expect("the referenced path item describes GET /x");
    assert_eq!(report.operation_id.as_deref(), Some("shared"));
}

// ── OpenAPI 3.2's `in: querystring` ──────────────────────────────────

#[test]
fn a_querystring_parameter_judges_the_whole_query_at_once() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "params", "in": "querystring", "required": true, "content": {
                "application/x-www-form-urlencoded": { "schema": {
                    "type": "object",
                    "required": ["a"],
                    "properties": { "a": { "type": "integer" } }
                } }
            } }
        ] } }
    }));
    assert!(errors(&validator, &RequestView::new("GET", "/x").with_query("a=1")).is_empty());
    assert_eq!(
        errors(&validator, &RequestView::new("GET", "/x").with_query("b=1")),
        ["querystring parameter \"params\" at /a: is required and was not sent"],
    );
}

// ── a description with nothing in it ─────────────────────────────────

#[test]
fn a_description_without_paths_matches_nothing() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" }
    }))
    .expect("the description must parse");
    let validator = Validator::new(spec);
    assert!(validator.spec().paths.is_none());
    assert!(matches!(
        validator.validate(&RequestView::new("GET", "/x")),
        Err(RoutingError::PathNotFound { .. }),
    ));
}

#[test]
fn a_parameter_reference_that_names_nothing_is_the_descriptions_fault() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [{ "$ref": "#/components/parameters/Gone" }] } }
    }));
    let report = validator
        .validate(&RequestView::new("GET", "/x"))
        .expect("the description describes GET /x");
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].location, Location::Description);
    assert!(
        report.errors[0]
            .to_string()
            .starts_with("description: has an unresolvable `$ref`"),
        "{report}",
    );
}

#[test]
fn a_percent_encoded_delimiter_is_data_rather_than_a_separator() {
    // `a%2Cb` is one item that contains a comma. Decoding before the
    // split would turn it into two.
    let validator = styled("form", false);
    let strings = validator_with_string_items();
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("id=3%2C4")
        ),
        ["query parameter \"id\": cannot be read: \"3,4\" is not an integer"],
    );
    let report = strings
        .validate(&RequestView::new("GET", "/x").with_query("id=a%2Cb"))
        .expect("the description describes GET /x");
    assert!(report.is_valid(), "{report}");
}

/// The same parameter with string items, so a comma inside one is legal.
fn validator_with_string_items() -> Validator {
    validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "id", "in": "query", "style": "form", "explode": false,
              "schema": { "type": "array", "items": { "type": "string" },
                          "minItems": 1, "maxItems": 1 } }
        ] } }
    }))
}

#[test]
fn a_percent_encoded_path_delimiter_is_data_too() {
    let validator = validator(json!({
        "/x/{id}": { "get": { "parameters": [
            { "name": "id", "in": "path", "required": true,
              "schema": { "type": "array", "items": { "type": "string" },
                          "minItems": 1, "maxItems": 1 } }
        ] } }
    }));
    let report = validator
        .validate(&RequestView::new("GET", "/x/a%2Cb"))
        .expect("the description describes GET /x/{id}");
    assert!(report.is_valid(), "{report}");
    assert_eq!(
        report.path_parameters,
        [("id".to_owned(), "a,b".to_owned())]
    );
}

// ── OpenAPI 3.2's `additionalOperations` ─────────────────────────────

fn with_copy() -> Validator {
    validator(json!({
        "/pets": {
            "get": { "operationId": "listPets" },
            "additionalOperations": {
                "COPY": {
                    "operationId": "copyPet",
                    "parameters": [
                        { "name": "destination", "in": "query", "required": true,
                          "schema": { "type": "string" } }
                    ]
                }
            }
        }
    }))
}

#[test]
fn a_non_standard_method_is_found_in_additional_operations() {
    let report = with_copy()
        .validate(&RequestView::new("COPY", "/pets").with_query("destination=/pets/8"))
        .expect("`additionalOperations` describes COPY /pets");
    assert!(report.is_valid(), "{report}");
    assert_eq!(report.operation_id.as_deref(), Some("copyPet"));
    // Reported as the description spells it, not lowercased.
    assert_eq!(report.method, "COPY");
}

#[test]
fn an_additional_operations_parameter_is_validated_like_any_other() {
    assert_eq!(
        errors(&with_copy(), &RequestView::new("COPY", "/pets")),
        ["query parameter \"destination\": is required and was not sent"],
    );
}

#[test]
fn a_non_standard_method_is_matched_case_sensitively() {
    // RFC 9110 makes method names case-sensitive, and the key here is
    // `COPY`.
    assert!(matches!(
        with_copy().validate(&RequestView::new("copy", "/pets")),
        Err(RoutingError::MethodNotAllowed { .. }),
    ));
}

#[test]
fn the_allowed_list_names_additional_operations_too() {
    let error = with_copy()
        .validate(&RequestView::new("DELETE", "/pets"))
        .expect_err("the path offers no DELETE");
    assert_eq!(
        error,
        RoutingError::MethodNotAllowed {
            template: "/pets".to_owned(),
            method: "DELETE".to_owned(),
            allowed: vec!["GET".to_owned(), "COPY".to_owned()],
        },
    );
}

// ── servers at every level ───────────────────────────────────────────

#[test]
fn an_operation_server_moves_just_that_operation() {
    let validator = validator(json!({
        "/pets": {
            "get": { "operationId": "listPets" },
            "post": { "operationId": "createPet", "servers": [{ "url": "https://api.example.com/v2" }] }
        }
    }));
    let report = validator
        .validate(&RequestView::new("POST", "/v2/pets"))
        .expect("the operation's own server carries the `/v2` prefix");
    assert_eq!(report.operation_id.as_deref(), Some("createPet"));
}

// ── bodies that arrived empty ────────────────────────────────────────

#[test]
fn an_empty_body_that_was_supplied_is_not_an_absent_body() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "required": true, "content": {
            "application/json": { "schema": { "type": "object" } },
            "text/plain": { "schema": { "type": "string" } }
        } } } }
    }));

    // Nothing supplied at all.
    assert_eq!(
        errors(&validator, &RequestView::new("POST", "/x")),
        ["body: is required and was not sent"],
    );

    // Supplied, empty, and claimed to be JSON — which it is not.
    let empty_json = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(b"".as_slice());
    let found = errors(&validator, &empty_json);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].starts_with("body: cannot be read: invalid JSON"),
        "{found:?}"
    );

    // Supplied, empty, and claimed to be text — which the empty string is.
    let empty_text = RequestView::new("POST", "/x")
        .with_header("content-type", "text/plain")
        .with_body(b"".as_slice());
    assert!(errors(&validator, &empty_text).is_empty());
}

// ── form bodies with an Encoding Object ──────────────────────────────

#[test]
fn a_repeated_form_field_becomes_the_array_it_stands_for() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/x-www-form-urlencoded": { "schema": {
                "type": "object",
                "properties": { "tags": { "type": "array", "items": { "type": "string" } } }
            } }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"tags=a&tags=b".as_slice());
    assert!(errors(&validator, &request).is_empty());
}

#[test]
fn a_form_field_honours_the_encoding_objects_style() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/x-www-form-urlencoded": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "ids": { "type": "array", "items": { "type": "integer" } }
                    }
                },
                "encoding": { "ids": { "style": "pipeDelimited", "explode": false } }
            }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"ids=1|2|3".as_slice());
    assert!(errors(&validator, &request).is_empty());

    let bad = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"ids=1|x".as_slice());
    assert_eq!(
        errors(&validator, &bad),
        ["body: cannot be read: \"x\" is not an integer"],
    );
}

#[test]
fn an_undescribed_form_field_is_still_offered_to_additional_properties() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/x-www-form-urlencoded": { "schema": {
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "additionalProperties": false
            } }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"name=Rex&extra=1".as_slice());
    assert_eq!(
        errors(&validator, &request),
        ["body at /extra: is not described, and additionalProperties is `false`"],
    );
}

// ── stray detection and exploded objects ─────────────────────────────

#[test]
fn an_exploded_object_parameter_is_not_mistaken_for_stray_parameters() {
    let paths = json!({
        "/x": { "get": { "parameters": [
            { "name": "filter", "in": "query", "style": "form", "explode": true,
              "schema": { "type": "object", "properties": {
                  "role": { "type": "string" }, "age": { "type": "integer" }
              } } }
        ] } }
    });
    let strict = with_options(paths, Options::new().reject_undescribed_query_parameters());
    let request = RequestView::new("GET", "/x").with_query("role=admin&age=42");
    assert!(errors(&strict, &request).is_empty());

    // Something the object does not declare is still a stray.
    let stray = RequestView::new("GET", "/x").with_query("role=admin&nope=1");
    assert_eq!(
        errors(&strict, &stray),
        ["query parameter \"nope\": is not described by this operation"],
    );
}

// ── the review of 87a5a41 ────────────────────────────────────────────

#[test]
fn an_operation_server_does_not_move_the_other_methods_on_that_path() {
    let validator = validator(json!({
        "/pets": {
            "get": { "operationId": "listPets", "servers": [{ "url": "/v1" }] },
            "post": { "operationId": "createPet", "servers": [{ "url": "/v2" }] }
        }
    }));
    assert_eq!(
        validator
            .validate(&RequestView::new("GET", "/v1/pets"))
            .expect("GET lives under /v1")
            .operation_id
            .as_deref(),
        Some("listPets"),
    );
    assert_eq!(
        validator
            .validate(&RequestView::new("POST", "/v2/pets"))
            .expect("POST lives under /v2")
            .operation_id
            .as_deref(),
        Some("createPet"),
    );
    // Each prefix belongs to one method only.
    assert!(matches!(
        validator.validate(&RequestView::new("GET", "/v2/pets")),
        Err(RoutingError::PathNotFound { .. }),
    ));
    assert!(matches!(
        validator.validate(&RequestView::new("POST", "/v1/pets")),
        Err(RoutingError::PathNotFound { .. }),
    ));
}

#[test]
fn a_referenced_path_items_servers_are_used_for_routing() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/pets": { "$ref": "#/components/pathItems/Shared" } },
        "components": { "pathItems": { "Shared": {
            "servers": [{ "url": "https://api.example.com/v2" }],
            "get": { "operationId": "shared" }
        } } }
    }))
    .expect("the description must parse");
    let report = Validator::new(spec)
        .validate(&RequestView::new("GET", "/v2/pets"))
        .expect("the referenced path item carries the /v2 prefix");
    assert_eq!(report.operation_id.as_deref(), Some("shared"));
}

#[test]
fn a_method_token_is_case_sensitive() {
    // RFC 9110 §9.1. `get` is a different method from `GET`, and no
    // Path Item Object describes it.
    let validator = validator(json!({ "/pets": { "get": { "operationId": "listPets" } } }));
    assert!(
        validator
            .validate(&RequestView::new("GET", "/pets"))
            .is_ok()
    );
    for spelling in ["get", "GeT", "Get"] {
        assert!(
            matches!(
                validator.validate(&RequestView::new(spelling, "/pets")),
                Err(RoutingError::MethodNotAllowed { .. }),
            ),
            "`{spelling}` must not be taken for GET",
        );
    }
}

#[test]
fn an_exploded_object_field_is_not_duplicated_beside_itself_in_a_form_body() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/x-www-form-urlencoded": {
                "schema": {
                    "type": "object",
                    "properties": { "filter": { "type": "object", "properties": {
                        "role": { "type": "string" }
                    } } },
                    "additionalProperties": false
                },
                "encoding": { "filter": { "style": "form", "explode": true } }
            }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/x-www-form-urlencoded")
        .with_body(b"role=admin".as_slice());
    // `role` is consumed into `filter`; it must not reappear at the root
    // and trip `additionalProperties: false`.
    assert!(errors(&validator, &request).is_empty());
}

#[test]
fn the_name_of_an_exploded_object_parameter_is_itself_a_stray() {
    // With `style: form, explode: true` the parameter's own name is
    // never serialized, so `?filter=garbage` names nothing.
    let paths = json!({
        "/x": { "get": { "parameters": [
            { "name": "filter", "in": "query", "style": "form", "explode": true,
              "schema": { "type": "object", "properties": { "role": { "type": "string" } } } }
        ] } }
    });
    let strict = with_options(paths, Options::new().reject_undescribed_query_parameters());
    assert_eq!(
        errors(
            &strict,
            &RequestView::new("GET", "/x").with_query("filter=garbage")
        ),
        ["query parameter \"filter\": is not described by this operation"],
    );
    assert!(
        errors(
            &strict,
            &RequestView::new("GET", "/x").with_query("role=admin")
        )
        .is_empty()
    );
}

#[test]
fn an_empty_body_with_no_media_type_is_still_a_body_that_was_supplied() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/json": { "schema": { "type": "object" } }
        } } } }
    }));
    // Nothing supplied: nothing to say, the body is not required.
    assert!(errors(&validator, &RequestView::new("POST", "/x")).is_empty());
    // Supplied and empty, with no media type to read it as.
    let supplied = RequestView::new("POST", "/x").with_body(b"".as_slice());
    assert_eq!(
        errors(&validator, &supplied),
        ["body: no media type was sent; expected one of: application/json"],
    );
}

#[test]
fn an_exact_integer_is_never_rounded_to_meet_a_fractional_bound() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "n", "in": "query",
              "schema": { "type": "integer", "maximum": 10.5, "minimum": -10.5 } }
        ] } }
    }));
    assert!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=10")
        )
        .is_empty()
    );
    assert!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=-10")
        )
        .is_empty()
    );
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=11")
        ),
        ["query parameter \"n\": 11 is above maximum 10.5"],
    );
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=-11")
        ),
        ["query parameter \"n\": -11 is below minimum -10.5"],
    );
}

// ── the review of e7bb350 ────────────────────────────────────────────

#[test]
fn a_field_written_beside_a_path_item_ref_is_not_dropped() {
    // The reference carries the operations; the required header is
    // written at the call site. Both apply.
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": {
            "$ref": "#/components/pathItems/Shared",
            "parameters": [
                { "name": "X-Local", "in": "header", "required": true,
                  "schema": { "type": "string" } }
            ]
        } },
        "components": { "pathItems": { "Shared": {
            "get": { "operationId": "shared" }
        } } }
    }))
    .expect("the description must parse");
    let validator = Validator::new(spec);

    assert_eq!(
        errors(&validator, &RequestView::new("GET", "/x")),
        ["header parameter \"X-Local\": is required and was not sent"],
    );
    let ok = RequestView::new("GET", "/x").with_header("x-local", "here");
    assert!(errors(&validator, &ok).is_empty());
}

#[test]
fn a_method_not_allowed_error_reads_as_http_would_write_it() {
    let validator = validator(json!({
        "/pets": {
            "get": { "operationId": "listPets" },
            "additionalOperations": { "COPY": { "operationId": "copyPet" } }
        }
    }));
    // The token the request carried, verbatim — and an `Allow` list of
    // real method tokens rather than OpenAPI's lowercase keys.
    let error = validator
        .validate(&RequestView::new("get", "/pets"))
        .expect_err("`get` is not `GET`");
    assert_eq!(
        error,
        RoutingError::MethodNotAllowed {
            template: "/pets".to_owned(),
            method: "get".to_owned(),
            allowed: vec!["GET".to_owned(), "COPY".to_owned()],
        },
    );
    assert_eq!(
        error.to_string(),
        "/pets describes no get operation (it has: GET, COPY)",
    );
}

#[test]
fn an_undescribed_method_at_an_operations_own_prefix_is_a_405_not_a_404() {
    let validator = validator(json!({
        "/pets": { "get": { "operationId": "listPets", "servers": [{ "url": "/v2" }] } }
    }));
    // `/v2/pets` is demonstrably a path this description serves.
    let error = validator
        .validate(&RequestView::new("DELETE", "/v2/pets"))
        .expect_err("the path offers no DELETE");
    assert_eq!(
        error,
        RoutingError::MethodNotAllowed {
            template: "/pets".to_owned(),
            method: "DELETE".to_owned(),
            allowed: vec!["GET".to_owned()],
        },
    );
    // A path it does not serve is still a 404.
    assert!(matches!(
        validator.validate(&RequestView::new("DELETE", "/v9/pets")),
        Err(RoutingError::PathNotFound { .. }),
    ));
}

#[test]
fn a_malformed_deep_object_name_is_reported_rather_than_ignored() {
    let paths = json!({
        "/x": { "get": { "parameters": [
            { "name": "filter", "in": "query", "style": "deepObject", "explode": true,
              "schema": { "type": "object", "properties": { "role": { "type": "string" } } } }
        ] } }
    });
    let strict = with_options(paths, Options::new().reject_undescribed_query_parameters());
    // None of these decode into anything, so none of them may pass as
    // "described".
    for query in [
        "filter=garbage",
        "filter[role=admin",
        "filter[role]junk=admin",
    ] {
        assert_eq!(
            errors(&strict, &RequestView::new("GET", "/x").with_query(query)).len(),
            1,
            "{query} must be reported",
        );
    }
    assert!(
        errors(
            &strict,
            &RequestView::new("GET", "/x").with_query("filter[role]=admin")
        )
        .is_empty(),
    );
}

#[test]
fn an_integer_enum_is_compared_by_value_not_by_representation() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/json": { "schema": {
                "type": "object",
                "properties": { "n": { "type": "integer", "enum": [9_007_199_254_740_994_i64] } }
            } }
        } } } }
    }));
    let matching = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(br#"{"n":9007199254740994}"#.as_slice());
    assert!(errors(&validator, &matching).is_empty());

    // Written with an exponent, the same value reaches `serde_json` as
    // an `f64` — indistinguishable from `9007199254740993.5`. It is not
    // rejected and it is not accepted: it is reported as unchecked.
    let exponent = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(br#"{"n":9.007199254740994e15}"#.as_slice());
    let found = errors(&validator, &exponent);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("body at /n: was NOT checked"),
        "{found:?}"
    );
}

#[test]
fn a_bound_whose_digits_were_lost_reports_that_it_could_not_check() {
    // `serde_json` parses 9007199254740993.5 as 9007199254740994.0 long
    // before this crate sees it, so the tie cannot be settled — and an
    // unsettled tie must not read as valid.
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/json": { "schema": {
                "type": "object",
                "properties": { "n": { "type": "integer", "maximum": 9_007_199_254_740_993.5_f64 } }
            } }
        } } } }
    }));
    let request = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(br#"{"n":9007199254740994}"#.as_slice());
    let found = errors(&validator, &request);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // A value clear of the boundary is still decided normally.
    let clear = RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(br#"{"n":1}"#.as_slice());
    assert!(errors(&validator, &clear).is_empty());
}

// ── the review of f28be6c ────────────────────────────────────────────

/// A body validated against one inline schema.
fn body_schema(schema: serde_json::Value) -> Validator {
    validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/json": { "schema": schema }
        } } } }
    }))
}

fn posted(body: &'static [u8]) -> RequestView<'static> {
    RequestView::new("POST", "/x")
        .with_header("content-type", "application/json")
        .with_body(body)
}

#[test]
fn a_not_whose_schema_cannot_be_applied_does_not_accept_the_value() {
    // The pattern never compiles, so nothing established that the value
    // fails the inner schema — and `not` may only accept on that basis.
    let validator = body_schema(json!({ "not": { "type": "string", "pattern": "(" } }));
    let found = errors(&validator, &posted(br#""anything""#));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // A `not` that really did reject still accepts.
    let sound = body_schema(json!({ "not": { "type": "string" } }));
    assert!(errors(&sound, &posted(b"1")).is_empty());
}

#[test]
fn an_any_of_that_could_not_be_applied_is_not_a_plain_mismatch() {
    let validator = body_schema(json!({
        "anyOf": [{ "type": "integer" }, { "type": "string", "pattern": "(" }]
    }));
    // A branch really matched, so the unreadable one does not matter.
    assert!(errors(&validator, &posted(b"1")).is_empty());

    // None matched — but the string branch was never applied, because
    // its pattern would not compile, so "no branch matched" is not
    // something that was established. (The value has to be a string to
    // reach the pattern at all: a type mismatch is decided first.)
    let found = errors(&validator, &posted(br#""x""#));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");
}

#[test]
fn a_one_of_with_an_unapplied_branch_cannot_count_its_matches() {
    let validator = body_schema(json!({
        "oneOf": [
            { "type": "string", "minLength": 1 },
            { "type": "string", "pattern": "(" }
        ]
    }));
    // Exactly one matched here — but the other branch was never
    // applied, and it might have matched too.
    let found = errors(&validator, &posted(br#""x""#));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // Two matches is a failure whatever the rest would have said.
    let two = body_schema(json!({
        "oneOf": [
            { "type": "integer", "minimum": 0 },
            { "type": "integer", "maximum": 10 },
            { "type": "string", "pattern": "(" }
        ]
    }));
    let found = errors(&two, &posted(b"5"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("exactly one is required"), "{found:?}");
}

#[test]
fn a_number_past_the_exact_range_is_not_declared_an_integer() {
    let validator = body_schema(json!({ "type": "integer" }));
    // `9007199254740993.5` and `9007199254740994` are the same `f64`,
    // so "has no fractional part" cannot be established of either.
    let found = errors(&validator, &posted(b"9007199254740993.5"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // A plain integer literal is parsed exactly and decided normally.
    assert!(errors(&validator, &posted(b"9007199254740994")).is_empty());
    // And a small fractional value is still simply the wrong type.
    assert_eq!(
        errors(&validator, &posted(b"1.5")),
        ["body: expected integer, got number"],
    );
}

#[test]
fn a_multi_typed_schema_inherits_the_same_caution() {
    let integer_only = body_schema(json!({ "type": ["integer", "null"] }));
    let found = errors(&integer_only, &posted(b"9007199254740993.5"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // `number` accepts it outright, so nothing is in doubt.
    let with_number = body_schema(json!({ "type": ["integer", "number"] }));
    assert!(errors(&with_number, &posted(b"9007199254740993.5")).is_empty());
}

#[test]
fn an_unchecked_error_still_says_where_in_the_body_it_happened() {
    let validator = body_schema(json!({
        "type": "object",
        "properties": { "user": {
            "type": "object",
            "properties": { "name": { "type": "string", "pattern": "(" } }
        } }
    }));
    let found = errors(&validator, &posted(br#"{"user":{"name":"x"}}"#));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("body at /user/name: was NOT checked"),
        "{found:?}"
    );
}

#[test]
fn a_path_item_reference_chain_is_followed_all_the_way() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": { "$ref": "#/components/pathItems/A" } },
        "components": { "pathItems": {
            "A": { "$ref": "#/components/pathItems/B" },
            "B": { "get": { "operationId": "deep" } }
        } }
    }))
    .expect("the description must parse");
    let report = Validator::new(spec)
        .validate(&RequestView::new("GET", "/x"))
        .expect("the chain reaches an operation");
    assert!(report.is_valid(), "{report}");
    assert_eq!(report.operation_id.as_deref(), Some("deep"));
}

#[test]
fn a_path_item_reference_that_cannot_be_followed_is_reported() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": {
            "$ref": "#/components/pathItems/Gone",
            "get": { "operationId": "local" }
        } }
    }))
    .expect("the description must parse");
    let validator = Validator::new(spec);
    let report = validator
        .validate(&RequestView::new("GET", "/x"))
        .expect("the local operation still describes GET /x");
    // The local half still validates; the half that could not be read
    // is reported rather than treated as absent.
    assert_eq!(
        report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["description: has an unresolvable `$ref`: #/components/pathItems/Gone"],
    );
}

// ── the review of 7ce9d3d ────────────────────────────────────────────

#[test]
fn a_number_bound_past_the_exact_range_is_not_silently_applied() {
    // `maximum: 9007199254740993.5` reaches this crate as
    // `9007199254740994.0`, so a value that lands on it cannot be
    // decided — `type: number` gets the same caution as `type: integer`.
    let validator =
        body_schema(json!({ "type": "number", "maximum": 9_007_199_254_740_993.5_f64 }));
    let found = errors(&validator, &posted(b"9007199254740994"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // Clear of the boundary, it is decided as usual, both ways.
    assert!(errors(&validator, &posted(b"1.5")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"1e300")).len(),
        1,
        "a value far above the maximum is still a plain failure",
    );
}

#[test]
fn ordinary_decimal_bounds_are_left_alone() {
    // Below 2^53 the usual floating-point caveats apply and are not
    // worth flagging: every implementation compares `0.1` with the
    // `f64` nearest `0.1`.
    let validator = body_schema(json!({ "type": "number", "maximum": 0.1, "minimum": 0.1 }));
    assert!(errors(&validator, &posted(b"0.1")).is_empty());
}

#[test]
fn a_number_enum_past_the_exact_range_reports_rather_than_guesses() {
    let validator = body_schema(json!({
        "type": "number",
        "enum": [9_007_199_254_740_994.0_f64]
    }));
    let found = errors(&validator, &posted(b"9007199254740994"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // Two numbers that differ as floats differed as written, so a
    // definite mismatch is still definite.
    assert_eq!(errors(&validator, &posted(b"1")).len(), 1);
}

#[test]
fn a_definite_failure_settles_a_schema_whatever_else_could_not_be_applied() {
    // `"x"` is definitely too short, so the inner schema definitely
    // fails and `not` definitely passes — the lookahead the pattern
    // wants, which this crate's regex engine will not compile, does not
    // get a say.
    let validator = body_schema(json!({
        "not": { "type": "string", "minLength": 2, "pattern": "(?=a)" }
    }));
    assert!(errors(&validator, &posted(br#""x""#)).is_empty());

    // With nothing definite either way, it is still unchecked.
    let undecided = body_schema(json!({
        "not": { "type": "string", "minLength": 1, "pattern": "(?=a)" }
    }));
    let found = errors(&undecided, &posted(br#""x""#));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");
}

#[test]
fn a_bare_unresolvable_path_item_reference_is_neither_a_404_nor_a_405() {
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": { "$ref": "#/components/pathItems/Gone" } }
    }))
    .expect("the description must parse");
    let error = Validator::new(spec)
        .validate(&RequestView::new("GET", "/x"))
        .expect_err("the path item could not be read");
    assert_eq!(
        error,
        RoutingError::Unresolved {
            template: "/x".to_owned(),
            reference: "#/components/pathItems/Gone".to_owned(),
        },
    );
}

#[test]
fn an_unreadable_path_item_does_not_claim_a_method_is_unavailable() {
    // A local `get` exists, but the unread half may describe `POST`, so
    // `MethodNotAllowed` would be a claim this crate cannot make.
    let spec: roas::v3_2::spec::Spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": {
            "$ref": "#/components/pathItems/Gone",
            "get": { "operationId": "local" }
        } }
    }))
    .expect("the description must parse");
    let validator = Validator::new(spec);
    assert!(matches!(
        validator.validate(&RequestView::new("POST", "/x")),
        Err(RoutingError::Unresolved { .. }),
    ));
    // The half that is readable still validates, and still says so.
    let report = validator
        .validate(&RequestView::new("GET", "/x"))
        .expect("the local operation describes GET /x");
    assert_eq!(
        report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["description: has an unresolvable `$ref`: #/components/pathItems/Gone"],
    );
}

// ── the review of d3b7d97 ────────────────────────────────────────────

#[test]
fn a_fraction_that_rounded_into_a_whole_number_is_not_called_an_integer() {
    // `9007199254740991.5` is stored as `9007199254740992.0`: from 2^52
    // up, consecutive floats are 1 apart, so the `.5` is gone. It looks
    // whole and never was.
    let validator = body_schema(json!({ "type": "integer" }));
    let found = errors(&validator, &posted(b"9007199254740991.5"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // An integer `serde_json` kept as an integer is exact at any
    // magnitude, and still decided normally.
    assert!(errors(&validator, &posted(b"9007199254740992")).is_empty());
    assert!(errors(&validator, &posted(b"9007199254740993")).is_empty());
}

#[test]
fn a_bound_that_rounded_into_a_whole_number_cannot_settle_a_tie() {
    let validator = body_schema(json!({
        "type": "integer",
        "maximum": 4_503_599_627_370_496.5_f64
    }));
    // The bound is stored as 4503599627370496.0, so a value that lands
    // on it is the undecidable case: it is below the maximum as
    // written, and equal to it as stored.
    let found = errors(&validator, &posted(b"4503599627370496"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");

    // The guard stays narrow — anything not on the tie is still
    // decided, in both directions.
    assert!(errors(&validator, &posted(b"1")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"4503599627370497")),
        ["body: 4503599627370497 is above maximum 4503599627370496"],
    );
}

#[test]
fn a_multiple_of_is_decided_on_mantissas_rather_than_by_dividing() {
    // 9007199254740992 / 1.5 is 6004799503160661.33, which an `f64`
    // stores as 6004799503160661.0 — a division would see a remainder
    // of zero and call it divisible. Both numbers are exactly the
    // decimals they were written as, so the answer comes from their
    // mantissas instead, and it is definite.
    let validator = body_schema(json!({ "type": "integer", "multipleOf": 1.5 }));
    assert_eq!(
        errors(&validator, &posted(b"9007199254740992")),
        ["body: 9007199254740992 is not a multiple of 1.5"],
    );
    assert!(errors(&validator, &posted(b"3")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"4")),
        ["body: 4 is not a multiple of 1.5"],
    );

    // A value that does not survive conversion to a float is not the
    // number being divided, so nothing can be said about it.
    let found = errors(&validator, &posted(b"9007199254740993"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");
}

#[test]
fn a_whole_step_divides_exactly_at_any_magnitude() {
    // Both sides whole means an integer remainder, with no float in it.
    let validator = body_schema(json!({ "type": "integer", "multipleOf": 2 }));
    assert!(errors(&validator, &posted(b"9007199254740992")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"9007199254740993")),
        ["body: 9007199254740993 is not a multiple of 2"],
    );
}

// ── the review of b5c686f ────────────────────────────────────────────

#[test]
fn a_small_fraction_that_rounded_away_is_not_called_an_integer_either() {
    // A fraction can round away at *any* magnitude — `1.0000000000000001`
    // is `1.0` — which is why the rule cannot be a magnitude cutoff.
    // Nothing distinguishes such a value from one written `1.0`, or from
    // one written `1`, except the lexeme, and that is gone.
    let validator = body_schema(json!({ "type": "integer" }));
    for spelling in [b"1.0000000000000001".as_slice(), b"1.0".as_slice()] {
        let found = errors(&validator, &posted(spelling));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("could NOT be established"),
            "{}: {found:?}",
            String::from_utf8_lossy(spelling),
        );
    }
    // An integer that was written as one is still simply an integer.
    assert!(errors(&validator, &posted(b"2251799813685248")).is_empty());

    // And a fraction `serde_json` did keep is still a definite type
    // failure rather than an undecidable one.
    assert_eq!(
        errors(&validator, &posted(b"2251799813685248.25")),
        ["body: expected integer, got number"],
    );
}

#[test]
fn a_multiple_of_is_not_proved_by_a_quotient_that_rounded_its_input() {
    // 9007199254740993 does not survive `f64` — it arrives as
    // ...992 — so the quotient comes out exactly 2 and hides a
    // remainder of 1.
    let validator = body_schema(json!({
        "type": "integer",
        "multipleOf": 4_503_599_627_370_496_i64
    }));
    // The step is 2^52, where a stored double no longer pins down the
    // number written, so neither instance can be decided against it.
    for spelling in [
        b"9007199254740993".as_slice(),
        b"9007199254740992".as_slice(),
    ] {
        let found = errors(&validator, &posted(spelling));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("could NOT be established"), "{found:?}");
    }

    // A fractional step that is exactly the decimal it was written as
    // is decided exactly too, on the mantissas rather than by dividing.
    let fractional = body_schema(json!({ "type": "integer", "multipleOf": 1.5 }));
    assert_eq!(
        errors(&fractional, &posted(b"9007199254740992")),
        ["body: 9007199254740992 is not a multiple of 1.5"],
    );
    assert!(errors(&fractional, &posted(b"3")).is_empty());

    // But a value that no longer survives conversion cannot be, since
    // the number being divided is not the number that arrived.
    let found = errors(&fractional, &posted(b"9007199254740993"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");
}

#[test]
fn the_multiple_of_tolerance_allows_for_rounding_and_nothing_more() {
    let validator = body_schema(json!({ "type": "number", "multipleOf": 1 }));
    // A different number, not a rounding artefact — a fixed 1e-9
    // tolerance used to wave this through.
    assert_eq!(
        errors(&validator, &posted(b"1.0000000005")),
        ["body: 1.0000000005 is not a multiple of 1"],
    );
    assert!(errors(&validator, &posted(b"2")).is_empty());

    // One ULP away from 1: inside any tolerance, and still a different
    // number. Neither accepted nor rejected.
    let found = errors(&validator, &posted(b"1.0000000000000002"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");

    // 0.3 / 0.1 is 2.9999999999999996: within rounding of a whole
    // quotient, which is not the same as being one.
    let tenths = body_schema(json!({ "type": "number", "multipleOf": 0.1 }));
    let found = errors(&tenths, &posted(b"0.3"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");
    assert_eq!(
        errors(&tenths, &posted(b"0.35")),
        ["body: 0.35 is not a multiple of 0.1"],
    );
}

// ── the review of 22f5b8b ────────────────────────────────────────────

#[test]
fn an_enum_member_that_lost_digits_does_not_produce_a_false_violation() {
    // `roas` holds a `number` enum as `f64`, so `9007199254740993`
    // arrives here as `9007199254740992`. The request value is exact
    // and unequal to it — but the two could have been the same number,
    // so a rejection would be a claim this crate cannot make.
    let validator = body_schema(json!({
        "type": "number",
        "enum": [9_007_199_254_740_993_i64]
    }));
    let found = errors(&validator, &posted(b"9007199254740993"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");

    // A number outside that uncertainty is still a definite mismatch.
    assert_eq!(
        errors(&validator, &posted(b"1")),
        ["body: 1 is not one of: 9007199254740992"],
    );
}

#[test]
fn a_bound_that_lost_digits_does_not_produce_a_false_violation_either() {
    let validator = body_schema(json!({
        "type": "number",
        "maximum": 9_007_199_254_740_993_i64
    }));
    // Inside the stored bound's uncertainty: unknowable, not a failure.
    let found = errors(&validator, &posted(b"9007199254740993"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");

    // Clear of it in either direction, the answer is definite.
    assert!(errors(&validator, &posted(b"1")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"1e300")).len(),
        1,
        "a value far above the maximum is a plain failure",
    );
}

#[test]
fn a_multiple_of_operand_that_lost_digits_decides_nothing() {
    // The step is stored as 9007199254740994, so a clean remainder
    // against it would prove nothing about the 9007199254740993.5 that
    // was written.
    let step = body_schema(json!({ "type": "number", "multipleOf": 9_007_199_254_740_993.5_f64 }));
    let found = errors(&step, &posted(b"9007199254740994"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");

    // And the same the other way round: a value that lost digits
    // cannot be divided by anything either.
    let value = body_schema(json!({ "type": "number", "multipleOf": 9_007_199_254_740_994_i64 }));
    let found = errors(&value, &posted(b"9007199254740993.5"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");
}

#[test]
fn exact_integers_are_still_decided_exactly_on_both_sides() {
    // The guard is about numbers that lost digits, not about size:
    // `integer` bounds come through `serde_json::Number`, which keeps
    // integers as integers, so these stay definite.
    let validator = body_schema(json!({
        "type": "integer",
        "minimum": 9_007_199_254_740_993_i64,
        "maximum": 9_007_199_254_740_995_i64
    }));
    assert!(errors(&validator, &posted(b"9007199254740994")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"9007199254740992")),
        ["body: 9007199254740992 is below minimum 9007199254740993"],
    );
    assert_eq!(
        errors(&validator, &posted(b"9007199254740996")),
        ["body: 9007199254740996 is above maximum 9007199254740995"],
    );
}

// ── the review of 6de60cd ────────────────────────────────────────────

#[test]
fn a_quotient_that_rounded_to_a_whole_number_is_not_proof_of_divisibility() {
    // 2814749767106564 / 1.25 is exactly 2251799813685251.2, which an
    // `f64` stores as 2251799813685251.0 — well below any quotient
    // cutoff, with a residual of exactly zero. Only exact arithmetic on
    // the two mantissas catches it.
    let validator = body_schema(json!({ "type": "integer", "multipleOf": 1.25 }));
    assert_eq!(
        errors(&validator, &posted(b"2814749767106564")),
        ["body: 2814749767106564 is not a multiple of 1.25"],
    );
    // And a value that really is a multiple is still accepted.
    assert!(errors(&validator, &posted(b"2814749767106565")).is_empty());
}

#[test]
fn exact_decimals_are_decided_and_inexact_ones_are_reported() {
    // `1.25`, `0.5` and `2.5` are exactly the decimals they print as,
    // so their divisibility has an exact answer.
    let halves = body_schema(json!({ "type": "number", "multipleOf": 0.5 }));
    assert!(errors(&halves, &posted(b"2.5")).is_empty());
    assert_eq!(
        errors(&halves, &posted(b"2.25")),
        ["body: 2.25 is not a multiple of 0.5"],
    );

    // `0.1` is not: the double is 0.1000000000000000055511151231257827,
    // and 0.3 is not a multiple of *that*, though it is of the 0.1 the
    // author wrote. Neither answer can be given, so neither is.
    let tenths = body_schema(json!({ "type": "number", "multipleOf": 0.1 }));
    let found = errors(&tenths, &posted(b"0.3"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be established"), "{found:?}");

    // A remainder far enough from zero to survive is still definite.
    assert_eq!(
        errors(&tenths, &posted(b"0.35")),
        ["body: 0.35 is not a multiple of 0.1"],
    );
}
