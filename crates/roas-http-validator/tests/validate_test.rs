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
            allowed: vec!["get".to_owned(), "post".to_owned()],
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
    assert_eq!(report.method, "get");
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
        ["body: at /name: is required and was not sent"],
    );
    assert_eq!(
        errors(&validator, &posting(br#"{"name":7}"#)),
        ["body: at /name: expected string, got integer"],
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
        ["body: at /a: is required and was not sent"]
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
        ["querystring parameter \"params\": at /a: is required and was not sent"],
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
