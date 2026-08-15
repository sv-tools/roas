//! The AsyncAPI `Reference` object and the `RefOr<T>` wrapper.
//!
//! Per the [Reference Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#referenceObject):
//! a single `$ref` holding a URI reference, resolved per RFC 3986. Unlike
//! the OpenAPI 3.1 Reference Object it carries no `summary` /
//! `description` overrides.
//!
//! Identical across AsyncAPI versions, so it lives in `common` (mirroring
//! how `roas` consolidated its `reference` type and `roas-arazzo` its
//! `reusable` one). AsyncAPI 3 accepts a reference almost everywhere an
//! object can appear; [`RefOr`] models that `oneOf`.

use crate::common::pointer;
use crate::validation::{Context, ValidateWithContext};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

/// A `$ref` to another part of this document, or to an external one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct Reference {
    /// **Required** The reference URI, e.g.
    /// `#/components/messages/userSignedUp`.
    #[serde(rename = "$ref")]
    pub reference: String,
}

impl Reference {
    /// The fragment part of a document-local reference.
    ///
    /// Returns `Some("/components/messages/x")` for `#/components/messages/x`
    /// and `None` for anything that targets another document (which this
    /// crate cannot resolve without a loader).
    #[must_use]
    pub fn local_pointer(&self) -> Option<&str> {
        self.reference.strip_prefix('#')
    }

    /// Whether the reference targets a resource outside this document.
    #[must_use]
    pub fn is_external(&self) -> bool {
        !self.reference.is_empty() && self.local_pointer().is_none()
    }

    /// The last segment of a local pointer, i.e. the component key for a
    /// `#/components/<kind>/<key>` reference.
    ///
    /// Returns `None` when the reference is external or has no segments.
    /// RFC 6901 escapes (`~1` → `/`, `~0` → `~`) are decoded, so a key
    /// containing a slash round-trips.
    #[must_use]
    pub fn local_key(&self) -> Option<String> {
        let tokens = pointer::tokens(self.local_pointer()?)?;
        let last = tokens.last()?;
        (!last.is_empty()).then(|| last.clone())
    }

    /// Whether this is a local reference into `#/components/<kind>/…`,
    /// returning the component key when it is.
    #[must_use]
    pub fn component_key(&self, kind: &str) -> Option<String> {
        let tokens = pointer::tokens(self.local_pointer()?)?;
        match tokens.as_slice() {
            [components, this_kind, key] if components == "components" && this_kind == kind => {
                (!key.is_empty()).then(|| key.clone())
            }
            _ => None,
        }
    }
}

/// Report a `$ref` that leaves the document, when the caller asked for
/// a self-contained one.
///
/// Shared with the version modules that carry `$ref` as a *field* of an
/// object — an AsyncAPI 2.6 Channel Item, a Schema Object — rather than
/// as a whole [`Reference`], so
/// [`ErrorOnExternalReference`](crate::validation::ValidationOptions::ErrorOnExternalReference)
/// means the same thing wherever a reference appears.
pub(crate) fn check_external(ctx: &mut Context, reference: &str) {
    let is_external = !reference.is_empty() && !reference.starts_with('#');
    if is_external && ctx.is_option(crate::validation::ValidationOptions::ErrorOnExternalReference)
    {
        ctx.error_field(
            "$ref",
            format!("external reference `{reference}` cannot be resolved by this crate"),
        );
    }
}

impl ValidateWithContext for Reference {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("$ref", &self.reference);
        check_external(ctx, &self.reference);
        crate::common::resolve::check_names_something(ctx, &self.reference);
    }
}

/// Either a concrete object `T` or a [`Reference`] to one.
///
/// Serializes untagged (the inner object directly). Deserialization is
/// hand-written rather than `#[serde(untagged)]`: it dispatches on the
/// presence of the discriminating `$ref` key in a single pass and then
/// deserializes only the chosen variant, so a malformed `Item` surfaces
/// its real error (e.g. `missing field \`host\``) instead of the opaque
/// "data did not match any variant".
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum RefOr<T> {
    Reference(Reference),
    Item(T),
}

impl<T> RefOr<T> {
    /// The concrete item, if this is not a reference.
    pub fn item(&self) -> Option<&T> {
        match self {
            RefOr::Item(t) => Some(t),
            RefOr::Reference(_) => None,
        }
    }

    /// The reference, if this is not a concrete item.
    pub fn reference(&self) -> Option<&Reference> {
        match self {
            RefOr::Reference(r) => Some(r),
            RefOr::Item(_) => None,
        }
    }
}

