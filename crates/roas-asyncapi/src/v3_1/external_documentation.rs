//! AsyncAPI v3.1 `External Documentation` object.
//!
//! Per [External Documentation Object](https://www.asyncapi.com/docs/reference/specification/v3.1.0#externalDocumentationObject).

use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct ExternalDocumentation {
    /// **Required** The URL for the target documentation.
    pub url: String,

    /// A short description of the target documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for ExternalDocumentation {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("url", &self.url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_and_requires_url() {
        let value = json!({ "url": "https://example.com/docs", "description": "Docs" });
        let docs: ExternalDocumentation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&docs).unwrap(), value);

        let mut ctx = Context::with_path(EnumSet::empty(), "#.externalDocs");
        docs.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());

        let mut ctx = Context::with_path(EnumSet::empty(), "#.externalDocs");
        ExternalDocumentation::default().validate_with_context(&mut ctx);
        assert!(ctx.errors[0] == "#.externalDocs.url: must not be empty");
    }
}
