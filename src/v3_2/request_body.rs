//! Request Body Object

use crate::common::helpers::{Context, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v3_2::media_type::MediaType;
use crate::v3_2::spec::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Describes a single request body.
///
/// Specification example:
/// ```yaml
/// description: user to add to the system
/// content:
///   'application/json':
///     schema:
///       $ref: '#/components/schemas/User'
///     examples:
///       user:
///         summary: User Example
///         externalValue: 'https://foo.bar/examples/user-example.json'
///   'application/xml':
///     schema:
///       $ref: '#/components/schemas/User'
///     examples:
///       user:
///         summary: User Example in XML
///         externalValue: 'https://foo.bar/examples/user-example.xml'
///   'text/plain':
///     examples:
///       user:
///         summary: User example in text plain format
///         externalValue: 'https://foo.bar/examples/user-example.txt'
///   '*/*':
///     examples:
///       user:
///         summary: User example in other format
///         externalValue: 'https://foo.bar/examples/user-example.whatever'
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct RequestBody {
    /// A brief description of the parameter.
    /// This could contain examples of use.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// **Required** The content of the request body.
    /// The key is a media type or media type range and the value describes it.
    /// For requests that match multiple keys, only the most specific key is applicable.
    /// e.g. `text/plain` overrides `text/*`
    pub content: BTreeMap<String, RefOr<MediaType>>,

    /// Determines if the request body is required in the request.
    /// Defaults to `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// `^x-` Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for RequestBody {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        for (k, v) in &self.content {
            v.validate_with_context(ctx, format!("{path}.content[{k}]"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_with_extensions() {
        let v = json!({
            "description": "A user",
            "required": true,
            "content": {
                "application/json": {"schema": {"type": "object"}}
            },
            "x-internal": "yes"
        });
        let rb: RequestBody = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(rb.required, Some(true));
        assert!(rb.extensions.is_some());
        assert_eq!(serde_json::to_value(&rb).unwrap(), v);
    }
}
