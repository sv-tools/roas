//! The Arazzo `Reusable` object and the `ReusableOr<T>` wrapper.
//!
//! Per the Reusable Object
//! ([v1.0](https://spec.openapis.org/arazzo/v1.0.1.html#reusable-object) /
//! [v1.1](https://spec.openapis.org/arazzo/v1.1.0.html#reusable-object)):
//! a runtime-expression reference to an object held in a version's
//! `Components`, optionally overriding its `value`. Unlike every other
//! object it does **not** allow `x-` extensions.
//!
//! Identical across Arazzo versions, so it lives in `common` (mirroring
//! how `roas` consolidated its `reference` type). Lists of parameters /
//! success actions / failure actions accept either a concrete object or
//! a `Reusable`; [`ReusableOr`] models that `oneOf`, analogous to
//! `roas`'s `RefOr<T>`.

use crate::validation::{Context, ValidateWithContext};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Reusable {
    /// **Required** A runtime expression referencing the desired object.
    pub reference: String,

    /// Sets a value for the referenced object (e.g. a parameter value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

impl ValidateWithContext for Reusable {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("reference", &self.reference);
    }
}

/// Either a concrete object `T` or a [`Reusable`] reference to one.
///
/// Serializes untagged (the inner object directly). Deserialization is
/// hand-written rather than `#[serde(untagged)]`: it dispatches on the
/// presence of the discriminating `reference` key in a single pass and
/// then deserializes only the chosen variant, so a malformed `Item`
/// surfaces its real error (e.g. `missing field \`name\``) instead of
/// the opaque "data did not match any variant".
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ReusableOr<T> {
    Reusable(Reusable),
    Item(T),
}

impl<'de, T> Deserialize<'de> for ReusableOr<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("reference").is_some() {
            serde_json::from_value(value)
                .map(ReusableOr::Reusable)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(ReusableOr::Item)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl<T: ValidateWithContext> ValidateWithContext for ReusableOr<T> {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            ReusableOr::Reusable(r) => r.validate_with_context(ctx),
            ReusableOr::Item(t) => t.validate_with_context(ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    /// Minimal stand-in for a concrete component (keeps these tests
    /// independent of any version feature).
    #[derive(Debug, Deserialize, Default)]
    struct Demo {
        name: String,
    }

    impl ValidateWithContext for Demo {
        fn validate_with_context(&self, ctx: &mut Context) {
            ctx.require_non_empty("name", &self.name);
        }
    }

    #[test]
    fn reusable_round_trips() {
        let r: Reusable =
            serde_json::from_value(json!({ "reference": "$components.parameters.foo" })).unwrap();
        assert_eq!(r.reference, "$components.parameters.foo");
        assert!(r.value.is_none());
    }

    #[test]
    fn reusable_or_picks_reusable_for_reference_key() {
        let v: ReusableOr<Demo> = serde_json::from_value(
            json!({ "reference": "$components.parameters.foo", "value": 1 }),
        )
        .unwrap();
        match v {
            ReusableOr::Reusable(r) => {
                assert_eq!(r.reference, "$components.parameters.foo");
                assert_eq!(r.value, Some(json!(1)));
            }
            ReusableOr::Item(_) => panic!("expected reusable variant"),
        }
    }

    #[test]
    fn reusable_or_picks_item_for_concrete_object() {
        let v: ReusableOr<Demo> = serde_json::from_value(json!({ "name": "petId" })).unwrap();
        match v {
            ReusableOr::Item(p) => assert_eq!(p.name, "petId"),
            ReusableOr::Reusable(_) => panic!("expected item variant"),
        }
    }

    #[test]
    fn malformed_item_surfaces_inner_error_not_opaque_variant_error() {
        // No `reference` key, so this dispatches to `Item(Demo)` and
        // fails with the real missing-field error rather than the
        // untagged-enum catch-all.
        let err = serde_json::from_value::<ReusableOr<Demo>>(json!({ "other": 1 })).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing field"), "got: {msg}");
        assert!(!msg.contains("did not match any variant"), "got: {msg}");
    }

    #[test]
    fn round_trips_through_yaml() {
        let v: ReusableOr<Demo> = serde_yaml_ng::from_str("name: petId\n").unwrap();
        assert!(matches!(v, ReusableOr::Item(_)));
        let r: ReusableOr<Demo> =
            serde_yaml_ng::from_str("reference: $components.parameters.foo\n").unwrap();
        assert!(matches!(r, ReusableOr::Reusable(_)));
    }

    #[test]
    fn validate_reusable_rejects_empty_reference() {
        let mut c = Context::with_path(EnumSet::empty(), "#.parameters[0]");
        let v: ReusableOr<Demo> = ReusableOr::Reusable(Reusable::default());
        v.validate_with_context(&mut c);
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.parameters[0].reference: must not be empty")
        );
    }

    #[test]
    fn validate_item_delegates_to_inner() {
        let mut c = Context::with_path(EnumSet::empty(), "#.parameters[0]");
        let v: ReusableOr<Demo> = ReusableOr::Item(Demo::default());
        v.validate_with_context(&mut c);
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.parameters[0].name: must not be empty")
        );
    }
}
