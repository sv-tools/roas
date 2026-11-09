//! Preparation findings name the line and column of what they are about
//! when the description's text is supplied as a source map.

use roas_arazzo::source_map::SourceMap;
use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{Options, PreparationDiagnostic, prepare};
use serde_json::json;

const TEXT: &str = r#"arazzo: 1.1.0
info:
  title: ""
  version: "1"
sourceDescriptions:
  - name: api
    url: https://example.com/openapi.json
    type: openapi
workflows:
  - workflowId: w
    steps:
      - stepId: a
        operationId: check
        parameters:
          - name: q
            in: query
            value: "caf\u00e9 \"x\" {$statusCode.extra}"
        successCriteria:
          - condition: "$statusCode == 'caf\u00e9' && ("
        onSuccess:
          - reference: $components.successActions.shared
components:
  successActions:
    shared:
      name: shared
      type: end
      criteria:
        - condition: >-
            $statusCode ==
            200 && $steps.typo.outputs.x
"#;

fn options() -> Options {
    Options::new().source(
        "api",
        "https://example.com/openapi.json",
        json!({
            "openapi": "3.1.0", "servers": [{ "url": "https://example.com" }],
            "paths": { "/check": { "get": { "operationId": "check" } } }
        }),
    )
}

fn diagnostics(text: &str, options: &Options) -> Vec<PreparationDiagnostic> {
    let value: serde_json::Value = serde_saphyr::from_str(text).unwrap();
    let description: Description = serde_json::from_value(value).unwrap();
    prepare(&description, options).unwrap_err().diagnostics
}

fn find<'a>(diagnostics: &'a [PreparationDiagnostic], path: &str) -> &'a PreparationDiagnostic {
    diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path == path)
        .unwrap_or_else(|| panic!("no finding at {path}: {diagnostics:#?}"))
}

/// `value[offset..]` is spelled at `position` in the text: as itself, or
/// as the escape standing for it.
fn spelled_at(value: &str, offset: usize, position: usize) -> bool {
    let raw = &TEXT[position..];
    match value[offset..].chars().next() {
        Some(c) => raw.starts_with(c) || raw.starts_with('\\') || raw.starts_with('\n'),
        // The end of a value is where its closing quote or line break is.
        None => raw.starts_with(['"', '\'', '\n']),
    }
}

#[test]
fn findings_carry_lines_and_columns_through_escapes_and_components() {
    let found = diagnostics(TEXT, &options().source_map(SourceMap::parse(TEXT).unwrap()));

    let title = find(&found, "#.info.title");
    let span = title.span.expect("a model finding is located");
    assert_eq!((span.start.line, span.start.column), (3, 10));
    assert_eq!(title.position, None);
    assert!(
        title
            .to_string()
            .starts_with("#.info.title at line 3, column 10: "),
        "{title}"
    );

    for (path, value, line) in [
        (
            "#.workflows[0].steps[0].parameters[0].value",
            "café \"x\" {$statusCode.extra}",
            17,
        ),
        (
            "#.workflows[0].steps[0].successCriteria[0].condition",
            "$statusCode == 'café' && (",
            19,
        ),
        (
            "#.components.successActions.shared.criteria[0].condition",
            "$statusCode == 200 && $steps.typo.outputs.x",
            30,
        ),
    ] {
        let finding = find(&found, path);
        let offset = finding.offset.expect("a string finding has an offset");
        let position = finding.position.expect("and a position");
        assert_eq!(position.line, line, "{finding}");
        assert!(
            spelled_at(value, offset, position.offset),
            "{finding}: byte {offset} of {value:?} vs {:?}",
            &TEXT[position.offset..]
        );
        assert!(
            finding.to_string().contains(&format!("at {position}")),
            "{finding}"
        );
    }
}

#[test]
fn without_a_map_findings_keep_their_byte_offsets() {
    let found = diagnostics(TEXT, &options());
    assert!(
        found
            .iter()
            .all(|finding| finding.span.is_none() && finding.position.is_none())
    );
    let condition = find(
        &found,
        "#.workflows[0].steps[0].successCriteria[0].condition",
    );
    assert!(condition.to_string().contains(" at byte "), "{condition}");
}

#[test]
fn an_offset_the_map_cannot_place_falls_back_to_the_field() {
    // An explicit indentation indicator under a more-indented first line:
    // the field is located, the character inside it is not reconstructed,
    // and nothing is guessed.
    let text = TEXT.replace(
        "- condition: \"$statusCode == 'caf\\u00e9' && (\"",
        "- condition: |2-\n                $statusCode == 'café' &&\n              (",
    );
    assert_ne!(text, TEXT);
    let found = diagnostics(
        &text,
        &options().source_map(SourceMap::parse(text.clone()).unwrap()),
    );
    let condition = find(
        &found,
        "#.workflows[0].steps[0].successCriteria[0].condition",
    );
    let span = condition.span.expect("located");
    assert_eq!(span.start.line, 20);
    assert_eq!(condition.position, None);
    let shown = condition.to_string();
    assert!(
        shown.contains(&format!("at {}, byte ", span.start)),
        "{shown}"
    );
    assert!(shown.contains(" of the value"), "{shown}");
}
