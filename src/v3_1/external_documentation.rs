//! References an external resource for extended documentation.

use crate::common::helpers::{Context, ValidateWithContext, validate_required_url};
use crate::v3_1::spec::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Allows referencing an external resource for extended documentation.
///
/// Specification example:
///
/// ```yaml
/// description: Find more info here
/// url: "https://example.com"
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ExternalDocumentation {
    /// **Required** The URL for the target documentation.
    /// Value MUST be in the format of a URL.
    pub url: String,

    /// A short description of the target documentation.
    /// [CommonMark](https://spec.commonmark.org)  syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for ExternalDocumentation {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_url(&self.url, ctx, format!("{path}.url"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Options;

    #[test]
    fn test_external_documentation_deserialize() {
        assert_eq!(
            serde_json::from_value::<ExternalDocumentation>(serde_json::json!({
                    "url": "https://swagger.io",
                    "description": "Find more info here"
            }))
            .unwrap(),
            ExternalDocumentation {
                url: String::from("https://swagger.io"),
                description: Some(String::from("Find more info here")),
                ..Default::default()
            },
            "deserialize",
        );
    }

    #[test]
    fn test_external_documentation_serialize() {
        assert_eq!(
            serde_json::to_value(ExternalDocumentation {
                url: String::from("https://swagger.io"),
                description: Some(String::from("Find more info here")),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "url": "https://swagger.io",
                "description": "Find more info here"
            }),
            "serialize",
        );
    }

    #[test]
    fn test_external_documentation_validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::empty());
        let ed = ExternalDocumentation {
            url: String::from("https://swagger.io"),
            description: Some(String::from("Find more info here")),
            ..Default::default()
        };
        ed.validate_with_context(&mut ctx, String::from("externalDocs"));
        assert!(
            ctx.errors.is_empty(),
            "Validation should pass: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&spec, Options::empty());
        let ed = ExternalDocumentation {
            description: Some(String::from("Find more info here")),
            ..Default::default()
        };
        ed.validate_with_context(&mut ctx, String::from("externalDocs"));
        assert!(
            ctx.errors
                .contains(&"externalDocs.url: must not be empty".to_string()),
            "Validation should fail: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&spec, Options::empty());
        let ed = ExternalDocumentation {
            url: String::from("invalid-url"),
            description: Some(String::from("Find more info here")),
            ..Default::default()
        };
        ed.validate_with_context(&mut ctx, String::from("externalDocs"));
        assert!(
            ctx.errors.contains(
                &"externalDocs.url: must be a valid URL, found `invalid-url`".to_string()
            ),
            "Validation should fail: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(
            &spec,
            Options::only(&Options::IgnoreEmptyExternalDocumentationUrl),
        );
        let ed = ExternalDocumentation {
            description: Some(String::from("Find more info here")),
            ..Default::default()
        };
        ed.validate_with_context(&mut ctx, String::from("externalDocs"));
        assert!(
            ctx.errors.is_empty(),
            "Validation should pass: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&spec, Options::only(&Options::IgnoreInvalidUrls));
        let ed = ExternalDocumentation {
            url: String::from("invalid-url"),
            description: Some(String::from("Find more info here")),
            ..Default::default()
        };
        ed.validate_with_context(&mut ctx, String::from("externalDocs"));
        assert!(
            ctx.errors.is_empty(),
            "Validation should pass: {:?}",
            ctx.errors
        );
    }
}
