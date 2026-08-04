//! AsyncAPI v2.6 `Components` object.
//!
//! Per [Components Object](https://www.asyncapi.com/docs/reference/specification/v2.6.0#componentsObject).
//!
//! 2.6 holds fourteen maps; v3 added `operations`, `replies`,
//! `replyAddresses`, `tags`, and `externalDocs` to that set.
//!
//! Only `messages`, `securitySchemes`, and `correlationIds` accept a
//! `$ref` in place of the object — the other maps are typed directly,
//! matching the schema field by field. `schemas` uses [`SubSchema`]
//! because draft-07 allows a boolean where a schema is expected.

use crate::common::bindings::Bindings;
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
    servers: Server => "servers",
    channels: RefOr<ChannelItem> => "channels",
    server_variables: ServerVariable => "serverVariables",
    messages: RefOr<Message> => "messages",
    security_schemes: RefOr<SecurityScheme> => "securitySchemes",
    parameters: Parameter => "parameters",
    correlation_ids: RefOr<CorrelationId> => "correlationIds",
    operation_traits: OperationTrait => "operationTraits",
    message_traits: MessageTrait => "messageTraits",
    server_bindings: Bindings => "serverBindings",
    channel_bindings: Bindings => "channelBindings",
    operation_bindings: Bindings => "operationBindings",
    message_bindings: Bindings => "messageBindings",
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

        // A `$ref` where the schema expects the object itself does not
        // parse.
        assert!(
            serde_json::from_value::<Components>(json!({
                "servers": { "s": { "$ref": "#/x" } }
            }))
            .is_err(),
        );
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
