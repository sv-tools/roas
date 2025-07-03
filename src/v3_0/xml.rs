//! XML Object

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{Context, ValidateWithContext, validate_optional_url};
use crate::v3_0::spec::Spec;

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
        validate_optional_url(&self.namespace, ctx, format!("{path}.namespace"));
    }
}

#[cfg(test)]
mod tests {
    use crate::validation::Options;

    use super::*;

    #[test]
    fn serialize() {
        assert_eq!(
            serde_json::to_string(&XML::default()).unwrap(),
            "{}",
            "empty object"
        );

        assert_eq!(
            serde_json::to_value(XML {
                name: Some("name".to_owned()),
                namespace: Some("https://example.com/schema/sample".to_owned()),
                prefix: Some("sample".to_owned()),
                attribute: Some(true),
                wrapped: Some(true),
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

        XML {
            namespace: Some("foo-bar".to_owned()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "xml".to_owned());
        assert_eq!(
            ctx.errors,
            vec!["xml.namespace: must be a valid URL, found `foo-bar`"],
            "invalid URL: {:?}",
            ctx.errors
        );
    }
}
