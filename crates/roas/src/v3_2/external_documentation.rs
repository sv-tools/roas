//! References an external resource for extended documentation.

use crate::common::helpers::validate_required_uri;
use crate::v3_2::spec::Spec;
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};
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
        // The OAS spec lists `url` as required. When
        // `IgnoreEmptyExternalDocumentationUrl` is set we silence the
        // required-string check, but still URI-validate any non-empty
        // value (whitespace etc. would still be reported).
        if self.url.is_empty() {
            if !ctx.is_option(Options::IgnoreEmptyExternalDocumentationUrl) {
                ctx.error(format!("{path}.url"), "must not be empty");
            }
        } else {
            validate_required_uri(&self.url, ctx, format!("{path}.url"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Options;
    use crate::validation::ValidationErrorsExt;

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
            ctx.errors.has_exact("externalDocs.url: must not be empty"),
            "Validation should fail: {:?}",
            ctx.errors
        );

        // OAS 3.2 schema gives `url` `format: uri-reference`, so a
        // relative path like `invalid-url` is structurally fine. Whitespace
        // is what fails URI validation.
        let mut ctx = Context::new(&spec, Options::empty());
        let ed = ExternalDocumentation {
            url: String::from("not a uri"),
            description: Some(String::from("Find more info here")),
            ..Default::default()
        };
        ed.validate_with_context(&mut ctx, String::from("externalDocs"));
        assert!(
            ctx.errors
                .has_exact("externalDocs.url: must be a valid URI, found `not a uri`"),
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
            url: String::from("not a uri"),
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
