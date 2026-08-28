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

// ── the review of f28be6c ────────────────────────────────────────────

/// A body validated against a schema written as JSON *text*.
///
/// `json!` with an `f64` literal rounds the number before `serde_json`
/// ever sees one, so a schema built that way cannot carry
/// `9007199254740993.5` no matter what the crate does with it. Text
/// keeps the literal, which is the whole point of the exercise.
fn body_schema_text(schema: &str) -> Validator {
    let spec = serde_json::from_str(&format!(
        r#"{{"openapi":"3.2.0","info":{{"title":"t","version":"1"}},
            "paths":{{"/x":{{"post":{{"requestBody":{{"content":{{
              "application/json":{{"schema":{schema}}}}}}}}}}}}}}}"#
    ))
    .expect("the description must parse");
    Validator::new(spec)
}

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

// ── the review of b5c686f ────────────────────────────────────────────

// ── the review of 22f5b8b ────────────────────────────────────────────

// ── the review of 6de60cd ────────────────────────────────────────────

// ── the review of 0586160 ────────────────────────────────────────────

// ── the review of 77b89d6 ────────────────────────────────────────────

#[test]
fn an_unresolvable_reference_is_not_the_requests_fault() {
    let validator = validator(json!({
        "/x": { "post": { "requestBody": { "content": {
            "application/json": { "schema": { "$ref": "#/components/schemas/Gone" } }
        } } } }
    }));
    let report = validator
        .validate(&posted(br#"{"anything":true}"#))
        .expect("the description describes POST /x");

    // Nothing was judged, so nothing was found wrong with the request.
    assert!(!report.is_valid());
    assert_eq!(report.violations().count(), 0, "{report}");
    assert_eq!(report.unchecked().count(), 1, "{report}");
}

// ── an integer `multipleOf` is exact again ───────────────────────────

// ── exact decimals ───────────────────────────────────────────────────
//
// `serde_json` is built with `arbitrary_precision`, so a number keeps
// the literal it was written as and every question below has an answer
// rather than a caveat.

#[test]
fn integer_ness_is_read_from_the_literal() {
    let validator = body_schema(json!({ "type": "integer" }));

    // Whole however it was spelled.
    for spelling in [
        b"1".as_slice(),
        b"1.0".as_slice(),
        b"100e-2".as_slice(),
        b"2e3".as_slice(),
    ] {
        assert!(
            errors(&validator, &posted(spelling)).is_empty(),
            "{} is an integer",
            String::from_utf8_lossy(spelling),
        );
    }

    // And definitely not whole — including the fractions a double eats.
    for spelling in [
        b"1.5".as_slice(),
        b"1.0000000000000001".as_slice(),
        b"2251799813685248.25".as_slice(),
        b"9007199254740991.5".as_slice(),
    ] {
        assert_eq!(
            errors(&validator, &posted(spelling)),
            ["body: expected integer, got number"],
            "{} is not an integer",
            String::from_utf8_lossy(spelling),
        );
    }
}

#[test]
fn bounds_are_compared_exactly_past_what_a_double_holds() {
    let validator = body_schema(json!({
        "type": "integer",
        "maximum": 9_007_199_254_740_992_i64
    }));
    assert!(errors(&validator, &posted(b"9007199254740992")).is_empty());
    // The pair a double makes equal.
    assert_eq!(
        errors(&validator, &posted(b"9007199254740993")),
        ["body: 9007199254740993 is above maximum 9007199254740992"],
    );
}

#[test]
fn a_fractional_bound_decides_the_value_that_used_to_tie_with_it() {
    // `maximum: 9007199254740993.5` and `9007199254740994` are one
    // double; as decimals the value is plainly above the bound.
    let validator = body_schema_text(r#"{"type":"integer","maximum":9007199254740993.5}"#);
    assert!(errors(&validator, &posted(b"9007199254740993")).is_empty());
    assert_eq!(errors(&validator, &posted(b"9007199254740994")).len(), 1);
}

#[test]
fn an_enum_matches_by_value_rather_than_by_spelling() {
    let validator = body_schema(json!({
        "type": "integer",
        "enum": [9_007_199_254_740_993_i64]
    }));
    for spelling in [
        b"9007199254740993".as_slice(),
        b"9007199254740993.0".as_slice(),
    ] {
        assert!(
            errors(&validator, &posted(spelling)).is_empty(),
            "{} is the member",
            String::from_utf8_lossy(spelling),
        );
    }
    // And the neighbour a double could not tell apart from it.
    assert_eq!(errors(&validator, &posted(b"9007199254740992")).len(), 1);
}

#[test]
fn divisibility_is_decided_for_decimal_steps() {
    // The case no amount of floating point could settle: 0.3 / 0.1 is
    // 2.9999999999999996 as doubles and exactly 3 as decimals.
    let tenths = body_schema(json!({ "type": "number", "multipleOf": 0.1 }));
    assert!(errors(&tenths, &posted(b"0.3")).is_empty());
    assert_eq!(
        errors(&tenths, &posted(b"0.35")),
        ["body: 0.35 is not a multiple of 0.1"],
    );

    // Prices, which is what `multipleOf` is usually for.
    let pennies = body_schema(json!({ "type": "number", "multipleOf": 0.01 }));
    assert!(errors(&pennies, &posted(b"1.23")).is_empty());
    assert_eq!(errors(&pennies, &posted(b"1.234")).len(), 1);
}

#[test]
fn divisibility_is_decided_for_integer_steps_at_any_size() {
    let validator = body_schema(json!({ "type": "integer", "multipleOf": 2 }));
    assert!(errors(&validator, &posted(b"4")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"5")),
        ["body: 5 is not a multiple of 2"],
    );

    let big = body_schema(json!({ "type": "integer", "multipleOf": 4_503_599_627_370_496_i64 }));
    assert!(errors(&big, &posted(b"9007199254740992")).is_empty());
    assert_eq!(errors(&big, &posted(b"9007199254740993")).len(), 1);
}

#[test]
fn a_step_written_with_extra_digits_is_no_longer_the_same_step() {
    // `1.0000000000000001` and `1` are one double and two decimals.
    let validator = body_schema_text(r#"{"type":"integer","multipleOf":1.0000000000000001}"#);
    assert_eq!(errors(&validator, &posted(b"1")).len(), 1);

    let plain = body_schema(json!({ "type": "integer", "multipleOf": 1 }));
    assert!(errors(&plain, &posted(b"1")).is_empty());
}

#[test]
fn a_quotient_that_would_have_rounded_is_decided_anyway() {
    // 2814749767106564 / 1.25 is 2251799813685251.2, which a double
    // stores as a whole number and calls divisible.
    let validator = body_schema(json!({ "type": "integer", "multipleOf": 1.25 }));
    assert_eq!(
        errors(&validator, &posted(b"2814749767106564")),
        ["body: 2814749767106564 is not a multiple of 1.25"],
    );
    assert!(errors(&validator, &posted(b"2814749767106565")).is_empty());
}

#[test]
fn zero_is_a_multiple_of_anything_however_it_was_written() {
    let validator = body_schema(json!({ "type": "number", "multipleOf": 1.5 }));
    for spelling in [b"0".as_slice(), b"0.0".as_slice(), b"-0.0".as_slice()] {
        assert!(
            errors(&validator, &posted(spelling)).is_empty(),
            "{} is zero",
            String::from_utf8_lossy(spelling),
        );
    }
    // And a number that merely underflows a double is not zero.
    assert_eq!(errors(&validator, &posted(b"1e-324")).len(), 1);
}

#[test]
fn a_literal_too_large_to_hold_is_reported_rather_than_approximated() {
    // Past `i128`: the one thing left that cannot be decided, and it
    // says so rather than guessing.
    let validator = body_schema(json!({ "type": "integer", "maximum": 10 }));
    let enormous: &[u8] = b"99999999999999999999999999999999999999999999";
    let found = errors(&validator, &posted(enormous));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be checked"), "{found:?}");
}

#[test]
fn a_caller_can_still_tell_a_violation_from_something_unchecked() {
    // A `pattern` this crate's regex engine will not compile leaves the
    // value unjudged, beside a bound that definitely failed.
    let validator = body_schema(json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "maximum": 10 },
            "code": { "type": "string", "pattern": "(?=a)" }
        }
    }));
    let report = validator
        .validate(&posted(br#"{"limit":100,"code":"x"}"#))
        .expect("the description describes POST /x");

    assert!(!report.is_valid());
    assert_eq!(report.violations().count(), 1, "{report}");
    assert_eq!(report.unchecked().count(), 1, "{report}");
}

#[test]
fn a_parameter_is_no_less_exact_than_a_body() {
    // Query values are text, and used to reach the schema by way of an
    // `f64`. `9007199254740993` is not representable as one.
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "n", "in": "query",
              "schema": { "type": "integer", "maximum": 9_007_199_254_740_992_i64 } }
        ] } }
    }));
    assert!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=9007199254740992")
        )
        .is_empty()
    );
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("n=9007199254740993")
        ),
        ["query parameter \"n\": 9007199254740993 is above maximum 9007199254740992"],
    );
}

