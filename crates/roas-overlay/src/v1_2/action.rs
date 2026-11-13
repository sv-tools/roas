//! Overlay v1.2 `actions` entries: an [`Action`] object, or a
//! [`ReusableActionReference`] to an entry of `components.actions`.
//!
//! See [Action Object](https://spec.openapis.org/overlay/v1.2.0.html#action-object)
//! and [Reusable Action Reference Object](https://spec.openapis.org/overlay/v1.2.0.html#reusable-action-reference-object).
//! The Action Object is unchanged from v1.1, but v1.2 lets its `target`
//! select primitive nodes: `update` and `copy` replace them with a
//! primitive value, and `remove` removes them.

use crate::apply::ActionReferenceError;
use crate::v1_2::components::{Components, find_reusable_action};
use crate::validation::{Context, ValidateWithContext};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Action {
    /// **Required** RFC 9535 JSONPath selecting the nodes to act on.
    pub target: String,

    /// CommonMark-flavored description of the action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Value merged into the selected nodes; a primitive value replaces
    /// selected primitive nodes. Mutually exclusive with `copy` and with
    /// `remove: true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<Value>,

    /// RFC 9535 JSONPath selecting exactly one source node in the
    /// working document. The source's value is merged into each
    /// `target` node using the same rules as `update`. Mutually
    /// exclusive with `update` and with `remove: true`.
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
    pub extensions: Option<BTreeMap<String, Value>>,
}

impl Action {
    /// Returns `true` if this action's `remove` field is set to `true`.
    #[must_use]
    pub fn is_remove(&self) -> bool {
        self.remove == Some(true)
    }

    /// Returns `true` if the action does something: it sets `update`,
    /// `copy` or `remove: true`.
    #[must_use]
    pub fn has_operation(&self) -> bool {
        self.is_remove() || self.update.is_some() || self.copy.is_some()
    }
}

/// Record the diagnostics `update`, `copy` and `remove` raise on their
/// own, wherever they are set: an invalid `copy` JSONPath, and the
/// combinations the spec makes mutually exclusive.
pub(crate) fn validate_operation_fields(
    update: Option<&Value>,
    copy: Option<&str>,
    remove: bool,
    ctx: &mut Context,
) {
    if let Some(copy) = copy {
        ctx.require_jsonpath("copy", copy);
    }

    // The spec says `update` "has no impact if the `remove` field of
    // this action object is `true`"; the same applies to `copy`. All
    // three combinations are rejected because they almost certainly
    // indicate an authoring mistake.
    if remove && update.is_some() {
        ctx.error("`remove: true` and `update` are mutually exclusive");
    }
    if remove && copy.is_some() {
        ctx.error("`remove: true` and `copy` are mutually exclusive");
    }
    if update.is_some() && copy.is_some() {
        ctx.error("`update` and `copy` are mutually exclusive");
    }
}

impl ValidateWithContext for Action {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_jsonpath("target", &self.target);
        validate_operation_fields(
            self.update.as_ref(),
            self.copy.as_deref(),
            self.is_remove(),
            ctx,
        );

        // Catch the silent-no-op authoring bug.
        if !self.has_operation() {
            ctx.error("action must specify one of `update`, `copy`, or `remove: true`");
        }
    }
}

/// An Overlay v1.2 Reusable Action Reference Object: applies the
/// reusable action `$ref` names to its own `target`.
///
/// The schema allows only `$ref`, `target`, `description` and `x-`
/// extensions here; `update`, `copy` and `remove` belong in the
/// reusable action's `fields`, and deserializing an [`ActionItem`]
/// that sets them beside `$ref` fails.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ReusableActionReference {
    /// **Required** `#/components/actions/<name>`, the name escaped per
    /// RFC 6901 (`~` as `~0`, `/` as `~1`).
    #[serde(rename = "$ref")]
    pub reference: String,

    /// **Required** RFC 9535 JSONPath selecting the nodes to act on.
    pub target: String,

    /// Overrides the `description` of the referenced action's `fields`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// `x-`-prefixed Specification Extensions. On resolution they are
    /// merged over the referenced `fields`' extensions, winning on
    /// shared keys.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, Value>>,
}

impl ReusableActionReference {
    /// The plain action this reference stands for: the referenced
    /// reusable action's `fields`, with this reference's `target`, its
    /// `description` when it has one, and its extensions merged over
    /// those of `fields`.
    ///
    /// # Errors
    ///
    /// [`ActionReferenceError::Malformed`] when `$ref` is not
    /// `#/components/actions/<name>`; [`ActionReferenceError::NotFound`]
    /// when `components.actions` has no such entry.
    pub fn resolve(&self, components: Option<&Components>) -> Result<Action, ActionReferenceError> {
        let reusable = find_reusable_action(components, &self.reference)?;
        let fields = reusable.fields.clone().unwrap_or_default();
        let description = self.description.clone().or(fields.description);
        let extensions = merge_extensions(fields.extensions, self.extensions.as_ref());
        Ok(Action {
            target: self.target.clone(),
            description,
            update: fields.update,
            copy: fields.copy,
            remove: fields.remove,
            extensions,
        })
    }
}

