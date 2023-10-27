use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{
    validate_optional_url, validate_required_string, Context, ValidateWithContext,
};
use crate::v2::spec::Spec;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct XML {
    /// Replaces the name of the element/attribute used for the described schema property.
    /// When defined within the Items Object (items),
    /// it will affect the name of the individual XML elements within the list.
    /// When defined alongside type being array (outside the items),
    /// it will affect the wrapping element and only if wrapped is true.
    /// If wrapped is false, it will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The URL of the namespace definition.
    /// Value SHOULD be in the form of a URL.
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
    /// Signifies whether the array is wrapped (for example, <books><book/><book/></books>) or
    /// unwrapped (<book/><book/>).
    /// Default value is false.
    /// The definition takes effect only when defined alongside type being array (outside the items).
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
        if let Some(name) = &self.name {
            validate_required_string(name, ctx, format!("{}.name", path));
        }
        validate_optional_url(&self.namespace, ctx, format!("{}.namespace", path));
    }
}
