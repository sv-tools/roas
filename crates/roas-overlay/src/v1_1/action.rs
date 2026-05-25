//! Overlay v1.1 `Action` object.
//!
//! See [§3.3 Action Object](https://spec.openapis.org/overlay/v1.1.0.html#action-object).
//! New in v1.1: the `copy` field — a JSONPath selecting a single
//! source node whose value is merged into each `target` node.

use crate::common::apply::compile_path;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Action {
    /// **Required** RFC 9535 JSONPath selecting the nodes to act on.
    pub target: String,

    /// CommonMark-flavored description of the action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Value (object, array, or primitive) merged into the selected
    /// nodes. Mutually exclusive with `copy` and with `remove: true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<serde_json::Value>,

    /// RFC 9535 JSONPath selecting exactly one source node in the
    /// working document. The source's value is merged into each
    /// `target` node using the same merging rules as `update`.
    /// Mutually exclusive with `update` and with `remove: true`.
    /// Added in Overlay v1.1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<String>,

    /// When `true`, removes the selected nodes from their container.
    /// Mutually exclusive with `update` and `copy`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remove: Option<bool>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Action {
    /// Returns `true` if this action's `remove` field is set to `true`.
    #[must_use]
    pub fn is_remove(&self) -> bool {
        self.remove == Some(true)
    }
}

/// Validate that `value` is a non-empty, syntactically valid RFC 9535
/// JSONPath, recording diagnostics under `<current>.<field>`.
fn validate_jsonpath(value: &str, ctx: &mut Context, field: &str) {
    if value.is_empty() {
        ctx.error_field(field, "must not be empty");
    } else if let Err(msg) = compile_path(value) {
        ctx.error_field(field, format!("invalid JSONPath query: {msg}"));
    }
}

impl ValidateWithContext for Action {
    fn validate_with_context(&self, ctx: &mut Context) {
        validate_jsonpath(&self.target, ctx, "target");

        if let Some(copy) = &self.copy {
            validate_jsonpath(copy, ctx, "copy");
        }

        // The spec says `update` "has no impact if the `remove` field
        // of this action object is `true`"; the same applies to
        // `copy`. We reject all three combinations because they
        // almost certainly indicate an authoring mistake.
        if self.is_remove() && self.update.is_some() {
            ctx.error("`remove: true` and `update` are mutually exclusive");
        }
        if self.is_remove() && self.copy.is_some() {
            ctx.error("`remove: true` and `copy` are mutually exclusive");
        }
        if self.update.is_some() && self.copy.is_some() {
            ctx.error("`update` and `copy` are mutually exclusive");
        }

        // Catch the silent-no-op authoring bug.
        if !self.is_remove() && self.update.is_none() && self.copy.is_none() {
            ctx.error("action must specify one of `update`, `copy`, or `remove: true`");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(action: &Action) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.actions[0]");
        action.validate_with_context(&mut ctx);
        ctx.errors.iter().map(|e| e.to_string()).collect()
    }

    #[test]
    fn deserialize_update_action_round_trips() {
        let a: Action = serde_json::from_value(json!({
            "target": "$.info",
            "update": { "description": "patched" }
        }))
        .unwrap();
        assert_eq!(a.target, "$.info");
        assert!(a.copy.is_none());

        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v,
            json!({ "target": "$.info", "update": { "description": "patched" } })
        );
    }

    #[test]
    fn deserialize_copy_action_round_trips() {
        let a: Action = serde_json::from_value(json!({
            "target": "$.paths['/dst']",
            "copy": "$.paths['/src']"
        }))
        .unwrap();
        assert_eq!(a.target, "$.paths['/dst']");
        assert_eq!(a.copy.as_deref(), Some("$.paths['/src']"));
        assert!(a.update.is_none());
        assert!(a.remove.is_none());

        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v,
            json!({
                "target": "$.paths['/dst']",
                "copy": "$.paths['/src']"
            }),
        );
    }

    #[test]
    fn deserialize_remove_action() {
        let a: Action =
            serde_json::from_value(json!({ "target": "$.paths['/x']", "remove": true })).unwrap();
        assert!(a.is_remove());
        assert!(a.update.is_none());
        assert!(a.copy.is_none());
    }

    #[test]
    fn deserialize_keeps_x_dash_extensions() {
        let a: Action = serde_json::from_value(json!({
            "target": "$",
            "update": {},
            "x-author": "me",
        }))
        .unwrap();
        assert!(a.extensions.as_ref().unwrap().contains_key("x-author"));
    }

    #[test]
    fn validate_empty_target_errors() {
        let errs = validate(&Action::default());
        assert!(
            errs.iter()
                .any(|s| s == "#.actions[0].target: must not be empty")
        );
    }

    #[test]
    fn validate_invalid_target_jsonpath_errors() {
        let a = Action {
            target: "not a path".into(),
            update: Some(json!({})),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(errs.iter().any(|s| s.contains("invalid JSONPath")));
    }

    #[test]
    fn validate_invalid_copy_jsonpath_errors() {
        let a = Action {
            target: "$.dst".into(),
            copy: Some("not a path".into()),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(
            errs.iter()
                .any(|s| s.contains("invalid JSONPath") && s.contains(".copy")),
            "expected invalid-copy diagnostic, got: {errs:?}",
        );
    }

    #[test]
    fn validate_empty_copy_string_errors() {
        let a = Action {
            target: "$.dst".into(),
            copy: Some(String::new()),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(
            errs.iter()
                .any(|s| s == "#.actions[0].copy: must not be empty"),
            "got: {errs:?}",
        );
    }

    #[test]
    fn validate_remove_and_update_together_errors() {
        let a = Action {
            target: "$.foo".into(),
            update: Some(json!({})),
            remove: Some(true),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(
            errs.iter()
                .any(|s| s.contains("`remove: true` and `update`"))
        );
    }

    #[test]
    fn validate_remove_and_copy_together_errors() {
        let a = Action {
            target: "$.foo".into(),
            copy: Some("$.src".into()),
            remove: Some(true),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(errs.iter().any(|s| s.contains("`remove: true` and `copy`")));
    }

    #[test]
    fn validate_update_and_copy_together_errors() {
        let a = Action {
            target: "$.foo".into(),
            update: Some(json!({})),
            copy: Some("$.src".into()),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(errs.iter().any(|s| s.contains("`update` and `copy`")));
    }

    #[test]
    fn validate_no_effect_action_errors() {
        let a = Action {
            target: "$.foo".into(),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(
            errs.iter()
                .any(|s| s.contains("must specify one of `update`, `copy`, or `remove: true`")),
            "got: {errs:?}",
        );
    }

    #[test]
    fn validate_remove_false_with_update_is_ok() {
        let a = Action {
            target: "$.foo".into(),
            update: Some(json!({})),
            remove: Some(false),
            ..Default::default()
        };
        assert!(validate(&a).is_empty());
    }

    #[test]
    fn validate_remove_false_with_copy_is_ok() {
        let a = Action {
            target: "$.foo".into(),
            copy: Some("$.src".into()),
            remove: Some(false),
            ..Default::default()
        };
        assert!(validate(&a).is_empty());
    }

    #[test]
    fn is_remove_only_true_when_remove_set_to_true() {
        let a = Action::default();
        assert!(!a.is_remove());

        let a = Action {
            remove: Some(false),
            ..Default::default()
        };
        assert!(!a.is_remove());

        let a = Action {
            remove: Some(true),
            ..Default::default()
        };
        assert!(a.is_remove());
    }
}
