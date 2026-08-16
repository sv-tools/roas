//! AsyncAPI v2.6 `Components` object.
//!
//! Per [Components Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#componentsObject).
//!
//! 2.6 holds fourteen maps; v3 added `operations`, `replies`,
//! `replyAddresses`, `tags`, and `externalDocs` to that set.
//!
//! Every map here takes `Object | Reference Object` except `channels`,
//! per the specification's own field table. `channels` is the one
//! exception it makes, a Channel Item carrying `$ref` as a field of its
//! own — which is why [`ChannelItem`] is not a [`RefOr`].
//!
//! The bundled `schema.json` disagrees: it gives the trait and binding
//! maps their objects directly, with no Reference alternative. The
//! prose is what this crate follows — AsyncAPI says so itself, and its
//! JSON Schemas do not track the specification one-for-one — so a
//! `$ref` in those maps is a reference here.

use crate::common::bindings::{
    ChannelBindings, MessageBindings, OperationBindings, ServerBindings,
};
use crate::common::reference::RefOr;
use crate::v2_6::channel_item::ChannelItem;
use crate::v2_6::correlation_id::CorrelationId;
use crate::v2_6::message::{Message, MessageTrait};
use crate::v2_6::operation::OperationTrait;
use crate::v2_6::parameter::Parameter;
use crate::v2_6::schema::SubSchema;
use crate::v2_6::security_scheme::SecurityScheme;
use crate::v2_6::server::{Server, ServerVariable};
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! component_maps {
    ($( $field:ident : $ty:ty => $name:literal ),+ $(,)?) => {
        #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
        pub struct Components {
            $(
                #[serde(rename = $name, default, skip_serializing_if = "BTreeMap::is_empty")]
                pub $field: BTreeMap<String, $ty>,
            )+

            /// `x-`-prefixed Specification Extensions.
            #[serde(flatten)]
            #[serde(with = "crate::common::extensions")]
            #[serde(skip_serializing_if = "Option::is_none")]
            pub extensions: Option<BTreeMap<String, serde_json::Value>>,
        }

        impl ValidateWithContext for Components {
            fn validate_with_context(&self, ctx: &mut Context) {
                $(
                    ctx.validate_map_keys($name, &self.$field);
                    for (key, value) in &self.$field {
                        ctx.in_key($name, key, |ctx| value.validate_with_context(ctx));
                    }
                )+
            }
        }
    };
}

