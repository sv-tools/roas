//! XML Object

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_optional_uri};
use crate::v3_2::spec::Spec;
use crate::validation::Options;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A metadata object that allows for more fine-tuned XML model definitions.
///
/// When using arrays, XML element names are not inferred (for singular/plural forms)
/// and the `name` property SHOULD be used to add that information.
/// See examples for expected behavior.
///
/// Examples:
///
/// * String item:
/// ```yaml
/// animals:
///   type: string
/// ```
///
/// ```xml
/// <animals>...</animals>
/// ```
///
/// * Array of strings:
/// ```yaml
/// animals:
///   type: array
///   items:
///     type: string
/// ```
///
/// ```xml
/// <animals>...</animals>
/// <animals>...</animals>
/// <animals>...</animals>
/// ```
///
/// * String with name replacement:
/// ```yaml
/// animals:
///   type: string
///   xml:
///     name: animal
/// ```
///
/// ```xml
/// <animal>...</animal>
/// ```
///
/// * XML Attribute, Prefix and Namespace
/// ```yaml
/// Person:
///   type: object
///   properties:
///     id:
///       type: integer
///       format: int32
///       xml:
///         attribute: true
///     name:
///       type: string
///       xml:
///         namespace: https://swagger.io/schema/sample
///         prefix: sample
/// ```
///
/// ```xml
/// <Person id="123">
///     <sample:name xmlns:sample="https://swagger.io/schema/sample">example</sample:name>
/// </Person>
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct XML {
    /// Replaces the name of the element/attribute used for the described schema property.
    /// When defined within `items`, it will affect the name of the individual XML elements
    /// within the list.
    /// When defined alongside `type` being `array` (outside the `items`),
    /// it will affect the wrapping element and only if `wrapped` is `true`.
    /// If `wrapped` is `false`, it will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The URI of the namespace definition.
    /// Value MUST be in the form of an absolute URI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// The prefix to be used for the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Declares whether the property definition translates to an attribute instead of an element.
    /// Default value is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribute: Option<bool>,

    /// MAY be used only for an array definition.
    /// Signifies whether the array is wrapped (for example, `<books><book/><book/></books>`) or
    /// unwrapped (`<book/><book/>`).
    /// Default value is `false`.
    /// The definition takes effect only when defined alongside `type` being `array` (outside the `items`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapped: Option<bool>,

    /// XML node-type hint (added in OAS 3.2). One of `element` (default),
    /// `attribute`, `text`, `cdata`, or `none`. When `nodeType` is set,
    /// the legacy boolean fields `attribute` and `wrapped` MUST NOT be
    /// used — `nodeType` supersedes them.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "nodeType")]
    pub node_type: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for XML {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_optional_uri(&self.namespace, ctx, format!("{path}.namespace"));
        // The OAS XML Object spec requires `namespace` to be an *absolute*
        // URI: a relative reference like `#/foo` or `bar/baz` is not
        // valid. Enforce a present `scheme:` prefix per RFC 3986
        // §3.1: `scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
        // Skip when the value already failed `validate_optional_uri`
        // (whitespace / control chars) so users see a single,
        // most-relevant error.
        if let Some(ns) = &self.namespace
            && !ns.is_empty()
            && !ctx.is_option(Options::IgnoreInvalidUrls)
            && !ns
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            let mut chars = ns.chars();
            let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
            let scheme_end = ns.find(':');
            let scheme_ok = first_ok
                && scheme_end.is_some_and(|i| {
                    ns[..i]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                });
            if !scheme_ok {
                ctx.error(
                    format!("{path}.namespace"),
                    format_args!("must be an absolute URI (with `<scheme>:` prefix), found `{ns}`"),
                );
            }
        }
        if let Some(nt) = &self.node_type {
            const ALLOWED: &[&str] = &["element", "attribute", "text", "cdata", "none"];
            if !ALLOWED.contains(&nt.as_str()) {
                ctx.error(
                    format!("{path}.nodeType"),
                    format_args!(
                        "must be one of `element`, `attribute`, `text`, `cdata`, `none`, found `{nt}`"
                    ),
                );
            }
            // OAS 3.2 supersedes the legacy `attribute`/`wrapped` booleans
            // with `nodeType`; mixing them is ambiguous.
            if self.attribute.is_some() {
                ctx.error(
                    path.clone(),
                    "`attribute` MUST NOT be present when `nodeType` is set",
                );
            }
            if self.wrapped.is_some() {
                ctx.error(
                    path.clone(),
                    "`wrapped` MUST NOT be present when `nodeType` is set",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Options;

    #[test]
    fn serialize() {
        assert_eq!(
            serde_json::to_string(&XML::default()).unwrap(),
            "{}",
            "empty object"
        );

        assert_eq!(
            serde_json::to_value(&XML {
                name: Some("name".to_owned()),
                namespace: Some("https://example.com/schema/sample".to_owned()),
                prefix: Some("sample".to_owned()),
                attribute: Some(true),
                wrapped: Some(true),
                node_type: None,
                extensions: {
                    let mut map = BTreeMap::new();
                    map.insert("x-internal-id".to_owned(), serde_json::Value::Null);
                    Some(map)
                },
            })
            .unwrap(),
            serde_json::json!({
                "name": "name",
                "namespace": "https://example.com/schema/sample",
                "prefix": "sample",
                "attribute": true,
                "wrapped": true,
                "x-internal-id": null,
            }),
            "all fields"
        );
    }

    #[test]
    fn deserialize() {
        assert_eq!(
            serde_json::from_value::<XML>(serde_json::json!({})).unwrap(),
            XML::default(),
            "empty object"
        );

        assert_eq!(
            serde_json::from_value::<XML>(serde_json::json!({
                "name": "name",
                "namespace": "https://example.com/schema/sample",
                "prefix": "sample",
                "attribute": true,
                "wrapped": true,
                "x-internal-id": null,
            }))
            .unwrap(),
            XML {
                name: Some("name".to_owned()),
                namespace: Some("https://example.com/schema/sample".to_owned()),
                prefix: Some("sample".to_owned()),
                attribute: Some(true),
                wrapped: Some(true),
                node_type: None,
                extensions: {
                    let mut map = BTreeMap::new();
                    map.insert("x-internal-id".to_owned(), serde_json::Value::Null);
                    Some(map)
                },
            },
            "all fields"
        );
    }

    #[test]
    fn validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        XML {
            name: Some("name".to_owned()),
            namespace: Some("https://example.com/schema/sample".to_owned()),
            prefix: Some("sample".to_owned()),
            attribute: Some(true),
            wrapped: Some(true),
            node_type: None,
            extensions: None,
        }
        .validate_with_context(&mut ctx, "xml".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        XML {
            namespace: Some("https://example.com/schema/sample".to_owned()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "xml".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        // Non-HTTP absolute URIs (urn:, mailto:, etc.) are accepted —
        // OAS 3.2 specifies an absolute URI here, not specifically a URL.
        let mut ctx = Context::new(&spec, Options::new());
        XML {
            namespace: Some("urn:example:ns:1".to_owned()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "xml".to_owned());
        assert!(ctx.errors.is_empty(), "urn accepted: {:?}", ctx.errors);

        // Whitespace / control chars are rejected.
        let mut ctx = Context::new(&spec, Options::new());
        XML {
            namespace: Some("not a uri".to_owned()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "xml".to_owned());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must be a valid URI")),
            "invalid URI: {:?}",
            ctx.errors
        );

        // Relative refs (no scheme) are rejected: namespace MUST be an
        // absolute URI per the OAS XML Object spec.
        for rel in ["#/foo", "bar/baz", "/relative/path"] {
            let mut ctx = Context::new(&spec, Options::new());
            XML {
                namespace: Some(rel.to_owned()),
                ..Default::default()
            }
            .validate_with_context(&mut ctx, "xml".to_owned());
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains("must be an absolute URI")),
                "relative `{rel}` rejected: {:?}",
                ctx.errors
            );
        }
    }
}
