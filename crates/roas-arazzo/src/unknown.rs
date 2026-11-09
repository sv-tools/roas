//! Members of an Arazzo description that its model does not know.
//!
//! An Arazzo object holds the fields its specification defines and
//! `x-`-prefixed extensions. Deserializing keeps both and drops anything
//! else without a word, so a misspelt `sucessCriteria` simply vanishes.
//! [`v1_0::Description::unknown_members`](crate::v1_0::Description::unknown_members)
//! and
//! [`v1_1::Description::unknown_members`](crate::v1_1::Description::unknown_members)
//! list what was dropped, by comparing the value a description was
//! deserialized from with the description serialized again.
//!
//! A member whose value is `null`, `[]` or `{}` is not listed: a known
//! optional field left empty is dropped the same way, so the two cannot be
//! told apart, and an empty member carries nothing that was lost.

use serde_json::Value;

/// A member of an Arazzo object that is neither one of its fields nor an
/// `x-` extension.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnknownMember {
    /// Model-style path of the member itself, as validation findings name
    /// fields: `#.workflows[0].steps[1].sucessCriteria`.
    pub path: String,
    /// The member's name as written.
    pub name: String,
}

/// What a description's model did not keep of the value it was read from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnknownMembers {
    /// The `arazzo` version the value declares, as written: a v1.0
    /// description keeps it here when it is upconverted to v1.1. Empty when
    /// the value has no string `arazzo` member.
    pub version: String,
    /// The unknown members, in document order.
    pub members: Vec<UnknownMember>,
}

impl UnknownMembers {
    /// Whether nothing was dropped.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// The members of `raw` that `known` — the model `raw` was deserialized
/// into, serialized again — does not have.
pub(crate) fn diff(raw: &Value, known: &Value) -> UnknownMembers {
    let mut members = Vec::new();
    // Depth first, in document order: children are pushed in reverse.
    let mut pending = vec![("#".to_owned(), raw, known)];
    while let Some((path, raw, known)) = pending.pop() {
        match (raw, known) {
            (Value::Object(raw), Value::Object(known)) => {
                let mut children = Vec::new();
                for (name, value) in raw {
                    let member = format!("{path}.{name}");
                    match known.get(name) {
                        Some(kept) => children.push((member, value, kept)),
                        None if name.starts_with("x-") || is_empty(value) => {}
                        None => members.push(UnknownMember {
                            path: member,
                            name: name.clone(),
                        }),
                    }
                }
                pending.extend(children.into_iter().rev());
            }
            (Value::Array(raw), Value::Array(known)) => pending.extend(
                raw.iter()
                    .zip(known)
                    .enumerate()
                    .rev()
                    .map(|(index, (raw, known))| (format!("{path}[{index}]"), raw, known)),
            ),
            _ => {}
        }
    }
    UnknownMembers {
        version: raw
            .get("arazzo")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        members,
    }
}

/// A value a known field left empty serializes to nothing.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(found: &UnknownMembers) -> Vec<(&str, &str)> {
        found
            .members
            .iter()
            .map(|member| (member.path.as_str(), member.name.as_str()))
            .collect()
    }

    #[test]
    fn members_the_model_dropped_are_listed_in_document_order() {
        let raw = json!({
            "arazzo": "1.0.1",
            "a": { "kept": 1, "lost": 2, "x-ext": 3 },
            "b": [{ "kept": 1 }, { "kept": 1, "typo": true }],
            "c": "scalar",
            "empty": null, "none": [], "nothing": {}
        });
        let known = json!({
            "arazzo": "1.0.1",
            "a": { "kept": 1, "x-ext": 3 },
            "b": [{ "kept": 1 }, { "kept": 1 }],
            "c": "scalar"
        });
        let found = diff(&raw, &known);
        assert_eq!(found.version, "1.0.1");
        assert_eq!(
            names(&found),
            [("#.a.lost", "lost"), ("#.b[1].typo", "typo")]
        );
        assert!(!found.is_empty());
    }

    #[test]
    fn a_value_kept_as_written_has_nothing_unknown() {
        let raw = json!({ "schema": { "anything": [1, { "deep": null }] } });
        let found = diff(&raw, &raw);
        assert!(found.is_empty());
        assert_eq!(found.version, "");
    }

    #[test]
    fn a_deep_value_is_walked_without_recursion() {
        let mut raw = json!(1);
        for _ in 0..100_000 {
            raw = Value::Array(vec![raw]);
        }
        assert!(diff(&raw, &raw).is_empty());
        // Dropping a deep `Value` recursively would overflow the stack.
        while let Value::Array(items) = &mut raw {
            let Some(inner) = items.pop() else { break };
            raw = inner;
        }
    }
}
