//! Following a `$ref` inside an input JSON Schema.
//!
//! Workflow inputs are JSON Schema, kept as plain JSON, and commonly
//! name a shared schema with `{ "$ref": "#/components/inputs/<name>" }`
//! — or a schema below one, `#/components/inputs/<name>/properties/id`.
//! This follows such a reference, and a chain of them, inside
//! `components.inputs`. The same job `RefOr::get_item` and `resolved_ref`
//! do for schemas in the roas and roas-asyncapi crates.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Where input schema components live.
const PREFIX: &str = "#/components/inputs/";

/// Follow `reference` to the schema at the end of its chain, reporting
/// the last reference reached as well.
///
/// The schema is `None` when a pointer names nothing, the chain loops,
/// or it leaves `#/components/inputs` (another document, or another
/// part of this one); the reference is then the last one reached.
pub(crate) fn follow<'a, 'r>(
    inputs: &'a BTreeMap<String, Value>,
    reference: &'r str,
) -> (Option<&'a Value>, &'r str)
where
    'a: 'r,
{
    let mut current = reference;
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    loop {
        let Some(target) = target(inputs, current) else {
            return (None, current);
        };
        match target.get("$ref").and_then(Value::as_str) {
            None => return (Some(target), current),
            Some(next) => {
                if !visited.insert(next) {
                    return (None, current);
                }
                current = next;
            }
        }
    }
}

/// The schema one reference names, without following any `$ref` there.
fn target<'a>(inputs: &'a BTreeMap<String, Value>, reference: &str) -> Option<&'a Value> {
    let rest = reference.strip_prefix(PREFIX)?;
    let decoded = percent_decode(rest)?;
    let mut tokens = decoded.split('/').map(unescape);
    let mut value = inputs.get(&tokens.next()?)?;
    for token in tokens {
        value = match value {
            Value::Object(map) => map.get(&token)?,
            Value::Array(list) => list.get(index(&token)?)?,
            _ => return None,
        };
    }
    Some(value)
}

/// An RFC 6901 reference token, unescaped.
fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// An array index token: digits only, without a leading zero.
fn index(token: &str) -> Option<usize> {
    if token.is_empty()
        || !token.bytes().all(|b| b.is_ascii_digit())
        || (token.len() > 1 && token.starts_with('0'))
    {
        return None;
    }
    token.parse().ok()
}