#[test]
fn a_decimal_parameter_keeps_its_digits() {
    let validator = validator(json!({
        "/x": { "get": { "parameters": [
            { "name": "price", "in": "query",
              "schema": { "type": "number", "multipleOf": 0.01 } }
        ] } }
    }));
    assert!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("price=1.23")
        )
        .is_empty()
    );
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("price=1.234")
        ),
        ["query parameter \"price\": 1.234 is not a multiple of 0.01"],
    );
    // And a value that is not a number at all is still refused.
    assert_eq!(
        errors(
            &validator,
            &RequestView::new("GET", "/x").with_query("price=cheap")
        ),
        ["query parameter \"price\": cannot be read: \"cheap\" is not a number"],
    );
}

// ── decoders for media types the crate does not read itself ──────────

/// The same body schema, plus one registered decoder.
fn body_schema_with(schema: serde_json::Value, options: Options) -> Validator {
    let spec = serde_json::from_value(json!({
        "openapi": "3.2.0",
        "info": { "title": "t", "version": "1" },
        "paths": { "/x": { "post": { "requestBody": { "content": {
            "application/xml": { "schema": schema.clone() },
            "multipart/form-data": { "schema": schema.clone() },
            "text/csv": { "schema": schema },
        } } } } },
    }))
    .expect("the description must parse");
    Validator::with_options(spec, options)
}

