use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{Options, execute, prepare, testing::Fake};
use serde_json::{Value, json};

const API: &str = "https://example.test/spec/openapi.json";

fn description(selector: Value) -> Description {
    let mut step = selector;
    step["stepId"] = json!("s");
    serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"Operations", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":API, "type":"openapi"}],
        "workflows":[{"workflowId":"w", "steps":[step]}]
    }))
    .unwrap()
}

fn by_id() -> Description {
    description(json!({"operationId":"$sourceDescriptions.api.check"}))
}

#[test]
fn bare_ids_ignore_supplied_non_openapi_documents() {
    let bare = description(json!({"operationId":"check"}));
    for other in [json!({"arazzo":"1.1.0"}), json!({"asyncapi":"3.0.0"})] {
        // Cover non-OpenAPI documents before and after the API in lookup order.
        for name in ["aaa", "zzz"] {
            let options = Options::new().source("api", API, api()).source(
                name,
                "https://example.test/other.json",
                other.clone(),
            );
            agree(&bare, &options, "GET", "https://example.test/v1/pets");

            let ambiguous =
                options
                    .clone()
                    .source("second", "https://example.test/second.json", api());
            assert!(
                prepare(&bare, &ambiguous)
                    .unwrap_err()
                    .to_string()
                    .contains("more than one")
            );
        }
        // An explicitly selected non-OpenAPI source is still a document error,
        // while a bare ID with no OpenAPI candidates is simply not found.
        let options = Options::new().source("api", API, other);
        assert!(
            prepare(&by_id(), &options)
                .unwrap_err()
                .to_string()
                .contains("not an OpenAPI")
        );
        assert!(
            prepare(&bare, &options)
                .unwrap_err()
                .to_string()
                .contains("in none")
        );
        let mut fake = Fake::new();
        assert!(
            execute(&bare, &options, &mut fake)
                .unwrap_err()
                .to_string()
                .contains("in none")
        );
        assert!(fake.sent().is_empty());
    }
}

#[test]
fn bare_ids_keep_invalid_openapi_sources_loud() {
    let bare = description(json!({"operationId":"check"}));
    for invalid in [
        json!({"openapi":"9.0.0"}),
        json!({"openapi":"3.1.0", "paths":false}),
        json!({"openapi":"3.1.0", "arazzo":"1.1.0"}),
        json!({"swagger":"2.0", "asyncapi":"3.0.0"}),
        json!({"openapi":"3.1.0", "paths":{"/bad":{"$ref":"#/missing"}}}),
    ] {
        let options = Options::new().source("api", API, api()).source(
            "invalid",
            "https://example.test/bad.json",
            invalid,
        );
        assert!(prepare(&bare, &options).is_err());
        let mut fake = Fake::new();
        assert!(execute(&bare, &options, &mut fake).is_err());
        assert!(fake.sent().is_empty());
    }
    // Existing unversioned OpenAPI compatibility must not be filtered out.
    let mut legacy = api();
    legacy.as_object_mut().unwrap().remove("openapi");
    agree(
        &bare,
        &Options::new().source("api", API, legacy),
        "GET",
        "https://example.test/v1/pets",
    );
}

fn api() -> Value {
    json!({"openapi":"3.1.0", "servers":[{"url":"/v1"}], "paths":{"/pets":{"get":{"operationId":"check"}}}})
}

fn agree(description: &Description, options: &Options, method: &str, url: &str) {
    let mut lazy = Fake::new().reply(200, &json!({}));
    let mut lazy_report = execute(description, options, &mut lazy).unwrap();
    let mut checked = Fake::new().reply(200, &json!({}));
    let mut checked_report = prepare(description, options)
        .unwrap()
        .execute(&mut checked)
        .unwrap();
    for step in lazy_report
        .steps
        .iter_mut()
        .chain(&mut checked_report.steps)
    {
        step.elapsed = std::time::Duration::ZERO;
    }
    assert_eq!(lazy_report, checked_report);
    assert!(checked_report.is_success());
    assert_eq!(lazy.sent(), checked.sent());
    assert_eq!(checked.sent()[0].method, method);
    assert_eq!(checked.sent()[0].url, url);
}

