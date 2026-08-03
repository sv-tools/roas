//! AsyncAPI v2.6 `Tag` object.
//!
//! Per [Tag Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#tagObject).

use crate::common::reference::RefOr;
use crate::v2_6::external_documentation::ExternalDocumentation;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Tag {
    /// **Required** The name of the tag.
    pub name: String,

    /// A short description, CommonMark-formatted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional external documentation for this tag.
    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Tag {
    fn validate_with_context(&self, ctx: &mut Context) {
        ctx.require_non_empty("name", &self.name);
        if let Some(docs) = &self.external_docs {
            ctx.in_field("externalDocs", |ctx| docs.validate_with_context(ctx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_with_external_docs() {
        let value = json!({
            "name": "user",
            "description": "User events",
            "externalDocs": { "url": "https://example.com" }
        });
        let tag: Tag = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&tag).unwrap(), value);

        let mut ctx = Context::with_path(EnumSet::empty(), "#.tags[0]");
        tag.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn requires_a_name_and_validates_nested_docs() {
        let tag: Tag = serde_json::from_value(json!({
            "name": "",
            "externalDocs": { "url": "" }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.tags[0]");
        tag.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.tags[0].name: must not be empty")
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.tags[0].externalDocs.url: must not be empty")
        );
    }
}
