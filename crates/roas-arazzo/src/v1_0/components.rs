//! Arazzo v1.0 `Components` object.
//!
//! Per [Components Object](https://spec.openapis.org/arazzo/v1.0.1.html#components-object):
//! reusable inputs, parameters, and success / failure actions
//! referenced from elsewhere via [`Reusable`](crate::v1_0::Reusable).
//! All map keys must match `^[a-zA-Z0-9\.\-_]+$`.

use crate::v1_0::failure_action::FailureAction;
use crate::v1_0::parameter::Parameter;
use crate::v1_0::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Components {
    /// Reusable JSON Schema 2020-12 schemas, referenced from workflow
    /// inputs. Kept as opaque JSON.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, serde_json::Value>,

    /// Reusable parameter objects.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Parameter>,

    /// Reusable success action objects.
    #[serde(
        rename = "successActions",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub success_actions: BTreeMap<String, SuccessAction>,

    /// Reusable failure action objects.
    #[serde(
        rename = "failureActions",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub failure_actions: BTreeMap<String, FailureAction>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Components {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.validate_map_keys("inputs", &self.inputs);
        ctx.validate_map_keys("parameters", &self.parameters);
        ctx.validate_map_keys("successActions", &self.success_actions);
        ctx.validate_map_keys("failureActions", &self.failure_actions);

        for (name, parameter) in &self.parameters {
            ctx.in_key("parameters", name, |ctx| {
                parameter.validate_with_context(ctx)
            });
        }
        for (name, action) in &self.success_actions {
            ctx.in_key("successActions", name, |ctx| {
                action.validate_with_context(ctx)
            });
        }
        for (name, action) in &self.failure_actions {
            ctx.in_key("failureActions", name, |ctx| {
                action.validate_with_context(ctx)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn deserialize_round_trips() {
        let c: Components = serde_json::from_value(json!({
            "parameters": {
                "petId": { "name": "petId", "in": "path", "value": "$inputs.petId" }
            }
        }))
        .unwrap();
        assert!(c.parameters.contains_key("petId"));

        let v = serde_json::to_value(&c).unwrap();
        assert!(v["parameters"]["petId"].is_object());
        assert!(v.get("inputs").is_none());
    }

    #[test]
    fn validate_rejects_bad_key_and_recurses() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        let mut parameters = BTreeMap::new();
        parameters.insert("bad key".to_owned(), Parameter::default());
        let c = Components {
            parameters,
            ..Default::default()
        };
        c.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(msgs.iter().any(|e| e.contains("key must match")));
        // recursion reaches the empty parameter name
        assert!(msgs.iter().any(|e| e.contains("name: must not be empty")));
    }

    #[test]
    fn validate_recurses_into_action_maps() {
        let c: Components = serde_json::from_value(json!({
            "successActions": { "a": { "name": "", "type": "end" } },
            "failureActions": { "b": { "name": "", "type": "end" } }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        c.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e == "#.components.successActions.a.name: must not be empty")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.components.failureActions.b.name: must not be empty")
        );
    }
}
