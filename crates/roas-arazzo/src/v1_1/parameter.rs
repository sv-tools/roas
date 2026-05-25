//! Arazzo v1.1 `Parameter` object.
//!
//! Per [Parameter Object](https://spec.openapis.org/arazzo/v1.1.0.html#parameter-object).
//! New in v1.1: the `querystring` and `channel` locations, and `value`
//! may now be a [`Selector`](crate::v1_1::Selector).

use crate::v1_1::selector::ValueOrSelector;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The named location a [`Parameter`] applies to.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    #[default]
    Path,
    Query,
    /// The raw query string. Added in v1.1.
    Querystring,
    Header,
    Cookie,
    /// An async channel. Added in v1.1.
    Channel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Parameter {
    /// **Required** The name of the parameter.
    pub name: String,

    /// The named location of the parameter. Required when the step
    /// targets an operation.
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_: Option<ParameterLocation>,

    /// **Required** The value to pass — a literal / runtime expression,
    /// or a `Selector`.
    pub value: ValueOrSelector,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Parameter {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("name", &self.name);
        ctx.in_field("value", |ctx| self.value.validate_with_context(ctx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1_1::selector::Selector;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn channel_and_querystring_locations_round_trip() {
        for s in ["querystring", "channel"] {
            let p: Parameter =
                serde_json::from_value(json!({ "name": "n", "in": s, "value": "v" })).unwrap();
            assert!(p.in_.is_some());
        }
    }

    #[test]
    fn value_can_be_a_selector() {
        let p: Parameter = serde_json::from_value(json!({
            "name": "petId",
            "value": { "context": "$response.body", "selector": "$.id", "type": "jsonpath" }
        }))
        .unwrap();
        assert!(matches!(p.value, ValueOrSelector::Selector(_)));
        let mut ctx = Context::with_path(EnumSet::empty(), "#.p");
        p.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn validate_rejects_empty_name_and_bad_selector() {
        let p = Parameter {
            name: String::new(),
            value: ValueOrSelector::Selector(Selector::default()),
            ..Default::default()
        };
        let mut ctx = Context::with_path(EnumSet::empty(), "#.p");
        p.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(msgs.iter().any(|e| e == "#.p.name: must not be empty"));
        assert!(
            msgs.iter()
                .any(|e| e == "#.p.value.context: must not be empty")
        );
    }
}
