//! Arazzo v1.0 `Components` object.
//!
//! Per [Components Object](https://spec.openapis.org/arazzo/v1.0.1.html#components-object):
//! reusable inputs, parameters, and success / failure actions
//! referenced from elsewhere via [`Reusable`](crate::v1_0::Reusable).
//! All map keys must match `^[a-zA-Z0-9\.\-_]+$`.

use crate::v1_0::failure_action::FailureAction;
use crate::v1_0::parameter::Parameter;
use crate::v1_0::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext, validate_map_keys};
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
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        validate_map_keys(&self.inputs, ctx, &format!("{path}.inputs"));
        validate_map_keys(&self.parameters, ctx, &format!("{path}.parameters"));
        validate_map_keys(
            &self.success_actions,
            ctx,
            &format!("{path}.successActions"),
        );
        validate_map_keys(
            &self.failure_actions,
            ctx,
            &format!("{path}.failureActions"),
        );

        for (name, parameter) in &self.parameters {
            parameter.validate_with_context(ctx, format!("{path}.parameters.{name}"));
        }
        for (name, action) in &self.success_actions {
            action.validate_with_context(ctx, format!("{path}.successActions.{name}"));
        }
        for (name, action) in &self.failure_actions {
            action.validate_with_context(ctx, format!("{path}.failureActions.{name}"));
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
        let mut ctx = Context::new(EnumSet::empty());
        let mut parameters = BTreeMap::new();
        parameters.insert("bad key".to_owned(), Parameter::default());
        let c = Components {
            parameters,
            ..Default::default()
        };
        c.validate_with_context(&mut ctx, "#.components".into());
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(msgs.iter().any(|e| e.contains("key must match")));
        // recursion reaches the empty parameter name
        assert!(msgs.iter().any(|e| e.contains("name: must not be empty")));
    }
}