impl<'de, T> Deserialize<'de> for RefOr<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("$ref").is_some() {
            serde_json::from_value(value)
                .map(RefOr::Reference)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(RefOr::Item)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl<T: ValidateWithContext> ValidateWithContext for RefOr<T> {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            RefOr::Reference(r) => r.validate_with_context(ctx),
            RefOr::Item(t) => t.validate_with_context(ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::ValidationOptions;
    use enumset::EnumSet;
    use serde_json::json;

    /// Minimal stand-in for a concrete component (keeps these tests
    /// independent of any version feature).
    #[derive(Debug, Deserialize, Default, PartialEq)]
    struct Demo {
        name: String,
    }

    impl ValidateWithContext for Demo {
        fn validate_with_context(&self, ctx: &mut Context) {
            ctx.require_non_empty("name", &self.name);
        }
    }

    #[test]
    fn reference_round_trips_under_dollar_ref() {
        let r: Reference =
            serde_json::from_value(json!({ "$ref": "#/components/messages/a" })).unwrap();
        assert_eq!(r.reference, "#/components/messages/a");
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({ "$ref": "#/components/messages/a" })
        );
    }

    #[test]
    fn local_pointer_and_external_classification() {
        let local = Reference {
            reference: "#/channels/user".into(),
        };
        assert_eq!(local.local_pointer(), Some("/channels/user"));
        assert!(!local.is_external());

        let external = Reference {
            reference: "./other.yaml#/channels/user".into(),
        };
        assert_eq!(external.local_pointer(), None);
        assert!(external.is_external());

        // An empty `$ref` is invalid rather than external; the
        // non-empty check reports it.
        let empty = Reference::default();
        assert!(!empty.is_external());
    }

    #[test]
    fn local_key_returns_last_segment_unescaped() {
        let r = Reference {
            reference: "#/components/messages/user~1signed~0up".into(),
        };
        assert_eq!(r.local_key().as_deref(), Some("user/signed~up"));

        let external = Reference {
            reference: "other.yaml#/x".into(),
        };
        assert_eq!(external.local_key(), None);

        let trailing = Reference {
            reference: "#/components/messages/".into(),
        };
        assert_eq!(trailing.local_key(), None);
    }

    #[test]
    fn component_key_matches_only_the_requested_kind() {
        let r = Reference {
            reference: "#/components/messages/signup".into(),
        };
        assert_eq!(r.component_key("messages").as_deref(), Some("signup"));
        assert_eq!(r.component_key("channels"), None);

        let nested = Reference {
            reference: "#/components/messages/signup/payload".into(),
        };
        assert_eq!(nested.component_key("messages"), None);

        let not_components = Reference {
            reference: "#/channels/user".into(),
        };
        assert_eq!(not_components.component_key("channels"), None);
    }

    #[test]
    fn component_keys_are_decoded_too() {
        let r = Reference {
            reference: "#/components/securitySchemes/oauth%2Dscheme".into(),
        };
        assert_eq!(
            r.component_key("securitySchemes").as_deref(),
            Some("oauth-scheme")
        );

        // Wrong kind, extra depth, and malformed escapes all decline.
        assert_eq!(r.component_key("messages"), None);
        let deep = Reference {
            reference: "#/components/messages/a/b".into(),
        };
        assert_eq!(deep.component_key("messages"), None);
        let malformed = Reference {
            reference: "#/components/messages/a~9b".into(),
        };
        assert_eq!(malformed.component_key("messages"), None);
    }

    #[test]
    fn ref_or_picks_reference_for_dollar_ref_key() {
        let v: RefOr<Demo> = serde_json::from_value(json!({ "$ref": "#/x" })).unwrap();
        assert_eq!(v.reference().map(|r| r.reference.as_str()), Some("#/x"));
        assert!(v.item().is_none());
    }

    #[test]
    fn ref_or_picks_item_for_concrete_object() {
        let v: RefOr<Demo> = serde_json::from_value(json!({ "name": "n" })).unwrap();
        assert_eq!(v.item().map(|d| d.name.as_str()), Some("n"));
        assert!(v.reference().is_none());
    }

    #[test]
    fn malformed_item_surfaces_inner_error_not_opaque_variant_error() {
        let err = serde_json::from_value::<RefOr<Demo>>(json!({ "other": 1 })).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing field"), "got: {msg}");
        assert!(!msg.contains("did not match any variant"), "got: {msg}");
    }

    #[test]
    fn round_trips_through_yaml() {
        let v: RefOr<Demo> = serde_yaml_ng::from_str("name: n\n").unwrap();
        assert!(matches!(v, RefOr::Item(_)));
        let r: RefOr<Demo> = serde_yaml_ng::from_str("$ref: '#/x'\n").unwrap();
        assert!(matches!(r, RefOr::Reference(_)));
    }

    #[test]
    fn validate_reference_rejects_empty_ref() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.channels.user");
        RefOr::<Demo>::Reference(Reference::default()).validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.channels.user.$ref: must not be empty")
        );
    }

    #[test]
    fn external_reference_only_reported_under_option() {
        let external = Reference {
            reference: "./other.yaml#/channels/user".into(),
        };

        let mut quiet = Context::with_path(EnumSet::empty(), "#.channels.user");
        external.validate_with_context(&mut quiet);
        assert!(quiet.errors.is_empty(), "got: {:?}", quiet.errors);

        let mut strict = Context::with_path(
            EnumSet::only(ValidationOptions::ErrorOnExternalReference),
            "#.channels.user",
        );
        external.validate_with_context(&mut strict);
        assert!(
            strict
                .errors
                .iter()
                .any(|e| e.contains("external reference")),
            "got: {:?}",
            strict.errors
        );
    }

    #[test]
    fn validate_item_delegates_to_inner() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.servers.prod");
        RefOr::Item(Demo::default()).validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.servers.prod.name: must not be empty")
        );
    }
}
