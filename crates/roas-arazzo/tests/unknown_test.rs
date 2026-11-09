//! A member is listed as unknown exactly when the model drops it: checked
//! on every bundled description, untouched and with a stray member added
//! to each of its objects in turn.
#![cfg(all(feature = "v1_0", feature = "v1_1"))]

use roas_arazzo::unknown::UnknownMembers;
use serde_json::Value;
use std::path::Path;

/// Every Arazzo description bundled with the workspace's tests.
fn fixtures() -> Vec<(String, Value)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for directory in [
        "roas-arazzo/tests/v1_0_data",
        "roas-arazzo/tests/v1_1_data",
        "roas-arazzo-executor/tests/data",
        "roas-arazzo-executor/tests/data/external-workflows",
        "roas-cli/tests/fixtures/source-graph",
    ] {
        // Another crate's fixtures are a bonus, not a requirement.
        let Ok(entries) = std::fs::read_dir(root.join(directory)) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let Ok(value) = serde_saphyr::from_str::<Value>(&text) else {
                continue;
            };
            if value.get("arazzo").is_some() {
                found.push((path.display().to_string(), value));
            }
        }
    }
    assert!(found.len() >= 5, "{}", found.len());
    found
}

/// What the model of `value`'s version does not keep, and what it keeps;
/// `None` when `value` is no description of it.
fn unknown(value: &Value) -> Option<(UnknownMembers, Value)> {
    if value["arazzo"].as_str().unwrap().starts_with("1.0") {
        let description: roas_arazzo::v1_0::Description =
            serde_json::from_value(value.clone()).ok()?;
        Some((
            description.unknown_members(value).unwrap(),
            serde_json::to_value(description).unwrap(),
        ))
    } else {
        let description: roas_arazzo::v1_1::Description =
            serde_json::from_value(value.clone()).ok()?;
        Some((
            description.unknown_members(value).unwrap(),
            serde_json::to_value(description).unwrap(),
        ))
    }
}

/// The model path and JSON pointer of every object in `value`.
fn objects(value: &Value, path: String, pointer: String, found: &mut Vec<(String, String)>) {
    match value {
        Value::Object(members) => {
            found.push((path.clone(), pointer.clone()));
            for (name, member) in members {
                let escaped = name.replace('~', "~0").replace('/', "~1");
                objects(
                    member,
                    format!("{path}.{name}"),
                    format!("{pointer}/{escaped}"),
                    found,
                );
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                objects(
                    item,
                    format!("{path}[{index}]"),
                    format!("{pointer}/{index}"),
                    found,
                );
            }
        }
        _ => {}
    }
}

#[test]
fn a_bundled_description_has_no_unknown_members() {
    for (name, value) in fixtures() {
        let (found, _) = unknown(&value).unwrap();
        assert!(found.is_empty(), "{name}: {:?}", found.members);
        assert_eq!(found.version, value["arazzo"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn a_stray_member_is_listed_exactly_when_the_model_drops_it() {
    // A misspelt field, and an extension where none is allowed.
    for stray in ["strayMember", "x-stray"] {
        let mut listed = 0;
        let mut kept_extension = 0;
        for (name, value) in fixtures() {
            let mut paths = Vec::new();
            objects(&value, "#".into(), String::new(), &mut paths);
            for (path, pointer) in paths {
                let mut changed = value.clone();
                let Some(Value::Object(members)) = changed.pointer_mut(&pointer) else {
                    unreachable!("{pointer} is an object");
                };
                members.insert(stray.into(), Value::from(1));
                // A new entry of a map of named objects is no description.
                let Some((found, kept)) = unknown(&changed) else {
                    continue;
                };
                let member = format!("{path}.{stray}");
                if kept.pointer(&format!("{pointer}/{stray}")).is_some() {
                    // Free-form JSON, such as a schema or a payload, keeps
                    // it, as does an object that allows extensions.
                    assert!(found.is_empty(), "{name}: {member}: {:?}", found.members);
                    kept_extension += 1;
                } else {
                    let paths: Vec<_> = found.members.iter().map(|m| m.path.as_str()).collect();
                    assert_eq!(paths, [member.as_str()], "{name}");
                    assert_eq!(found.members[0].name, stray);
                    if stray.starts_with("x-") {
                        // Kept wherever extensions are allowed.
                        assert!(!found.members[0].extensions_allowed, "{name}: {member}");
                    }
                    listed += 1;
                }
            }
        }
        if stray == "x-stray" {
            // Most objects allow extensions; a Reusable Object does not.
            assert!(kept_extension > 150, "{kept_extension}");
            assert!(listed > 0, "{listed}");
        } else {
            // Most objects are model objects, not free-form JSON.
            assert!(listed > 150, "{listed}");
        }
    }
}