#[test]
fn inline_and_local_path_item_references_agree_across_supported_versions() {
    for version in ["2.0", "3.0.4", "3.1.1", "3.2.0"] {
        for referenced in [false, true] {
            let mut value = api();
            if version == "2.0" {
                value.as_object_mut().unwrap().remove("openapi");
                value["swagger"] = json!(version);
                value["host"] = json!("example.test");
                value["schemes"] = json!(["https"]);
                value["basePath"] = json!("/v1");
            } else {
                value["openapi"] = json!(version);
            }
            if referenced {
                value["definitions"] = json!({"petPath": value["paths"]["/pets"].clone()});
                value["paths"]["/pets"] = json!({"$ref":"#/definitions/petPath"});
            }
            let options = Options::new().source("api", API, value);
            agree(&by_id(), &options, "GET", "https://example.test/v1/pets");
            agree(
                &description(
                    json!({"operationPath":"{$sourceDescriptions.api.url}#/paths/~1pets/get"}),
                ),
                &options,
                "GET",
                "https://example.test/v1/pets",
            );
        }
    }
}

#[test]
fn relative_servers_defaults_variables_and_precedence() {
    for (server, expected) in [
        (json!([{"url":"/v1"}]), "https://example.test/v1/pets"),
        (json!([{"url":"../v2"}]), "https://example.test/v2/pets"),
        (json!([{"url":"."}]), "https://example.test/spec/pets"),
        (
            json!([{"url":"//other.test/v1"}]),
            "https://other.test/v1/pets",
        ),
        (json!([]), "https://example.test/pets"),
        (
            json!([{"url":"/{version}","variables":{"version":{"default":"v4"}}}]),
            "https://example.test/v4/pets",
        ),
    ] {
        let mut value = api();
        value["servers"] = server;
        agree(
            &by_id(),
            &Options::new().source("api", API, value),
            "GET",
            expected,
        );
    }
    let mut value = api();
    value.as_object_mut().unwrap().remove("servers");
    agree(
        &by_id(),
        &Options::new().source("api", API, value.clone()),
        "GET",
        "https://example.test/pets",
    );
    value["paths"]["/pets"]["servers"] = json!([{"url":"/path"}]);
    // Retained executor policy: an empty operation array falls through.
    value["paths"]["/pets"]["get"]["servers"] = json!([]);
    agree(
        &by_id(),
        &Options::new().source("api", API, value.clone()),
        "GET",
        "https://example.test/path/pets",
    );
    value["paths"]["/pets"]["get"]["servers"] = json!([{"url":"/operation"}]);
    agree(
        &by_id(),
        &Options::new().source("api", API, value.clone()),
        "GET",
        "https://example.test/operation/pets",
    );
    agree(
        &by_id(),
        &Options::new()
            .source("api", API, value)
            .base_url("api", "https://override.test/base/"),
        "GET",
        "https://override.test/base/pets",
    );
}

#[test]
fn swagger_operation_scheme_and_retrieval_host_defaults() {
    let value = json!({"swagger":"2.0", "basePath":"/v2", "paths":{"/pets":{"get":{"operationId":"check", "schemes":["http"]}}}});
    agree(
        &by_id(),
        &Options::new().source("api", "https://example.test:8443/spec.json", value),
        "GET",
        "http://example.test:8443/v2/pets",
    );
}

