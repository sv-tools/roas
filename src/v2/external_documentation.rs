//! External Documentation Object

use std::collections::BTreeMap;
use std::ops::Add;

use crate::common::helpers::{validate_url, Context, ValidateWithContext};
use serde::{Deserialize, Serialize};

use crate::v2::spec::Spec;

/// Allows referencing an external resource for extended documentation.
///
/// Specification example:
///
/// ```yaml
/// description: Find more info here
/// url: https://swagger.io
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ExternalDocumentation {
    /// **Required** The URL for the target documentation.
    /// Value MUST be in the format of a URL.
    pub url: String,

    /// A short description of the target documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for ExternalDocumentation {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_url(&Some(self.url.clone()), ctx, path.add(".url"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
