use roas::validation::{IGNORE_UNUSED, Options};
use roas_codegen::{
    DiagnosticKind, Fidelity, Input, Severity, SourceDocument, SourceError, SourceVersion, validate,
};
use std::fs;
use std::path::Path;
use url::Url;

fn fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new("tests/data").join(name)).expect("fixture exists")
}

fn uri(name: &str) -> Url {
    Url::parse(&format!("file:///{name}")).unwrap()
}

/// What a fixture is held to: `roas`'s full validation minus the rules
/// about unused components and undeclared tags, which say nothing
/// about the schemas being generated.
fn lenient() -> enumset::EnumSet<Options> {
    IGNORE_UNUSED | Options::IgnoreMissingTags
}

fn parse(name: &str) -> SourceDocument {
    SourceDocument::parse(Input::Json(fixture(name)), uri(name)).expect("parses")
}

#[test]
fn every_version_parses_and_normalizes_to_3_2() {
    for (name, version) in [
        ("petstore-2.0.json", SourceVersion::V2),
        ("petstore-3.0.json", SourceVersion::V3_0),
        ("ref-siblings-3.1.json", SourceVersion::V3_1),
        ("petstore-3.2.json", SourceVersion::V3_2),
    ] {
        let document = parse(name);
        assert_eq!(document.version(), version, "{name}");
        assert_eq!(document.uri().as_str(), format!("file:///{name}"));
        assert!(document.spec().components.is_some(), "{name}");
        // The raw view is the source as given, not the converted one.
        let key = if version == SourceVersion::V2 {
            "swagger"
        } else {
            "openapi"
        };
        assert!(document.raw().get(key).is_some(), "{name}");
        let validated = validate(document, lenient()).expect("valid");
        assert!(
            validated
                .diagnostics()
                .iter()
                .all(|d| d.severity == Severity::Warning),
            "{name}: {:?}",
            validated.diagnostics()
        );
    }
}

#[test]
fn fidelity_follows_the_input() {
    let json = parse("petstore-3.2.json");
    let expected = if cfg!(feature = "exact-numbers") {
        Fidelity::Exact
    } else {
        Fidelity::F64
    };
    assert_eq!(json.fidelity(), expected);

    let yaml = SourceDocument::parse(
        Input::Yaml(fixture("petstore-3.1.yaml")),
        uri("petstore-3.1.yaml"),
    )
    .expect("yaml parses");
    assert_eq!(yaml.fidelity(), Fidelity::F64);
    assert_eq!(yaml.version(), SourceVersion::V3_1);

    let value: serde_json::Value = serde_json::from_slice(&fixture("petstore-3.2.json")).unwrap();
    let from_value =
        SourceDocument::parse(Input::Value(value), uri("value")).expect("value parses");
    assert_eq!(from_value.fidelity(), Fidelity::Unknown);
}

