//! Request Body Object

use crate::common::helpers::{Context, ValidateWithContext};
use crate::v3_0::media_type::MediaType;
use crate::v3_0::spec::Spec;
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
    pub content: BTreeMap<String, MediaType>,

    /// Determines if the request body is required in the request.
    /// Defaults to `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

impl ValidateWithContext<Spec> for RequestBody {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        for (k, v) in &self.content {
            v.validate_with_context(ctx, format!("{path}.content[{k}]"));
        }
    }
}
