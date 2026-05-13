//! Operation Object

use crate::common::helpers::validate_required_string;
use crate::common::reference::RefOr;
use crate::v3_2::callback::Callback;
use crate::v3_2::external_documentation::ExternalDocumentation;
use crate::v3_2::parameter::Parameter;
use crate::v3_2::request_body::RequestBody;
use crate::v3_2::response::Responses;
use crate::v3_2::server::Server;
use crate::v3_2::spec::Spec;
use crate::v3_2::tag::Tag;
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

    /// The OAS 3.2 JSON Schema does not require `responses`; an
    /// Operation without it is valid. `Option<Responses>` reflects
    /// that.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responses: Option<Responses>,

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
                let tag_path = format!("{path}.tags[{i}]");
                validate_required_string(tag, ctx, tag_path.clone());
                if tag.is_empty() {
                    continue;
                }
                let reference = format!("#/tags/{tag}");
                if let Ok(spec_tag) = RefOr::<Tag>::new_ref(reference.clone()).get_item(ctx.spec) {
                    if ctx.visit(reference.clone()) {
                        spec_tag.validate_with_context(ctx, reference);
                    }
                } else if !ctx.is_option(Options::IgnoreMissingTags) {
                    ctx.error(tag_path, format_args!("`{tag}` not found in spec"));
                }
            }
        }

        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{path}.parameters[{i}]"));
            }
        }

        if let Some(request_body) = &self.request_body {
            request_body.validate_with_context(ctx, format!("{path}.requestBody"));
        }

        if let Some(servers) = &self.servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(ctx, format!("{path}.servers[{i}]"));
            }
        }

        if let Some(callbacks) = &self.callbacks {
            for (k, v) in callbacks {
                v.validate_with_context(ctx, format!("{path}.callbacks[{k}]"));
            }
        }

        // Per the OAS 3.2 JSON Schema, `responses` is no longer required
        // (it was REQUIRED in 3.0 / 3.1). When present, validate it.
        if let Some(responses) = &self.responses {
            responses.validate_with_context(ctx, format!("{path}.responses"));
        }

        if let Some(external_doc) = &self.external_docs {
            external_doc.validate_with_context(ctx, format!("{path}.externalDocs"));
        }

        // Operation-level `security`: validated here so it runs everywhere
        // an Operation is reached (including operations nested inside
        // `Callback` and `Webhooks` path items). The shared helper resolves
        // `oauth2` scopes against the scheme's flows and accepts free-form
        // role-name arrays for the other scheme types per OAS 3.1.
        if let Some(sec) = &self.security {
            crate::v3_2::validation::validate_security_requirements(
                ctx,
                &format!("{path}.security"),
                sec,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_2::response::{Response, Responses};
    use crate::v3_2::tag::Tag;
    use crate::validation::Context;
    use crate::validation::ValidationErrorsExt;

    fn ok_responses() -> Responses {
        Responses {
            responses: Some(BTreeMap::from([(
                "200".to_owned(),
                RefOr::new_item(Response {
                    description: Some("ok".into()),
                    ..Default::default()
                }),
            )])),
            ..Default::default()
        }
    }

    #[test]
    fn missing_responses_is_accepted() {
        // Per the OAS 3.2 JSON Schema, `responses` is optional on
        // Operation. An Operation without one should validate clean.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Operation::default().validate_with_context(&mut ctx, "op".into());
        assert!(
            !ctx.errors.mentions(".responses"),
            "no responses errors expected: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn validate_walks_tags_servers_external_docs() {
        let spec = Spec {
            tags: Some(vec![Tag {
                name: "pets".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        let op = Operation {
            tags: Some(vec!["pets".into(), "".into(), "missing".into()]),
            servers: Some(vec![Server {
                url: "".into(),
                ..Default::default()
            }]),
            external_docs: Some(ExternalDocumentation {
                url: "".into(),
                description: None,
                extensions: None,
            }),
            responses: Some(ok_responses()),
            ..Default::default()
        };

        let mut ctx = Context::new(&spec, Options::new());
        op.validate_with_context(&mut ctx, "op".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("op.tags[1]") && e.contains("must not be empty")),
            "empty tag: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("`missing` not found in spec")),
            "missing tag: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors.mentions("op.servers[0].url"),
            "server.url: {:?}",
            ctx.errors
        );
        assert!(
            ctx.errors.mentions("op.externalDocs.url"),
            "externalDocs.url: {:?}",
            ctx.errors
        );

        // With IgnoreMissingTags, the missing-tag error is silenced.
        let mut ctx = Context::new(&spec, Options::IgnoreMissingTags.only());
        op.validate_with_context(&mut ctx, "op".into());
        assert!(
            !ctx.errors.mentions("not found in spec"),
            "missing-tags should be silenced: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn op_level_security_runs_through_helper() {
        let spec = Spec::default();
        let op = Operation {
            responses: Some(ok_responses()),
            security: Some(vec![{
                let mut req = BTreeMap::new();
                req.insert("missing-scheme".to_owned(), vec![]);
                req
            }]),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        op.validate_with_context(&mut ctx, "op".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("op.security") && e.contains("missing-scheme")),
            "expected op-level security error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn documentation_extensions_round_trip_via_generic_extensions() {
        // 3.2 dropped typed support for the Redoc-specific `x-codeSamples`
        // (and its `x-code-samples` alias) plus `x-tags`. The keys still
        // survive round-trip through the generic `extensions` map.
        let samples = serde_json::json!([
            {
                "lang": "curl",
                "label": "cURL",
                "source": "curl https://example.com/pets"
            }
        ]);
        let value = serde_json::json!({
            "responses": {"200": {"description": "OK"}},
            "x-codeSamples": samples.clone(),
            "x-tags": ["sdk", "docs"]
        });
        let operation: Operation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            operation
                .extensions
                .as_ref()
                .and_then(|m| m.get("x-codeSamples")),
            Some(&samples)
        );
        assert_eq!(
            operation.extensions.as_ref().and_then(|m| m.get("x-tags")),
            Some(&serde_json::json!(["sdk", "docs"]))
        );
        assert_eq!(serde_json::to_value(&operation).unwrap(), value);

        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        operation.validate_with_context(&mut ctx, "operation".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }
}