#[test]
fn the_2_0_conversion_reports_what_it_drops() {
    let document = parse("discriminator-2.0.json");
    let losses = document.normalization();
    let keywords: Vec<&str> = losses
        .iter()
        .map(|d| match &d.kind {
            DiagnosticKind::NormalizationLoss { keyword } => keyword.as_str(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        keywords,
        ["discriminator", "collectionFormat"],
        "{losses:?}"
    );
    assert_eq!(losses[0].schema_id.pointer, "/definitions/Pet");
    assert_eq!(losses[0].pointer, "/definitions/Pet/discriminator");
    assert_eq!(
        losses[1].pointer,
        "/paths/~1pets/get/parameters/0/collectionFormat"
    );
    assert!(losses.iter().all(|d| d.severity == Severity::Warning));
    // The converted model really has lost it.
    let pet = &document
        .spec()
        .components
        .as_ref()
        .unwrap()
        .schemas
        .as_ref()
        .unwrap()["Pet"];
    let json = serde_json::to_value(pet).unwrap();
    assert!(json.get("discriminator").is_none(), "{json}");
    // And the same document, validated, still carries the report.
    let validated = validate(document, lenient()).expect("valid");
    assert_eq!(validated.diagnostics().len(), 2);
}

#[test]
fn restricted_references_are_reported_after_validation_passes() {
    let document = parse("restricted-refs-3.2.json");
    // The external reference would make `roas` fetch; it is skipped instead.
    let validated = validate(document, lenient()).expect("valid with externals skipped");
    let diagnostics = validated.diagnostics();
    let summary: Vec<(String, &DiagnosticKind)> = diagnostics
        .iter()
        .map(|d| (d.pointer.clone(), &d.kind))
        .collect();
    assert_eq!(diagnostics.len(), 3, "{summary:?}");
    assert!(
        matches!(&diagnostics[0].kind, DiagnosticKind::AnchorReference { reference } if reference == "#Pet")
    );
    assert_eq!(
        diagnostics[0].schema_id.pointer,
        "/components/schemas/Anchor"
    );
    assert!(matches!(
        &diagnostics[1].kind,
        DiagnosticKind::ExternalReference { .. }
    ));
    assert!(matches!(
        &diagnostics[2].kind,
        DiagnosticKind::IdRebasing { .. }
    ));
    assert!(diagnostics.iter().all(|d| d.severity == Severity::Error));
    assert_eq!(
        diagnostics[1].to_string(),
        "error: file:///restricted-refs-3.2.json#/components/schemas/External/$ref: `$ref: https://example.com/schemas.json#/Pet` points outside the document; external references are not resolved yet, so this schema is not generated"
    );
}

#[test]
fn a_failed_validation_returns_the_document() {
    let document = parse("invalid-3.2.json");
    let error = validate(document, Options::empty()).expect_err("dangling reference");
    assert_eq!(error.document.version(), SourceVersion::V3_2);
    assert!(!error.errors.is_empty());
    assert!(error.to_string().contains("Missing"), "{error}");
    // The document is intact and can be validated again.
    let again = validate(*error.document, Options::empty());
    assert!(again.is_err());
}

#[test]
fn input_errors_are_specific() {
    let json = SourceDocument::parse(Input::Json(b"{".to_vec()), uri("x")).unwrap_err();
    assert!(matches!(json, SourceError::Json(_)), "{json}");
    let yaml = SourceDocument::parse(Input::Yaml(b"a: [".to_vec()), uri("x")).unwrap_err();
    assert!(matches!(yaml, SourceError::Yaml(_)), "{yaml}");
    let array = SourceDocument::parse(Input::Json(b"[]".to_vec()), uri("x")).unwrap_err();
    assert!(matches!(array, SourceError::NotAnObject), "{array}");
    let none = SourceDocument::parse(Input::Json(b"{}".to_vec()), uri("x")).unwrap_err();
    assert!(matches!(none, SourceError::NoVersion), "{none}");
    let unknown = SourceDocument::parse(Input::Json(br#"{"openapi": "4.0.0"}"#.to_vec()), uri("x"))
        .unwrap_err();
    assert!(
        matches!(
            unknown,
            SourceError::UnknownVersion {
                field: "openapi",
                ..
            }
        ),
        "{unknown}"
    );
    let swagger =
        SourceDocument::parse(Input::Json(br#"{"swagger": 2}"#.to_vec()), uri("x")).unwrap_err();
    assert!(
        matches!(
            swagger,
            SourceError::UnknownVersion {
                field: "swagger",
                ..
            }
        ),
        "{swagger}"
    );
    let bad = SourceDocument::parse(
        Input::Json(br#"{"openapi": "3.2.0", "info": 5}"#.to_vec()),
        uri("x"),
    )
    .unwrap_err();
    assert!(
        matches!(
            bad,
            SourceError::Parse {
                version: SourceVersion::V3_2,
                ..
            }
        ),
        "{bad}"
    );
    assert!(bad.to_string().contains("OpenAPI 3.2"));
}

#[test]
fn sibling_keywords_on_references_survive_normalization() {
    let document = parse("ref-siblings-3.1.json");
    let pet = &document
        .spec()
        .components
        .as_ref()
        .unwrap()
        .schemas
        .as_ref()
        .unwrap()["Pet"];
    let json = serde_json::to_value(pet).unwrap();
    assert_eq!(json["properties"]["id"]["maxLength"], 32);
    assert_eq!(json["properties"]["id"]["x-primary"], true);
}
