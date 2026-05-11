//! Operation Object

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_required_string};
use crate::common::reference::RefOr;
use crate::v3_0::callback::Callback;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::v3_0::parameter::Parameter;
use crate::v3_0::request_body::RequestBody;
use crate::v3_0::response::Responses;
use crate::v3_0::server::Server;
use crate::v3_0::spec::Spec;
use crate::v3_0::tag::Tag;
use crate::validation::Options;
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

    /// ReDoc/Redocly extension with code samples associated with this operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-codeSamples")]
    pub x_code_samples: Option<Vec<CodeSample>>,

    /// ReDoc/Redocly extension with operation badges.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-badges")]
    pub x_badges: Option<Vec<Badge>>,

    /// Documentation extension with extra operation examples.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "x-examples")]
    pub x_examples: Option<BTreeMap<String, serde_json::Value>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// ReDoc/Redocly `x-codeSamples` extension entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct CodeSample {
    /// **Required** Code sample language.
    pub lang: String,

    /// Optional display label for the language tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// **Required** Code sample source code.
    pub source: String,

    /// Allows extensions on the code sample extension object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

/// ReDoc/Redocly `x-badges` extension entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Badge {
    /// **Required** Badge text.
    pub name: String,

    /// Optional badge position supported by documentation renderers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,

    /// Optional badge color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Allows extensions on the badge extension object.
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
                let path = format!("{path}.tags[{i}]");
                validate_required_string(tag, ctx, path.clone());
                if tag.is_empty() {
                    continue;
                }
                let reference = format!("#/tags/{tag}");
                if let Ok(spec_tag) = RefOr::<Tag>::new_ref(reference.clone()).get_item(ctx.spec) {
                    if ctx.visit(reference.clone()) {
                        spec_tag.validate_with_context(ctx, reference);
                    }
                } else if !ctx.is_option(Options::IgnoreMissingTags) {
                    ctx.error(path, format_args!(".tags[{i}]: `{tag}` not found in spec"));
                }
            }
        }

        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.clone().iter().enumerate() {
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

        if let Some(samples) = &self.x_code_samples {
            for (i, sample) in samples.iter().enumerate() {
                sample.validate_with_context(ctx, format!("{path}.x-codeSamples[{i}]"));
            }
        }

        if let Some(badges) = &self.x_badges {
            for (i, badge) in badges.iter().enumerate() {
                badge.validate_with_context(ctx, format!("{path}.x-badges[{i}]"));
            }
        }

        if let Some(callbacks) = &self.callbacks {
            for (k, v) in callbacks {
                v.validate_with_context(ctx, format!("{path}.callbacks[{k}]"));
            }
        }

        self.responses
            .validate_with_context(ctx, format!("{path}.responses"));

        if let Some(external_doc) = &self.external_docs {
            external_doc.validate_with_context(ctx, format!("{path}.externalDocs"));
        }

        // Operation-level `security`: validated here so it runs everywhere
        // an Operation is reached (including operations nested inside
        // `Callback` path items, which `validate_path_item` does not visit).
        // The shared helper enforces the scope-by-scheme-type rule, walks
        // all four OAuth2 flows, and reports missing schemes.
        if let Some(sec) = &self.security {
            crate::v3_0::validation::validate_security_requirements(
                ctx,
                &format!("{path}.security"),
                sec,
            );
        }
    }
}

impl ValidateWithContext<Spec> for CodeSample {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.lang, ctx, format!("{path}.lang"));
        validate_required_string(&self.source, ctx, format!("{path}.source"));
    }
}

impl ValidateWithContext<Spec> for Badge {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::validation::Options;

    #[test]
    fn validate_walks_tags_servers_external_docs() {
        // Build a Spec with one declared tag so the operation's
        // `tags[0]` resolves; the second tag references a missing tag,
        // covering the IgnoreMissingTags branch.
        let spec = Spec {
            tags: Some(vec![crate::v3_0::tag::Tag {
                name: "pets".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };

        let op = Operation {
            tags: Some(vec!["pets".into(), "".into(), "missing".into()]),
            servers: Some(vec![crate::v3_0::server::Server {
                url: "".into(),
                ..Default::default()
            }]),
            external_docs: Some(crate::v3_0::external_documentation::ExternalDocumentation {
                url: "".into(),
                description: None,
                extensions: None,
            }),
            responses: crate::v3_0::response::Responses {
                default: Some(crate::common::reference::RefOr::new_item(
                    crate::v3_0::response::Response {
                        description: "ok".into(),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut ctx = Context::new(&spec, Options::new());
        op.validate_with_context(&mut ctx, "op".into());
        // Empty tag string surfaces the required-string error.
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("op.tags[1]") && e.contains("must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
        // Missing tag surfaces the not-found-in-spec error.
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("`missing` not found in spec")),
            "errors: {:?}",
            ctx.errors
        );
        // Server URL empty surfaces the per-server validator.
        assert!(
            ctx.errors.iter().any(|e| e.contains("op.servers[0].url")),
            "errors: {:?}",
            ctx.errors
        );
        // ExternalDocs URL empty surfaces.
        assert!(
            ctx.errors.iter().any(|e| e.contains("op.externalDocs.url")),
            "errors: {:?}",
            ctx.errors
        );

        // With IgnoreMissingTags, the missing-tag error is silenced.
        let mut ctx = Context::new(&spec, Options::IgnoreMissingTags.only());
        op.validate_with_context(&mut ctx, "op".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("not found in spec")),
            "missing-tags should be silenced: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn documentation_extensions_round_trip_and_validate() {
        let value = serde_json::json!({
            "responses": {
                "200": {
                    "description": "OK"
                }
            },
            "x-codeSamples": [
                {
                    "lang": "curl",
                    "label": "cURL",
                    "source": "curl https://example.com/pets"
                }
            ],
            "x-badges": [
                {
                    "name": "Beta",
                    "position": "before",
                    "color": "purple"
                }
            ],
            "x-examples": {
                "request": {
                    "id": 1
                }
            }
        });
        let operation: Operation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&operation).unwrap(), value);

        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        operation.validate_with_context(&mut ctx, "operation".to_owned());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&spec, Options::new());
        CodeSample::default().validate_with_context(&mut ctx, "sample".to_owned());
        assert_eq!(ctx.errors.len(), 2, "expected lang/source errors");

        let mut ctx = Context::new(&spec, Options::new());
        Badge::default().validate_with_context(&mut ctx, "badge".to_owned());
        assert_eq!(ctx.errors, vec!["badge.name: must not be empty"]);
    }
}