component_maps! {
    schemas: SubSchema => "schemas",
    servers: RefOr<Server> => "servers",
    channels: ChannelItem => "channels",
    server_variables: RefOr<ServerVariable> => "serverVariables",
    messages: RefOr<Message> => "messages",
    security_schemes: RefOr<SecurityScheme> => "securitySchemes",
    parameters: RefOr<Parameter> => "parameters",
    correlation_ids: RefOr<CorrelationId> => "correlationIds",
    operation_traits: RefOr<OperationTrait> => "operationTraits",
    message_traits: RefOr<MessageTrait> => "messageTraits",
    server_bindings: RefOr<ServerBindings> => "serverBindings",
    channel_bindings: RefOr<ChannelBindings> => "channelBindings",
    operation_bindings: RefOr<OperationBindings> => "operationBindings",
    message_bindings: RefOr<MessageBindings> => "messageBindings",
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_every_component_map() {
        let value = json!({
            "schemas": { "user": { "type": "object" } },
            "servers": { "prod": { "url": "amqp://e", "protocol": "amqp" } },
            "channels": { "user": { "description": "d" } },
            "serverVariables": { "port": { "default": "5672" } },
            "messages": { "signup": { "name": "Signup" } },
            "securitySchemes": { "sasl": { "type": "scramSha256" } },
            "parameters": { "userId": { "description": "id" } },
            "correlationIds": { "trace": { "location": "$message.header#/id" } },
            "operationTraits": { "common": { "summary": "s" } },
            "messageTraits": { "common": { "contentType": "application/json" } },
            "serverBindings": { "kafka": { "kafka": {} } },
            "channelBindings": { "kafka": { "kafka": {} } },
            "operationBindings": { "kafka": { "kafka": {} } },
            "messageBindings": { "kafka": { "kafka": {} } },
            "x-owner": "team"
        });
        let components: Components = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(components.schemas.len(), 1);
        assert_eq!(components.message_bindings.len(), 1);
        assert_eq!(serde_json::to_value(&components).unwrap(), value);

        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        components.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn v3_only_maps_are_not_component_fields_here() {
        // `operations`, `replies`, `replyAddresses`, `tags` and
        // `externalDocs` arrived in v3.
        let components: Components = serde_json::from_value(json!({
            "operations": { "o": {} },
            "replies": { "r": {} },
            "tags": { "t": { "name": "x" } }
        }))
        .unwrap();
        assert_eq!(serde_json::to_value(&components).unwrap(), json!({}));
    }

    #[test]
    fn a_component_channel_keeps_its_ref_siblings() {
        // `$ref` is a Channel Item *field*, so it coexists with the
        // rest instead of replacing them.
        let value = json!({
            "channels": {
                "user": { "$ref": "#/channels/other", "description": "d", "deprecated": true }
            }
        });
        let components: Components = serde_json::from_value(value.clone()).unwrap();
        let channel = &components.channels["user"];
        assert_eq!(channel.reference.as_deref(), Some("#/channels/other"));
        assert_eq!(channel.description.as_deref(), Some("d"));
        assert!(channel.is_reference());
        assert_eq!(serde_json::to_value(&components).unwrap(), value);
    }

    #[test]
    fn a_schema_ref_is_a_reference_object() {
        // "Any time a Schema Object can be used, a Reference Object can
        // be used in its place … the `$ref` keyword MUST follow the
        // behavior described by Reference Object instead of the one in
        // JSON Schema definition." A Reference Object is `$ref` alone,
        // so its siblings are ignored — and dropped.
        let components: Components = serde_json::from_value(json!({
            "schemas": {
                "user": { "$ref": "#/components/schemas/base", "description": "ignored" }
            }
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&components).unwrap(),
            json!({ "schemas": { "user": { "$ref": "#/components/schemas/base" } } })
        );

        // Nested schema positions take one too.
        let value = json!({
            "schemas": {
                "user": {
                    "type": "object",
                    "properties": { "p": { "$ref": "#/components/schemas/base" } }
                }
            }
        });
        let components: Components = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&components).unwrap(), value);
    }

    #[test]
    fn schemas_accept_the_boolean_form() {
        // `components.schemas: { "Never": false }` is a legal draft-07
        // schema map.
        let value = json!({ "schemas": { "Never": false, "Any": true } });
        let components: Components = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(components.schemas.len(), 2);
        assert_eq!(serde_json::to_value(&components).unwrap(), value);
    }

    #[test]
    fn only_the_maps_the_schema_allows_take_a_reference() {
        // `messages`, `securitySchemes` and `correlationIds` are
        // `oneOf: [Reference, …]`; the rest are typed directly.
        let referenced = json!({
            "messages": { "a": { "$ref": "#/components/messages/b" } },
            "securitySchemes": { "s": { "$ref": "#/x" } },
            "correlationIds": { "c": { "$ref": "#/x" } }
        });
        let components: Components = serde_json::from_value(referenced.clone()).unwrap();
        assert_eq!(serde_json::to_value(&components).unwrap(), referenced);

        // `servers`, `serverVariables` and `parameters` delegate to
        // definitions that also allow a Reference, so those round-trip
        // as references rather than collapsing to `{}`.
        let delegated = json!({
            "servers": { "s": { "$ref": "#/x" } },
            "serverVariables": { "v": { "$ref": "#/x" } },
            "parameters": { "p": { "$ref": "#/x" } }
        });
        let components: Components = serde_json::from_value(delegated.clone()).unwrap();
        assert_eq!(serde_json::to_value(&components).unwrap(), delegated);

        // The trait and binding maps take a Reference too, whatever the
        // bundled schema says — a trait has no required field, so an
        // unreferenced `$ref` would otherwise be swallowed whole.
        let traits = json!({
            "operationTraits": { "t": { "$ref": "#/x" } },
            "messageBindings": { "b": { "$ref": "#/x" } }
        });
        let components: Components = serde_json::from_value(traits.clone()).unwrap();
        assert_eq!(serde_json::to_value(&components).unwrap(), traits);
        assert!(components.operation_traits["t"].reference().is_some());
    }

    #[test]
    fn empty_components_serialize_to_an_empty_object() {
        let components = Components::default();
        assert_eq!(serde_json::to_value(&components).unwrap(), json!({}));

        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        components.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn invalid_keys_and_nested_errors_are_reported() {
        let components: Components = serde_json::from_value(json!({
            "messages": { "bad key": {} },
            "servers": { "prod": { "url": "", "protocol": "" } },
            "correlationIds": { "trace": { "location": "nope" } }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        components.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e.contains("#.components.messages.bad key"))
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.components.servers.prod.url: must not be empty")
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.components.correlationIds.trace.location"))
        );
    }
}
