//! Arazzo v1.1 `Components` object.
//!
//! Per [Components Object](https://spec.openapis.org/arazzo/v1.1.0.html#components-object):
//! reusable inputs, parameters, and success / failure actions. All map
//! keys must match `^[a-zA-Z0-9\.\-_]+$`.

use crate::v1_1::failure_action::FailureAction;
use crate::v1_1::parameter::Parameter;
use crate::v1_1::success_action::SuccessAction;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Components {
    /// Reusable JSON Schema 2020-12 schemas. Kept as opaque JSON.
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

crate::common::reusable::impl_resolve_reusable! {
    Components, crate::v1_1::description::Description;
    Parameter => "parameters", parameters;
    SuccessAction => "successActions", success_actions;
    FailureAction => "failureActions", failure_actions;
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
    fn reusables_resolve_against_components_and_description() {
        use crate::v1_1::{Description, ResolveError, ReusableOr};

        let description: Description = serde_json::from_value(json!({
            "arazzo": "1.1.0",
            "info": { "title": "T", "version": "1" },
            "sourceDescriptions": [{ "name": "api", "url": "https://x.test/api.json" }],
            "workflows": [{ "workflowId": "w", "steps": [{ "stepId": "s", "operationId": "op" }] }],
            "components": {
                "parameters": { "page": { "name": "page", "in": "query", "value": 1 } },
                "successActions": { "done": { "name": "done", "type": "end" } },
                "failureActions": { "stop": { "name": "stop", "type": "end" } }
            }
        }))
        .unwrap();
        let components = description.components.as_ref().unwrap();
        let entry = |reference: &str| json!({ "reference": reference });

        let parameter: ReusableOr<Parameter> = serde_json::from_value(
            json!({ "reference": "$components.parameters.page", "value": 2 }),
        )
        .unwrap();
        assert_eq!(parameter.get_item(components).unwrap().name, "page");
        assert_eq!(parameter.get_item(&description).unwrap().name, "page");
        assert_eq!(parameter.value_override(), Some(&json!(2)));

        let success: ReusableOr<SuccessAction> =
            serde_json::from_value(entry("$components.successActions.done")).unwrap();
        assert_eq!(success.get_item(components).unwrap().name, "done");
        assert_eq!(success.get_item(&description).unwrap().name, "done");

        let failure: ReusableOr<FailureAction> =
            serde_json::from_value(entry("$components.failureActions.stop")).unwrap();
        assert_eq!(failure.get_item(components).unwrap().name, "stop");
        assert_eq!(failure.get_item(&description).unwrap().name, "stop");

        // A success action is not a failure action, whatever its name.
        let crossed: ReusableOr<FailureAction> =
            serde_json::from_value(entry("$components.successActions.done")).unwrap();
        assert!(matches!(
            crossed.get_item(components),
            Err(ResolveError::WrongKind {
                expected: "failureActions",
                ..
            })
        ));

        // Without components there is nothing to find.
        let bare = Description {
            components: None,
            ..description.clone()
        };
        assert_eq!(
            parameter.get_item(&bare).unwrap_err(),
            ResolveError::NotFound("$components.parameters.page".to_owned())
        );
    }

    #[test]
    fn validate_recurses_into_action_maps() {
        let c: Components = serde_json::from_value(json!({
            "inputs": { "in1": { "type": "string" } },
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
        assert!(msgs.iter().any(|e| e.contains("name: must not be empty")));
    }
}
