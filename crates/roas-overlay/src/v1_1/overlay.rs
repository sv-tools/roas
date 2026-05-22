//! Overlay v1.1 root document.

use crate::apply::{
    ActionOutcome, Apply, ApplyError, ApplyErrorKind, ApplyOptions, ApplyReport, Operation,
};
use crate::common::apply::{compile_path, locate, merge_json, remove_at};
use crate::v1_1::action::Action;
use crate::v1_1::info::Info;
use crate::v1_1::version::Version;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Root Overlay v1.1 document.
///
/// See [§3.1 Overlay Object](https://spec.openapis.org/overlay/v1.1.0.html#overlay-object).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Overlay {
    /// **Required** `1.1.x` per the schema's pattern `^1\.1\.\d+$`.
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
        let mut ctx = Context::new(options);
        let path = "#".to_owned();

        self.info
            .validate_with_context(&mut ctx, format!("{path}.info"));

        if self.actions.is_empty() {
            ctx.error(format!("{path}.actions"), "must contain at least one entry");
        }
        for (i, action) in self.actions.iter().enumerate() {
            action.validate_with_context(&mut ctx, format!("{path}.actions[{i}]"));
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

#[cfg(feature = "v1_0")]
impl From<crate::v1_0::Overlay> for Overlay {
    /// Upconvert an Overlay v1.0 document to v1.1. Additive: only the
    /// `overlay` version string changes (to `1.1.0`); v1.0 `Info` and
    /// `Action` map structurally because v1.1 only adds optional
    /// fields (`Info.description`, `Action.copy`).
    fn from(o: crate::v1_0::Overlay) -> Self {
        Self {
            overlay: Version::V1_1_0(),
            info: Info {
                title: o.info.title,
                version: o.info.version,
                description: None,
                extensions: o.info.extensions,
            },
            extends: o.extends,
            actions: o
                .actions
                .into_iter()
                .map(|a| Action {
                    target: a.target,
                    description: a.description,
                    update: a.update,
                    copy: None,
                    remove: a.remove,
                    extensions: a.extensions,
                })
                .collect(),
            extensions: o.extensions,
        }
    }
}

fn apply_action(
    index: usize,
    action: &Action,
    doc: &mut Value,
    options: EnumSet<ApplyOptions>,
) -> Result<ActionOutcome, ApplyError> {
    let err = |kind| ApplyError {
        action_index: index,
        target: action.target.clone(),
        kind,
    };

    let path =
        compile_path(&action.target).map_err(|msg| err(ApplyErrorKind::InvalidJsonPath(msg)))?;
    let pointers = locate(doc, &path);

    let operation = if action.is_remove() {
        Operation::Remove
    } else if action.copy.is_some() {
        Operation::Copy
    } else {
        Operation::Update
    };
    let no_effect = !action.is_remove() && action.update.is_none() && action.copy.is_none();

    if pointers.is_empty() {
        if options.contains(ApplyOptions::ErrorOnZeroMatch) {
            return Err(err(ApplyErrorKind::ZeroMatch));
        }
        return Ok(ActionOutcome {
            index,
            target: action.target.clone(),
            operation,
            matched: 0,
        });
    }

    // Spec §4.4: `target` MUST resolve to objects or arrays for every
    // action — checked up front so a failure leaves the working copy
    // untouched.
    let kinds: Vec<NodeKind> = pointers.iter().map(|p| classify(doc, p)).collect();
    if kinds.iter().any(|k| matches!(k, NodeKind::Primitive)) {
        return Err(err(ApplyErrorKind::PrimitiveActionTarget));
    }
    if options.contains(ApplyOptions::ErrorOnMixedKindMatch) && !uniform_kinds(&kinds) {
        return Err(err(ApplyErrorKind::MixedKindMatch));
    }

    if no_effect {
        return Ok(ActionOutcome {
            index,
            target: action.target.clone(),
            operation,
            matched: 0,
        });
    }

    if action.is_remove() {
        // Process in reverse to preserve earlier pointer validity
        // when siblings live in the same array. Count only successful
        // removes.
        let mut removed = 0;
        for ptr in pointers.iter().rev() {
            if remove_at(doc, ptr) {
                removed += 1;
            }
        }
        return Ok(ActionOutcome {
            index,
            target: action.target.clone(),
            operation,
            matched: removed,
        });
    }

    // Resolve the effective merge value: either the inline `update`,
    // or — for v1.1 `copy` actions — the value at the `copy` source
    // JSONPath. The source must resolve to exactly one node.
    let effective_update: Value = if let Some(copy_src) = &action.copy {
        let copy_path =
            compile_path(copy_src).map_err(|msg| err(ApplyErrorKind::InvalidJsonPath(msg)))?;
        let src_pointers = locate(doc, &copy_path);
        if src_pointers.is_empty() {
            return Err(err(ApplyErrorKind::CopySourceNotFound(copy_src.clone())));
        }
        if src_pointers.len() > 1 {
            return Err(err(ApplyErrorKind::CopySourceMultiple(copy_src.clone())));
        }
        doc.pointer(&src_pointers[0])
            .expect("located pointer must resolve in same doc snapshot")
            .clone()
    } else {
        action
            .update
            .as_ref()
            .expect("no_effect path covers the all-None case")
            .clone()
    };

    // Update / copy share the same per-node merge rules. Array
    // targets append `effective_update` as a single entry; object
    // targets recurse per §4.4.3.1.
    for (ptr, kind) in pointers.iter().zip(kinds.iter()) {
        if let Some(node) = doc.pointer_mut(ptr) {
            match kind {
                NodeKind::Array => {
                    if let Value::Array(arr) = node {
                        arr.push(effective_update.clone());
                    }
                }
                NodeKind::Object => merge_json(node, &effective_update),
                NodeKind::Primitive | NodeKind::Missing => {
                    // Primitives rejected above; Missing can't occur
                    // because pointers were just located.
                }
            }
        }
    }
    Ok(ActionOutcome {
        index,
        target: action.target.clone(),
        operation,
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
            "overlay": "1.1.0",
            "info": { "title": "T", "version": "1" },
            "actions": [ { "target": "$.x", "update": {} } ]
        }"#;
        let o = parse(json);
        assert_eq!(o.overlay, Version::V1_1_0());
        assert_eq!(o.info.title, "T");
        assert_eq!(o.actions.len(), 1);
    }

    #[test]
    fn deserialize_rejects_non_1_1_overlay_version() {
        let err = serde_json::from_value::<Overlay>(json!({
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1" },
            "actions": [ { "target": "$" } ]
        }))
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("\"1.0.0\"") && msg.contains("1.1"));
    }

    #[test]
    fn validate_rejects_empty_actions_vec() {
        let o = Overlay {
            overlay: Version::V1_1_0(),
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

    fn ovl(actions: Vec<Action>) -> Overlay {
        Overlay {
            overlay: Version::V1_1_0(),
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
    fn apply_copy_merges_source_value_into_target() {
        // Copy the `/source` path under `/dest`.
        let o = ovl(vec![Action {
            target: "$.paths['/dest']".into(),
            copy: Some("$.paths['/source']".into()),
            ..Default::default()
        }]);
        let mut doc = json!({
            "paths": {
                "/source": {
                    "get": { "summary": "the source" }
                },
                "/dest": {}
            }
        });
        let r = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(r.actions[0].operation, Operation::Copy);
        assert_eq!(r.actions[0].matched, 1);
        assert_eq!(
            doc["paths"]["/dest"],
            json!({ "get": { "summary": "the source" } }),
        );
        // Source must be unchanged.
        assert_eq!(
            doc["paths"]["/source"],
            json!({ "get": { "summary": "the source" } }),
        );
    }

    #[test]
    fn apply_copy_against_array_target_appends_source_as_single_entry() {
        let o = ovl(vec![Action {
            target: "$.parameters".into(),
            copy: Some("$.shared_parameter".into()),
            ..Default::default()
        }]);
        let mut doc = json!({
            "parameters": [ { "name": "page", "in": "query" } ],
            "shared_parameter": { "name": "limit", "in": "query" }
        });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(
            doc["parameters"],
            json!([
                { "name": "page", "in": "query" },
                { "name": "limit", "in": "query" }
            ]),
        );
    }

    #[test]
    fn apply_copy_source_not_found_errors_and_rolls_back() {
        let o = ovl(vec![Action {
            target: "$.dest".into(),
            copy: Some("$.missing".into()),
            ..Default::default()
        }]);
        let mut doc = json!({ "dest": {} });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        match &err.kind {
            ApplyErrorKind::CopySourceNotFound(s) => assert_eq!(s, "$.missing"),
            other => panic!("expected CopySourceNotFound, got {other:?}"),
        }
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_copy_source_multiple_matches_errors_and_rolls_back() {
        let o = ovl(vec![Action {
            target: "$.dest".into(),
            copy: Some("$.items[*]".into()),
            ..Default::default()
        }]);
        let mut doc = json!({
            "dest": {},
            "items": [ { "a": 1 }, { "b": 2 } ]
        });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert!(matches!(err.kind, ApplyErrorKind::CopySourceMultiple(_)));
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_copy_with_invalid_jsonpath_errors_and_rolls_back() {
        let o = ovl(vec![Action {
            target: "$.dest".into(),
            copy: Some("not a path".into()),
            ..Default::default()
        }]);
        let mut doc = json!({ "dest": {} });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert!(matches!(err.kind, ApplyErrorKind::InvalidJsonPath(_)));
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_remove_with_copy_field_still_removes() {
        // Validation would flag this combination, but apply is
        // defensive: `remove: true` wins (matches the `is_remove`
        // branch above any update/copy logic).
        let o = ovl(vec![Action {
            target: "$.paths['/x']".into(),
            copy: Some("$.somewhere".into()),
            remove: Some(true),
            ..Default::default()
        }]);
        let mut doc = json!({ "paths": { "/x": { "get": {} } } });
        let r = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(r.actions[0].operation, Operation::Remove);
        assert!(!doc["paths"].as_object().unwrap().contains_key("/x"));
    }

    #[test]
    fn apply_update_then_copy_actions_compose() {
        let o = ovl(vec![
            Action {
                target: "$.dest".into(),
                update: Some(json!({ "tag": "first" })),
                ..Default::default()
            },
            Action {
                target: "$.dest".into(),
                copy: Some("$.src".into()),
                ..Default::default()
            },
        ]);
        let mut doc = json!({
            "dest": {},
            "src": { "tag": "second", "extra": 7 }
        });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        // First action added tag=first. Second action copies src
        // (tag=second, extra=7) over dest, with merge semantics:
        // tag is replaced by "second", extra is added.
        assert_eq!(doc["dest"], json!({ "tag": "second", "extra": 7 }),);
    }

    #[test]
    fn apply_zero_match_default_is_no_op() {
        let o = ovl(vec![Action {
            target: "$.nope".into(),
            update: Some(json!({})),
            ..Default::default()
        }]);
        let mut doc = json!({ "foo": 1 });
        let snapshot = doc.clone();
        let r = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(r.actions[0].matched, 0);
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_zero_match_strict_errors_and_rolls_back() {
        let o = ovl(vec![Action {
            target: "$.nope".into(),
            update: Some(json!({})),
            ..Default::default()
        }]);
        let mut doc = json!({ "foo": 1 });
        let snapshot = doc.clone();
        let err = o
            .apply(&mut doc, ApplyOptions::ErrorOnZeroMatch.into())
            .unwrap_err();
        assert_eq!(err.kind, ApplyErrorKind::ZeroMatch);
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
        assert_eq!(err.kind, ApplyErrorKind::PrimitiveActionTarget);
        assert_eq!(doc, snapshot);
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
    fn apply_mixed_kind_lax_treats_each_match_per_its_kind() {
        // Exercises both the uniform_kinds check (no strict opt) and
        // the per-match Array vs Object branches.
        let o = ovl(vec![Action {
            target: "$.choices[*]".into(),
            update: Some(json!({ "z": 1 })),
            ..Default::default()
        }]);
        let mut doc = json!({
            "choices": [ { "a": 1 }, [ 1, 2 ] ]
        });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["choices"][0], json!({ "a": 1, "z": 1 }));
        assert_eq!(doc["choices"][1], json!([1, 2, { "z": 1 }]));
    }

    #[test]
    fn apply_action_with_no_effect_reports_matched_zero() {
        // Defensive: validate flags this, but if it reaches apply
        // anyway it's a true no-op.
        let o = ovl(vec![Action {
            target: "$.foo".into(),
            ..Default::default()
        }]);
        let mut doc = json!({ "foo": { "a": 1 } });
        let snapshot = doc.clone();
        let r = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(r.actions[0].matched, 0);
        assert_eq!(doc, snapshot);
    }

    #[cfg(feature = "v1_0")]
    #[test]
    fn from_v1_0_overlay_upconverts_additive_fields_to_none() {
        use crate::v1_0;
        let src = v1_0::Overlay {
            overlay: v1_0::version::Version::V1_0_0(),
            info: v1_0::Info {
                title: "T".into(),
                version: "1".into(),
                extensions: None,
            },
            extends: Some("./base.yaml".into()),
            actions: vec![v1_0::Action {
                target: "$.info".into(),
                description: Some("Note".into()),
                update: Some(json!({ "x": 1 })),
                remove: None,
                extensions: None,
            }],
            extensions: None,
        };
        let dst: Overlay = src.into();
        assert_eq!(dst.overlay, Version::V1_1_0());
        assert_eq!(dst.info.title, "T");
        assert!(dst.info.description.is_none());
        assert_eq!(dst.extends.as_deref(), Some("./base.yaml"));
        assert_eq!(dst.actions.len(), 1);
        assert_eq!(dst.actions[0].target, "$.info");
        assert!(dst.actions[0].copy.is_none());
        assert_eq!(dst.actions[0].update.as_ref().unwrap(), &json!({ "x": 1 }));
    }
}
