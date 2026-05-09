//! Holds a set of reusable objects for different aspects of the OAS.

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_string_matches};
use crate::common::reference::RefOr;
use crate::v3_1::callback::Callback;
use crate::v3_1::example::Example;
use crate::v3_1::header::Header;
use crate::v3_1::link::Link;
use crate::v3_1::parameter::Parameter;
use crate::v3_1::path_item::PathItem;
use crate::v3_1::request_body::RequestBody;
use crate::v3_1::response::Response;
use crate::v3_1::schema::Schema;
use crate::v3_1::security_scheme::SecurityScheme;
use crate::v3_1::spec::Spec;
use crate::validation::Options;
use lazy_regex::regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    /// An object to hold reusable Schema Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemas: Option<BTreeMap<String, RefOr<Schema>>>,

    /// An object to hold reusable Response Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responses: Option<BTreeMap<String, RefOr<Response>>>,

    /// An object to hold reusable Parameter Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, RefOr<Parameter>>>,

    /// An object to hold reusable Example Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<BTreeMap<String, RefOr<Example>>>,

    /// An object to hold reusable Request Body Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_bodies: Option<BTreeMap<String, RefOr<RequestBody>>>,

    /// An object to hold reusable Header Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, RefOr<Header>>>,

    /// An object to hold reusable Security Scheme Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_schemes: Option<BTreeMap<String, RefOr<SecurityScheme>>>,

    /// An object to hold reusable Link Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, RefOr<Link>>>,

    /// An object to hold reusable Callback Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<BTreeMap<String, RefOr<Callback>>>,

    /// An object to hold reusable Path Item Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_items: Option<BTreeMap<String, PathItem>>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Components {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        let re = regex!(r"^[a-zA-Z0-9.\-_]+$");

        if let Some(objs) = &self.schemas {
            for (name, obj) in objs {
                let reference = format!("#/components/schemas/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedSchemas) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.schemas[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.schemas[{name}]"));
            }
        }

        if let Some(objs) = &self.responses {
            for (name, obj) in objs {
                let reference = format!("#/components/responses/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedResponses) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.responses[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.responses[{name}]"));
            }
        }

        if let Some(objs) = &self.parameters {
            for (name, obj) in objs {
                let reference = format!("#/components/parameters/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedParameters) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.parameters[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.parameters[{name}]"));
            }
        }

        if let Some(objs) = &self.examples {
            for (name, obj) in objs {
                let reference = format!("#/components/examples/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedExamples) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.examples[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.examples[{name}]"));
            }
        }

        if let Some(objs) = &self.request_bodies {
            for (name, obj) in objs {
                let reference = format!("#/components/requestBodies/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedRequestBodies)
                {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.requestBodies[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.requestBodies[{name}]"));
            }
        }

        if let Some(objs) = &self.headers {
            for (name, obj) in objs {
                let reference = format!("#/components/headers/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedHeaders) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.headers[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.headers[{name}]"));
            }
        }

        if let Some(objs) = &self.security_schemes {
            for (name, obj) in objs {
                let reference = format!("#/components/securitySchemes/{name}");
                if !ctx.is_visited(&reference)
                    && !ctx.is_option(Options::IgnoreUnusedSecuritySchemes)
                {
                    ctx.error(reference.clone(), "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.securitySchemes[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.securitySchemes[{name}]"));
                if let Ok(SecurityScheme::OAuth2(oauth2)) = obj.get_item(ctx.spec) {
                    let mut check_unused = |scopes: &BTreeMap<String, String>| {
                        for scope in scopes.keys() {
                            let r = format!("{reference}/{scope}");
                            if !ctx.is_visited(&r)
                                && !ctx.is_option(Options::IgnoreUnusedSecuritySchemes)
                            {
                                ctx.error(r, "unused");
                            }
                        }
                    };
                    if let Some(flow) = &oauth2.flows.implicit {
                        check_unused(&flow.scopes);
                    }
                    if let Some(flow) = &oauth2.flows.password {
                        check_unused(&flow.scopes);
                    }
                    if let Some(flow) = &oauth2.flows.client_credentials {
                        check_unused(&flow.scopes);
                    }
                    if let Some(flow) = &oauth2.flows.authorization_code {
                        check_unused(&flow.scopes);
                    }
                }
            }
        }

        if let Some(objs) = &self.path_items {
            // operationId uniqueness is enforced by the pre-pass in
            // `Spec::validate_with_context`; re-inserting here would
            // double-count and report a false duplicate.
            for (name, item) in objs {
                let reference = format!("#/components/pathItems/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedPathItems) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.pathItems[<name>]"));
                item.validate_with_context(ctx, format!("{path}.pathItems[{name}]"));
            }
        }

        if let Some(objs) = &self.links {
            for (name, obj) in objs {
                let reference = format!("#/components/links/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedLinks) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.links[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.links[{name}]"));
            }
        }

        if let Some(objs) = &self.callbacks {
            for (name, obj) in objs {
                let reference = format!("#/components/callbacks/{name}");
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedCallbacks) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, re, ctx, format!("{path}.callbacks[<name>]"));
                obj.validate_with_context(ctx, format!("{path}.callbacks[{name}]"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::helpers::Context;
    use crate::v3_1::operation::Operation;
    use crate::v3_1::parameter::{InQuery, Parameter};
    use crate::v3_1::response::{Response, Responses};
    use crate::v3_1::schema::{Schema, SingleSchema, StringSchema};
    use crate::v3_1::security_scheme::{
        AuthorizationCodeOAuth2Flow, ClientCredentialsOAuth2Flow, ImplicitOAuth2Flow, OAuth2Flows,
        OAuth2SecurityScheme, PasswordOAuth2Flow,
    };
    use serde_json::json;

    fn map_with<T>(name: &str, t: T) -> BTreeMap<String, RefOr<T>> {
        BTreeMap::from([(name.to_owned(), RefOr::new_item(t))])
    }

    #[test]
    fn round_trip_all_kinds() {
        let v = json!({
            "schemas": {"S": {"type": "string"}},
            "responses": {"R": {"description": "ok"}},
            "parameters": {"P": {"name": "q", "in": "query", "schema": {"type": "string"}}},
            "examples": {"E": {"value": 1}},
            "requestBodies": {"RB": {"content": {"application/json": {"schema": {"type": "object"}}}}},
            "headers": {"H": {"description": "h", "schema": {"type": "string"}}},
            "securitySchemes": {"SS": {"type": "http", "scheme": "Basic"}},
            "links": {"L": {"operationId": "op"}},
            "callbacks": {"CB": {"{$request.body#/cb}": {"post": {"responses": {"200": {"description": "ok"}}}}}},
            "pathItems": {"PI": {"get": {"responses": {"200": {"description": "ok"}}}}},
            "x-tra": "yes"
        });
        let comp: Components = serde_json::from_value(v.clone()).unwrap();
        // Round-trip preserves all maps.
        let re: Components = serde_json::from_value(serde_json::to_value(&comp).unwrap()).unwrap();
        assert_eq!(re, comp);
    }

    fn ok_responses() -> Responses {
        Responses {
            responses: Some(BTreeMap::from([(
                "200".to_owned(),
                RefOr::new_item(Response {
                    description: "ok".into(),
                    ..Default::default()
                }),
            )])),
            ..Default::default()
        }
    }

    #[test]
    fn unused_components_each_kind_reports() {
        let mut ops_map = BTreeMap::new();
        ops_map.insert(
            "get".to_owned(),
            Operation {
                responses: Some(ok_responses()),
                ..Default::default()
            },
        );
        let path_item = PathItem {
            operations: Some(ops_map),
            ..Default::default()
        };

        let comp = Components {
            schemas: Some(map_with(
                "S",
                Schema::Single(Box::new(SingleSchema::String(StringSchema::default()))),
            )),
            responses: Some(map_with(
                "R",
                Response {
                    description: "ok".into(),
                    ..Default::default()
                },
            )),
            parameters: Some(map_with(
                "P",
                Parameter::Query(InQuery {
                    name: "q".into(),
                    description: None,
                    required: None,
                    deprecated: None,
                    allow_empty_value: None,
                    style: None,
                    explode: None,
                    allow_reserved: None,
                    schema: Some(RefOr::new_item(Schema::Single(Box::new(
                        SingleSchema::String(StringSchema::default()),
                    )))),
                    example: None,
                    examples: None,
                    content: None,
                    extensions: None,
                }),
            )),
            examples: Some(map_with("E", Example::default())),
            request_bodies: Some(map_with("RB", RequestBody::default())),
            headers: Some(map_with(
                "H",
                Header {
                    schema: Some(RefOr::new_item(Schema::Single(Box::new(
                        SingleSchema::String(StringSchema::default()),
                    )))),
                    ..Default::default()
                },
            )),
            security_schemes: Some(map_with(
                "SS",
                SecurityScheme::OAuth2(Box::new(OAuth2SecurityScheme {
                    flows: OAuth2Flows {
                        implicit: Some(ImplicitOAuth2Flow {
                            authorization_url: "https://x.example/auth".into(),
                            refresh_url: None,
                            scopes: BTreeMap::from([("read".to_owned(), "Read".to_owned())]),
                            extensions: None,
                        }),
                        password: Some(PasswordOAuth2Flow {
                            token_url: "https://x.example/t".into(),
                            refresh_url: None,
                            scopes: BTreeMap::from([("write".to_owned(), "Write".to_owned())]),
                            extensions: None,
                        }),
                        client_credentials: Some(ClientCredentialsOAuth2Flow {
                            token_url: "https://x.example/t".into(),
                            refresh_url: None,
                            scopes: BTreeMap::from([("admin".to_owned(), "Admin".to_owned())]),
                            extensions: None,
                        }),
                        authorization_code: Some(AuthorizationCodeOAuth2Flow {
                            authorization_url: "https://x.example/auth".into(),
                            token_url: "https://x.example/t".into(),
                            refresh_url: None,
                            scopes: BTreeMap::from([("delete".to_owned(), "Delete".to_owned())]),
                            extensions: None,
                        }),
                        extensions: None,
                    },
                    description: None,
                    extensions: None,
                })),
            )),
            links: Some(map_with(
                "L",
                Link {
                    operation_id: Some("does-not-exist".into()),
                    ..Default::default()
                },
            )),
            callbacks: Some(map_with("CB", Callback::default())),
            path_items: Some(BTreeMap::from([("PI".to_owned(), path_item)])),
            extensions: None,
        };
        let spec = Spec {
            components: Some(comp.clone()),
            ..Default::default()
        };
        // Use empty options so the IgnoreUnusedPathItems default in
        // Options::new() doesn't suppress the pathItems unused check.
        let mut ctx = Context::new(&spec, Options::empty());
        comp.validate_with_context(&mut ctx, "#.components".into());
        for path in [
            "#/components/schemas/S",
            "#/components/responses/R",
            "#/components/parameters/P",
            "#/components/examples/E",
            "#/components/requestBodies/RB",
            "#/components/headers/H",
            "#/components/securitySchemes/SS",
            "#/components/links/L",
            "#/components/callbacks/CB",
            "#/components/pathItems/PI",
        ] {
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains(path) && e.contains("unused")),
                "expected `{path}: unused`: {:?}",
                ctx.errors
            );
        }
        // OAuth2 unused-scope detection covers ALL four flows in 3.1.
        for scope in ["read", "write", "admin", "delete"] {
            let p = format!("#/components/securitySchemes/SS/{scope}");
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e.contains(&p) && e.contains("unused")),
                "expected unused scope `{scope}`: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn ignored_unused_options_silence_each_kind() {
        let comp = Components {
            schemas: Some(map_with(
                "S",
                Schema::Single(Box::new(SingleSchema::String(StringSchema::default()))),
            )),
            responses: Some(map_with(
                "R",
                Response {
                    description: "ok".into(),
                    ..Default::default()
                },
            )),
            examples: Some(map_with("E", Example::default())),
            request_bodies: Some(map_with("RB", RequestBody::default())),
            headers: Some(map_with(
                "H",
                Header {
                    schema: Some(RefOr::new_item(Schema::Single(Box::new(
                        SingleSchema::String(StringSchema::default()),
                    )))),
                    ..Default::default()
                },
            )),
            security_schemes: None,
            links: Some(map_with(
                "L",
                Link {
                    operation_id: Some("dne".into()),
                    ..Default::default()
                },
            )),
            callbacks: Some(map_with("CB", Callback::default())),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp.clone()),
            ..Default::default()
        };
        let opts = Options::IgnoreUnusedSchemas
            | Options::IgnoreUnusedResponses
            | Options::IgnoreUnusedParameters
            | Options::IgnoreUnusedExamples
            | Options::IgnoreUnusedRequestBodies
            | Options::IgnoreUnusedHeaders
            | Options::IgnoreUnusedLinks
            | Options::IgnoreUnusedCallbacks;
        let mut ctx = Context::new(&spec, opts);
        comp.validate_with_context(&mut ctx, "#.components".into());
        assert!(
            ctx.errors.iter().all(|e| !e.contains("unused")),
            "no unused errors when ignored: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn invalid_component_name_reported() {
        let mut schemas: BTreeMap<String, RefOr<Schema>> = BTreeMap::new();
        schemas.insert(
            "bad name".to_owned(),
            RefOr::new_item(Schema::Single(Box::new(SingleSchema::String(
                StringSchema::default(),
            )))),
        );
        let comp = Components {
            schemas: Some(schemas),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp.clone()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::IgnoreUnusedSchemas.only());
        comp.validate_with_context(&mut ctx, "#.components".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must match pattern")),
            "expected pattern error: {:?}",
            ctx.errors
        );
    }
}
