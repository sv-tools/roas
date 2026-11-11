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

/// Where a [`Reusable`] finds objects of type `T`.
///
/// A reference is the runtime expression `$components.<kind>.<name>`;
/// an implementation says how its kind is spelled and looks a name up.
/// Each version's `Components` and `Description` implement it for the
/// three kinds a `Reusable` may name: parameters, success actions and
/// failure actions.
pub trait ResolveReusable<T> {
    /// The kind as a reference spells it, e.g. `parameters`.
    const KIND: &'static str;

    /// The component of this kind called `name`.
    fn component(&self, name: &str) -> Option<&T>;
}

/// Why a [`Reusable`] did not lead to an object.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolveError {
    /// The reference is not `$components.<expected>.<name>`: it names
    /// another kind of component, or is not a component reference.
    WrongKind {
        reference: String,
        expected: &'static str,
    },
    /// The reference is well-formed but the components hold nothing
    /// under that name.
    NotFound(String),
}

impl ResolveError {
    /// The reference that did not resolve, as the document spells it.
    #[must_use]
    pub fn reference(&self) -> &str {
        match self {
            ResolveError::WrongKind { reference, .. } | ResolveError::NotFound(reference) => {
                reference
            }
        }
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::WrongKind {
                reference,
                expected,
            } => write!(
                f,
                "reference `{reference}` must be of the form `$components.{expected}.<name>`"
            ),
            ResolveError::NotFound(reference) => write!(f, "reference `{reference}` not found"),
        }
    }
}

impl std::error::Error for ResolveError {}

impl<T> ReusableOr<T> {
    /// The object itself, or the component a [`Reusable`] names in
    /// `spec` — a version's `Components` or `Description`.
    ///
    /// A `Reusable`'s `value` is not applied; read it with
    /// [`value_override`](Self::value_override).
    pub fn get_item<'a, S>(&'a self, spec: &'a S) -> Result<&'a T, ResolveError>
    where
        S: ResolveReusable<T>,
    {
        let reusable = match self {
            ReusableOr::Item(item) => return Ok(item),
            ReusableOr::Reusable(reusable) => reusable,
        };
        let reference = reusable.reference.as_str();
        // A component name may itself contain dots, so everything after
        // the kind is the name.
        let name = reference
            .strip_prefix("$components.")
            .and_then(|rest| rest.strip_prefix(S::KIND))
            .and_then(|rest| rest.strip_prefix('.'))
            .ok_or_else(|| ResolveError::WrongKind {
                reference: reference.to_owned(),
                expected: S::KIND,
            })?;
        spec.component(name)
            .ok_or_else(|| ResolveError::NotFound(reference.to_owned()))
    }

    /// The `value` a [`Reusable`] sets for the object it references,
    /// e.g. a parameter value. `None` for an inline object.
    #[must_use]
    pub fn value_override(&self) -> Option<&serde_json::Value> {
        match self {
            ReusableOr::Reusable(reusable) => reusable.value.as_ref(),
            ReusableOr::Item(_) => None,
        }
    }
}

/// Implement [`ResolveReusable`] for a version's `Components` and
/// `Description`, one kind per `Type => "kind", field;` entry.
#[cfg(any(feature = "v1_0", feature = "v1_1"))]
macro_rules! impl_resolve_reusable {
    ($components:ty, $description:ty; $($item:ty => $kind:literal, $field:ident;)+) => {$(
        impl $crate::common::reusable::ResolveReusable<$item> for $components {
            const KIND: &'static str = $kind;

            fn component(&self, name: &str) -> Option<&$item> {
                self.$field.get(name)
            }
        }

        impl $crate::common::reusable::ResolveReusable<$item> for $description {
            const KIND: &'static str = $kind;

            fn component(&self, name: &str) -> Option<&$item> {
                self.components.as_ref()?.$field.get(name)
            }
        }
    )+};
}
#[cfg(any(feature = "v1_0", feature = "v1_1"))]
pub(crate) use impl_resolve_reusable;

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
    use std::collections::BTreeMap;

    /// Minimal stand-in for a concrete component (keeps these tests
    /// independent of any version feature).
    #[derive(Debug, Deserialize, Default)]
    struct Demo {
        name: String,
    }

    impl ResolveReusable<Demo> for BTreeMap<String, Demo> {
        const KIND: &'static str = "demos";

        fn component(&self, name: &str) -> Option<&Demo> {
            self.get(name)
        }
    }

    fn demos() -> BTreeMap<String, Demo> {
        BTreeMap::from([("a.b".to_owned(), Demo { name: "A".into() })])
    }

    fn reusable(reference: &str) -> ReusableOr<Demo> {
        ReusableOr::Reusable(Reusable {
            reference: reference.to_owned(),
            value: Some(json!(1)),
        })
    }

    #[test]
    fn get_item_returns_an_inline_item_as_is() {
        let entry = ReusableOr::Item(Demo { name: "X".into() });
        assert_eq!(entry.get_item(&demos()).unwrap().name, "X");
        assert_eq!(entry.value_override(), None);
    }

    #[test]
    fn get_item_follows_a_reference_whose_name_has_dots() {
        let entry = reusable("$components.demos.a.b");
        assert_eq!(entry.get_item(&demos()).unwrap().name, "A");
        assert_eq!(entry.value_override(), Some(&json!(1)));
    }

    #[test]
    fn get_item_tells_a_missing_name_from_another_kind() {
        let err = reusable("$components.demos.nope")
            .get_item(&demos())
            .unwrap_err();
        assert_eq!(
            err,
            ResolveError::NotFound("$components.demos.nope".to_owned())
        );
        assert_eq!(err.reference(), "$components.demos.nope");
        assert_eq!(
            err.to_string(),
            "reference `$components.demos.nope` not found"
        );

        // A kind that merely starts the same way is still another kind.
        for reference in ["$components.parameters.a.b", "$components.demosX.a", "a.b"] {
            let err = reusable(reference).get_item(&demos()).unwrap_err();
            assert_eq!(
                err,
                ResolveError::WrongKind {
                    reference: reference.to_owned(),
                    expected: "demos",
                }
            );
            assert_eq!(err.reference(), reference);
            assert_eq!(
                err.to_string(),
                format!("reference `{reference}` must be of the form `$components.demos.<name>`")
            );
        }
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
        let v: ReusableOr<Demo> = crate::test_yaml::from_str("name: petId\n").unwrap();
        assert!(matches!(v, ReusableOr::Item(_)));
        let r: ReusableOr<Demo> =
            crate::test_yaml::from_str("reference: $components.parameters.foo\n").unwrap();
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
