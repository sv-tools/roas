//! Example object.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{validate_optional_url, Context, ValidateWithContext};
use crate::v3_0::spec::Spec;

/// Example object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Example {
    /// Short description for the example.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Long description for the example.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Embedded literal example.
    /// The `value` field and `externalValue` field are mutually exclusive.
    /// To represent examples of media types that cannot naturally represented in JSON or YAML,
    /// use a string value to contain the example, escaping where necessary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,

    /// A URL that points to the literal example.
    /// This provides the capability to reference examples that cannot easily
    /// be included in JSON or YAML documents.
    /// The `value` field and `externalValue` field are mutually exclusive.
    #[serde(rename = "externalValue")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_value: Option<String>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Example {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if self.value.is_some() && self.external_value.is_some() {
            ctx.errors.push(format!(
                "{}: value and externalValue are mutually exclusive",
                path
            ));
        }
        validate_optional_url(&self.external_value, ctx, format!("{}.externalValue", path));
    }
}
