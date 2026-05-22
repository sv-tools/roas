//! Overlay v1.0 root document.

use crate::apply::{
    ActionOutcome, Apply, ApplyError, ApplyErrorKind, ApplyOptions, ApplyReport, Operation,
};
use crate::common::apply::{compile_path, locate, merge_json, remove_at};
use crate::v1_0::action::Action;
use crate::v1_0::info::Info;
use crate::v1_0::version::Version;
use crate::validation::{
    Context, Error, Validate, ValidateWithContext, ValidationOptions, validate_required_string,
};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Root Overlay v1.0 document.
///
/// See [§3.1 Overlay Object](https://spec.openapis.org/overlay/v1.0.0.html#overlay-object).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Overlay {
    /// **Required** `1.0.x` per the schema's pattern `^1\.0\.\d+$`.
    pub overlay: Version,

    /// **Required** Metadata about the overlay.
    pub info: Info,

    /// URI reference identifying the target document the overlay
    /// applies to. Absolute or relative per RFC 3986.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,

    /// **Required** Ordered, non-empty list of actions applied
    /// sequentially.
    pub actions: Vec<Action>,

    /// `x-`-prefixed Specification Extensions on the root.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Overlay {
    fn validate_inner(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        let mut ctx: Context<Overlay> = Context::new(options);
        let path = "#".to_owned();

        self.info
            .validate_with_context(&mut ctx, format!("{path}.info"));

        if self.actions.is_empty() {
            ctx.error(format!("{path}.actions"), "must contain at least one entry");
        }
        for (i, action) in self.actions.iter().enumerate() {
            action.validate_with_context(&mut ctx, format!("{path}.actions[{i}]"));
        }

        if let Some(extends) = &self.extends
            && !ctx.is_option(ValidationOptions::IgnoreExtendsFormat)
        {
            validate_required_string(extends, &mut ctx, format!("{path}.extends"));
        }

        ctx.into_result()
    }
}

impl Validate for Overlay {
    fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        self.validate_inner(options)
    }
}

impl Apply for Overlay {
    fn apply(
        &self,
        target: &mut Value,
        options: EnumSet<ApplyOptions>,
    ) -> Result<ApplyReport, ApplyError> {
        // Work on a clone so a mid-pipeline failure leaves `target`
        // untouched. Commit only on success.
        let mut working = target.clone();
        let mut report = ApplyReport::default();

        for (index, action) in self.actions.iter().enumerate() {
            let outcome = apply_action(index, action, &mut working, options)?;
            report.actions.push(outcome);
        }

        *target = working;
        Ok(report)
    }
}