fn posted_as(media_type: &'static str, body: &'static [u8]) -> RequestView<'static> {
    RequestView::new("POST", "/x")
        .with_header("content-type", media_type)
        .with_body(body)
}

#[test]
fn an_unreadable_media_type_is_reported_when_no_decoder_is_given() {
    let validator = body_schema_with(json!({ "type": "object" }), Options::new());
    let found = errors(&validator, &posted_as("application/xml", b"<pet/>"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("was NOT checked"), "{found:?}");
}

#[test]
fn a_decoder_lets_the_schema_judge_a_body_the_crate_cannot_read() {
    // A stand-in for whatever XML mapping a caller's clients use — the
    // point is that the choice is theirs, not this crate's.
    let options = Options::new().decoder("application/xml", |bytes, _media_type| {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let name = text
            .strip_prefix("<pet><name>")
            .and_then(|rest| rest.strip_suffix("</name></pet>"))
            .ok_or_else(|| "not a <pet>".to_owned())?;
        Ok(json!({ "name": name }))
    });
    let validator = body_schema_with(
        json!({
            "type": "object",
            "required": ["name"],
            "properties": { "name": { "type": "string", "minLength": 2 } }
        }),
        options,
    );

    assert!(
        errors(
            &validator,
            &posted_as("application/xml", b"<pet><name>Rex</name></pet>")
        )
        .is_empty()
    );

    // The decoded value is judged like any other, pointer and all.
    assert_eq!(
        errors(
            &validator,
            &posted_as("application/xml", b"<pet><name>R</name></pet>")
        ),
        ["body at /name: is shorter than minLength 2 (1 characters)"],
    );

    // And a decoder that cannot read the bytes reports why.
    assert_eq!(
        errors(&validator, &posted_as("application/xml", b"<dog/>")),
        ["body: cannot be read: not a <pet>"],
    );
}

#[test]
fn a_decoder_can_handle_multipart_without_this_crate_owning_a_parser() {
    // The decoder is handed the header as it arrived, `boundary` and
    // all — without it there is no way to split a multipart body, which
    // is why RFC 7578 makes the parameter required.
    let options = Options::new().decoder("multipart/form-data", |bytes, content_type| {
        let boundary = content_type
            .split(';')
            .filter_map(|parameter| parameter.trim().strip_prefix("boundary="))
            .next()
            .ok_or_else(|| "no boundary in the content type".to_owned())?;
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let mut parts = serde_json::Map::new();
        for part in text.split(&format!("--{boundary}")) {
            if let Some((name, value)) = part.trim().split_once('=') {
                parts.insert(name.to_owned(), value.into());
            }
        }
        Ok(serde_json::Value::Object(parts))
    });
    let validator = body_schema_with(
        json!({
            "type": "object",
            "required": ["title"],
            "properties": { "title": { "type": "string" } }
        }),
        options,
    );

    // A boundary the decoder could not have guessed.
    let sent = "multipart/form-data; boundary=xY7zQ";
    assert!(errors(&validator, &posted_as(sent, b"--xY7zQ\ntitle=hello\n")).is_empty(),);
    assert_eq!(
        errors(&validator, &posted_as(sent, b"--xY7zQ\nother=hello\n")),
        ["body at /title: is required and was not sent"],
    );

    // And without the parameter the decoder says so, rather than this
    // crate having quietly dropped it.
    assert_eq!(
        errors(
            &validator,
            &posted_as("multipart/form-data", b"title=hello")
        ),
        ["body: cannot be read: no boundary in the content type"],
    );
}

#[test]
fn a_decoder_is_reached_through_a_range_too() {
    let options = Options::new().decoder("text/*", |bytes, media_type| {
        Ok(json!({ "read_as": media_type, "length": bytes.len() }))
    });
    let validator = body_schema_with(
        json!({
            "type": "object",
            "properties": { "read_as": { "type": "string", "enum": ["text/csv"] } }
        }),
        options,
    );
    // `text/*` would otherwise have been read by the built-in text
    // decoder as a plain string; the registration takes precedence.
    assert!(errors(&validator, &posted_as("text/csv", b"a,b\n")).is_empty());
}

// ── the review of 0fd58e8 ────────────────────────────────────────────

#[test]
fn an_extreme_exponent_is_reported_rather_than_overflowing() {
    // Valid JSON literals, every one of which used to run the scale
    // past `i32` — a panic in a checked build.
    let validator = body_schema(json!({ "type": "number", "multipleOf": 1 }));
    for spelling in [b"10e2147483647".as_slice(), b"1e-2147483648".as_slice()] {
        let found = errors(&validator, &posted(spelling));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("could NOT be checked")
                || found[0].contains("could NOT be established"),
            "{}: {found:?}",
            String::from_utf8_lossy(spelling),
        );
    }
}

