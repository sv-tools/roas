//! Every unknown member is listed by `unknown_members` and (on extensible
//! objects) preserved across round-tripping and reported by validation:
//! checked on every bundled description, untouched and with a stray member
//! added to each of its objects in turn.
#![cfg(all(feature = "v1_0", feature = "v1_1"))]

#[path = "../src/test_yaml.rs"]
mod yaml;

use enumset::EnumSet;
use roas_arazzo::unknown::UnknownMembers;
use roas_arazzo::validation::{Validate, ValidationOptions};
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
            let Ok(value) = yaml::from_str::<Value>(&text) else {
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

/// What the model of `value`'s version considers unknown, what it round-trips,
/// and whether validation reports an unknown-field finding at `member_path`.
/// `None` when `value` is no description of it.
fn inspect(value: &Value, member_path: Option<&str>) -> Option<(UnknownMembers, Value, bool)> {
    if value["arazzo"].as_str().unwrap().starts_with("1.0") {
        let description: roas_arazzo::v1_0::Description =
            serde_json::from_value(value.clone()).ok()?;
        let found = description.unknown_members(value).unwrap();
        let kept = serde_json::to_value(&description).unwrap();
        let validated_as_unknown = member_path.is_some_and(|path| {
            description
                .validate(EnumSet::empty())
                .err()
                .is_some_and(|err| {
                    err.errors
                        .iter()
                        .any(|e| e.path == path && e.is_about_key())
                })
        });
        if validated_as_unknown {
            // IgnoreUnknownFields must suppress that finding.
            let suppressed = description
                .validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
                .err()
                .is_none_or(|err| !err.errors.iter().any(|e| e.path == member_path.unwrap()));
            assert!(
                suppressed,
                "IgnoreUnknownFields did not suppress {member_path:?}"
            );
        }
        Some((found, kept, validated_as_unknown))
    } else {
        let description: roas_arazzo::v1_1::Description =
            serde_json::from_value(value.clone()).ok()?;
        let found = description.unknown_members(value).unwrap();
        let kept = serde_json::to_value(&description).unwrap();
        let validated_as_unknown = member_path.is_some_and(|path| {
            description
                .validate(EnumSet::empty())
                .err()
                .is_some_and(|err| {
                    err.errors
                        .iter()
                        .any(|e| e.path == path && e.is_about_key())
                })
        });
        if validated_as_unknown {
            let suppressed = description
                .validate(EnumSet::only(ValidationOptions::IgnoreUnknownFields))
                .err()
                .is_none_or(|err| !err.errors.iter().any(|e| e.path == member_path.unwrap()));
            assert!(
                suppressed,
                "IgnoreUnknownFields did not suppress {member_path:?}"
            );
        }
        Some((found, kept, validated_as_unknown))
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
        let (found, _, _) = inspect(&value, None).unwrap();
        assert!(found.is_empty(), "{name}: {:?}", found.members);
        assert_eq!(found.version, value["arazzo"].as_str().unwrap(), "{name}");
    }
}

#[test]
fn a_stray_member_is_listed_and_extensible_objects_preserve_and_validate_it() {
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
                let member = format!("{path}.{stray}");
                // A new entry of a map of named objects is no description.
                let Some((found, kept, validated_as_unknown)) = inspect(&changed, Some(&member))
                else {
                    continue;
                };
                let preserved_on_round_trip = kept.pointer(&format!("{pointer}/{stray}")).is_some();
                let reusable = kept.pointer(&pointer).unwrap().get("reference").is_some();
                if found.is_empty() {
                    // Free-form JSON (such as a schema or a payload) keeps
                    // both keys, and extensible objects accept `x-stray`.
                    assert!(
                        preserved_on_round_trip,
                        "{name}: {member} not listed as unknown yet dropped"
                    );
                    assert!(
                        !validated_as_unknown,
                        "{name}: {member} not unknown yet failed validation"
                    );
                    kept_extension += 1;
                } else {
                    let paths: Vec<_> = found.members.iter().map(|m| m.path.as_str()).collect();
                    assert_eq!(paths, [member.as_str()], "{name}");
                    assert_eq!(found.members[0].name, stray);
                    // Every object but a Reusable Object allows extensions.
                    assert_eq!(
                        found.members[0].extensions_allowed, !reusable,
                        "{name}: {member}"
                    );
                    if reusable {
                        // Reusable objects drop extra keys during deserialization.
                        assert!(
                            !preserved_on_round_trip,
                            "{name}: {member} on Reusable should not serialize"
                        );
                    } else {
                        // Extensible model objects preserve unknown keys for
                        // lossless round-tripping and report them in validation.
                        assert!(
                            preserved_on_round_trip,
                            "{name}: {member} should round-trip on extensible object"
                        );
                        assert!(
                            validated_as_unknown,
                            "{name}: {member} should fail validation"
                        );
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