/// A URI fragment, percent-decoded; `None` for a malformed escape.
fn percent_decode(fragment: &str) -> Option<String> {
    let bytes = fragment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = fragment.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Give a version's `Components` and `Description` the input-schema
/// resolver.
#[cfg(any(feature = "v1_0", feature = "v1_1"))]
macro_rules! impl_input_schema_refs {
    ($version:ident) => {
        impl $crate::$version::Components {
            /// The input schema a `$ref` names: `#/components/inputs/<name>`,
            /// or any schema below one (`…/properties/<name>`, `…/items`),
            /// following a chain of such `$ref`s to its end.
            ///
            /// `None` when a pointer names nothing, the chain loops, or it
            /// leaves `#/components/inputs`. A `$ref` beside other keywords
            /// is followed too; the keywords stay where they are written.
            ///
            /// ```
            #[doc = concat!("use roas_arazzo::", stringify!($version), "::Components;")]
            /// use serde_json::json;
            ///
            /// let components: Components = serde_json::from_value(json!({
            ///     "inputs": {
            ///         "pet": { "type": "object", "properties": { "id": { "type": "integer" } } },
            ///         "petId": { "$ref": "#/components/inputs/pet/properties/id" }
            ///     }
            /// })).unwrap();
            ///
            /// let id = components.resolve_input_ref("#/components/inputs/petId").unwrap();
            /// assert_eq!(id["type"], "integer");
            /// assert_eq!(
            ///     components.resolved_input_ref("#/components/inputs/petId"),
            ///     "#/components/inputs/pet/properties/id",
            /// );
            /// ```
            #[must_use]
            pub fn resolve_input_ref(&self, reference: &str) -> Option<&serde_json::Value> {
                $crate::common::input_schema::follow(&self.inputs, reference).0
            }

            /// The last `$ref` [`resolve_input_ref`](Self::resolve_input_ref)
            /// follows from `reference`: the target of an alias, or, when
            /// the chain dangles, loops, or leaves `#/components/inputs`,
            /// the last reference it reached.
            #[must_use]
            pub fn resolved_input_ref<'a>(&'a self, reference: &'a str) -> &'a str {
                $crate::common::input_schema::follow(&self.inputs, reference).1
            }
        }

        impl $crate::$version::Description {
            /// [`Components::resolve_input_ref`] on this description's
            /// components; `None` without any.
            #[must_use]
            pub fn resolve_input_ref(&self, reference: &str) -> Option<&serde_json::Value> {
                self.components.as_ref()?.resolve_input_ref(reference)
            }

            /// [`Components::resolved_input_ref`] on this description's
            /// components; `reference` itself without any.
            #[must_use]
            pub fn resolved_input_ref<'a>(&'a self, reference: &'a str) -> &'a str {
                match &self.components {
                    Some(components) => components.resolved_input_ref(reference),
                    None => reference,
                }
            }
        }
    };
}
#[cfg(any(feature = "v1_0", feature = "v1_1"))]
pub(crate) use impl_input_schema_refs;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inputs() -> BTreeMap<String, Value> {
        serde_json::from_value(json!({
            "pet": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "a/b c": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "boolean" } },
                    "owner": { "$ref": "#/components/inputs/owner" }
                },
                "allOf": [ { "type": "object" }, { "$ref": "#/components/inputs/owner" } ]
            },
            "owner": { "type": "object" },
            "alias": { "$ref": "#/components/inputs/pet/properties/owner" },
            "loop": { "$ref": "#/components/inputs/loop" },
            "ping": { "$ref": "#/components/inputs/pong" },
            "pong": { "$ref": "#/components/inputs/ping" },
            "away": { "$ref": "other.yaml#/pet" }
        }))
        .unwrap()
    }

    #[test]
    fn follows_pointers_into_and_between_schemas() {
        let inputs = inputs();
        let found = |r: &str| follow(&inputs, r).0.map(|v| v["type"].clone());
        let last = |r: &str| follow(&inputs, r).1.to_owned();

        assert_eq!(found("#/components/inputs/pet"), Some(json!("object")));
        assert_eq!(
            found("#/components/inputs/pet/properties/id"),
            Some(json!("integer"))
        );
        assert_eq!(
            found("#/components/inputs/pet/properties/a~1b%20c"),
            Some(json!("string"))
        );
        assert_eq!(
            found("#/components/inputs/pet/properties/tags/items"),
            Some(json!("boolean"))
        );
        assert_eq!(
            found("#/components/inputs/pet/allOf/0"),
            Some(json!("object"))
        );
        assert_eq!(last("#/components/inputs/pet"), "#/components/inputs/pet");
        assert_eq!(found("#/components/inputs/alias"), Some(json!("object")));
        assert_eq!(
            last("#/components/inputs/alias"),
            "#/components/inputs/owner"
        );
        assert_eq!(
            last("#/components/inputs/pet/allOf/1"),
            "#/components/inputs/owner"
        );
    }

    #[test]
    fn stops_where_the_chain_stops() {
        let inputs = inputs();
        let follow = |r: &str| {
            let (found, last) = follow(&inputs, r);
            (found.is_some(), last.to_owned())
        };
        assert_eq!(
            follow("#/components/inputs/loop"),
            (false, "#/components/inputs/loop".into())
        );
        assert_eq!(
            follow("#/components/inputs/ping"),
            (false, "#/components/inputs/ping".into())
        );
        assert_eq!(
            follow("#/components/inputs/away"),
            (false, "other.yaml#/pet".into())
        );
        assert_eq!(
            follow("#/components/inputs/missing"),
            (false, "#/components/inputs/missing".into())
        );
        assert_eq!(
            follow("#/components/inputs/pet/properties/id/type/x"),
            (false, "#/components/inputs/pet/properties/id/type/x".into())
        );
        assert_eq!(
            follow("#/components/inputs/pet/allOf/01"),
            (false, "#/components/inputs/pet/allOf/01".into())
        );
        assert_eq!(
            follow("#/components/inputs/pet%zz"),
            (false, "#/components/inputs/pet%zz".into())
        );
        assert_eq!(
            follow("#/components/inputs/pet%ff"),
            (false, "#/components/inputs/pet%ff".into())
        );
        assert_eq!(follow("#/workflows/0"), (false, "#/workflows/0".into()));
    }
}