#[test]
fn a_multi_typed_schema_gives_the_same_answer_as_a_single_one() {
    // A literal past `i128` is undecidable against `integer`, and that
    // has to survive being listed beside another type rather than
    // becoming a violation.
    let enormous: &[u8] = b"99999999999999999999999999999999999999999999";

    let single = body_schema(json!({ "type": "integer" }));
    let found = errors(&single, &posted(enormous));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be checked"), "{found:?}");

    let multi = body_schema(json!({ "type": ["integer", "null"] }));
    let found = errors(&multi, &posted(enormous));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be checked"), "{found:?}");

    // But a type that really does accept it settles the question.
    let with_number = body_schema(json!({ "type": ["integer", "number"] }));
    assert!(errors(&with_number, &posted(enormous)).is_empty());

    // And a plain mismatch is still a plain failure.
    let strings = body_schema(json!({ "type": ["string", "null"] }));
    assert_eq!(
        errors(&strings, &posted(b"1")),
        ["body: expected one of [string, null], got integer"],
    );
}

#[test]
fn unique_items_compares_numbers_by_value_not_by_spelling() {
    let validator = body_schema(json!({
        "type": "array",
        "items": { "type": "number" },
        "uniqueItems": true
    }));
    // The same number written three ways is one item, not three.
    assert_eq!(
        errors(&validator, &posted(b"[1.0, 1.00, 1]")).len(),
        2,
        "each repeat is reported",
    );
    assert!(errors(&validator, &posted(b"[1, 2, 3]")).is_empty());
    assert!(errors(&validator, &posted(b"[1.0, 1.5]")).is_empty());
}

