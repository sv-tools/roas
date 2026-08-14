//! Upconversion from AsyncAPI v3.0 to v3.1.
//!
//! v3.1 left the object model alone — the only additions are this
//! version's own `schemaFormat` media types and the `ros2` bindings —
//! so every conversion here is a field-for-field remap with nothing
//! dropped or approximated. The one value that changes is the
//! `asyncapi` version string itself.
//!
//! Available when both the `v3_0` and `v3_1` features are enabled.

use crate::common::reference::RefOr;
use crate::{v3_0, v3_1};
use std::collections::BTreeMap;

/// Convert a `RefOr<A>` into a `RefOr<B>`, leaving a `$ref` untouched.
fn map_ref_or<A, B: From<A>>(value: RefOr<A>) -> RefOr<B> {
    match value {
        RefOr::Reference(reference) => RefOr::Reference(reference),
        RefOr::Item(item) => RefOr::Item(item.into()),
    }
}

fn map_map<A, B: From<A>>(map: BTreeMap<String, RefOr<A>>) -> BTreeMap<String, RefOr<B>> {
    map.into_iter()
        .map(|(key, value)| (key, map_ref_or(value)))
        .collect()
}

/// Convert a map of plain (non-referenceable) values.
fn map_plain_map<A, B: From<A>>(map: BTreeMap<String, A>) -> BTreeMap<String, B> {
    map.into_iter()
        .map(|(key, value)| (key, value.into()))
        .collect()
}

fn map_vec<A, B: From<A>>(items: Vec<RefOr<A>>) -> Vec<RefOr<B>> {
    items.into_iter().map(map_ref_or).collect()
}

fn map_boxed<A, B: From<A>>(value: A) -> Box<B> {
    Box::new(value.into())
}

