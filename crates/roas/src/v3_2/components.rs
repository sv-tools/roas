//! Holds a set of reusable objects for different aspects of the OAS.

use crate::common::helpers::{validate_string_matches, validate_unknown_fields};
use crate::common::reference::RefOr;
use crate::v3_2::callback::Callback;
use crate::v3_2::example::Example;
use crate::v3_2::header::Header;
use crate::v3_2::link::Link;
use crate::v3_2::parameter::Parameter;
use crate::v3_2::path_item::PathItem;
use crate::v3_2::request_body::RequestBody;
use crate::v3_2::response::Response;
use crate::v3_2::schema::{Schema, SchemaRef};
use crate::v3_2::security_scheme::SecurityScheme;
use crate::v3_2::spec::Spec;
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};
use lazy_regex::regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    /// An object to hold reusable Schema Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemas: Option<BTreeMap<String, RefOr<Schema, SchemaRef>>>,

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

    /// An object to hold reusable Media Type Objects (added in OAS 3.2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_types: Option<BTreeMap<String, RefOr<crate::v3_2::media_type::MediaType>>>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    ///
    /// Any other key the object does not define is kept here too, so the document round-trips;
    /// validation reports it unless [`crate::validation::Options::IgnoreUnknownFields`] is set.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Components {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_unknown_fields(&self.extensions, ctx, &path);
        let re = regex!(r"^[a-zA-Z0-9.\-_]+$");

        // Pass 1: validate every component key and every component body that
        // was not already validated through a `$ref` walk. Running this before
        // unused checks ensures component-to-component references (including
        // reverse-alphabetical aliases like `ZAlias -> ATarget`) mark their
        // targets visited before pass 2 runs, and prevents duplicate errors on
        // referenced components.
        macro_rules! validate_bag {
            ($map:expr, $section:literal) => {
                if let Some(objs) = &$map {
                    for (name, obj) in objs {
                        validate_string_matches(
                            name,
                            re,
                            ctx,
                            format!(concat!("{}.", $section, "[<name>]"), path),
                        );
                        let reference = format!(concat!("#/components/", $section, "/{}"), name);
                        if ctx.mark_validated(reference) {
                            obj.validate_with_context(
                                ctx,
                                format!(concat!("{}.", $section, "[{}]"), path, name),
                            );
                        }
                    }
                }
            };
        }

        validate_bag!(self.schemas, "schemas");
        validate_bag!(self.responses, "responses");
        validate_bag!(self.parameters, "parameters");
        validate_bag!(self.examples, "examples");
        validate_bag!(self.request_bodies, "requestBodies");
        validate_bag!(self.headers, "headers");
        validate_bag!(self.security_schemes, "securitySchemes");
        if let Some(objs) = &self.path_items {
            // operationId uniqueness is enforced by the pre-pass in
            // `Spec::validate_with_context`; re-inserting here would
            // double-count and report a false duplicate.
            for (name, item) in objs {
                validate_string_matches(name, re, ctx, format!("{path}.pathItems[<name>]"));
                let reference = format!("#/components/pathItems/{name}");
                if ctx.mark_validated(reference) {
                    item.validate_with_context(ctx, format!("{path}.pathItems[{name}]"));
                }
            }
        }
        validate_bag!(self.links, "links");
        validate_bag!(self.callbacks, "callbacks");
        validate_bag!(self.media_types, "mediaTypes");

        // Pass 2: report unused components and unused OAuth2 scopes now that
        // every cross-component reference has been visited.
        macro_rules! check_unused_bag {
            ($map:expr, $section:literal, $opt:expr) => {
                if let Some(objs) = &$map
                    && !ctx.is_option($opt)
                {
                    for name in objs.keys() {
                        let reference = format!(concat!("#/components/", $section, "/{}"), name);
                        if !ctx.is_visited(&reference) {
                            ctx.error(reference, "unused");
                        }
                    }
                }
            };
        }

        check_unused_bag!(self.schemas, "schemas", Options::IgnoreUnusedSchemas);
        check_unused_bag!(self.responses, "responses", Options::IgnoreUnusedResponses);
        check_unused_bag!(
            self.parameters,
            "parameters",
            Options::IgnoreUnusedParameters
        );
        check_unused_bag!(self.examples, "examples", Options::IgnoreUnusedExamples);
        check_unused_bag!(
            self.request_bodies,
            "requestBodies",
            Options::IgnoreUnusedRequestBodies
        );
        check_unused_bag!(self.headers, "headers", Options::IgnoreUnusedHeaders);
        if let Some(objs) = &self.security_schemes
            && !ctx.is_option(Options::IgnoreUnusedSecuritySchemes)
        {
            for (name, obj) in objs {
                let reference = format!("#/components/securitySchemes/{name}");
                if !ctx.is_visited(&reference) {
                    ctx.error(reference.clone(), "unused");
                }
                if let Ok(SecurityScheme::OAuth2(oauth2)) = obj.get_item(ctx.spec) {
                    let mut check_unused = |scopes: &BTreeMap<String, String>| {
                        for scope in scopes.keys() {
                            let r = format!("{reference}/{scope}");
                            if !ctx.is_visited(&r) {
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
                    if let Some(flow) = &oauth2.flows.device_authorization {
                        check_unused(&flow.scopes);
                    }
                }
            }
        }
        check_unused_bag!(self.path_items, "pathItems", Options::IgnoreUnusedPathItems);
        check_unused_bag!(self.links, "links", Options::IgnoreUnusedLinks);
        check_unused_bag!(self.callbacks, "callbacks", Options::IgnoreUnusedCallbacks);
        check_unused_bag!(
            self.media_types,
            "mediaTypes",
            Options::IgnoreUnusedMediaTypes
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::reference::ReferenceObject;
    use crate::v3_2::operation::Operation;
    use crate::v3_2::parameter::{InQuery, Parameter};
    use crate::v3_2::response::{Response, Responses};
    use crate::v3_2::schema::{Schema, SingleSchema, StringSchema};
    use crate::v3_2::security_scheme::{
        AuthorizationCodeOAuth2Flow, ClientCredentialsOAuth2Flow, ImplicitOAuth2Flow, OAuth2Flows,
        OAuth2SecurityScheme, PasswordOAuth2Flow,
    };
    use crate::validation::Context;
    use crate::validation::ValidationErrorsExt;
    use serde_json::json;

    fn map_with<T, R: ReferenceObject>(name: &str, t: T) -> BTreeMap<String, RefOr<T, R>> {
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
            "mediaTypes": {"MT": {"schema": {"type": "object"}}},
            "x-tra": "yes"
        });
        let comp: Components = serde_json::from_value(v.clone()).unwrap();
        // Round-trip preserves all maps.
        let re: Components = serde_json::from_value(serde_json::to_value(&comp).unwrap()).unwrap();
        assert_eq!(re, comp);
    }

    #[test]
    fn content_ref_into_components_media_types_marks_used() {
        // OAS 3.2: `content` map values are RefOr<MediaType>, so
        // `{"$ref": "#/components/mediaTypes/JsonOk"}` should resolve and
        // mark the component as used (no spurious unused-detection error).
        use crate::v3_2::media_type::MediaType;
        use crate::v3_2::path_item::{PathItem, Paths};
        use crate::v3_2::response::Response;

        let mut content = BTreeMap::new();
        content.insert(
            "application/json".to_owned(),
            RefOr::<MediaType>::new_ref("#/components/mediaTypes/JsonOk".to_owned()),
        );
        let resp = Response {
            description: Some("ok".into()),
            content: Some(content),
            ..Default::default()
        };
        let mut ops = BTreeMap::new();
        ops.insert(
            "get".to_owned(),
            crate::v3_2::operation::Operation {
                responses: Some(crate::v3_2::response::Responses {
                    responses: Some(BTreeMap::from([("200".to_owned(), RefOr::new_item(resp))])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let mut paths = Paths::default();
        paths.paths.insert(
            "/x".to_owned(),
            PathItem {
                operations: Some(ops),
                ..Default::default()
            },
        );

        let mt = MediaType {
            schema: Some(RefOr::new_item(Schema::Single(Box::new(
                SingleSchema::Object(crate::v3_2::schema::ObjectSchema::default()),
            )))),
            ..Default::default()
        };
        let comp = Components {
            media_types: Some(BTreeMap::from([("JsonOk".to_owned(), RefOr::new_item(mt))])),
            ..Default::default()
        };
        let spec = crate::v3_2::spec::Spec {
            info: crate::v3_2::info::Info {
                title: "x".into(),
                version: "1".into(),
                ..Default::default()
            },
            paths: Some(paths),
            components: Some(comp),
            ..Default::default()
        };
        use crate::validation::Validate;
        spec.validate(crate::validation::Options::new(), None)
            .unwrap();
    }

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
                    description: Some("ok".into()),
                    ..Default::default()
                },
            )),
            parameters: Some(map_with(
                "P",
                Parameter::Query(Box::new(InQuery {
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
                })),
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
                    deprecated: None,
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
                        device_authorization: None,
                        extensions: None,
                    },
                    description: None,
                    oauth2_metadata_url: None,
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
            media_types: None,
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
                    description: Some("ok".into()),
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
            !ctx.errors.mentions("unused"),
            "no unused errors when ignored: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn unused_device_authorization_flow_scope_reported() {
        // line 183: device_authorization flow scope unused-detection
        use crate::v3_2::security_scheme::{
            DeviceAuthorizationOAuth2Flow, OAuth2Flows, OAuth2SecurityScheme,
        };
        let comp = Components {
            security_schemes: Some(map_with(
                "SS",
                SecurityScheme::OAuth2(Box::new(OAuth2SecurityScheme {
                    flows: OAuth2Flows {
                        device_authorization: Some(DeviceAuthorizationOAuth2Flow {
                            device_authorization_url: "https://x.example/device".into(),
                            token_url: "https://x.example/token".into(),
                            scopes: BTreeMap::from([("device:read".to_owned(), "Read".to_owned())]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                })),
            )),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp.clone()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::empty());
        comp.validate_with_context(&mut ctx, "#.components".into());
        let scope_path = "#/components/securitySchemes/SS/device:read";
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(scope_path) && e.contains("unused")),
            "expected unused device_authorization scope: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn unused_media_types_component_reported() {
        // line 229: media_types entry unused-detection
        use crate::v3_2::media_type::MediaType;
        let comp = Components {
            media_types: Some(BTreeMap::from([(
                "JsonOk".to_owned(),
                RefOr::new_item(MediaType::default()),
            )])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp.clone()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::empty());
        comp.validate_with_context(&mut ctx, "#.components".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("#/components/mediaTypes/JsonOk") && e.contains("unused")),
            "expected unused media_types/JsonOk: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn invalid_component_name_reported() {
        let mut schemas: BTreeMap<String, RefOr<Schema, SchemaRef>> = BTreeMap::new();
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
            ctx.errors.mentions("must match pattern"),
            "expected pattern error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn reverse_alphabetical_alias_not_falsely_unused_and_no_duplicate_errors() {
        use crate::validation::Validate;
        let spec: Spec = serde_json::from_value(json!({
            "openapi": "3.2.0",
            "info": { "title": "T", "version": "1.0" },
            "paths": {
                "/pets": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/ZAlias" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "ATarget": { "type": "string", "minLength": 5, "maxLength": 1 },
                    "ZAlias": { "$ref": "#/components/schemas/ATarget" }
                }
            }
        }))
        .unwrap();
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            !err.mentions("unused"),
            "neither ATarget nor ZAlias is unused: {:?}",
            err.errors
        );
        let min_max_errors: Vec<_> = err
            .errors
            .iter()
            .filter(|e| e.contains("minLength"))
            .collect();
        assert_eq!(
            min_max_errors.len(),
            1,
            "referenced component error must be reported once, got: {:?}",
            err.errors
        );

        // Even when ZAlias itself is unused, ATarget is referenced by ZAlias
        // and must not be falsely flagged unused due to BTreeMap order.
        let unref_spec: Spec = serde_json::from_value(json!({
            "openapi": "3.2.0",
            "info": { "title": "T", "version": "1.0" },
            "components": {
                "schemas": {
                    "ATarget": { "type": "string" },
                    "ZAlias": { "$ref": "#/components/schemas/ATarget" }
                }
            }
        }))
        .unwrap();
        let err2 = unref_spec.validate(Options::new(), None).unwrap_err();
        assert!(
            !err2.mentions("#/components/schemas/ATarget"),
            "ATarget is referenced by ZAlias: {:?}",
            err2.errors
        );
        assert!(err2.mentions("#/components/schemas/ZAlias"));
    }

    #[test]
    fn define_media_type_inserts_and_validates_name() {
        let mut spec = Spec::default();
        let mt = crate::v3_2::media_type::MediaType::default();
        let r = spec.define_media_type("JsonOk", mt).unwrap();
        assert_eq!(r, RefOr::new_ref("#/components/mediaTypes/JsonOk"));
        assert!(
            spec.define_media_type("bad name", Default::default())
                .is_err()
        );
    }

    #[test]
    fn external_and_dangling_aliases_do_not_poison_visited_set() {
        use crate::validation::Validate;
        let spec: Spec = serde_json::from_value(json!({
            "openapi": "3.2.0",
            "info": { "title": "T", "version": "1.0" },
            "paths": {
                "/a": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/ExtAlias" }
                                    }
                                }
                            }
                        }
                    }
                },
                "/b": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "https://example.test/pet.json#/Pet" }
                                    }
                                }
                            }
                        }
                    }
                },
                "/c": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/DanglingAlias" }
                                    }
                                }
                            }
                        }
                    }
                },
                "/d": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/Missing" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "components": {
                "schemas": {
                    "ExtAlias": { "$ref": "https://example.test/pet.json#/Pet" },
                    "DanglingAlias": { "$ref": "#/components/schemas/Missing" }
                }
            }
        }))
        .unwrap();
        let err = spec.validate(Options::new(), None).unwrap_err();
        assert!(
            err.mentions_all(&["paths[/b]", "https://example.test/pet.json#/Pet"]),
            "external ref on /b must not be silently skipped: {:?}",
            err.errors
        );
        assert!(
            err.mentions_all(&["paths[/d]", "#/components/schemas/Missing"]),
            "dangling ref on /d must not be silently skipped: {:?}",
            err.errors
        );
    }
}
