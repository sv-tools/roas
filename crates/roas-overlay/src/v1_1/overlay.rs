//! Overlay v1.1 root document.

use crate::apply::{Apply, ApplyError, ApplyOptions, ApplyReport};
use crate::common::action::{ActionFields, PrimitiveTargets, apply_action};
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

        ctx.in_field("info", |ctx| self.info.validate_with_context(ctx));

        if self.actions.is_empty() {
            ctx.error_field("actions", "must contain at least one entry");
        }
        for (i, action) in self.actions.iter().enumerate() {
            ctx.in_index("actions", i, |ctx| action.validate_with_context(ctx));
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
            let outcome = apply_action(
                index,
                &action_fields(action),
                &mut working,
                options,
                PrimitiveTargets::Reject,
            )?;
            report.actions.push(outcome);
        }

        *target = working;
        Ok(report)
    }
}

/// The fields the shared apply step reads from a v1.1 action.
fn action_fields(action: &Action) -> ActionFields<'_> {
    ActionFields {
        target: &action.target,
        update: action.update.as_ref(),
        copy: action.copy.as_deref(),
        remove: action.is_remove(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{ApplyErrorKind, Operation};
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
    fn apply_copy_against_array_target_appends_object_source_as_single_entry() {
        // v1.1 §3.3: an object/primitive merge value is *appended* as
        // one element to an array target.
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
    fn apply_update_array_value_into_array_target_concatenates() {
        // v1.1 §3.3: an *array* merge value is concatenated with the
        // target array (element-wise), not nested as one element.
        // This is the v1.1 divergence from v1.0.
        let o = ovl(vec![Action {
            target: "$.tags".into(),
            update: Some(json!(["new-a", "new-b"])),
            ..Default::default()
        }]);
        let mut doc = json!({ "tags": ["existing"] });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["tags"], json!(["existing", "new-a", "new-b"]));
    }

    #[test]
    fn apply_copy_array_value_into_array_target_concatenates() {
        let o = ovl(vec![Action {
            target: "$.dst".into(),
            copy: Some("$.src".into()),
            ..Default::default()
        }]);
        let mut doc = json!({
            "dst": [1, 2],
            "src": [3, 4]
        });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["dst"], json!([1, 2, 3, 4]));
    }

    #[test]
    fn apply_update_object_value_into_array_target_appends() {
        let o = ovl(vec![Action {
            target: "$.list".into(),
            update: Some(json!({ "added": true })),
            ..Default::default()
        }]);
        let mut doc = json!({ "list": [1, 2] });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["list"], json!([1, 2, { "added": true }]));
    }

    #[test]
    fn apply_conflicting_update_and_copy_errors_and_rolls_back() {
        // Validate flags this; apply fails fast rather than silently
        // dropping `update`.
        let o = ovl(vec![Action {
            target: "$.dest".into(),
            update: Some(json!({ "from": "update" })),
            copy: Some("$.src".into()),
            ..Default::default()
        }]);
        let mut doc = json!({ "dest": {}, "src": { "from": "copy" } });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert_eq!(err.kind, ApplyErrorKind::ConflictingMergeSources);
        assert_eq!(doc, snapshot);
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
