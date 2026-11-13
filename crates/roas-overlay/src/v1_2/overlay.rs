//! Overlay v1.2 root document.

use crate::apply::{Apply, ApplyError, ApplyErrorKind, ApplyOptions, ApplyReport};
use crate::common::action::{ActionFields, PrimitiveTargets, apply_action};
use crate::v1_2::action::{Action, ActionItem, ReusableActionReference};
use crate::v1_2::components::Components;
use crate::v1_2::info::Info;
use crate::v1_2::version::Version;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Root Overlay v1.2 document.
///
/// See [Overlay Object](https://spec.openapis.org/overlay/v1.2.0.html#overlay-object).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Overlay {
    /// **Required** `1.2.x` per the schema's pattern `^1\.2\.\d+$`.
    pub overlay: Version,

    /// URI reference identifying this overlay document, and its base
    /// URI. Must not contain a fragment.
    #[serde(rename = "$self", skip_serializing_if = "Option::is_none")]
    pub self_uri: Option<String>,

    /// **Required** Metadata about the overlay.
    pub info: Info,

    /// URI reference identifying the target document the overlay
    /// applies to, resolved against the base URI. Must not contain a
    /// fragment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,

    /// **Required** Ordered, non-empty list of actions applied
    /// sequentially.
    pub actions: Vec<ActionItem>,

    /// Reusable actions that `actions` entries refer to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,

    /// `x-`-prefixed Specification Extensions on the root.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, Value>>,
}

impl Overlay {
    fn validate_inner(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        let mut ctx = Context::new(options);

        if let Some(self_uri) = &self.self_uri {
            reject_fragment(&mut ctx, "$self", self_uri);
        }
        ctx.in_field("info", |ctx| self.info.validate_with_context(ctx));
        if let Some(extends) = &self.extends {
            reject_fragment(&mut ctx, "extends", extends);
        }

        if self.actions.is_empty() {
            ctx.error_field("actions", "must contain at least one entry");
        }
        for (i, item) in self.actions.iter().enumerate() {
            ctx.in_index("actions", i, |ctx| match item {
                ActionItem::Action(action) => action.validate_with_context(ctx),
                ActionItem::Reference(reference) => self.validate_reference(reference, ctx),
            });
        }

        if let Some(components) = &self.components {
            ctx.in_field("components", |ctx| components.validate_with_context(ctx));
        }

        ctx.into_result()
    }

    /// A reference's own checks. The referenced `fields` are checked
    /// once, under `components`; here only that the reference resolves
    /// to an action that does something.
    fn validate_reference(&self, reference: &ReusableActionReference, ctx: &mut Context) {
        ctx.require_jsonpath("target", &reference.target);
        match reference.resolve(self.components.as_ref()) {
            Err(reason) => ctx.error_field("$ref", reason.to_string()),
            Ok(action) if !action.has_operation() => ctx.error_field(
                "$ref",
                "the referenced action's `fields` must specify one of `update`, `copy`, \
                 or `remove: true`",
            ),
            Ok(_) => {}
        }
    }
}

/// The schema's `$comment` for `$self` and `extends`: "MUST NOT contain
/// a fragment".
fn reject_fragment(ctx: &mut Context, field: &str, uri: &str) {
    if uri.contains('#') {
        ctx.error_field(field, "must not contain a fragment (`#…`)");
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

        for (index, item) in self.actions.iter().enumerate() {
            let action = item
                .resolve(self.components.as_ref())
                .map_err(|reason| ApplyError {
                    action_index: index,
                    target: item.target().to_owned(),
                    kind: ApplyErrorKind::ActionReference(reason),
                })?;
            let outcome = apply_action(
                index,
                &action_fields(&action),
                &mut working,
                options,
                PrimitiveTargets::Replace,
            )?;
            report.actions.push(outcome);
        }

        *target = working;
        Ok(report)
    }
}

/// The fields the shared apply step reads from a v1.2 action.
fn action_fields(action: &Action) -> ActionFields<'_> {
    ActionFields {
        target: &action.target,
        update: action.update.as_ref(),
        copy: action.copy.as_deref(),
        remove: action.is_remove(),
    }
}

