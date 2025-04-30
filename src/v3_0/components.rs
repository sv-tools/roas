//! Holds a set of reusable objects for different aspects of the OAS.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::common::helpers::{Context, PushError, ValidateWithContext, validate_string_matches};
use crate::common::reference::RefOr;
use crate::v3_0::callback::Callback;
use crate::v3_0::example::Example;
use crate::v3_0::header::Header;
use crate::v3_0::link::Link;
use crate::v3_0::parameter::Parameter;
use crate::v3_0::request_body::RequestBody;
use crate::v3_0::response::Response;
use crate::v3_0::schema::Schema;
use crate::v3_0::security_scheme::SecurityScheme;
use crate::v3_0::spec::Spec;
use crate::validation::Options;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
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
    #[serde(rename = "requestBodies")]
    pub request_bodies: Option<BTreeMap<String, RefOr<RequestBody>>>,

    /// An object to hold reusable Header Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, RefOr<Header>>>,

    /// An object to hold reusable Security Scheme Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "securitySchemes")]
    pub security_schemes: Option<BTreeMap<String, RefOr<SecurityScheme>>>,

    /// An object to hold reusable Link Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<BTreeMap<String, RefOr<Link>>>,

    /// An object to hold reusable Callback Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<BTreeMap<String, RefOr<Callback>>>,

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
        let re = Regex::new(r"^[a-zA-Z0-9.\-_]+$").unwrap();

        if let Some(objs) = &self.schemas {
            for (name, obj) in objs {
                let reference = format!("#/components/schemas/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedSchemas) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.schemas[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.schemas[{}]", path, name));
            }
        }

        if let Some(objs) = &self.responses {
            for (name, obj) in objs {
                let reference = format!("#/components/responses/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedResponses) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.responses[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.responses[{}]", path, name));
            }
        }

        if let Some(objs) = &self.parameters {
            for (name, obj) in objs {
                let reference = format!("#/components/parameters/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedParameters) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.parameters[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.parameters[{}]", path, name));
            }
        }

        if let Some(objs) = &self.examples {
            for (name, obj) in objs {
                let reference = format!("#/components/examples/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedExamples) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.examples[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.examples[{}]", path, name));
            }
        }

        if let Some(objs) = &self.request_bodies {
            for (name, obj) in objs {
                let reference = format!("#/components/requestBodies/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedRequestBodies)
                {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.requestBodies[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.requestBodies[{}]", path, name));
            }
        }

        if let Some(objs) = &self.headers {
            for (name, obj) in objs {
                let reference = format!("#/components/headers/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedHeaders) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.headers[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.headers[{}]", path, name));
            }
        }

        if let Some(objs) = &self.security_schemes {
            for (name, obj) in objs {
                let reference = format!("#/components/securitySchemes/{}", name);
                if !ctx.is_visited(&reference)
                    && !ctx.is_option(Options::IgnoreUnusedSecuritySchemes)
                {
                    ctx.error(reference.clone(), "unused");
                }
                validate_string_matches(
                    name,
                    &re,
                    ctx,
                    format!("{}.securitySchemes[<name>]", path),
                );
                obj.validate_with_context(ctx, format!("{}.securitySchemes[{}]", path, name));
                if let Ok(SecurityScheme::OAuth2(oauth2)) = obj.get_item(ctx.spec) {
                    if let Some(flow) = &oauth2.flows.implicit {
                        for scope in flow.scopes.keys() {
                            let reference = format!("{}/{}", reference, scope);
                            if !ctx.is_visited(&reference)
                                && !ctx.is_option(Options::IgnoreUnusedSecuritySchemes)
                            {
                                ctx.error(reference, "unused");
                            }
                        }
                    }
                }
            }
        }

        if let Some(objs) = &self.links {
            for (name, obj) in objs {
                let reference = format!("#/components/links/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedLinks) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.links[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.links[{}]", path, name));
            }
        }

        if let Some(objs) = &self.callbacks {
            for (name, obj) in objs {
                let reference = format!("#/components/callbacks/{}", name);
                if !ctx.is_visited(&reference) && !ctx.is_option(Options::IgnoreUnusedCallbacks) {
                    ctx.error(reference, "unused");
                }
                validate_string_matches(name, &re, ctx, format!("{}.callbacks[<name>]", path));
                obj.validate_with_context(ctx, format!("{}.callbacks[{}]", path, name));
            }
        }
    }
}
