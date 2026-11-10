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

    // (path, the decoded byte the finding points at, where it is written)
    for (path, offset, line, column) in [
        // `.extra`, after the `\u00e9` and `\"` escapes.
        ("#.workflows[0].steps[0].parameters[0].value", 22, 17, 49),
        // The end of the value: its closing quote.
        (
            "#.workflows[0].steps[0].successCriteria[0].condition",
            27,
            19,
            56,
        ),
        // `$steps`, on the second line of a folded block.
        (
            "#.components.successActions.shared.criteria[0].condition",
            22,
            30,
            20,
        ),
    ] {
        let finding = find(&found, path);
        assert_eq!(finding.offset, Some(offset), "{finding}");
        let position = finding.position.expect("a position");
        assert_eq!(
            (position.line, position.column),
            (line, column),
            "{finding}"
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

#[test]
fn a_path_two_members_could_spell_is_not_located() {
    // `payload.a.b` is both `a` → `b` and the member `a.b`.
    let text = TEXT.replace(
        "        operationId: check\n",
        "        operationId: check\n        requestBody:\n          payload:\n            a:\n              b: \"plain text here\"\n            a.b: \"x {$inputs.} y\"\n",
    );
    assert_ne!(text, TEXT);
    let found = diagnostics(
        &text,
        &options().source_map(SourceMap::parse(text.clone()).unwrap()),
    );
    let payload = find(&found, "#.workflows[0].steps[0].requestBody.payload.a.b");
    assert_eq!((payload.span, payload.position), (None, None), "{payload}");
    assert!(payload.to_string().contains(" at byte "), "{payload}");
}

#[test]
fn a_finding_about_the_callers_options_is_not_placed_in_the_document() {
    // A member that happens to spell the same path as the option.
    let text = format!("{TEXT}options:\n  inputs: whatever\n");
    let map = SourceMap::parse(text.clone()).unwrap();
    let found = diagnostics(&text, &options().source_map(map).inputs(json!(1)));
    let inputs = find(&found, "options.inputs");
    assert_eq!((inputs.span, inputs.position), (None, None), "{inputs}");
}

#[test]
fn a_finding_about_a_members_name_is_placed_at_the_name() {
    let text = TEXT.replace(
        "components:\n",
        "components:\n  inputs:\n    bad key:\n      type: object\n",
    );
    assert_ne!(text, TEXT);
    let found = diagnostics(
        &text,
        &options().source_map(SourceMap::parse(text.clone()).unwrap()),
    );
    let key = find(&found, "#.components.inputs.bad key");
    let span = key.span.expect("located");
    assert_eq!((span.start.line, span.start.column), (24, 5), "{key}");
}

#[test]
fn an_unknown_member_a_merge_brings_in_is_located_at_the_merge() {
    let text = format!(
        "x-e: &e {{timeoutMs: 5}}\n{}",
        TEXT.replace(
            "        operationId: check\n",
            "        operationId: check\n        <<: *e\n",
        )
    );
    let value: serde_json::Value = serde_saphyr::from_str(&text).unwrap();
    let description: Description = serde_json::from_value(value.clone()).unwrap();
    let unknown = description.unknown_members(&value).unwrap();
    let options = options()
        .source_map(SourceMap::parse(text.clone()).unwrap())
        .unknown_members(unknown);
    let found = prepare(&description, &options).unwrap_err().diagnostics;
    let member = find(&found, "#.workflows[0].steps[0].timeoutMs");
    // At the `*e` in the step, not at the anchor on line 1.
    let span = member.span.expect("the name is located");
    assert_eq!((span.start.line, span.start.column), (15, 13));
}

#[test]
fn an_unknown_member_is_an_error_located_at_its_name() {
    let text = TEXT.replace(
        "        operationId: check\n",
        "        operationId: check\n        timeoutMs: 5\n",
    );
    let value: serde_json::Value = serde_saphyr::from_str(&text).unwrap();
    let description: Description = serde_json::from_value(value.clone()).unwrap();
    let unknown = description.unknown_members(&value).unwrap();
    let options = options()
        .source_map(SourceMap::parse(text.clone()).unwrap())
        .unknown_members(unknown);
    let found = prepare(&description, &options).unwrap_err().diagnostics;
    let member = find(&found, "#.workflows[0].steps[0].timeoutMs");
    assert!(member.is_error());
    assert_eq!(member.step_id.as_deref(), Some("a"));
    let span = member.span.expect("the name is located");
    assert_eq!((span.start.line, span.start.column), (14, 9));
    assert_eq!(
        member.to_string(),
        "#.workflows[0].steps[0].timeoutMs at line 14, column 9: `timeoutMs` is not a field \
         of Arazzo 1.1.0; an extension's name starts with `x-`"
    );

    // Unless the caller says what was dropped, nothing is known to be.
    let found = diagnostics(&text, &self::options());
    assert!(
        found
            .iter()
            .all(|finding| !finding.path.ends_with("timeoutMs")),
        "{found:#?}"
    );
}
