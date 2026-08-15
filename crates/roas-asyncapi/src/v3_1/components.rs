//! AsyncAPI v3.1 `Components` object.
//!
//! Per [Components Object](https://www.asyncapi.com/docs/reference/specification/v3.1.0#componentsObject).
//!
//! Every map is keyed by a component name matching
//! `^[A-Za-z0-9\.\-_]+$`; the values are the same objects that may
//! appear inline elsewhere in the document.

use crate::common::bindings::Bindings;
use crate::common::reference::RefOr;
use crate::v3_1::channel::Channel;
use crate::v3_1::correlation_id::CorrelationId;
use crate::v3_1::external_documentation::ExternalDocumentation;
use crate::v3_1::message::{Message, MessageTrait};
use crate::v3_1::operation::{Operation, OperationReply, OperationReplyAddress, OperationTrait};
use crate::v3_1::parameter::Parameter;
use crate::v3_1::schema::SchemaOrMultiFormat;
use crate::v3_1::security_scheme::SecurityScheme;
use crate::v3_1::server::{Server, ServerVariable};
use crate::v3_1::tag::Tag;
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
    schemas: RefOr<SchemaOrMultiFormat> => "schemas",
    servers: RefOr<Server> => "servers",
    channels: RefOr<Channel> => "channels",
    operations: RefOr<Operation> => "operations",
    messages: RefOr<Message> => "messages",
    security_schemes: RefOr<SecurityScheme> => "securitySchemes",
    server_variables: RefOr<ServerVariable> => "serverVariables",
    parameters: RefOr<Parameter> => "parameters",
    correlation_ids: RefOr<CorrelationId> => "correlationIds",
    replies: RefOr<OperationReply> => "replies",
    reply_addresses: RefOr<OperationReplyAddress> => "replyAddresses",
    external_docs: RefOr<ExternalDocumentation> => "externalDocs",
    tags: RefOr<Tag> => "tags",
    operation_traits: RefOr<OperationTrait> => "operationTraits",
    message_traits: RefOr<MessageTrait> => "messageTraits",
    server_bindings: RefOr<Bindings> => "serverBindings",
    channel_bindings: RefOr<Bindings> => "channelBindings",
    operation_bindings: RefOr<Bindings> => "operationBindings",
    message_bindings: RefOr<Bindings> => "messageBindings",
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
            "servers": { "prod": { "host": "h", "protocol": "kafka" } },
            "channels": { "user": { "address": "user" } },
            "operations": { "send": { "action": "send", "channel": { "$ref": "#/channels/user" } } },
            "messages": { "signup": { "name": "Signup" } },
            "securitySchemes": { "sasl": { "type": "scramSha256" } },
            "serverVariables": { "port": { "default": "9092" } },
            "parameters": { "userId": { "description": "id" } },
            "correlationIds": { "trace": { "location": "$message.header#/id" } },
            "replies": { "ack": { "channel": { "$ref": "#/channels/replies" } } },
            "replyAddresses": { "to": { "location": "$message.header#/replyTo" } },
            "externalDocs": { "main": { "url": "https://example.com" } },
            "tags": { "user": { "name": "user" } },
            "operationTraits": { "common": { "title": "t" } },
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
    fn empty_components_serialize_to_an_empty_object() {
        let components = Components::default();
        assert_eq!(serde_json::to_value(&components).unwrap(), json!({}));

        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        components.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }

    #[test]
    fn invalid_keys_are_reported_per_map() {
        let components: Components = serde_json::from_value(json!({
            "messages": { "bad key": {} },
            "schemas": { "also/bad": { "type": "string" } }
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
                .any(|e| e.contains("#.components.schemas.also/bad"))
        );
    }

    #[test]
    fn nested_component_errors_carry_their_path() {
        let components: Components = serde_json::from_value(json!({
            "servers": { "prod": { "host": "", "protocol": "" } },
            "correlationIds": { "trace": { "location": "nope" } },
            "securitySchemes": { "http": { "type": "http" } }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.components");
        components.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e == "#.components.servers.prod.host: must not be empty")
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.components.correlationIds.trace.location"))
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.components.securitySchemes.http.scheme"))
        );
    }
}
