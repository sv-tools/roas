//! Operation Object

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{validate_required_string, Context, ValidateWithContext};
use crate::common::reference::{Ref, RefOr, ResolveReference};
use crate::v3_0::callback::Callback;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::parameter::Parameter;
use crate::v3_0::request_body::RequestBody;
use crate::v3_0::response::Responses;
use crate::v3_0::security_scheme::SecurityScheme;
use crate::v3_0::server::Server;
use crate::v3_0::spec::Spec;
use crate::v3_0::tag::Tag;
use crate::validation::Options;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Operation {
    /// A list of tags for API documentation control.
    /// Tags can be used for logical grouping of operations by resources or any other qualifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    /// A short summary which by default SHOULD override that of the referenced component.
    /// If the referenced object-type does not allow a summary field, then this field has no effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A verbose explanation of the operation behavior.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional external documentation for this operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalDocs")]
    pub external_docs: Option<ExternalDocumentation>,

    /// Unique string used to identify the operation.
    /// The id MUST be unique among all operations described in the API.
    /// Tools and libraries MAY use the operationId to uniquely identify an operation, therefore,
    /// it is recommended to follow common programming naming conventions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationId")]
    pub operation_id: Option<String>,

    /// A list of parameters that are applicable for this operation.
    /// If a parameter is already defined at the Path Item, the new definition will override it,
    /// but can never remove it.
    /// The list MUST NOT include duplicated parameters.
    /// A unique parameter is defined by a combination of a name and location.
    /// The list can use the Reference Object to link to parameters that are defined at the Swagger Object's parameters.
    /// There can be one "body" parameter at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RefOr<Parameter>>>,

    /// The request body applicable for this operation.
    /// The `requestBody` is only supported in HTTP methods where the HTTP 1.1 specification
    /// [RFC7231](https://www.rfc-editor.org/rfc/rfc7231) has explicitly defined semantics for request bodies.
    /// In other cases where the HTTP spec is vague, `requestBody` SHALL be ignored by consumers.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "requestBody")]
    pub request_body: Option<RefOr<RequestBody>>,

    /// **Required** The list of possible responses as they are returned from executing this operation.
    pub responses: Responses,

    /// A map of possible out-of band callbacks related to the parent operation.
    /// The key is a unique identifier for the Callback Object.
    /// Each value in the map is a Callback Object that describes a request that
    /// may be initiated by the API provider and the expected responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<BTreeMap<String, RefOr<Callback>>>,

    /// Declares this operation to be deprecated.
    /// Usage of the declared operation should be refrained.
    /// Default value is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    /// A declaration of which security mechanisms can be used for this operation.
    /// The list of values includes alternative security requirement objects that can be used.
    /// Only one of the security requirement objects need to be satisfied to authorize a request.
    /// To make security optional, an empty security requirement (`{}`) can be included in the array.
    /// This definition overrides any declared top-level `security`.
    /// To remove a top-level security declaration, an empty array can be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<BTreeMap<String, Vec<String>>>>,

    /// An alternative `server` array to service this operation.
    /// If an alternative `server` object is specified at the Path Item Object or Root level,
    /// it will be overridden by this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<Server>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Operation {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        // do not validate operation_id, it is already validated in PathItem

        if let Some(tags) = &self.tags {
            for (i, tag) in tags.iter().enumerate() {
                let path = format!("{}.tags[{}]", path, i);
                validate_required_string(tag, ctx, path.clone());
                if tag.is_empty() {
                    continue;
                }
                let reference = format!("#/tags/{}", tag);
                if let Ok(spec_tag) = Ref::new(reference.clone()).resolve::<Spec, Tag>(ctx.spec) {
                    if ctx.visited.insert(reference.clone()) {
                        spec_tag.validate_with_context(ctx, reference);
                    }
                } else if !ctx.options.contains(Options::IgnoreMissingTags) {
                    ctx.errors
                        .push(format!("{}.tags[{}]: `{}` not found in spec", path, i, tag));
                }
            }
        }

        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.clone().iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{}.parameters[{}]", path, i));
            }
        }

        if let Some(request_body) = &self.request_body {
            request_body.validate_with_context(ctx, format!("{}.requestBody", path));
        }

        if let Some(servers) = &self.servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(ctx, format!("{}.servers[{}]", path, i));
            }
        }

        if let Some(callbacks) = &self.callbacks {
            for (k, v) in callbacks {
                v.validate_with_context(ctx, format!("{}.callbacks[{}]", path, k));
            }
        }

        self.responses
            .validate_with_context(ctx, format!("{}.responses", path));

        if let Some(external_doc) = &self.external_docs {
            external_doc.validate_with_context(ctx, format!("{}.externalDocs", path));
        }

        if let Some(security) = &self.security {
            for (i, security) in security.iter().enumerate() {
                for (name, scopes) in security {
                    let path = format!("{}.security[{}][{}]", path, i, name);
                    let reference = format!("#/components/securitySchemes/{}", name);
                    let spec_ref = RefOr::<SecurityScheme>::new_ref(reference.clone());
                    spec_ref.validate_with_context(ctx, path.clone());
                    if !scopes.is_empty() {
                        if let Ok(SecurityScheme::OAuth2(oauth2)) = spec_ref.get_item(ctx.spec) {
                            for scope in scopes {
                                ctx.visited.insert(format!("{}/{}", reference, scope));
                                let mut found = false;
                                if let Some(flow) = &oauth2.flows.implicit {
                                    found = found || flow.scopes.contains_key(scope)
                                }
                                if !found {
                                    if let Some(flow) = &oauth2.flows.password {
                                        found = found || flow.scopes.contains_key(scope)
                                    }
                                }
                                if !found {
                                    if let Some(flow) = &oauth2.flows.client_credentials {
                                        found = found || flow.scopes.contains_key(scope)
                                    }
                                }
                                if !found {
                                    if let Some(flow) = &oauth2.flows.authorization_code {
                                        found = found || flow.scopes.contains_key(scope)
                                    }
                                }
                                if !found {
                                    ctx.errors.push(format!(
                                        "{}: scope `{}` not found in spec by reference `{}`",
                                        path, scope, reference
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
