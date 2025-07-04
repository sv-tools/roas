//! Represents a possible design-time link for a response

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::v3_1::server::Server;
use crate::v3_1::spec::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The Link object represents a possible design-time link for a response.
/// The presence of a link does not guarantee the caller’s ability to successfully invoke it,
/// rather it provides a known relationship and traversal mechanism between responses and other operations.
///
/// Unlike dynamic links (i.e. links provided in the response payload),
/// the OAS linking mechanism does not require link information in the runtime response.
//
/// For computing links, and providing instructions to execute them,
/// a runtime expression is used for accessing values in an operation and using them as parameters
/// while invoking the linked operation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Link {
    /// A relative or absolute URI reference to an OAS operation.
    /// This field is mutually exclusive of the operationId field, and MUST point to an Operation Object.
    /// Relative operationRef values MAY be used to locate an existing Operation Object in the OpenAPI definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationRef")]
    pub operation_ref: Option<String>,

    /// The name of an existing, resolvable OAS operation,
    /// as defined with a unique operationId.
    /// This field is mutually exclusive of the operationRef field.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,

    /// A map representing parameters to pass to an operation as specified with operationId
    /// or identified via operationRef.
    /// The key is the parameter name to be used, whereas the value can be a constant
    /// or an expression to be evaluated and passed to the linked operation.
    /// The parameter name can be qualified using the parameter location [{in}.]{name} for operations
    /// that use the same parameter name in different locations (e.g. path.id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, serde_json::Value>>,

    /// A literal value or {expression} to use as a request body when calling the target operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "requestBody")]
    pub request_body: Option<serde_json::Value>,

    /// A description of the link.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A server object to be used by the target operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<Server>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Link {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(operation_id) = &self.operation_id {
            if !ctx
                .visited
                .contains(format!("#/paths/operations/{operation_id}").as_str())
            {
                ctx.error(
                    path.clone(),
                    format_args!(".operationId: missing operation with id `{operation_id}`"),
                );
            }
        }
        if let Some(server) = &self.server {
            server.validate_with_context(ctx, format!("{path}.server"));
        }
    }
}