#[cfg(feature = "v1_1")]
impl From<crate::v1_1::Overlay> for Overlay {
    /// Upconvert an Overlay v1.1 document to v1.2. Additive: the
    /// `overlay` version string becomes `1.2.0`, and every action stays
    /// an inline action; `$self` and `components` stay unset.
    fn from(o: crate::v1_1::Overlay) -> Self {
        Self {
            overlay: Version::V1_2_0(),
            self_uri: None,
            info: Info {
                title: o.info.title,
                version: o.info.version,
                description: o.info.description,
                extensions: o.info.extensions,
            },
            extends: o.extends,
            actions: o
                .actions
                .into_iter()
                .map(|a| {
                    ActionItem::Action(Action {
                        target: a.target,
                        description: a.description,
                        update: a.update,
                        copy: a.copy,
                        remove: a.remove,
                        extensions: a.extensions,
                    })
                })
                .collect(),
            components: None,
            extensions: o.extensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{ActionReferenceError, Operation};
    use serde_json::json;

    fn parse(value: Value) -> Overlay {
        serde_json::from_value(value).unwrap()
    }

    fn overlay_with(actions: Value, components: Value) -> Overlay {
        parse(json!({
            "overlay": "1.2.0",
            "info": { "title": "T", "version": "1" },
            "actions": actions,
            "components": components,
        }))
    }

    fn validation_errors(o: &Overlay) -> Vec<String> {
        match o.validate(EnumSet::empty()) {
            Ok(()) => Vec::new(),
            Err(err) => err.errors.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn deserialize_full_document_round_trips() {
        let wire = json!({
            "overlay": "1.2.0",
            "$self": "https://example.com/overlays/errors.yaml",
            "info": { "title": "T", "version": "1", "description": "D" },
            "extends": "../openapi.yaml",
            "actions": [
                { "target": "$.info", "update": { "x-a": 1 } },
                { "$ref": "#/components/actions/errors", "target": "$.paths.*.get" }
            ],
            "components": {
                "actions": { "errors": { "fields": { "update": { "responses": {} } } } }
            },
            "x-root": true
        });
        let o = parse(wire.clone());
        assert_eq!(o.overlay, Version::V1_2_0());
        assert_eq!(
            o.self_uri.as_deref(),
            Some("https://example.com/overlays/errors.yaml")
        );
        assert!(matches!(o.actions[1], ActionItem::Reference(_)));
        assert_eq!(serde_json::to_value(&o).unwrap(), wire);
        assert!(validation_errors(&o).is_empty());
    }

    #[test]
    fn deserialize_rejects_non_1_2_overlay_version() {
        let err = serde_json::from_value::<Overlay>(json!({
            "overlay": "1.1.0",
            "info": { "title": "T", "version": "1" },
            "actions": [ { "target": "$", "remove": true } ]
        }))
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("\"1.1.0\"") && msg.contains("1.2"),
            "got: {msg}"
        );
    }

    #[test]
    fn validate_rejects_fragments_in_self_and_extends() {
        let mut o = overlay_with(json!([{ "target": "$", "remove": true }]), json!({}));
        o.self_uri = Some("overlay.yaml#top".into());
        o.extends = Some("openapi.yaml#/info".into());
        assert_eq!(
            validation_errors(&o),
            [
                "#.$self: must not contain a fragment (`#…`)",
                "#.extends: must not contain a fragment (`#…`)",
            ],
        );
    }

    #[test]
    fn validate_rejects_empty_actions() {
        let o = overlay_with(json!([]), json!({}));
        assert_eq!(
            validation_errors(&o),
            ["#.actions: must contain at least one entry"]
        );
    }

    #[test]
    fn validate_reports_reference_problems_at_the_reference() {
        let o = overlay_with(
            json!([
                { "$ref": "#/components/actions/missing", "target": "$.a" },
                { "$ref": "#/components/actions/a/b", "target": "$.a" },
                { "$ref": "#/components/actions/empty", "target": "not a path" }
            ]),
            json!({ "actions": { "empty": { "fields": { "description": "Nothing" } } } }),
        );
        let errs = validation_errors(&o);
        assert_eq!(errs.len(), 4, "got: {errs:?}");
        assert!(errs[0].starts_with("#.actions[0].$ref: `$ref` \"#/components/actions/missing\""));
        assert!(errs[1].starts_with("#.actions[1].$ref: `$ref` \"#/components/actions/a/b\""));
        assert!(errs[2].starts_with("#.actions[2].target: invalid JSONPath"));
        assert!(errs[3].starts_with("#.actions[2].$ref: the referenced action's `fields`"));
    }

    #[test]
    fn validate_reports_conflicting_fields_once_under_components() {
        let o = overlay_with(
            json!([
                { "$ref": "#/components/actions/bad", "target": "$.a" },
                { "$ref": "#/components/actions/bad", "target": "$.b" }
            ]),
            json!({ "actions": { "bad": { "fields": { "update": {}, "remove": true } } } }),
        );
        assert_eq!(
            validation_errors(&o),
            ["#.components.actions.bad.fields: `remove: true` and `update` are mutually exclusive"],
        );
    }

    #[test]
    fn apply_resolves_references_per_target() {
        let o = overlay_with(
            json!([
                { "$ref": "#/components/actions/not-found", "target": "$.paths.*.get" },
                { "$ref": "#/components/actions/not-found", "target": "$.paths.*.put" }
            ]),
            json!({ "actions": { "not-found": { "fields": {
                "update": { "responses": { "404": { "description": "Not found" } } }
            } } } }),
        );
        let mut doc = json!({
            "paths": {
                "/a": { "get": { "responses": {} } },
                "/b": { "get": { "responses": {} }, "put": { "responses": {} } }
            }
        });
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions[0].matched, 2);
        assert_eq!(report.actions[1].matched, 1);
        let not_found = json!({ "404": { "description": "Not found" } });
        assert_eq!(doc["paths"]["/a"]["get"]["responses"], not_found);
        assert_eq!(doc["paths"]["/b"]["get"]["responses"], not_found);
        assert_eq!(doc["paths"]["/b"]["put"]["responses"], not_found);
    }

    #[test]
    fn apply_unresolvable_reference_errors_and_rolls_back() {
        let o = overlay_with(
            json!([
                { "target": "$.info", "update": { "title": "Changed" } },
                { "$ref": "#/components/actions/missing", "target": "$.info" }
            ]),
            json!({}),
        );
        let mut doc = json!({ "info": { "title": "API" } });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert_eq!(err.action_index, 1);
        assert_eq!(err.target, "$.info");
        assert_eq!(
            err.kind,
            ApplyErrorKind::ActionReference(ActionReferenceError::NotFound(
                "#/components/actions/missing".into()
            )),
        );
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_update_replaces_primitive_targets() {
        let o = overlay_with(
            json!([{ "target": "$.paths.*.get.summary", "update": "Renamed" }]),
            json!({}),
        );
        let mut doc = json!({
            "paths": {
                "/a": { "get": { "summary": "A" } },
                "/b": { "get": { "summary": "B" } }
            }
        });
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions[0].operation, Operation::Update);
        assert_eq!(report.actions[0].matched, 2);
        assert_eq!(doc["paths"]["/a"]["get"]["summary"], "Renamed");
        assert_eq!(doc["paths"]["/b"]["get"]["summary"], "Renamed");
    }

    #[test]
    fn apply_copy_replaces_primitive_target() {
        let o = overlay_with(
            json!([{ "target": "$.info.title", "copy": "$.info['x-public-title']" }]),
            json!({}),
        );
        let mut doc = json!({ "info": { "title": "Internal", "x-public-title": "Public" } });
        o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(doc["info"]["title"], "Public");
    }

    #[test]
    fn apply_remove_drops_primitive_array_elements() {
        // The spec's "Remove a primitive array element" example.
        let o = overlay_with(
            json!([{ "target": "$.paths.*.get.tags[?@ == 'dummy']", "remove": true }]),
            json!({}),
        );
        let mut doc = json!({
            "paths": { "/a": { "get": { "tags": ["dummy", "pets", "dummy"] } } }
        });
        let report = o.apply(&mut doc, EnumSet::empty()).unwrap();
        assert_eq!(report.actions[0].matched, 2);
        assert_eq!(doc["paths"]["/a"]["get"]["tags"], json!(["pets"]));
    }

    #[test]
    fn apply_object_value_on_primitive_target_errors_and_rolls_back() {
        let o = overlay_with(
            json!([{ "target": "$.info.title", "update": { "nested": true } }]),
            json!({}),
        );
        let mut doc = json!({ "info": { "title": "API" } });
        let snapshot = doc.clone();
        let err = o.apply(&mut doc, EnumSet::empty()).unwrap_err();
        assert_eq!(
            err.kind,
            ApplyErrorKind::NonPrimitiveValueForPrimitiveTarget
        );
        assert_eq!(doc, snapshot);
    }

    #[test]
    fn apply_mixed_primitive_and_object_targets_strict_errors() {
        let o = overlay_with(
            json!([{ "target": "$.choices[*]", "update": "x" }]),
            json!({}),
        );
        let mut doc = json!({ "choices": [ "a", { "b": 1 } ] });
        let err = o
            .apply(&mut doc, ApplyOptions::ErrorOnMixedKindMatch.into())
            .unwrap_err();
        assert_eq!(err.kind, ApplyErrorKind::MixedKindMatch);
    }

    #[cfg(feature = "v1_1")]
    #[test]
    fn from_v1_1_overlay_upconverts_to_inline_actions() {
        use crate::v1_1;
        let src = v1_1::Overlay {
            overlay: v1_1::Version::V1_1_0(),
            info: v1_1::Info {
                title: "T".into(),
                version: "1".into(),
                description: Some("D".into()),
                extensions: None,
            },
            extends: Some("./base.yaml".into()),
            actions: vec![v1_1::Action {
                target: "$.dst".into(),
                copy: Some("$.src".into()),
                ..Default::default()
            }],
            extensions: None,
        };
        let dst: Overlay = src.into();
        assert_eq!(dst.overlay, Version::V1_2_0());
        assert_eq!(dst.info.description.as_deref(), Some("D"));
        assert_eq!(dst.extends.as_deref(), Some("./base.yaml"));
        assert!(dst.self_uri.is_none());
        assert!(dst.components.is_none());
        let ActionItem::Action(action) = &dst.actions[0] else {
            panic!("expected an inline action, got {:?}", dst.actions[0]);
        };
        assert_eq!(action.copy.as_deref(), Some("$.src"));
    }
}