fn apply_action(
    index: usize,
    action: &Action,
    doc: &mut Value,
    options: EnumSet<ApplyOptions>,
) -> Result<ActionOutcome, ApplyError> {
    let path = compile_path(&action.target).map_err(|msg| ApplyError {
        action_index: index,
        target: action.target.clone(),
        kind: ApplyErrorKind::InvalidJsonPath(msg),
    })?;

    let pointers = locate(doc, &path);

    if pointers.is_empty() {
        if options.contains(ApplyOptions::ErrorOnZeroMatch) {
            return Err(ApplyError {
                action_index: index,
                target: action.target.clone(),
                kind: ApplyErrorKind::ZeroMatch,
            });
        }
        let operation = if action.is_remove() {
            Operation::Remove
        } else {
            Operation::Update
        };
        return Ok(ActionOutcome {
            index,
            target: action.target.clone(),
            operation,
            matched: 0,
        });
    }

    if action.is_remove() {
        // Process in reverse to preserve earlier pointer validity
        // when siblings live in the same array.
        for ptr in pointers.iter().rev() {
            remove_at(doc, ptr);
        }
        return Ok(ActionOutcome {
            index,
            target: action.target.clone(),
            operation: Operation::Remove,
            matched: pointers.len(),
        });
    }

    if let Some(update) = &action.update {
        // §4.4: when `update` is set, targets must be objects or arrays.
        // We check kinds up front so failure leaves the working copy
        // unchanged for this action.
        let kinds: Vec<NodeKind> = pointers.iter().map(|p| classify(doc, p)).collect();

        if kinds.iter().any(|k| matches!(k, NodeKind::Primitive)) {
            return Err(ApplyError {
                action_index: index,
                target: action.target.clone(),
                kind: ApplyErrorKind::UpdateOnPrimitiveTarget,
            });
        }

        if options.contains(ApplyOptions::ErrorOnMixedKindMatch) && !uniform_kinds(&kinds) {
            return Err(ApplyError {
                action_index: index,
                target: action.target.clone(),
                kind: ApplyErrorKind::MixedKindMatch,
            });
        }

        for ptr in &pointers {
            if let Some(node) = doc.pointer_mut(ptr) {
                merge_json(node, update);
            }
        }
        return Ok(ActionOutcome {
            index,
            target: action.target.clone(),
            operation: Operation::Update,
            matched: pointers.len(),
        });
    }

    // `remove: false` (or absent) and no `update`: silently no-op
    // (the spec doesn't forbid an action that does nothing).
    Ok(ActionOutcome {
        index,
        target: action.target.clone(),
        operation: Operation::Update,
        matched: pointers.len(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Object,
    Array,
    Primitive,
    Missing,
}

fn classify(doc: &Value, pointer: &str) -> NodeKind {
    match doc.pointer(pointer) {
        None => NodeKind::Missing,
        Some(Value::Object(_)) => NodeKind::Object,
        Some(Value::Array(_)) => NodeKind::Array,
        Some(_) => NodeKind::Primitive,
    }
}

fn uniform_kinds(kinds: &[NodeKind]) -> bool {
    let mut iter = kinds
        .iter()
        .copied()
        .filter(|k| !matches!(k, NodeKind::Missing));
    let Some(first) = iter.next() else {
        return true;
    };
    iter.all(|k| k == first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(s: &str) -> Overlay {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn deserialize_minimal_round_trips() {
        let json = r#"{
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1" },
            "actions": [ { "target": "$.x", "update": {} } ]
        }"#;
        let o = parse(json);
        assert_eq!(o.overlay, Version::V1_0_0());
        assert_eq!(o.info.title, "T");
        assert_eq!(o.actions.len(), 1);
        assert!(o.extends.is_none());
        assert!(o.extensions.is_none());
    }

    #[test]
    fn deserialize_with_extends_and_extensions() {
        let json = r#"{
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1" },
            "extends": "./base.yaml",
            "actions": [ { "target": "$", "update": {} } ],
            "x-team": "platform",
            "skipped": 1
        }"#;
        let o = parse(json);
        assert_eq!(o.extends.as_deref(), Some("./base.yaml"));
        let ext = o.extensions.as_ref().unwrap();
        assert!(ext.contains_key("x-team"));
        assert!(!ext.contains_key("skipped"));
    }

    #[test]
    fn serialize_skips_optional_none_fields() {
        let o = Overlay {
            overlay: Version::V1_0_0(),
            info: Info {
                title: "T".into(),
                version: "1".into(),
                ..Default::default()
            },
            extends: None,
            actions: vec![Action {
                target: "$".into(),
                ..Default::default()
            }],
            extensions: None,
        };
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(
            v,
            json!({
                "overlay": "1.0.0",
                "info": { "title": "T", "version": "1" },
                "actions": [ { "target": "$" } ]
            }),
        );
    }

    #[test]
    fn deserialize_rejects_non_1_0_overlay_version() {
        let err = serde_json::from_value::<Overlay>(json!({
            "overlay": "2.0.0",
            "info": { "title": "T", "version": "1" },
            "actions": [ { "target": "$" } ]
        }))
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("\"2.0.0\"") && msg.contains("1.0"),
            "expected error to mention the bad version and the schema, got: {msg}",
        );
    }

    #[test]
    fn validate_rejects_empty_actions_vec() {
        let o = Overlay {
            overlay: Version::V1_0_0(),
            info: Info {
                title: "T".into(),
                version: "1".into(),
                ..Default::default()
            },
            actions: vec![],
            extends: None,
            extensions: None,
        };
        let err = o.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.actions: must contain at least one entry")
        );
    }

    #[test]
    fn validate_recurses_into_info_and_actions() {
        let o = Overlay {
            overlay: Version::V1_0_0(),
            info: Info::default(), // empty title/version
            actions: vec![Action {
                target: "".into(), // empty
                ..Default::default()
            }],
            extends: None,
            extensions: None,
        };
        let err = o.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.info.title: must not be empty")
        );
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.info.version: must not be empty")
        );
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.actions[0].target: must not be empty")
        );
    }

    #[test]
    fn validate_extends_must_not_be_empty_unless_ignored() {
        let o = Overlay {
            overlay: Version::V1_0_0(),
            info: Info {
                title: "T".into(),
                version: "1".into(),
                ..Default::default()
            },
            actions: vec![Action {
                target: "$".into(),
                ..Default::default()
            }],
            extends: Some(String::new()),
            extensions: None,
        };
        let err = o.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.extends: must not be empty")
        );

        let ok = o.validate(ValidationOptions::IgnoreExtendsFormat.into());
        assert!(ok.is_ok());
    }

    fn ovl(actions: Vec<Action>) -> Overlay {
        Overlay {
            overlay: Version::V1_0_0(),
            info: Info {
                title: "T".into(),
                version: "1".into(),
                ..Default::default()
            },
            extends: None,
            actions,
            extensions: None,
        }
    }

    #[test]
    fn apply_update_merges_into_selected_object() {
        let o = ovl(vec![Action {
            target: "$.info".into(),
            update: Some(json!({ "description": "patched" })),
            ..Default::default()
        }]);
        let mut doc = json!({
            "openapi": "3.1.0",
            "info": { "title": "API", "version": "1.0.0" },
            "paths": {}
        });
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions.len(), 1);
        assert_eq!(report.actions[0].matched, 1);
        assert_eq!(report.actions[0].operation, Operation::Update);
        assert_eq!(doc["info"]["description"], "patched");
        assert_eq!(doc["info"]["title"], "API"); // preserved
    }

    #[test]
    fn apply_remove_drops_selected_node() {
        let o = ovl(vec![Action {
            target: "$.paths['/x']".into(),
            remove: Some(true),
            ..Default::default()
        }]);
        let mut doc = json!({
            "paths": { "/x": { "get": {} }, "/y": { "get": {} } }
        });
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions[0].operation, Operation::Remove);
        assert!(!doc["paths"].as_object().unwrap().contains_key("/x"));
        assert!(doc["paths"].as_object().unwrap().contains_key("/y"));
    }

    #[test]
    fn apply_zero_match_default_is_no_op_with_count_zero() {
        let o = ovl(vec![Action {
            target: "$.nope".into(),
            update: Some(json!({})),
            ..Default::default()
        }]);
        let mut doc = json!({ "foo": 1 });
        let snapshot = doc.clone();
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions[0].matched, 0);
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_zero_match_strict_errors_and_rolls_back() {
        let o = ovl(vec![
            Action {
                target: "$.foo".into(),
                update: Some(json!({ "x": 1 })),
                ..Default::default()
            },
            Action {
                target: "$.nope".into(),
                update: Some(json!({})),
                ..Default::default()
            },
        ]);
        let mut doc = json!({ "foo": { "a": 0 } });
        let snapshot = doc.clone();
        let err = o
            .apply(&mut doc, ApplyOptions::ErrorOnZeroMatch.into())
            .unwrap_err();
        assert_eq!(err.action_index, 1);
        assert_eq!(err.kind, ApplyErrorKind::ZeroMatch);
        // First action's mutation must be rolled back.
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_invalid_jsonpath_errors_and_does_not_touch_target() {
        let o = ovl(vec![Action {
            target: "not a path".into(),
            update: Some(json!({})),
            ..Default::default()
        }]);
        let mut doc = json!({ "x": 1 });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert!(matches!(err.kind, ApplyErrorKind::InvalidJsonPath(_)));
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_update_on_primitive_target_errors() {
        let o = ovl(vec![Action {
            target: "$.info.title".into(),
            update: Some(json!({ "ignored": true })),
            ..Default::default()
        }]);
        let mut doc = json!({ "info": { "title": "API" } });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert_eq!(err.kind, ApplyErrorKind::UpdateOnPrimitiveTarget);
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_multiple_remove_targets_in_array_preserves_indices() {
        let o = ovl(vec![Action {
            target: "$.items[?@.delete == true]".into(),
            remove: Some(true),
            ..Default::default()
        }]);
        let mut doc = json!({
            "items": [
                { "id": 0, "delete": true },
                { "id": 1 },
                { "id": 2, "delete": true },
                { "id": 3 }
            ]
        });
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions[0].matched, 2);
        assert_eq!(doc, json!({ "items": [ { "id": 1 }, { "id": 3 } ] }),);
    }

    #[test]
    fn apply_sequential_actions_compose() {
        let o = ovl(vec![
            Action {
                target: "$.info".into(),
                update: Some(json!({ "description": "v1" })),
                ..Default::default()
            },
            Action {
                target: "$.info".into(),
                update: Some(json!({ "description": "v2" })),
                ..Default::default()
            },
        ]);
        let mut doc = json!({ "info": { "title": "API" } });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["info"]["description"], "v2");
    }

    #[test]
    fn apply_mixed_kind_strict_errors() {
        let o = ovl(vec![Action {
            target: "$.choices[*]".into(),
            update: Some(json!({ "z": 1 })),
            ..Default::default()
        }]);
        let mut doc = json!({
            "choices": [ { "a": 1 }, [ 1, 2 ] ]
        });
        let snapshot = doc.clone();
        let err = o
            .apply(&mut doc, ApplyOptions::ErrorOnMixedKindMatch.into())
            .unwrap_err();
        assert_eq!(err.kind, ApplyErrorKind::MixedKindMatch);
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_mixed_kind_lax_silently_merges_uniformly() {
        let o = ovl(vec![Action {
            target: "$.choices[*]".into(),
            update: Some(json!({ "z": 1 })),
            ..Default::default()
        }]);
        let mut doc = json!({
            "choices": [ { "a": 1 }, [ 1, 2 ] ]
        });
        // Without ErrorOnMixedKindMatch, no error; per-node merge runs.
        // Object gets the new key; array (shape mismatch with update
        // object) gets replaced by the update value.
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["choices"][0], json!({ "a": 1, "z": 1 }));
        assert_eq!(doc["choices"][1], json!({ "z": 1 }));
    }

    #[test]
    fn apply_remove_only_no_update_runs_clean() {
        let o = ovl(vec![Action {
            target: "$.foo".into(),
            // Neither remove nor update — should be a clean no-op
            // with matched > 0.
            ..Default::default()
        }]);
        let mut doc = json!({ "foo": { "a": 1 } });
        let r = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(r.actions[0].operation, Operation::Update);
        assert_eq!(r.actions[0].matched, 1);
        assert_eq!(doc, json!({ "foo": { "a": 1 } }));
    }
}
