//! Arazzo v1.0 `Reusable` object and the `ReusableOr<T>` wrapper.
//!
//! Per [Reusable Object](https://spec.openapis.org/arazzo/v1.0.1.html#reusable-object):
//! a runtime-expression reference to an object held in
//! [`Components`](crate::v1_0::Components), optionally overriding its
//! `value`. Unlike every other object it does **not** allow `x-`
//! extensions.
//!
//! Lists of parameters / success actions / failure actions accept
//! either a concrete object or a `Reusable`; [`ReusableOr`] models that
//! `oneOf`, analogous to `roas`'s `RefOr<T>`.

use crate::validation::{Context, ValidateWithContext, validate_required_string};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Reusable {
    /// **Required** A runtime expression referencing the desired object.
    pub reference: String,

    /// Sets a value for the referenced object (e.g. a parameter value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

impl ValidateWithContext for Reusable {
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        validate_required_string(&self.reference, ctx, format!("{path}.reference"));
    }
}

/// Either a concrete object `T` or a [`Reusable`] reference to one.
///
/// `Reusable` is tried first during deserialization; its required
/// `reference` key distinguishes it from the concrete object, which has
/// its own distinct required fields.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ReusableOr<T> {
    Reusable(Reusable),
    Item(T),
}

impl<T: ValidateWithContext> ValidateWithContext for ReusableOr<T> {
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        match self {
            ReusableOr::Reusable(r) => r.validate_with_context(ctx, path),
            ReusableOr::Item(t) => t.validate_with_context(ctx, path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1_0::parameter::Parameter;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn reusable_round_trips() {
        let r: Reusable =
            serde_json::from_value(json!({ "reference": "$components.parameters.foo" })).unwrap();
        assert_eq!(r.reference, "$components.parameters.foo");
        assert!(r.value.is_none());
    }

    #[test]
    fn reusable_or_picks_reusable_for_reference_key() {
        let v: ReusableOr<Parameter> = serde_json::from_value(
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
        let v: ReusableOr<Parameter> =
            serde_json::from_value(json!({ "name": "petId", "value": "$inputs.petId" })).unwrap();
        match v {
            ReusableOr::Item(p) => assert_eq!(p.name, "petId"),
            ReusableOr::Reusable(_) => panic!("expected item variant"),
        }
    }

    #[test]
    fn validate_reusable_rejects_empty_reference() {
        let mut c = Context::new(EnumSet::empty());
        let v: ReusableOr<Parameter> = ReusableOr::Reusable(Reusable::default());
        v.validate_with_context(&mut c, "#.parameters[0]".into());
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.parameters[0].reference: must not be empty")
        );
    }

    #[test]
    fn validate_item_delegates_to_inner() {
        let mut c = Context::new(EnumSet::empty());
        let v: ReusableOr<Parameter> = ReusableOr::Item(Parameter::default());
        v.validate_with_context(&mut c, "#.parameters[0]".into());
        assert!(
            c.errors
                .iter()
                .any(|e| e == "#.parameters[0].name: must not be empty")
        );
    }
}