#[test]
fn unique_items_reaches_numbers_nested_inside_items() {
    let validator = body_schema(json!({
        "type": "array",
        "items": { "type": "object" },
        "uniqueItems": true
    }));
    assert_eq!(
        errors(&validator, &posted(br#"[{"n":1.0},{"n":1.00}]"#)),
        ["body at /1: repeats an earlier item, but uniqueItems is set"],
    );
    assert!(errors(&validator, &posted(br#"[{"n":1},{"n":2}]"#)).is_empty());
}

#[test]
fn a_number_schemas_bounds_are_exact_too() {
    // The gap this closed: `NumberSchema`'s bounds were `f64` in
    // `roas`, so `maximum: 9007199254740993` arrived as `…992` and the
    // value that equals it was reported as a violation. The same bound
    // on a `type: integer` schema was already exact, so one word in the
    // description changed the verdict.
    let as_number = body_schema_text(r#"{"type":"number","maximum":9007199254740993}"#);
    let as_integer = body_schema_text(r#"{"type":"integer","maximum":9007199254740993}"#);

    for validator in [&as_number, &as_integer] {
        assert!(errors(validator, &posted(b"9007199254740993")).is_empty());
        assert_eq!(errors(validator, &posted(b"9007199254740994")).len(), 1);
    }
}

#[test]
fn a_number_schemas_enum_is_exact_too() {
    let validator = body_schema_text(r#"{"type":"number","enum":[9007199254740993]}"#);
    assert!(errors(&validator, &posted(b"9007199254740993")).is_empty());
    // The neighbour a double could not tell apart from it.
    assert_eq!(errors(&validator, &posted(b"9007199254740992")).len(), 1);
}

#[test]
fn a_fractional_bound_on_a_number_schema_keeps_its_fraction() {
    let validator = body_schema_text(r#"{"type":"number","minimum":0.1,"maximum":0.3}"#);
    assert!(errors(&validator, &posted(b"0.2")).is_empty());
    assert!(errors(&validator, &posted(b"0.1")).is_empty());
    assert_eq!(
        errors(&validator, &posted(b"0.05")),
        ["body: 0.05 is below minimum 0.1"],
    );
}

#[test]
fn unique_items_says_when_it_cannot_compare_rather_than_assuming() {
    // Both literals are past `i128`, so neither can be read — and they
    // are not written identically, so whether they are the same number
    // is exactly what cannot be established. Calling the array unique
    // would be asserting it.
    let validator = body_schema(json!({
        "type": "array",
        "items": true,
        "uniqueItems": true
    }));
    let unreadable: &[u8] = b"[99999999999999999999999999999999999999999999, 99999999999999999999999999999999999999999999.0]";
    let found = errors(&validator, &posted(unreadable));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("could NOT be compared"), "{found:?}");

    // The same literal twice is the same number, unreadable or not.
    let identical: &[u8] = b"[99999999999999999999999999999999999999999999, 99999999999999999999999999999999999999999999]";
    assert_eq!(
        errors(&validator, &posted(identical)),
        ["body at /1: repeats an earlier item, but uniqueItems is set"],
    );
}

#[test]
fn a_definite_difference_settles_a_comparison_an_unreadable_number_cannot() {
    // The `tag`s differ and anyone can see it, so the objects differ —
    // whatever the unreadable `n`s would have said.
    let validator = body_schema(json!({
        "type": "array",
        "items": true,
        "uniqueItems": true
    }));
    let body: &[u8] = br#"[
        {"tag":"a","n":99999999999999999999999999999999999999999999},
        {"tag":"b","n":99999999999999999999999999999999999999999999.0}
    ]"#;
    assert!(errors(&validator, &posted(body)).is_empty());
}

// ── where exactness stops, and why ───────────────────────────────────

/// The same body schema, written as YAML rather than JSON.
fn body_schema_yaml(schema: &str) -> Validator {
    let spec = serde_yaml_ng::from_str(&format!(
        "openapi: 3.2.0\n\
         info: {{ title: t, version: '1' }}\n\
         paths:\n  \
           /x:\n    \
             post:\n      \
               requestBody:\n        \
                 content:\n          \
                   application/json:\n            \
                     schema: {schema}\n"
    ))
    .expect("the description must parse");
    Validator::new(spec)
}

#[test]
fn a_yaml_description_keeps_integer_bounds_well_past_a_double() {
    // 2^53 + 1: the first integer a double cannot hold, and the reason
    // this crate stopped using one.
    let past_a_double = body_schema_yaml("{ type: integer, maximum: 9007199254740993 }");
    assert!(errors(&past_a_double, &posted(b"9007199254740993")).is_empty());
    assert_eq!(
        errors(&past_a_double, &posted(b"9007199254740994")).len(),
        1
    );

    // 2^63: past `i64::MAX`, which YAML also carries intact.
    let past_i64 = body_schema_yaml("{ type: integer, maximum: 9223372036854775808 }");
    assert!(errors(&past_i64, &posted(b"9223372036854775808")).is_empty());
    assert_eq!(errors(&past_i64, &posted(b"9223372036854775809")).len(), 1);

    // The limit is `i128`, and it is the validator's own: 38 digits
    // survive, and this crate could not have held more anyway.
    let widest = format!("{{ type: integer, maximum: {} }}", "9".repeat(38));
    let widest = body_schema_yaml(&widest);
    assert!(errors(&widest, &posted(b"1")).is_empty());
}

#[test]
fn a_yaml_description_keeps_ordinary_decimal_bounds() {
    let validator = body_schema_yaml("{ type: number, multipleOf: 0.01 }");
    assert!(errors(&validator, &posted(b"1.23")).is_empty());
    assert_eq!(errors(&validator, &posted(b"1.234")).len(), 1);
}

/// Characterization, not aspiration.
///
/// `serde_yaml_ng` reads a scalar through an `f64` before `serde_json`
/// is involved, so a fractional literal carrying more precision than a
/// double is already rounded when `roas` builds the `Spec` — upstream
/// of anything this crate or `exact-numbers` can reach. This pins where
/// that boundary is, and will fail if the YAML parser ever stops losing
/// it, which is the point.
#[test]
fn a_yaml_fractional_bound_past_a_double_is_rounded_before_the_crate_sees_it() {
    let from_yaml = body_schema_yaml("{ type: integer, maximum: 9007199254740993.5 }");
    let from_json = body_schema_text(r#"{"type":"integer","maximum":9007199254740993.5}"#);

    // Both agree below the boundary.
    assert!(errors(&from_yaml, &posted(b"9007199254740993")).is_empty());
    assert!(errors(&from_json, &posted(b"9007199254740993")).is_empty());

    // And disagree on it: JSON kept the `.5`, YAML rounded to `…994`.
    assert_eq!(errors(&from_json, &posted(b"9007199254740994")).len(), 1);
    assert!(
        errors(&from_yaml, &posted(b"9007199254740994")).is_empty(),
        "known limit: the YAML parser rounded the bound",
    );
}