#[test]
fn path_fragment_decoding_and_canonical_literal_urls() {
    let mut value = api();
    value["paths"] = json!({"/café~x":{"get":{"operationId":"check"}}});
    let options = Options::new().source("api", API, value);
    for path in [
        "{$sourceDescriptions.api.url}#%2Fpaths%2F~1caf%C3%A9~0x%2Fget",
        "https://EXAMPLE.test:443/spec/./openapi.json#/paths/~1caf%C3%A9~0x/get",
    ] {
        agree(
            &description(json!({"operationPath":path})),
            &options,
            "GET",
            "https://example.test/v1/café~x",
        );
    }
    for fragment in [
        "%",
        "%GG",
        "%FF",
        "/paths/~2/get",
        "anchor",
        "/paths/~1missing/get",
    ] {
        let description = description(
            json!({"operationPath":format!("{{$sourceDescriptions.api.url}}#{fragment}")}),
        );
        assert!(prepare(&description, &options).is_err(), "{fragment}");
        let mut fake = Fake::new();
        assert!(execute(&description, &options, &mut fake).is_err());
        assert!(fake.sent().is_empty());
    }
}

#[test]
fn broken_circular_and_overlapping_refs_and_duplicate_ids_fail_before_requests() {
    for (item, definitions, message) in [
        (json!({"$ref":"#/missing"}), json!({}), "missing"),
        (
            json!({"$ref":"#/definitions/a"}),
            json!({"a":{"$ref":"#/definitions/b"},"b":{"$ref":"#/definitions/a"}}),
            "circular",
        ),
        (
            json!({"$ref":"#/definitions/a", "get":{"operationId":"check"}}),
            json!({"a":{"get":{"operationId":"other"}}}),
            "overlapping",
        ),
        (json!({"$ref":42}), json!({}), "URI string"),
        (
            json!({"$ref":"#/definitions/a"}),
            json!({"a":false}),
            "not an object",
        ),
        (
            json!({"get":{"operationId":"check"},"post":{"operationId":"check"}}),
            json!({}),
            "duplicated",
        ),
        (
            json!({"get":false}),
            json!({}),
            "operation must be an object",
        ),
        (
            json!({"get":{"operationId":42}}),
            json!({}),
            "operationId must be a string",
        ),
    ] {
        let mut value = api();
        value["paths"]["/pets"] = item;
        value["definitions"] = definitions;
        let options = Options::new().source("api", API, value);
        assert!(
            prepare(&by_id(), &options)
                .unwrap_err()
                .to_string()
                .contains(message)
        );
        let mut fake = Fake::new();
        assert!(
            execute(&by_id(), &options, &mut fake)
                .unwrap_err()
                .to_string()
                .contains(message)
        );
        assert!(fake.sent().is_empty());
    }
}

#[test]
fn invalid_servers_explain_missing_bases_and_bad_templates() {
    for (location, servers, expected) in [
        ("relative.json", json!([{"url":"/v1"}]), "no retrieval base"),
        ("file:///tmp/spec.json", json!([]), "HTTP(S) origin"),
        (API, json!([{"url":"/{missing}"}]), "no string default"),
        (API, json!([{"url":"/{missing"}]), "unclosed"),
        (
            API,
            json!([{"url":"https://example.test/?a=1"}]),
            "query strings",
        ),
        (API, json!([{"url":"https://example.test/#x"}]), "fragments"),
        (API, json!([{}]), "server.url"),
        (API, json!({}), "servers must be an array"),
        (API, json!([{"url":"http://["}]), "invalid"),
    ] {
        let mut value = api();
        value["servers"] = servers;
        let options = Options::new().source("api", location, value);
        assert!(
            prepare(&by_id(), &options)
                .unwrap_err()
                .to_string()
                .contains(expected),
            "{expected}"
        );
        agree(
            &by_id(),
            &options.base_url("api", "https://override.test"),
            "GET",
            "https://override.test/pets",
        );
    }
}

