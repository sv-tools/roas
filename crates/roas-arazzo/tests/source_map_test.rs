//! Every string in the bundled fixtures can be located character by
//! character, so the source map's decoding agrees with the parser on
//! real documents, not only on hand-picked scalars.
#![cfg(feature = "source-map")]

use roas_arazzo::source_map::SourceMap;
use serde_json::Value;
use std::path::Path;

fn walk(value: &Value, path: String, strings: &mut Vec<(String, String)>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                // A dotted or bracketed name is ambiguous in a model path.
                if !name.contains(['.', '[']) {
                    walk(member, format!("{path}.{name}"), strings);
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(item, format!("{path}[{index}]"), strings);
            }
        }
        Value::String(text) => strings.push((path, text.clone())),
        _ => {}
    }
}

#[test]
fn every_fixture_string_maps_back_to_its_spelling() {
    let mut checked = 0;
    for directory in ["tests/v1_0_data", "tests/v1_1_data"] {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join(directory);
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let value: Value = serde_saphyr::from_str(&text).unwrap();
            let map = SourceMap::parse(text.clone()).unwrap();
            let mut strings = Vec::new();
            walk(&value, "#".into(), &mut strings);
            for (field, decoded) in strings {
                let range = map
                    .locate(&field)
                    .unwrap_or_else(|| panic!("{}: {field}", path.display()));
                assert!(range.start.offset <= range.end.offset);
                for (at, c) in decoded.char_indices() {
                    let position = map
                        .locate_in(&field, at)
                        .unwrap_or_else(|| panic!("{}: {field} at {at}", path.display()));
                    let raw = &text[position.offset..];
                    // Spelled as itself, as an escape, or a folded line break.
                    assert!(
                        raw.starts_with(c)
                            || raw.starts_with('\\')
                            || (c == ' ' && raw.starts_with(['\n', '\r']))
                            || (c == '\n' && raw.starts_with(['\n', '\r'])),
                        "{}: {field} at {at}: {c:?} vs {:?}",
                        path.display(),
                        &raw[..raw.len().min(8)]
                    );
                    checked += 1;
                }
                assert!(map.locate_in(&field, decoded.len()).is_some());
            }
        }
    }
    assert!(checked > 1000, "{checked}");
}