/// `base` with every entry of `overrides` inserted over it; `None` when
/// both are empty.
fn merge_extensions(
    base: Option<BTreeMap<String, Value>>,
    overrides: Option<&BTreeMap<String, Value>>,
) -> Option<BTreeMap<String, Value>> {
    let mut merged = base.unwrap_or_default();
    if let Some(overrides) = overrides {
        merged.extend(overrides.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

/// One entry of an Overlay v1.2 `actions` list.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ActionItem {
    /// An inline Action Object.
    Action(Action),
    /// A reference to an entry of `components.actions`.
    Reference(ReusableActionReference),
}

impl ActionItem {
    /// The entry's `target` JSONPath.
    #[must_use]
    pub fn target(&self) -> &str {
        match self {
            ActionItem::Action(action) => &action.target,
            ActionItem::Reference(reference) => &reference.target,
        }
    }

    /// The entry as a plain action: an inline action as it is, a
    /// reference resolved through
    /// [`ReusableActionReference::resolve`].
    ///
    /// # Errors
    ///
    /// The reference's [`ActionReferenceError`], when it cannot be
    /// resolved.
    pub fn resolve(
        &self,
        components: Option<&Components>,
    ) -> Result<Cow<'_, Action>, ActionReferenceError> {
        match self {
            ActionItem::Action(action) => Ok(Cow::Borrowed(action)),
            ActionItem::Reference(reference) => reference.resolve(components).map(Cow::Owned),
        }
    }
}

impl From<Action> for ActionItem {
    fn from(action: Action) -> Self {
        ActionItem::Action(action)
    }
}

impl From<ReusableActionReference> for ActionItem {
    fn from(reference: ReusableActionReference) -> Self {
        ActionItem::Reference(reference)
    }
}

/// The fields a Reusable Action Reference Object may not set.
const OPERATION_FIELDS: [&str; 3] = ["update", "copy", "remove"];

impl<'de> Deserialize<'de> for ActionItem {
    /// An entry with `$ref` is a reference, any other an inline action.
    /// A reference that also sets `update`, `copy` or `remove` is
    /// rejected rather than having those fields dropped.
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(de)?;
        if value.get("$ref").is_none() {
            return Action::deserialize(value)
                .map(ActionItem::Action)
                .map_err(D::Error::custom);
        }
        let operation_field = OPERATION_FIELDS
            .iter()
            .find(|field| value.get(**field).is_some());
        if let Some(field) = operation_field {
            return Err(D::Error::custom(format!(
                "a Reusable Action Reference Object (`$ref`) cannot set `{field}`; \
                 set it in the reusable action's `fields`",
            )));
        }
        ReusableActionReference::deserialize(value)
            .map(ActionItem::Reference)
            .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v1_2::components::{ReusableAction, ReusableActionFields};
    use enumset::EnumSet;
    use serde_json::json;

    fn validate(action: &Action) -> Vec<String> {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.actions[0]");
        action.validate_with_context(&mut ctx);
        ctx.errors.iter().map(|e| e.to_string()).collect()
    }

    fn components(name: &str, fields: ReusableActionFields) -> Components {
        Components {
            actions: BTreeMap::from([(
                name.to_owned(),
                ReusableAction {
                    fields: Some(fields),
                    ..Default::default()
                },
            )]),
            extensions: None,
        }
    }

    #[test]
    fn deserialize_inline_action() {
        let item: ActionItem =
            serde_json::from_value(json!({ "target": "$.info", "update": { "x": 1 } })).unwrap();
        let ActionItem::Action(action) = &item else {
            panic!("expected an inline action, got {item:?}");
        };
        assert_eq!(action.update, Some(json!({ "x": 1 })));
        assert_eq!(item.target(), "$.info");
    }

    #[test]
    fn deserialize_reference_round_trips() {
        let wire = json!({
            "$ref": "#/components/actions/errors",
            "target": "$.paths.*.get",
            "description": "Here",
            "x-note": 1,
        });
        let item: ActionItem = serde_json::from_value(wire.clone()).unwrap();
        let ActionItem::Reference(reference) = &item else {
            panic!("expected a reference, got {item:?}");
        };
        assert_eq!(reference.reference, "#/components/actions/errors");
        assert_eq!(item.target(), "$.paths.*.get");
        assert_eq!(serde_json::to_value(&item).unwrap(), wire);
    }

    #[test]
    fn deserialize_rejects_reference_with_operation_fields() {
        for field in OPERATION_FIELDS {
            let mut wire = json!({ "$ref": "#/components/actions/a", "target": "$" });
            wire[field] = json!(true);
            let err = serde_json::from_value::<ActionItem>(wire).unwrap_err();
            assert!(
                err.to_string().contains(&format!("cannot set `{field}`")),
                "got: {err}",
            );
        }
    }

    #[test]
    fn deserialize_reference_without_target_fails() {
        let err = serde_json::from_value::<ActionItem>(json!({ "$ref": "#/components/actions/a" }))
            .unwrap_err();
        assert!(err.to_string().contains("target"), "got: {err}");
    }

    #[test]
    fn resolve_takes_fields_then_reference_target_description_and_extensions() {
        let components = components(
            "errors",
            ReusableActionFields {
                description: Some("From fields".into()),
                update: Some(json!({ "responses": {} })),
                extensions: Some(BTreeMap::from([
                    ("x-a".to_owned(), json!("fields")),
                    ("x-b".to_owned(), json!("fields")),
                ])),
                ..Default::default()
            },
        );
        let reference = ReusableActionReference {
            reference: "#/components/actions/errors".into(),
            target: "$.paths.*.get".into(),
            description: Some("From reference".into()),
            extensions: Some(BTreeMap::from([("x-b".to_owned(), json!("reference"))])),
        };
        let action = reference.resolve(Some(&components)).unwrap();
        assert_eq!(action.target, "$.paths.*.get");
        assert_eq!(action.description.as_deref(), Some("From reference"));
        assert_eq!(action.update, Some(json!({ "responses": {} })));
        assert_eq!(
            action.extensions,
            Some(BTreeMap::from([
                ("x-a".to_owned(), json!("fields")),
                ("x-b".to_owned(), json!("reference")),
            ])),
        );
    }

    #[test]
    fn resolve_keeps_fields_description_when_reference_has_none() {
        let components = components(
            "a",
            ReusableActionFields {
                description: Some("From fields".into()),
                remove: Some(true),
                ..Default::default()
            },
        );
        let reference = ReusableActionReference {
            reference: "#/components/actions/a".into(),
            target: "$.x".into(),
            ..Default::default()
        };
        let action = reference.resolve(Some(&components)).unwrap();
        assert_eq!(action.description.as_deref(), Some("From fields"));
        assert!(action.is_remove());
        assert!(action.extensions.is_none());
    }

    #[test]
    fn resolve_unescapes_the_name() {
        let components = components(
            "a/b~c",
            ReusableActionFields {
                remove: Some(true),
                ..Default::default()
            },
        );
        let reference = ReusableActionReference {
            reference: "#/components/actions/a~1b~0c".into(),
            target: "$.x".into(),
            ..Default::default()
        };
        assert!(reference.resolve(Some(&components)).unwrap().is_remove());
    }

    #[test]
    fn resolve_reports_unknown_and_malformed_references() {
        let components = components("a", ReusableActionFields::default());
        let reference = |r: &str| ReusableActionReference {
            reference: r.into(),
            target: "$".into(),
            ..Default::default()
        };

        let missing = reference("#/components/actions/b");
        assert_eq!(
            missing.resolve(Some(&components)),
            Err(ActionReferenceError::NotFound(
                "#/components/actions/b".into()
            )),
        );
        assert!(matches!(
            reference("#/components/actions/a").resolve(None),
            Err(ActionReferenceError::NotFound(_)),
        ));
        for malformed in [
            "#/components/actions/",
            "#/components/actions/a/b",
            "#/components/actions/a~2",
            "#/components/actions/a~",
            "#/components/schemas/a",
            "other.yaml#/components/actions/a",
        ] {
            assert_eq!(
                reference(malformed).resolve(Some(&components)),
                Err(ActionReferenceError::Malformed(malformed.into())),
                "{malformed}",
            );
        }
    }

    #[test]
    fn item_resolve_borrows_inline_actions() {
        let item = ActionItem::Action(Action {
            target: "$".into(),
            remove: Some(true),
            ..Default::default()
        });
        assert!(matches!(item.resolve(None), Ok(Cow::Borrowed(_))));
    }

    #[test]
    fn validate_reports_operation_conflicts_and_no_op() {
        let conflicting = Action {
            target: "$.foo".into(),
            update: Some(json!({})),
            copy: Some("$.src".into()),
            remove: Some(true),
            ..Default::default()
        };
        let errs = validate(&conflicting);
        assert!(
            errs.iter()
                .any(|s| s.contains("`remove: true` and `update`"))
        );
        assert!(errs.iter().any(|s| s.contains("`remove: true` and `copy`")));
        assert!(errs.iter().any(|s| s.contains("`update` and `copy`")));

        let no_op = Action {
            target: "$.foo".into(),
            ..Default::default()
        };
        assert!(
            validate(&no_op)
                .iter()
                .any(|s| s.contains("must specify one of `update`, `copy`, or `remove: true`")),
        );
    }

    #[test]
    fn validate_reports_invalid_target_and_copy() {
        let a = Action {
            target: "not a path".into(),
            copy: Some(String::new()),
            ..Default::default()
        };
        let errs = validate(&a);
        assert!(errs.iter().any(|s| s.contains(".target: invalid JSONPath")));
        assert!(
            errs.iter()
                .any(|s| s == "#.actions[0].copy: must not be empty")
        );
    }
}
