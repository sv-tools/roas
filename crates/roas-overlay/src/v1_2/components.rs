//! Overlay v1.2 `components`: reusable actions that `actions` entries
//! refer to by `$ref`.
//!
//! See [Components Object](https://spec.openapis.org/overlay/v1.2.0.html#components-object)
//! and [Reusable Action Object](https://spec.openapis.org/overlay/v1.2.0.html#reusable-action-object).

use crate::apply::ActionReferenceError;
use crate::common::apply::unescape_pointer_segment;
use crate::v1_2::action::validate_operation_fields;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The Components Object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Components {
    /// Reusable actions by name, referred to as
    /// `#/components/actions/<name>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, ReusableAction>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, Value>>,
}

/// What a `$ref` to a reusable action starts with.
const ACTION_REFERENCE_PREFIX: &str = "#/components/actions/";

/// The reusable action `reference` (`#/components/actions/<name>`)
/// names in `components`: [`ActionReferenceError::Malformed`] when
/// `reference` is not of that form, [`ActionReferenceError::NotFound`]
/// when there are no components or no action of that name.
pub(crate) fn find_reusable_action<'a>(
    components: Option<&'a Components>,
    reference: &str,
) -> Result<&'a ReusableAction, ActionReferenceError> {
    let name = action_name(reference)
        .ok_or_else(|| ActionReferenceError::Malformed(reference.to_owned()))?;
    components
        .and_then(|components| components.actions.get(&name))
        .ok_or_else(|| ActionReferenceError::NotFound(reference.to_owned()))
}

/// The unescaped name in `#/components/actions/<name>`, per the
/// schema's pattern `^#/components/actions/(?:[^~/]|~0|~1)+$`.
fn action_name(reference: &str) -> Option<String> {
    let escaped = reference.strip_prefix(ACTION_REFERENCE_PREFIX)?;
    let valid = !escaped.is_empty() && !escaped.contains('/') && has_only_valid_escapes(escaped);
    valid.then(|| unescape_pointer_segment(escaped))
}

/// Whether every `~` in `segment` starts `~0` or `~1` (RFC 6901).
fn has_only_valid_escapes(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    segment
        .match_indices('~')
        .all(|(at, _)| matches!(bytes.get(at + 1), Some(b'0' | b'1')))
}

impl ValidateWithContext for Components {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.in_field("actions", |ctx| {
            for (name, action) in &self.actions {
                ctx.in_field(name, |ctx| action.validate_with_context(ctx));
            }
        });
    }
}

/// A Reusable Action Object: action fields without a `target`, applied
/// wherever a Reusable Action Reference Object names it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ReusableAction {
    /// Describes the reusable action itself; a resolved action takes
    /// its description from `fields` or the reference instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The action's fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<ReusableActionFields>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, Value>>,
}

impl ValidateWithContext for ReusableAction {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(fields) = &self.fields {
            ctx.in_field("fields", |ctx| fields.validate_with_context(ctx));
        }
    }
}

/// The `fields` of a Reusable Action Object: an Action Object without
/// `target`, every field optional.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ReusableActionFields {
    /// Description of the resolved action, unless the reference sets
    /// its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// As the Action Object's `update`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<Value>,

    /// As the Action Object's `copy`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<String>,

    /// As the Action Object's `remove`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remove: Option<bool>,

    /// `x-`-prefixed Specification Extensions, carried into the
    /// resolved action.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, Value>>,
}

impl ValidateWithContext for ReusableActionFields {
    /// The checks that hold for `fields` on their own. Whether the
    /// resolved action does anything is checked where it is referenced.
    fn validate_with_context(&self, ctx: &mut Context) {
        validate_operation_fields(
            self.update.as_ref(),
            self.copy.as_deref(),
            self.remove == Some(true),
            ctx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn deserialize_round_trips() {
        let wire = json!({
            "actions": {
                "errors": {
                    "description": "Adds error responses",
                    "fields": { "update": { "responses": {} }, "x-a": 1 },
                    "x-b": 2
                }
            },
            "x-c": 3
        });
        let components: Components = serde_json::from_value(wire.clone()).unwrap();
        let errors = &components.actions["errors"];
        assert_eq!(errors.description.as_deref(), Some("Adds error responses"));
        let fields = errors.fields.as_ref().unwrap();
        assert_eq!(fields.update, Some(json!({ "responses": {} })));
        assert_eq!(serde_json::to_value(&components).unwrap(), wire);
    }

    #[test]
    fn action_name_follows_the_schema_pattern() {
        assert_eq!(action_name("#/components/actions/a").as_deref(), Some("a"));
        assert_eq!(
            action_name("#/components/actions/~1paths~0x").as_deref(),
            Some("/paths~x"),
        );
        for bad in [
            "#/components/actions/",
            "#/components/actions/a/b",
            "#/components/actions/~",
            "#/components/actions/~2",
            "#/components/action/a",
        ] {
            assert_eq!(action_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn validate_reports_conflicting_fields_under_their_name() {
        let components: Components = serde_json::from_value(json!({
            "actions": {
                "bad": { "fields": { "update": {}, "copy": "$.x" } },
                "partial": { "fields": { "description": "No operation yet" } }
            }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        components.validate_with_context(&mut ctx);
        let errs: Vec<String> = ctx.errors.iter().map(ToString::to_string).collect();
        assert_eq!(
            errs,
            ["#.components.actions.bad.fields: `update` and `copy` are mutually exclusive"],
        );
    }
}