#[test]
fn base_overrides_keep_absolute_http_validation_and_normalization() {
    for (base, expected) in [
        ("/relative", "base URL override"),
        ("http://[", "base URL override"),
        ("file:///tmp/api", "HTTP(S) origin"),
        ("https://override.test/?q=1", "query strings"),
        ("https://override.test/#fragment", "fragments"),
    ] {
        let options = Options::new()
            .source("api", API, api())
            .base_url("api", base);
        assert!(
            prepare(&by_id(), &options)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        let mut fake = Fake::new();
        assert!(
            execute(&by_id(), &options, &mut fake)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert!(fake.sent().is_empty());
    }
    agree(
        &by_id(),
        &Options::new()
            .source("api", API, api())
            .base_url("api", "https://OVERRIDE.test:443/v1/"),
        "GET",
        "https://override.test/v1/pets",
    );
}

#[test]
fn path_item_annotation_overlaps_follow_the_documented_rejection_policy() {
    for field in ["summary", "description", "x-owner"] {
        let mut value = api();
        value["components"] = json!({"pathItems":{"pet":value["paths"]["/pets"].clone()}});
        value["components"]["pathItems"]["pet"][field] = json!("shared");
        value["paths"]["/pets"] = json!({"$ref":"#/components/pathItems/pet", field:"local"});
        let options = Options::new().source("api", API, value);
        let expected = format!("overlapping Path Item field `{field}`");
        assert!(
            prepare(&by_id(), &options)
                .unwrap_err()
                .to_string()
                .contains(&expected)
        );
        let mut fake = Fake::new();
        assert!(
            execute(&by_id(), &options, &mut fake)
                .unwrap_err()
                .to_string()
                .contains(&expected)
        );
        assert!(fake.sent().is_empty());
    }
}

#[test]
fn openapi_32_query_and_additional_methods_and_unsupported_versions() {
    let mut value = api();
    value["openapi"] = json!("3.2.0");
    value["paths"]["/pets"] = json!({"query":{"operationId":"query"},"additionalOperations":{"PROPFIND":{"operationId":"props"}}});
    let options = Options::new().source("api", API, value);
    for (id, method) in [("query", "QUERY"), ("props", "PROPFIND")] {
        agree(
            &description(json!({"operationId":format!("$sourceDescriptions.api.{id}")})),
            &options,
            method,
            "https://example.test/v1/pets",
        );
    }
    agree(
        &description(
            json!({"operationPath":"{$sourceDescriptions.api.url}#/paths/~1pets/additionalOperations/PROPFIND"}),
        ),
        &options,
        "PROPFIND",
        "https://example.test/v1/pets",
    );
    for version in [json!("3.3.0"), json!("3.1"), json!("3.1.x"), json!(42)] {
        let mut value = api();
        value["openapi"] = version;
        assert!(prepare(&by_id(), &Options::new().source("api", API, value)).is_err());
    }
}

#[test]
fn local_refs_without_absolute_document_uri_and_nonoverlapping_siblings() {
    let value = json!({"paths":{"/pets":{"$ref":"#/shared", "get":{"operationId":"check"}}}, "shared":{"servers":[{"url":"https://example.test/v1"}]}});
    agree(
        &by_id(),
        &Options::new().source("api", "relative.json", value),
        "GET",
        "https://example.test/v1/pets",
    );
}

#[test]
fn legacy_supplied_external_documents_are_borrowed_and_conflicting_locations_fail() {
    let mut value = api();
    value["paths"]["/pets"] = json!({"$ref":"/parts.json#/item"});
    let part = json!({"item":{"get":{"operationId":"check"}}});
    let options = Options::new()
        .source("api", API, value)
        .source("parts", "https://example.test/parts.json", part.clone())
        .source(
            "unrelated",
            "https://unrelated.test/api.json",
            json!({"asyncapi":"3.0.0"}),
        );
    agree(&by_id(), &options, "GET", "https://example.test/v1/pets");
    let options = options.source(
        "conflict",
        "https://example.test/parts.json",
        json!({"item":{"post":{}}}),
    );
    assert!(
        prepare(&by_id(), &options)
            .unwrap_err()
            .to_string()
            .contains("multiple supplied documents")
    );
}

#[test]
fn pointer_to_a_reusable_operation_requires_a_unique_mount() {
    let mut value = api();
    value["paths"]["/pets"] = json!({"$ref":"#/components/pathItems/Item"});
    value["components"] = json!({"pathItems":{"Item":{"get":{}}}});
    let description = description(
        json!({"operationPath":"{$sourceDescriptions.api.url}#/components/pathItems/Item/get"}),
    );
    agree(
        &description,
        &Options::new().source("api", API, value.clone()),
        "GET",
        "https://example.test/v1/pets",
    );
    value["paths"]["/other"] = value["paths"]["/pets"].clone();
    assert!(
        prepare(&description, &Options::new().source("api", API, value))
            .unwrap_err()
            .to_string()
            .contains("mounted at more than one path")
    );
}

#[test]
fn malformed_documents_and_additional_operations_are_rejected() {
    for method in ["GET", "POST", "QUERY"] {
        let value = json!({"openapi":"3.2.0","paths":{"/pets":{
            "additionalOperations":{method:{"operationId":"check"}}
        }}});
        let error = prepare(&by_id(), &Options::new().source("api", API, value))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("must use its fixed Path Item field"),
            "{error}"
        );
    }
    for value in [
        json!({"swagger":"1.2"}),
        json!({"swagger":"2.0","openapi":"3.1.0"}),
        json!({"arazzo":"1.1.0"}),
        json!({"asyncapi":"3.0.0"}),
        json!({"openapi":"3.2.0","$self":42}),
        json!({"openapi":"3.2.0","$self":"https://example.test/#x"}),
        json!({"openapi":"3.2.0","paths":false}),
        json!({"openapi":"3.2.0","paths":{"no-slash":{}}}),
        json!({"openapi":"3.2.0","paths":{"/pets":{"additionalOperations":false}}}),
        json!({"openapi":"3.2.0","paths":{"/pets":{"additionalOperations":{"BAD METHOD":{}}}}}),
        json!({"openapi":"3.2.0","paths":{"/pets":{"get":{},"additionalOperations":{"GET":{}}}}}),
    ] {
        assert!(prepare(&by_id(), &Options::new().source("api", API, value)).is_err());
    }
    let mut value = api();
    value["openapi"] = json!("3.2.0");
    value["$self"] = json!("relative.json");
    assert!(
        prepare(
            &by_id(),
            &Options::new().source("api", "relative.json", value.clone())
        )
        .is_err()
    );
    value["$self"] = json!("https://identity.test/api.json");
    value["servers"] = json!([{"url":"https://endpoint.test"}]);
    agree(
        &by_id(),
        &Options::new().source("api", "relative.json", value),
        "GET",
        "https://endpoint.test/pets",
    );
    let mut value = api();
    value["paths"]["/pets"] = json!({"$ref":"elsewhere.json#/item"});
    assert!(
        prepare(
            &by_id(),
            &Options::new().source("api", "relative.json", value)
        )
        .unwrap_err()
        .to_string()
        .contains("reference base")
    );
}

#[test]
fn duplicate_mounts_across_aliases_and_malformed_server_defaults_are_not_guessed() {
    let value = api();
    let options = Options::new()
        .source("api", API, value.clone())
        .source("same", API, value);
    let path = description(json!({"operationPath":format!("{API}#/paths/~1pets/get")}));
    assert!(
        prepare(&path, &options)
            .unwrap_err()
            .to_string()
            .contains("more than one source")
    );
    for server in [
        json!({"url":"/{v}", "variables":{"v":{"default":42}}}),
        json!({"url":"/{v}", "variables":{"v":{"default":"{nested}"}}}),
        json!({"url":"/stray}"}),
    ] {
        let mut value = api();
        value["servers"] = json!([server]);
        assert!(prepare(&by_id(), &Options::new().source("api", API, value)).is_err());
    }
}