impl From<v3_0::Document> for v3_1::Document {
    fn from(value: v3_0::Document) -> Self {
        Self {
            // The only value that actually changes.
            asyncapi: v3_1::Version::V3_1_0(),
            id: value.id,
            info: value.info.into(),
            servers: map_map(value.servers),
            default_content_type: value.default_content_type,
            channels: map_map(value.channels),
            operations: map_map(value.operations),
            components: value.components.map(Into::into),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Info> for v3_1::Info {
    fn from(value: v3_0::Info) -> Self {
        Self {
            title: value.title,
            version: value.version,
            description: value.description,
            terms_of_service: value.terms_of_service,
            contact: value.contact.map(Into::into),
            license: value.license.map(Into::into),
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Contact> for v3_1::Contact {
    fn from(value: v3_0::Contact) -> Self {
        Self {
            name: value.name,
            url: value.url,
            email: value.email,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::License> for v3_1::License {
    fn from(value: v3_0::License) -> Self {
        Self {
            name: value.name,
            url: value.url,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Tag> for v3_1::Tag {
    fn from(value: v3_0::Tag) -> Self {
        Self {
            name: value.name,
            description: value.description,
            external_docs: value.external_docs.map(map_ref_or),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::ExternalDocumentation> for v3_1::ExternalDocumentation {
    fn from(value: v3_0::ExternalDocumentation) -> Self {
        Self {
            url: value.url,
            description: value.description,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Server> for v3_1::Server {
    fn from(value: v3_0::Server) -> Self {
        Self {
            host: value.host,
            protocol: value.protocol,
            pathname: value.pathname,
            protocol_version: value.protocol_version,
            title: value.title,
            summary: value.summary,
            description: value.description,
            variables: map_map(value.variables),
            security: map_vec(value.security),
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            bindings: value.bindings.map(map_ref_or),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::ServerVariable> for v3_1::ServerVariable {
    fn from(value: v3_0::ServerVariable) -> Self {
        Self {
            enum_values: value.enum_values,
            default: value.default,
            description: value.description,
            examples: value.examples,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Channel> for v3_1::Channel {
    fn from(value: v3_0::Channel) -> Self {
        Self {
            address: value.address,
            messages: map_map(value.messages),
            parameters: map_map(value.parameters),
            title: value.title,
            summary: value.summary,
            description: value.description,
            servers: value.servers,
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            bindings: value.bindings.map(map_ref_or),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Parameter> for v3_1::Parameter {
    fn from(value: v3_0::Parameter) -> Self {
        Self {
            enum_values: value.enum_values,
            default: value.default,
            description: value.description,
            examples: value.examples,
            location: value.location,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::OperationAction> for v3_1::OperationAction {
    fn from(value: v3_0::OperationAction) -> Self {
        match value {
            v3_0::OperationAction::Send => Self::Send,
            v3_0::OperationAction::Receive => Self::Receive,
        }
    }
}

impl From<v3_0::Operation> for v3_1::Operation {
    fn from(value: v3_0::Operation) -> Self {
        Self {
            action: value.action.into(),
            channel: value.channel,
            messages: value.messages,
            reply: value.reply.map(map_ref_or),
            traits: map_vec(value.traits),
            title: value.title,
            summary: value.summary,
            description: value.description,
            security: map_vec(value.security),
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            bindings: value.bindings.map(map_ref_or),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::OperationTrait> for v3_1::OperationTrait {
    fn from(value: v3_0::OperationTrait) -> Self {
        Self {
            title: value.title,
            summary: value.summary,
            description: value.description,
            security: map_vec(value.security),
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            bindings: value.bindings.map(map_ref_or),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::OperationReply> for v3_1::OperationReply {
    fn from(value: v3_0::OperationReply) -> Self {
        Self {
            address: value.address.map(map_ref_or),
            channel: value.channel,
            messages: value.messages,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::OperationReplyAddress> for v3_1::OperationReplyAddress {
    fn from(value: v3_0::OperationReplyAddress) -> Self {
        Self {
            location: value.location,
            description: value.description,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Message> for v3_1::Message {
    fn from(value: v3_0::Message) -> Self {
        Self {
            headers: value.headers.map(Into::into),
            payload: value.payload.map(Into::into),
            correlation_id: value.correlation_id.map(map_ref_or),
            content_type: value.content_type,
            name: value.name,
            title: value.title,
            summary: value.summary,
            description: value.description,
            deprecated: value.deprecated,
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            bindings: value.bindings.map(map_ref_or),
            examples: value.examples.into_iter().map(Into::into).collect(),
            traits: map_vec(value.traits),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::MessageTrait> for v3_1::MessageTrait {
    fn from(value: v3_0::MessageTrait) -> Self {
        Self {
            headers: value.headers.map(Into::into),
            correlation_id: value.correlation_id.map(map_ref_or),
            content_type: value.content_type,
            name: value.name,
            title: value.title,
            summary: value.summary,
            description: value.description,
            deprecated: value.deprecated,
            tags: map_vec(value.tags),
            external_docs: value.external_docs.map(map_ref_or),
            bindings: value.bindings.map(map_ref_or),
            examples: value.examples.into_iter().map(Into::into).collect(),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::MessageExample> for v3_1::MessageExample {
    fn from(value: v3_0::MessageExample) -> Self {
        Self {
            name: value.name,
            summary: value.summary,
            headers: value.headers,
            payload: value.payload,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::CorrelationId> for v3_1::CorrelationId {
    fn from(value: v3_0::CorrelationId) -> Self {
        Self {
            location: value.location,
            description: value.description,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::SchemaOrMultiFormat> for v3_1::SchemaOrMultiFormat {
    fn from(value: v3_0::SchemaOrMultiFormat) -> Self {
        match value {
            v3_0::SchemaOrMultiFormat::Bool(b) => Self::Bool(b),
            v3_0::SchemaOrMultiFormat::MultiFormat(m) => Self::MultiFormat(m.into()),
            v3_0::SchemaOrMultiFormat::Schema(s) => Self::Schema(map_boxed(*s)),
        }
    }
}

impl From<v3_0::MultiFormatSchema> for v3_1::MultiFormatSchema {
    fn from(value: v3_0::MultiFormatSchema) -> Self {
        Self {
            // A 3.0 document's dialect is still one 3.1 accepts, so the
            // format carries over untouched.
            schema_format: value.schema_format,
            schema: value.schema,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::SubSchema> for v3_1::SubSchema {
    fn from(value: v3_0::SubSchema) -> Self {
        match value {
            v3_0::SubSchema::Bool(b) => Self::Bool(b),
            v3_0::SubSchema::Schema(s) => Self::Schema(map_boxed(*s)),
        }
    }
}

impl From<v3_0::Items> for v3_1::Items {
    fn from(value: v3_0::Items) -> Self {
        match value {
            v3_0::Items::Single(schema) => Self::Single(map_boxed(*schema)),
            v3_0::Items::Tuple(schemas) => {
                Self::Tuple(schemas.into_iter().map(Into::into).collect())
            }
        }
    }
}

impl From<v3_0::Dependency> for v3_1::Dependency {
    fn from(value: v3_0::Dependency) -> Self {
        match value {
            v3_0::Dependency::Required(names) => Self::Required(names),
            v3_0::Dependency::Schema(schema) => Self::Schema(schema.into()),
        }
    }
}

impl From<v3_0::SchemaType> for v3_1::SchemaType {
    fn from(value: v3_0::SchemaType) -> Self {
        match value {
            v3_0::SchemaType::Single(name) => Self::Single(name),
            v3_0::SchemaType::Multiple(names) => Self::Multiple(names),
        }
    }
}

impl From<v3_0::Schema> for v3_1::Schema {
    fn from(value: v3_0::Schema) -> Self {
        fn sub(map: BTreeMap<String, v3_0::SubSchema>) -> BTreeMap<String, v3_1::SubSchema> {
            map.into_iter().map(|(k, v)| (k, v.into())).collect()
        }
        fn list(items: Option<Vec<v3_0::SubSchema>>) -> Option<Vec<v3_1::SubSchema>> {
            items.map(|items| items.into_iter().map(Into::into).collect())
        }

        Self {
            reference: value.reference,
            id: value.id,
            dialect: value.dialect,
            comment: value.comment,
            schema_type: value.schema_type.map(Into::into),
            title: value.title,
            description: value.description,
            format: value.format,
            default: value.default,
            enum_values: value.enum_values,
            const_value: value.const_value,
            examples: value.examples,
            definitions: sub(value.definitions),
            properties: sub(value.properties),
            required: value.required,
            additional_properties: value.additional_properties.map(Into::into),
            pattern_properties: sub(value.pattern_properties),
            property_names: value.property_names.map(|value| map_boxed(*value)),
            dependencies: value
                .dependencies
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            min_properties: value.min_properties,
            max_properties: value.max_properties,
            items: value.items.map(Into::into),
            additional_items: value.additional_items.map(|value| map_boxed(*value)),
            contains: value.contains.map(|value| map_boxed(*value)),
            min_items: value.min_items,
            max_items: value.max_items,
            unique_items: value.unique_items,
            min_length: value.min_length,
            max_length: value.max_length,
            pattern: value.pattern,
            content_media_type: value.content_media_type,
            content_encoding: value.content_encoding,
            minimum: value.minimum,
            maximum: value.maximum,
            exclusive_minimum: value.exclusive_minimum,
            exclusive_maximum: value.exclusive_maximum,
            multiple_of: value.multiple_of,
            all_of: list(value.all_of),
            any_of: list(value.any_of),
            one_of: list(value.one_of),
            not: value.not.map(|value| map_boxed(*value)),
            if_schema: value.if_schema.map(|value| map_boxed(*value)),
            then_schema: value.then_schema.map(|value| map_boxed(*value)),
            else_schema: value.else_schema.map(|value| map_boxed(*value)),
            discriminator: value.discriminator,
            external_docs: value.external_docs.map(map_ref_or),
            deprecated: value.deprecated,
            read_only: value.read_only,
            write_only: value.write_only,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::ApiKeyLocation> for v3_1::ApiKeyLocation {
    fn from(value: v3_0::ApiKeyLocation) -> Self {
        match value {
            v3_0::ApiKeyLocation::User => Self::User,
            v3_0::ApiKeyLocation::Password => Self::Password,
            v3_0::ApiKeyLocation::Query => Self::Query,
            v3_0::ApiKeyLocation::Header => Self::Header,
            v3_0::ApiKeyLocation::Cookie => Self::Cookie,
        }
    }
}

impl From<v3_0::SecuritySchemeType> for v3_1::SecuritySchemeType {
    fn from(value: v3_0::SecuritySchemeType) -> Self {
        match value {
            v3_0::SecuritySchemeType::UserPassword => Self::UserPassword,
            v3_0::SecuritySchemeType::ApiKey => Self::ApiKey,
            v3_0::SecuritySchemeType::X509 => Self::X509,
            v3_0::SecuritySchemeType::SymmetricEncryption => Self::SymmetricEncryption,
            v3_0::SecuritySchemeType::AsymmetricEncryption => Self::AsymmetricEncryption,
            v3_0::SecuritySchemeType::HttpApiKey => Self::HttpApiKey,
            v3_0::SecuritySchemeType::Http => Self::Http,
            v3_0::SecuritySchemeType::OAuth2 => Self::OAuth2,
            v3_0::SecuritySchemeType::OpenIdConnect => Self::OpenIdConnect,
            v3_0::SecuritySchemeType::Plain => Self::Plain,
            v3_0::SecuritySchemeType::ScramSha256 => Self::ScramSha256,
            v3_0::SecuritySchemeType::ScramSha512 => Self::ScramSha512,
            v3_0::SecuritySchemeType::Gssapi => Self::Gssapi,
        }
    }
}

impl From<v3_0::SecurityScheme> for v3_1::SecurityScheme {
    fn from(value: v3_0::SecurityScheme) -> Self {
        Self {
            scheme_type: value.scheme_type.into(),
            description: value.description,
            in_: value.in_.map(Into::into),
            name: value.name,
            scheme: value.scheme,
            bearer_format: value.bearer_format,
            flows: value.flows.map(Into::into),
            open_id_connect_url: value.open_id_connect_url,
            scopes: value.scopes,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::OAuthFlows> for v3_1::OAuthFlows {
    fn from(value: v3_0::OAuthFlows) -> Self {
        Self {
            implicit: value.implicit.map(Into::into),
            password: value.password.map(Into::into),
            client_credentials: value.client_credentials.map(Into::into),
            authorization_code: value.authorization_code.map(Into::into),
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::OAuthFlow> for v3_1::OAuthFlow {
    fn from(value: v3_0::OAuthFlow) -> Self {
        Self {
            authorization_url: value.authorization_url,
            token_url: value.token_url,
            refresh_url: value.refresh_url,
            available_scopes: value.available_scopes,
            extensions: value.extensions,
        }
    }
}

impl From<v3_0::Components> for v3_1::Components {
    fn from(value: v3_0::Components) -> Self {
        Self {
            schemas: map_plain_map(value.schemas),
            servers: map_map(value.servers),
            channels: map_map(value.channels),
            operations: map_map(value.operations),
            messages: map_map(value.messages),
            security_schemes: map_map(value.security_schemes),
            server_variables: map_map(value.server_variables),
            parameters: map_map(value.parameters),
            correlation_ids: map_map(value.correlation_ids),
            replies: map_map(value.replies),
            reply_addresses: map_map(value.reply_addresses),
            external_docs: map_map(value.external_docs),
            tags: map_map(value.tags),
            operation_traits: map_map(value.operation_traits),
            message_traits: map_map(value.message_traits),
            server_bindings: map_map(value.server_bindings),
            channel_bindings: map_map(value.channel_bindings),
            operation_bindings: map_map(value.operation_bindings),
            message_bindings: map_map(value.message_bindings),
            extensions: value.extensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{v3_0, v3_1};
    use serde_json::json;

    #[test]
    fn a_minimal_document_converts() {
        let from: v3_0::Document = serde_json::from_value(json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" }
        }))
        .unwrap();
        let converted: v3_1::Document = from.into();
        assert_eq!(
            serde_json::to_value(&converted).unwrap(),
            json!({ "asyncapi": "3.1.0", "info": { "title": "T", "version": "1" } })
        );
    }

    #[test]
    fn every_enum_variant_maps_to_its_counterpart() {
        // The document fixture cannot carry every variant at once, so
        // walk them directly: each must land on the same spelling.
        for scheme_type in [
            v3_0::SecuritySchemeType::UserPassword,
            v3_0::SecuritySchemeType::ApiKey,
            v3_0::SecuritySchemeType::X509,
            v3_0::SecuritySchemeType::SymmetricEncryption,
            v3_0::SecuritySchemeType::AsymmetricEncryption,
            v3_0::SecuritySchemeType::HttpApiKey,
            v3_0::SecuritySchemeType::Http,
            v3_0::SecuritySchemeType::OAuth2,
            v3_0::SecuritySchemeType::OpenIdConnect,
            v3_0::SecuritySchemeType::Plain,
            v3_0::SecuritySchemeType::ScramSha256,
            v3_0::SecuritySchemeType::ScramSha512,
            v3_0::SecuritySchemeType::Gssapi,
        ] {
            let converted: v3_1::SecuritySchemeType = scheme_type.into();
            assert_eq!(converted.as_str(), scheme_type.as_str());
        }

        for location in [
            v3_0::ApiKeyLocation::User,
            v3_0::ApiKeyLocation::Password,
            v3_0::ApiKeyLocation::Query,
            v3_0::ApiKeyLocation::Header,
            v3_0::ApiKeyLocation::Cookie,
        ] {
            let converted: v3_1::ApiKeyLocation = location.into();
            assert_eq!(
                serde_json::to_value(converted).unwrap(),
                serde_json::to_value(location).unwrap()
            );
        }
    }

    #[test]
    fn single_form_items_convert_alongside_the_tuple_form() {
        let from: v3_0::Schema = serde_json::from_value(json!({
            "items": { "type": "string" },
            "additionalItems": true
        }))
        .unwrap();
        let converted: v3_1::Schema = from.into();
        assert!(matches!(converted.items, Some(v3_1::Items::Single(_))));
        assert_eq!(
            serde_json::to_value(&converted).unwrap(),
            json!({ "items": { "type": "string" }, "additionalItems": true })
        );
    }

    #[test]
    fn actions_and_boolean_schemas_survive() {
        let from: v3_0::Document = serde_json::from_value(json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1" },
            "channels": { "c": { "address": "c", "messages": { "m": { "payload": true } } } },
            "operations": {
                "s": { "action": "send", "channel": { "$ref": "#/channels/c" } },
                "r": { "action": "receive", "channel": { "$ref": "#/channels/c" } }
            }
        }))
        .unwrap();
        let converted: v3_1::Document = from.into();

        let send = converted.operations["s"].item().unwrap();
        assert_eq!(send.action, v3_1::OperationAction::Send);
        let receive = converted.operations["r"].item().unwrap();
        assert_eq!(receive.action, v3_1::OperationAction::Receive);

        let message = converted.channels["c"].item().unwrap().messages["m"]
            .item()
            .unwrap();
        assert!(matches!(
            message.payload.as_ref(),
            Some(v3_1::SchemaOrMultiFormat::Bool(true))
        ));
    }
}
