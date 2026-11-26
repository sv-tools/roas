//! Explicit typed pass that strips unknown (non-`x-`) keys from non-schema
//! OpenAPI objects before version conversions serialize the source document.
//!
//! Replacing the thread-local serializer flag keeps `Serialize` pure and
//! exhaustive struct destructuring guarantees that newly added fields are
//! walked at compile time.
//!
//! - Objects modelled with [`crate::common::extensions`] drop keys that are
//!   not `x-` Specification Extensions (and, for OAS 2.0 split-by-type types,
//!   keys outside that object kind's OAS 2.0 keyword set).
//! - Vendor-extension payload structs (`Logo`, `CodeSample`, `Badge`,
//!   `TagGroup`, `v2::spec::Server`) are preserved intact because any JSON
//!   shape is valid inside an `x-*` extension value.
//! - v3.0 and v3.1 `Schema` objects keep their `schema_keywords` map intact,
//!   while still recursing into their nested OpenAPI objects (`discriminator`,
//!   `xml`, `external_docs`) and subschemas.

#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
pub(crate) trait StripUnknownFields {
    fn strip_unknown_fields(&mut self);
}

#[cfg(all(feature = "v2", feature = "v3_0"))]
mod v2_impl {
    use super::StripUnknownFields;
    use crate::common::bool_or::BoolOr;
    use crate::common::extensions::{retain_extensions_only, retain_extensions_or};
    use crate::common::reference::RefOr;
    use crate::v2::external_documentation::ExternalDocumentation;
    use crate::v2::header::{
        ArrayHeader, BooleanHeader, HEADER_KEYWORDS, Header, IntegerHeader, NumberHeader,
        StringHeader,
    };
    use crate::v2::info::{Contact, Info, License};
    use crate::v2::items::{
        ArrayItem, BooleanItem, ITEMS_KEYWORDS, IntegerItem, Items, NumberItem, StringItem,
    };
    use crate::v2::operation::Operation;
    use crate::v2::parameter::{
        ArrayParameter, BooleanParameter, FileParameter, InBody, InFormData, InHeader, InPath,
        InQuery, IntegerParameter, NON_BODY_PARAMETER_KEYWORDS, NumberParameter, Parameter,
        StringParameter,
    };
    use crate::v2::path_item::{PathItem, Paths};
    use crate::v2::response::{Response, Responses};
    use crate::v2::schema::{
        AllOfSchema, ArraySchema, BooleanSchema, IntegerSchema, NullSchema, NumberSchema,
        ObjectSchema, SCHEMA_KEYWORDS, Schema, StringSchema,
    };
    use crate::v2::security_scheme::{
        ApiKeySecurityScheme, BasicSecurityScheme, OAuth2SecurityScheme, SecurityScheme,
    };
    use crate::v2::spec::Spec;
    use crate::v2::tag::Tag;
    use crate::v2::xml::XML;

    impl StripUnknownFields for Spec {
        fn strip_unknown_fields(&mut self) {
            let Spec {
                swagger: _,
                info,
                host: _,
                base_path: _,
                schemes: _,
                consumes: _,
                produces: _,
                paths,
                definitions,
                parameters,
                responses,
                security_definitions,
                security: _,
                tags,
                external_docs,
                x_servers: _,
                x_tag_groups: _,
                extensions,
            } = self;
            retain_extensions_only(extensions);
            strip_info(info);
            strip_paths(paths);
            if let Some(defs) = definitions {
                for schema in defs.values_mut() {
                    strip_schema(schema);
                }
            }
            if let Some(params) = parameters {
                for param in params.values_mut() {
                    strip_parameter(param);
                }
            }
            if let Some(resps) = responses {
                for resp in resps.values_mut() {
                    strip_response(resp);
                }
            }
            if let Some(schemes) = security_definitions {
                for scheme in schemes.values_mut() {
                    strip_security_scheme(scheme);
                }
            }
            if let Some(tags) = tags {
                for tag in tags {
                    strip_tag(tag);
                }
            }
            if let Some(docs) = external_docs {
                strip_external_docs(docs);
            }
        }
    }

    fn strip_ref_or<T>(item: &mut RefOr<T>, f: impl FnOnce(&mut T)) {
        if let RefOr::Item(inner) = item {
            f(inner);
        }
    }

    fn strip_info(info: &mut Info) {
        let Info {
            title: _,
            description: _,
            terms_of_service: _,
            contact,
            license,
            version: _,
            x_logo: _,
            extensions,
        } = info;
        retain_extensions_only(extensions);
        if let Some(c) = contact {
            let Contact {
                name: _,
                url: _,
                email: _,
                extensions,
            } = c;
            retain_extensions_only(extensions);
        }
        if let Some(l) = license {
            let License {
                name: _,
                url: _,
                extensions,
            } = l;
            retain_extensions_only(extensions);
        }
    }

    fn strip_external_docs(docs: &mut ExternalDocumentation) {
        let ExternalDocumentation {
            url: _,
            description: _,
            extensions,
        } = docs;
        retain_extensions_only(extensions);
    }

    fn strip_xml(xml: &mut XML) {
        let XML {
            name: _,
            namespace: _,
            prefix: _,
            attribute: _,
            wrapped: _,
            extensions,
        } = xml;
        retain_extensions_only(extensions);
    }

    fn strip_tag(tag: &mut Tag) {
        let Tag {
            name: _,
            description: _,
            external_docs,
            x_display_name: _,
            extensions,
        } = tag;
        retain_extensions_only(extensions);
        if let Some(docs) = external_docs {
            strip_external_docs(docs);
        }
    }

    fn strip_paths(paths: &mut Paths) {
        let Paths { paths, extensions } = paths;
        retain_extensions_only(extensions);
        for item in paths.values_mut() {
            strip_path_item(item);
        }
    }

    fn strip_path_item(item: &mut PathItem) {
        let PathItem {
            reference: _,
            operations,
            parameters,
            extensions,
        } = item;
        retain_extensions_only(extensions);
        if let Some(ops) = operations {
            for op in ops.values_mut() {
                strip_operation(op);
            }
        }
        if let Some(params) = parameters {
            for param in params {
                strip_ref_or(param, strip_parameter);
            }
        }
    }

    fn strip_operation(op: &mut Operation) {
        let Operation {
            tags: _,
            summary: _,
            description: _,
            external_docs,
            operation_id: _,
            consumes: _,
            produces: _,
            parameters,
            responses,
            schemes: _,
            deprecated: _,
            security: _,
            x_code_samples: _,
            extensions,
        } = op;
        retain_extensions_only(extensions);
        if let Some(docs) = external_docs {
            strip_external_docs(docs);
        }
        if let Some(params) = parameters {
            for param in params {
                strip_ref_or(param, strip_parameter);
            }
        }
        strip_responses(responses);
    }

    fn strip_responses(responses: &mut Responses) {
        let Responses {
            default,
            responses,
            extensions,
        } = responses;
        retain_extensions_only(extensions);
        if let Some(def) = default {
            strip_ref_or(def, strip_response);
        }
        if let Some(map) = responses {
            for resp in map.values_mut() {
                strip_ref_or(resp, strip_response);
            }
        }
    }

    fn strip_response(resp: &mut Response) {
        let Response {
            description: _,
            schema,
            headers,
            examples: _,
            x_summary: _,
            extensions,
        } = resp;
        retain_extensions_only(extensions);
        if let Some(s) = schema {
            strip_ref_or(s, strip_schema);
        }
        if let Some(headers) = headers {
            for header in headers.values_mut() {
                strip_header(header);
            }
        }
    }

    fn strip_header(header: &mut Header) {
        match header {
            Header::String(StringHeader {
                description: _,
                format: _,
                default: _,
                enum_values: _,
                max_length: _,
                min_length: _,
                pattern: _,
                extensions,
            }) => retain_extensions_or(extensions, HEADER_KEYWORDS),
            Header::Integer(IntegerHeader {
                description: _,
                format: _,
                default: _,
                enum_values: _,
                minimum: _,
                exclusive_minimum: _,
                maximum: _,
                exclusive_maximum: _,
                multiple_of: _,
                extensions,
            }) => retain_extensions_or(extensions, HEADER_KEYWORDS),
            Header::Number(NumberHeader {
                description: _,
                format: _,
                default: _,
                enum_values: _,
                minimum: _,
                exclusive_minimum: _,
                maximum: _,
                exclusive_maximum: _,
                multiple_of: _,
                extensions,
            }) => retain_extensions_or(extensions, HEADER_KEYWORDS),
            Header::Boolean(BooleanHeader {
                description: _,
                default: _,
                extensions,
            }) => retain_extensions_or(extensions, HEADER_KEYWORDS),
            Header::Array(ArrayHeader {
                description: _,
                items,
                default: _,
                collection_format: _,
                max_items: _,
                min_items: _,
                unique_items: _,
                extensions,
            }) => {
                retain_extensions_or(extensions, HEADER_KEYWORDS);
                strip_items(items);
            }
        }
    }

    fn strip_items(items: &mut Items) {
        match items {
            Items::String(s) => {
                let StringItem {
                    format: _,
                    default: _,
                    enum_values: _,
                    max_length: _,
                    min_length: _,
                    pattern: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_or(extensions, ITEMS_KEYWORDS);
            }
            Items::Integer(i) => {
                let IntegerItem {
                    format: _,
                    default: _,
                    enum_values: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    extensions,
                } = i.as_mut();
                retain_extensions_or(extensions, ITEMS_KEYWORDS);
            }
            Items::Number(n) => {
                let NumberItem {
                    format: _,
                    default: _,
                    enum_values: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    extensions,
                } = n.as_mut();
                retain_extensions_or(extensions, ITEMS_KEYWORDS);
            }
            Items::Boolean(b) => {
                let BooleanItem {
                    default: _,
                    extensions,
                } = b.as_mut();
                retain_extensions_or(extensions, ITEMS_KEYWORDS);
            }
            Items::Array(a) => {
                let ArrayItem {
                    items,
                    default: _,
                    collection_format: _,
                    max_items: _,
                    min_items: _,
                    unique_items: _,
                    extensions,
                } = a.as_mut();
                retain_extensions_or(extensions, ITEMS_KEYWORDS);
                strip_items(items);
            }
        }
    }

    fn strip_parameter(param: &mut Parameter) {
        match param {
            Parameter::Body(body) => {
                let InBody {
                    name: _,
                    description: _,
                    required: _,
                    schema,
                    x_examples: _,
                    extensions,
                } = body.as_mut();
                retain_extensions_only(extensions);
                strip_ref_or(schema, strip_schema);
            }
            Parameter::Header(h) => match h.as_mut() {
                InHeader::String(p) => strip_string_param(p),
                InHeader::Integer(p) => strip_integer_param(p),
                InHeader::Number(p) => strip_number_param(p),
                InHeader::Boolean(p) => strip_boolean_param(p),
                InHeader::Array(p) => strip_array_param(p),
            },
            Parameter::Query(q) => match q.as_mut() {
                InQuery::String(p) => strip_string_param(p),
                InQuery::Integer(p) => strip_integer_param(p),
                InQuery::Number(p) => strip_number_param(p),
                InQuery::Boolean(p) => strip_boolean_param(p),
                InQuery::Array(p) => strip_array_param(p),
            },
            Parameter::Path(p) => match p.as_mut() {
                InPath::String(p) => strip_string_param(p),
                InPath::Integer(p) => strip_integer_param(p),
                InPath::Number(p) => strip_number_param(p),
                InPath::Boolean(p) => strip_boolean_param(p),
                InPath::Array(p) => strip_array_param(p),
            },
            Parameter::FormData(f) => match f.as_mut() {
                InFormData::String(p) => strip_string_param(p),
                InFormData::Integer(p) => strip_integer_param(p),
                InFormData::Number(p) => strip_number_param(p),
                InFormData::Boolean(p) => strip_boolean_param(p),
                InFormData::Array(p) => strip_array_param(p),
                InFormData::File(FileParameter {
                    name: _,
                    description: _,
                    required: _,
                    default: _,
                    x_examples: _,
                    extensions,
                }) => retain_extensions_or(extensions, NON_BODY_PARAMETER_KEYWORDS),
            },
        }
    }

    fn strip_string_param(p: &mut StringParameter) {
        let StringParameter {
            name: _,
            description: _,
            required: _,
            allow_empty_value: _,
            format: _,
            default: _,
            enum_values: _,
            max_length: _,
            min_length: _,
            pattern: _,
            x_examples: _,
            extensions,
        } = p;
        retain_extensions_or(extensions, NON_BODY_PARAMETER_KEYWORDS);
    }

    fn strip_integer_param(p: &mut IntegerParameter) {
        let IntegerParameter {
            name: _,
            description: _,
            required: _,
            allow_empty_value: _,
            format: _,
            default: _,
            enum_values: _,
            minimum: _,
            exclusive_minimum: _,
            maximum: _,
            exclusive_maximum: _,
            multiple_of: _,
            x_examples: _,
            extensions,
        } = p;
        retain_extensions_or(extensions, NON_BODY_PARAMETER_KEYWORDS);
    }

    fn strip_number_param(p: &mut NumberParameter) {
        let NumberParameter {
            name: _,
            description: _,
            allow_empty_value: _,
            required: _,
            format: _,
            default: _,
            enum_values: _,
            minimum: _,
            exclusive_minimum: _,
            maximum: _,
            exclusive_maximum: _,
            multiple_of: _,
            x_examples: _,
            extensions,
        } = p;
        retain_extensions_or(extensions, NON_BODY_PARAMETER_KEYWORDS);
    }

    fn strip_boolean_param(p: &mut BooleanParameter) {
        let BooleanParameter {
            name: _,
            description: _,
            allow_empty_value: _,
            required: _,
            default: _,
            x_examples: _,
            extensions,
        } = p;
        retain_extensions_or(extensions, NON_BODY_PARAMETER_KEYWORDS);
    }

    fn strip_array_param(p: &mut ArrayParameter) {
        let ArrayParameter {
            name: _,
            description: _,
            allow_empty_value: _,
            required: _,
            items,
            default: _,
            collection_format: _,
            max_items: _,
            min_items: _,
            unique_items: _,
            x_examples: _,
            extensions,
        } = p;
        retain_extensions_or(extensions, NON_BODY_PARAMETER_KEYWORDS);
        strip_items(items);
    }

    fn strip_schema(schema: &mut Schema) {
        match schema {
            Schema::String(s) => {
                let StringSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    max_length: _,
                    min_length: _,
                    pattern: _,
                    read_only: _,
                    xml,
                    external_docs,
                    example: _,
                    x_nullable: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                if let Some(x) = xml {
                    strip_xml(x);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Integer(i) => {
                let IntegerSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    read_only: _,
                    xml,
                    external_docs,
                    example: _,
                    x_nullable: _,
                    extensions,
                } = i.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                if let Some(x) = xml {
                    strip_xml(x);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Number(n) => {
                let NumberSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    read_only: _,
                    xml,
                    external_docs,
                    example: _,
                    x_nullable: _,
                    extensions,
                } = n.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                if let Some(x) = xml {
                    strip_xml(x);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Boolean(b) => {
                let BooleanSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    default: _,
                    read_only: _,
                    xml,
                    external_docs,
                    example: _,
                    x_nullable: _,
                    extensions,
                } = b.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                if let Some(x) = xml {
                    strip_xml(x);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Array(a) => {
                let ArraySchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    items,
                    default: _,
                    max_items: _,
                    min_items: _,
                    unique_items: _,
                    read_only: _,
                    xml,
                    external_docs,
                    example: _,
                    x_nullable: _,
                    extensions,
                } = a.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                if let Some(it) = items {
                    strip_ref_or(it, strip_schema);
                }
                if let Some(x) = xml {
                    strip_xml(x);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Null(n) => {
                let NullSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    read_only: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions,
                } = n.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                if let Some(x) = xml {
                    strip_xml(x);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Object(o) => strip_object_schema(o),
            Schema::AllOf(a) => {
                let AllOfSchema {
                    all_of,
                    title: _,
                    description: _,
                    external_docs,
                    discriminator: _,
                    example: _,
                    x_nullable: _,
                    extensions,
                } = a.as_mut();
                retain_extensions_or(extensions, SCHEMA_KEYWORDS);
                for sub in all_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
        }
    }

    fn strip_object_schema(o: &mut ObjectSchema) {
        let ObjectSchema {
            schema_type: _,
            title: _,
            description: _,
            properties,
            default: _,
            max_properties: _,
            min_properties: _,
            additional_properties,
            required: _,
            discriminator: _,
            read_only: _,
            xml,
            external_docs,
            example: _,
            x_nullable: _,
            all_of,
            extensions,
        } = o;
        retain_extensions_or(extensions, SCHEMA_KEYWORDS);
        if let Some(props) = properties {
            for prop in props.values_mut() {
                strip_ref_or(prop, strip_schema);
            }
        }
        if let Some(BoolOr::Item(ap)) = additional_properties {
            strip_ref_or(ap, strip_schema);
        }
        if let Some(x) = xml {
            strip_xml(x);
        }
        if let Some(d) = external_docs {
            strip_external_docs(d);
        }
        if let Some(subs) = all_of {
            for sub in subs {
                strip_ref_or(sub, strip_object_schema);
            }
        }
    }

    fn strip_security_scheme(scheme: &mut SecurityScheme) {
        match scheme {
            SecurityScheme::Basic(BasicSecurityScheme {
                description: _,
                extensions,
            }) => retain_extensions_only(extensions),
            SecurityScheme::ApiKey(ApiKeySecurityScheme {
                name: _,
                location: _,
                description: _,
                extensions,
            }) => retain_extensions_only(extensions),
            SecurityScheme::OAuth2(OAuth2SecurityScheme {
                flow: _,
                authorization_url: _,
                token_url: _,
                scopes,
                description: _,
                extensions,
            }) => {
                retain_extensions_only(extensions);
                retain_extensions_only(&mut scopes.extensions);
            }
        }
    }
}

#[cfg(all(feature = "v3_0", feature = "v3_1"))]
mod v3_0_impl {
    use super::StripUnknownFields;
    use crate::common::bool_or::BoolOr;
    use crate::common::extensions::retain_extensions_only;
    use crate::common::reference::RefOr;
    use crate::v3_0::callback::Callback;
    use crate::v3_0::components::Components;
    use crate::v3_0::discriminator::Discriminator;
    use crate::v3_0::example::Example;
    use crate::v3_0::external_documentation::ExternalDocumentation;
    use crate::v3_0::header::Header;
    use crate::v3_0::info::{Contact, Info, License};
    use crate::v3_0::link::Link;
    use crate::v3_0::media_type::{Encoding, MediaType};
    use crate::v3_0::operation::Operation;
    use crate::v3_0::parameter::{InCookie, InHeader, InPath, InQuery, Parameter};
    use crate::v3_0::path_item::{PathItem, Paths};
    use crate::v3_0::request_body::RequestBody;
    use crate::v3_0::response::{Response, Responses};
    use crate::v3_0::schema::{
        AllOfSchema, AnyOfSchema, ArraySchema, BooleanSchema, IntegerSchema, NotSchema, NullSchema,
        NumberSchema, ObjectSchema, OneOfSchema, Schema, SingleSchema, StringSchema,
    };
    use crate::v3_0::security_scheme::{
        ApiKeySecurityScheme, AuthorizationCodeOAuth2Flow, ClientCredentialsOAuth2Flow,
        HttpSecurityScheme, ImplicitOAuth2Flow, OAuth2Flows, OAuth2SecurityScheme,
        OpenIdConnectSecurityScheme, PasswordOAuth2Flow, SecurityScheme,
    };
    use crate::v3_0::server::{Server, ServerVariable};
    use crate::v3_0::spec::Spec;
    use crate::v3_0::tag::Tag;
    use crate::v3_0::xml::XML;

    impl StripUnknownFields for Spec {
        fn strip_unknown_fields(&mut self) {
            let Spec {
                openapi: _,
                info,
                servers,
                paths,
                components,
                security: _,
                tags,
                external_docs,
                x_tag_groups: _,
                extensions,
            } = self;
            retain_extensions_only(extensions);
            strip_info(info);
            if let Some(servers) = servers {
                for server in servers {
                    strip_server(server);
                }
            }
            strip_paths(paths);
            if let Some(comp) = components {
                strip_components(comp);
            }
            if let Some(tags) = tags {
                for tag in tags {
                    strip_tag(tag);
                }
            }
            if let Some(docs) = external_docs {
                strip_external_docs(docs);
            }
        }
    }

    fn strip_ref_or<T>(item: &mut RefOr<T>, f: impl FnOnce(&mut T)) {
        if let RefOr::Item(inner) = item {
            f(inner);
        }
    }

    fn strip_info(info: &mut Info) {
        let Info {
            title: _,
            description: _,
            terms_of_service: _,
            contact,
            license,
            version: _,
            x_logo: _,
            extensions,
        } = info;
        retain_extensions_only(extensions);
        if let Some(c) = contact {
            let Contact {
                name: _,
                url: _,
                email: _,
                extensions,
            } = c;
            retain_extensions_only(extensions);
        }
        if let Some(l) = license {
            let License {
                name: _,
                url: _,
                extensions,
            } = l;
            retain_extensions_only(extensions);
        }
    }

    fn strip_server(server: &mut Server) {
        let Server {
            url: _,
            description: _,
            variables,
            extensions,
        } = server;
        retain_extensions_only(extensions);
        if let Some(vars) = variables {
            for var in vars.values_mut() {
                let ServerVariable {
                    enum_values: _,
                    default: _,
                    description: _,
                    extensions,
                } = var;
                retain_extensions_only(extensions);
            }
        }
    }

    fn strip_external_docs(docs: &mut ExternalDocumentation) {
        let ExternalDocumentation {
            url: _,
            description: _,
            extensions,
        } = docs;
        retain_extensions_only(extensions);
    }

    fn strip_xml(xml: &mut XML) {
        let XML {
            name: _,
            namespace: _,
            prefix: _,
            attribute: _,
            wrapped: _,
            extensions,
        } = xml;
        retain_extensions_only(extensions);
    }

    fn strip_discriminator(disc: &mut Discriminator) {
        let Discriminator {
            property_name: _,
            mapping: _,
            extensions,
        } = disc;
        retain_extensions_only(extensions);
    }

    fn strip_tag(tag: &mut Tag) {
        let Tag {
            name: _,
            description: _,
            external_docs,
            x_display_name: _,
            extensions,
        } = tag;
        retain_extensions_only(extensions);
        if let Some(docs) = external_docs {
            strip_external_docs(docs);
        }
    }

    fn strip_example(example: &mut Example) {
        let Example {
            summary: _,
            description: _,
            value: _,
            external_value: _,
            extensions,
        } = example;
        retain_extensions_only(extensions);
    }

    fn strip_link(link: &mut Link) {
        let Link {
            operation_ref: _,
            operation_id: _,
            parameters: _,
            request_body: _,
            description: _,
            server,
            extensions,
        } = link;
        retain_extensions_only(extensions);
        if let Some(s) = server {
            strip_server(s);
        }
    }

    fn strip_components(comp: &mut Components) {
        let Components {
            schemas,
            responses,
            parameters,
            examples,
            request_bodies,
            headers,
            security_schemes,
            links,
            callbacks,
            extensions,
        } = comp;
        retain_extensions_only(extensions);
        if let Some(map) = schemas {
            for s in map.values_mut() {
                strip_ref_or(s, strip_schema);
            }
        }
        if let Some(map) = responses {
            for r in map.values_mut() {
                strip_ref_or(r, strip_response);
            }
        }
        if let Some(map) = parameters {
            for p in map.values_mut() {
                strip_ref_or(p, strip_parameter);
            }
        }
        if let Some(map) = examples {
            for e in map.values_mut() {
                strip_ref_or(e, strip_example);
            }
        }
        if let Some(map) = request_bodies {
            for rb in map.values_mut() {
                strip_ref_or(rb, strip_request_body);
            }
        }
        if let Some(map) = headers {
            for h in map.values_mut() {
                strip_ref_or(h, strip_header);
            }
        }
        if let Some(map) = security_schemes {
            for ss in map.values_mut() {
                strip_ref_or(ss, strip_security_scheme);
            }
        }
        if let Some(map) = links {
            for l in map.values_mut() {
                strip_ref_or(l, strip_link);
            }
        }
        if let Some(map) = callbacks {
            for cb in map.values_mut() {
                strip_ref_or(cb, strip_callback);
            }
        }
    }

    fn strip_paths(paths: &mut Paths) {
        let Paths { paths, extensions } = paths;
        retain_extensions_only(extensions);
        for item in paths.values_mut() {
            strip_path_item(item);
        }
    }

    fn strip_callback(cb: &mut Callback) {
        let Callback { paths, extensions } = cb;
        retain_extensions_only(extensions);
        for item in paths.values_mut() {
            strip_path_item(item);
        }
    }

    fn strip_path_item(item: &mut PathItem) {
        let PathItem {
            reference: _,
            summary: _,
            description: _,
            operations,
            servers,
            parameters,
            extensions,
        } = item;
        retain_extensions_only(extensions);
        if let Some(ops) = operations {
            for op in ops.values_mut() {
                strip_operation(op);
            }
        }
        if let Some(servers) = servers {
            for s in servers {
                strip_server(s);
            }
        }
        if let Some(params) = parameters {
            for p in params {
                strip_ref_or(p, strip_parameter);
            }
        }
    }

    fn strip_operation(op: &mut Operation) {
        let Operation {
            tags: _,
            summary: _,
            description: _,
            external_docs,
            operation_id: _,
            parameters,
            request_body,
            responses,
            callbacks,
            deprecated: _,
            security: _,
            servers,
            x_code_samples: _,
            x_badges: _,
            x_examples: _,
            extensions,
        } = op;
        retain_extensions_only(extensions);
        if let Some(docs) = external_docs {
            strip_external_docs(docs);
        }
        if let Some(params) = parameters {
            for p in params {
                strip_ref_or(p, strip_parameter);
            }
        }
        if let Some(rb) = request_body {
            strip_ref_or(rb, strip_request_body);
        }
        strip_responses(responses);
        if let Some(cbs) = callbacks {
            for cb in cbs.values_mut() {
                strip_ref_or(cb, strip_callback);
            }
        }
        if let Some(servers) = servers {
            for s in servers {
                strip_server(s);
            }
        }
    }

    fn strip_parameter(param: &mut Parameter) {
        match param {
            Parameter::Path(p) => {
                let InPath {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    style: _,
                    explode: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
            Parameter::Query(p) => {
                let InQuery {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    allow_empty_value: _,
                    style: _,
                    explode: _,
                    allow_reserved: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
            Parameter::Header(p) => {
                let InHeader {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    style: _,
                    explode: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
            Parameter::Cookie(p) => {
                let InCookie {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    style: _,
                    explode: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
        }
    }

    fn strip_param_payload(
        schema: &mut Option<RefOr<Schema>>,
        examples: &mut Option<std::collections::BTreeMap<String, RefOr<Example>>>,
        content: &mut Option<std::collections::BTreeMap<String, MediaType>>,
    ) {
        if let Some(s) = schema {
            strip_ref_or(s, strip_schema);
        }
        if let Some(map) = examples {
            for e in map.values_mut() {
                strip_ref_or(e, strip_example);
            }
        }
        if let Some(map) = content {
            for mt in map.values_mut() {
                strip_media_type(mt);
            }
        }
    }

    fn strip_header(header: &mut Header) {
        let Header {
            description: _,
            required: _,
            deprecated: _,
            allow_empty_value: _,
            style: _,
            explode: _,
            allow_reserved: _,
            schema,
            example: _,
            examples,
            content,
            extensions,
        } = header;
        retain_extensions_only(extensions);
        strip_param_payload(schema, examples, content);
    }

    fn strip_request_body(rb: &mut RequestBody) {
        let RequestBody {
            description: _,
            content,
            required: _,
            extensions,
        } = rb;
        retain_extensions_only(extensions);
        for mt in content.values_mut() {
            strip_media_type(mt);
        }
    }

    fn strip_media_type(mt: &mut MediaType) {
        let MediaType {
            schema,
            example: _,
            examples,
            encoding,
            extensions,
        } = mt;
        retain_extensions_only(extensions);
        if let Some(s) = schema {
            strip_ref_or(s, strip_schema);
        }
        if let Some(map) = examples {
            for e in map.values_mut() {
                strip_ref_or(e, strip_example);
            }
        }
        if let Some(map) = encoding {
            for enc in map.values_mut() {
                strip_encoding(enc);
            }
        }
    }

    fn strip_encoding(enc: &mut Encoding) {
        let Encoding {
            content_type: _,
            headers,
            style: _,
            explode: _,
            allow_reserved: _,
            extensions,
        } = enc;
        retain_extensions_only(extensions);
        if let Some(map) = headers {
            for h in map.values_mut() {
                strip_ref_or(h, strip_header);
            }
        }
    }

    fn strip_responses(responses: &mut Responses) {
        let Responses {
            default,
            responses,
            extensions,
        } = responses;
        retain_extensions_only(extensions);
        if let Some(def) = default {
            strip_ref_or(def, strip_response);
        }
        if let Some(map) = responses {
            for r in map.values_mut() {
                strip_ref_or(r, strip_response);
            }
        }
    }

    fn strip_response(resp: &mut Response) {
        let Response {
            description: _,
            headers,
            content,
            links,
            x_summary: _,
            extensions,
        } = resp;
        retain_extensions_only(extensions);
        if let Some(map) = headers {
            for h in map.values_mut() {
                strip_ref_or(h, strip_header);
            }
        }
        if let Some(map) = content {
            for mt in map.values_mut() {
                strip_media_type(mt);
            }
        }
        if let Some(map) = links {
            for l in map.values_mut() {
                strip_ref_or(l, strip_link);
            }
        }
    }

    fn strip_security_scheme(scheme: &mut SecurityScheme) {
        match scheme {
            SecurityScheme::HTTP(s) => {
                let HttpSecurityScheme {
                    scheme: _,
                    bearer_format: _,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
            SecurityScheme::ApiKey(s) => {
                let ApiKeySecurityScheme {
                    name: _,
                    location: _,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
            SecurityScheme::OAuth2(s) => {
                let OAuth2SecurityScheme {
                    flows,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
                let OAuth2Flows {
                    implicit,
                    password,
                    client_credentials,
                    authorization_code,
                    extensions,
                } = flows;
                retain_extensions_only(extensions);
                if let Some(ImplicitOAuth2Flow {
                    authorization_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = implicit
                {
                    retain_extensions_only(extensions);
                }
                if let Some(PasswordOAuth2Flow {
                    token_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = password
                {
                    retain_extensions_only(extensions);
                }
                if let Some(ClientCredentialsOAuth2Flow {
                    token_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = client_credentials
                {
                    retain_extensions_only(extensions);
                }
                if let Some(AuthorizationCodeOAuth2Flow {
                    authorization_url: _,
                    token_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = authorization_code
                {
                    retain_extensions_only(extensions);
                }
            }
            SecurityScheme::OpenIdConnect(s) => {
                let OpenIdConnectSecurityScheme {
                    open_id_connect_url: _,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
        }
    }

    /// Recurse into `Schema` to strip unknown keys from nested non-schema
    /// OpenAPI objects (`discriminator`, `xml`, `external_docs`) while leaving
    /// `schema.extensions` (`schema_keywords`) untouched.
    fn strip_schema(schema: &mut Schema) {
        match schema {
            Schema::AllOf(s) => {
                let AllOfSchema {
                    all_of,
                    discriminator,
                    extensions: _,
                } = s.as_mut();
                for sub in all_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
            }
            Schema::AnyOf(s) => {
                let AnyOfSchema {
                    any_of,
                    discriminator,
                    extensions: _,
                } = s.as_mut();
                for sub in any_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
            }
            Schema::OneOf(s) => {
                let OneOfSchema {
                    one_of,
                    discriminator,
                    extensions: _,
                } = s.as_mut();
                for sub in one_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
            }
            Schema::Not(s) => {
                let NotSchema { not, extensions: _ } = s.as_mut();
                strip_ref_or(not, strip_schema);
            }
            Schema::Single(s) => match s.as_mut() {
                SingleSchema::String(StringSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    x_enum_descriptions: _,
                    x_enum_varnames: _,
                    x_enum_names: _,
                    max_length: _,
                    min_length: _,
                    pattern: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Integer(IntegerSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    x_enum_descriptions: _,
                    x_enum_varnames: _,
                    x_enum_names: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Number(NumberSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    x_enum_descriptions: _,
                    x_enum_varnames: _,
                    x_enum_names: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Boolean(BooleanSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    default: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Array(ArraySchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    items,
                    default: _,
                    max_items: _,
                    min_items: _,
                    unique_items: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(it) = items {
                        strip_ref_or(it, strip_schema);
                    }
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Null(NullSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Object(ObjectSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    properties,
                    default: _,
                    max_properties: _,
                    min_properties: _,
                    additional_properties,
                    x_additional_properties_name: _,
                    discriminator,
                    required: _,
                    read_only: _,
                    write_only: _,
                    nullable: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    extensions: _,
                }) => {
                    if let Some(props) = properties {
                        for p in props.values_mut() {
                            strip_ref_or(p, strip_schema);
                        }
                    }
                    if let Some(BoolOr::Item(ap)) = additional_properties {
                        strip_ref_or(ap, strip_schema);
                    }
                    if let Some(d) = discriminator {
                        strip_discriminator(d);
                    }
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
            },
        }
    }
}

#[cfg(all(feature = "v3_1", feature = "v3_2"))]
mod v3_1_impl {
    use super::StripUnknownFields;
    use crate::common::bool_or::BoolOr;
    use crate::common::extensions::retain_extensions_only;
    use crate::common::reference::RefOr;
    use crate::v3_1::callback::Callback;
    use crate::v3_1::components::Components;
    use crate::v3_1::discriminator::Discriminator;
    use crate::v3_1::example::Example;
    use crate::v3_1::external_documentation::ExternalDocumentation;
    use crate::v3_1::header::Header;
    use crate::v3_1::info::{Contact, Info, License};
    use crate::v3_1::link::Link;
    use crate::v3_1::media_type::{Encoding, MediaType};
    use crate::v3_1::operation::Operation;
    use crate::v3_1::parameter::{InCookie, InHeader, InPath, InQuery, Parameter};
    use crate::v3_1::path_item::{PathItem, Paths};
    use crate::v3_1::request_body::RequestBody;
    use crate::v3_1::response::{Response, Responses};
    use crate::v3_1::schema::{
        AllOfSchema, AnyOfSchema, ArraySchema, BooleanSchema, IntegerSchema, MultiSchema,
        NotSchema, NullSchema, NumberSchema, ObjectSchema, OneOfSchema, Schema, SchemaRef,
        SingleSchema, StringSchema,
    };
    use crate::v3_1::security_scheme::{
        ApiKeySecurityScheme, AuthorizationCodeOAuth2Flow, ClientCredentialsOAuth2Flow,
        HttpSecurityScheme, ImplicitOAuth2Flow, MutualTLSSecurityScheme, OAuth2Flows,
        OAuth2SecurityScheme, OpenIdConnectSecurityScheme, PasswordOAuth2Flow, SecurityScheme,
    };
    use crate::v3_1::server::{Server, ServerVariable};
    use crate::v3_1::spec::Spec;
    use crate::v3_1::tag::Tag;
    use crate::v3_1::xml::XML;

    impl StripUnknownFields for Spec {
        fn strip_unknown_fields(&mut self) {
            let Spec {
                openapi: _,
                info,
                json_schema_dialect: _,
                servers,
                paths,
                webhooks,
                components,
                security: _,
                tags,
                external_docs,
                x_tag_groups: _,
                extensions,
            } = self;
            retain_extensions_only(extensions);
            strip_info(info);
            if let Some(servers) = servers {
                for server in servers {
                    strip_server(server);
                }
            }
            if let Some(paths) = paths {
                strip_paths(paths);
            }
            if let Some(webhooks) = webhooks {
                strip_paths(webhooks);
            }
            if let Some(comp) = components {
                strip_components(comp);
            }
            if let Some(tags) = tags {
                for tag in tags {
                    strip_tag(tag);
                }
            }
            if let Some(docs) = external_docs {
                strip_external_docs(docs);
            }
        }
    }

    fn strip_ref_or<T, R>(item: &mut RefOr<T, R>, f: impl FnOnce(&mut T)) {
        if let RefOr::Item(inner) = item {
            f(inner);
        }
    }

    fn strip_info(info: &mut Info) {
        let Info {
            title: _,
            summary: _,
            description: _,
            terms_of_service: _,
            contact,
            license,
            version: _,
            x_logo: _,
            extensions,
        } = info;
        retain_extensions_only(extensions);
        if let Some(c) = contact {
            let Contact {
                name: _,
                url: _,
                email: _,
                extensions,
            } = c;
            retain_extensions_only(extensions);
        }
        if let Some(l) = license {
            let License {
                name: _,
                identifier: _,
                url: _,
                extensions,
            } = l;
            retain_extensions_only(extensions);
        }
    }

    fn strip_server(server: &mut Server) {
        let Server {
            url: _,
            description: _,
            variables,
            extensions,
        } = server;
        retain_extensions_only(extensions);
        if let Some(vars) = variables {
            for var in vars.values_mut() {
                let ServerVariable {
                    enum_values: _,
                    default: _,
                    description: _,
                    extensions,
                } = var;
                retain_extensions_only(extensions);
            }
        }
    }

    fn strip_external_docs(docs: &mut ExternalDocumentation) {
        let ExternalDocumentation {
            url: _,
            description: _,
            extensions,
        } = docs;
        retain_extensions_only(extensions);
    }

    fn strip_xml(xml: &mut XML) {
        let XML {
            name: _,
            namespace: _,
            prefix: _,
            attribute: _,
            wrapped: _,
            extensions,
        } = xml;
        retain_extensions_only(extensions);
    }

    fn strip_discriminator(disc: &mut Discriminator) {
        let Discriminator {
            property_name: _,
            mapping: _,
            extensions,
        } = disc;
        retain_extensions_only(extensions);
    }

    fn strip_tag(tag: &mut Tag) {
        let Tag {
            name: _,
            description: _,
            external_docs,
            x_display_name: _,
            extensions,
        } = tag;
        retain_extensions_only(extensions);
        if let Some(docs) = external_docs {
            strip_external_docs(docs);
        }
    }

    fn strip_example(example: &mut Example) {
        let Example {
            summary: _,
            description: _,
            value: _,
            external_value: _,
            extensions,
        } = example;
        retain_extensions_only(extensions);
    }

    fn strip_link(link: &mut Link) {
        let Link {
            operation_ref: _,
            operation_id: _,
            parameters: _,
            request_body: _,
            description: _,
            server,
            extensions,
        } = link;
        retain_extensions_only(extensions);
        if let Some(s) = server {
            strip_server(s);
        }
    }

    fn strip_components(comp: &mut Components) {
        let Components {
            schemas,
            responses,
            parameters,
            examples,
            request_bodies,
            headers,
            security_schemes,
            links,
            callbacks,
            path_items,
            extensions,
        } = comp;
        retain_extensions_only(extensions);
        if let Some(map) = schemas {
            for s in map.values_mut() {
                strip_ref_or(s, strip_schema);
            }
        }
        if let Some(map) = responses {
            for r in map.values_mut() {
                strip_ref_or(r, strip_response);
            }
        }
        if let Some(map) = parameters {
            for p in map.values_mut() {
                strip_ref_or(p, strip_parameter);
            }
        }
        if let Some(map) = examples {
            for e in map.values_mut() {
                strip_ref_or(e, strip_example);
            }
        }
        if let Some(map) = request_bodies {
            for rb in map.values_mut() {
                strip_ref_or(rb, strip_request_body);
            }
        }
        if let Some(map) = headers {
            for h in map.values_mut() {
                strip_ref_or(h, strip_header);
            }
        }
        if let Some(map) = security_schemes {
            for ss in map.values_mut() {
                strip_ref_or(ss, strip_security_scheme);
            }
        }
        if let Some(map) = links {
            for l in map.values_mut() {
                strip_ref_or(l, strip_link);
            }
        }
        if let Some(map) = callbacks {
            for cb in map.values_mut() {
                strip_ref_or(cb, strip_callback);
            }
        }
        if let Some(map) = path_items {
            for pi in map.values_mut() {
                strip_path_item(pi);
            }
        }
    }

    fn strip_paths(paths: &mut Paths) {
        let Paths { paths, extensions } = paths;
        retain_extensions_only(extensions);
        for item in paths.values_mut() {
            strip_path_item(item);
        }
    }

    fn strip_callback(cb: &mut Callback) {
        let Callback { paths, extensions } = cb;
        retain_extensions_only(extensions);
        for item in paths.values_mut() {
            strip_path_item(item);
        }
    }

    fn strip_path_item(item: &mut PathItem) {
        let PathItem {
            reference: _,
            summary: _,
            description: _,
            operations,
            servers,
            parameters,
            extensions,
        } = item;
        retain_extensions_only(extensions);
        if let Some(ops) = operations {
            for op in ops.values_mut() {
                strip_operation(op);
            }
        }
        if let Some(servers) = servers {
            for s in servers {
                strip_server(s);
            }
        }
        if let Some(params) = parameters {
            for p in params {
                strip_ref_or(p, strip_parameter);
            }
        }
    }

    fn strip_operation(op: &mut Operation) {
        let Operation {
            tags: _,
            summary: _,
            description: _,
            external_docs,
            operation_id: _,
            parameters,
            request_body,
            responses,
            callbacks,
            deprecated: _,
            security: _,
            servers,
            x_code_samples: _,
            x_tags: _,
            extensions,
        } = op;
        retain_extensions_only(extensions);
        if let Some(docs) = external_docs {
            strip_external_docs(docs);
        }
        if let Some(params) = parameters {
            for p in params {
                strip_ref_or(p, strip_parameter);
            }
        }
        if let Some(rb) = request_body {
            strip_ref_or(rb, strip_request_body);
        }
        if let Some(resps) = responses {
            strip_responses(resps);
        }
        if let Some(cbs) = callbacks {
            for cb in cbs.values_mut() {
                strip_ref_or(cb, strip_callback);
            }
        }
        if let Some(servers) = servers {
            for s in servers {
                strip_server(s);
            }
        }
    }

    fn strip_parameter(param: &mut Parameter) {
        match param {
            Parameter::Path(p) => {
                let InPath {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    style: _,
                    explode: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
            Parameter::Query(p) => {
                let InQuery {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    allow_empty_value: _,
                    style: _,
                    explode: _,
                    allow_reserved: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
            Parameter::Header(p) => {
                let InHeader {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    style: _,
                    explode: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
            Parameter::Cookie(p) => {
                let InCookie {
                    name: _,
                    description: _,
                    required: _,
                    deprecated: _,
                    style: _,
                    explode: _,
                    schema,
                    example: _,
                    examples,
                    content,
                    extensions,
                } = p.as_mut();
                retain_extensions_only(extensions);
                strip_param_payload(schema, examples, content);
            }
        }
    }

    fn strip_param_payload(
        schema: &mut Option<RefOr<Schema, SchemaRef>>,
        examples: &mut Option<std::collections::BTreeMap<String, RefOr<Example>>>,
        content: &mut Option<std::collections::BTreeMap<String, MediaType>>,
    ) {
        if let Some(s) = schema {
            strip_ref_or(s, strip_schema);
        }
        if let Some(map) = examples {
            for e in map.values_mut() {
                strip_ref_or(e, strip_example);
            }
        }
        if let Some(map) = content {
            for mt in map.values_mut() {
                strip_media_type(mt);
            }
        }
    }

    fn strip_header(header: &mut Header) {
        let Header {
            description: _,
            required: _,
            deprecated: _,
            style: _,
            explode: _,
            schema,
            example: _,
            examples,
            content,
            extensions,
        } = header;
        retain_extensions_only(extensions);
        strip_param_payload(schema, examples, content);
    }

    fn strip_request_body(rb: &mut RequestBody) {
        let RequestBody {
            description: _,
            content,
            required: _,
            extensions,
        } = rb;
        retain_extensions_only(extensions);
        for mt in content.values_mut() {
            strip_media_type(mt);
        }
    }

    fn strip_media_type(mt: &mut MediaType) {
        let MediaType {
            schema,
            example: _,
            examples,
            encoding,
            extensions,
        } = mt;
        retain_extensions_only(extensions);
        if let Some(s) = schema {
            strip_ref_or(s, strip_schema);
        }
        if let Some(map) = examples {
            for e in map.values_mut() {
                strip_ref_or(e, strip_example);
            }
        }
        if let Some(map) = encoding {
            for enc in map.values_mut() {
                strip_encoding(enc);
            }
        }
    }

    fn strip_encoding(enc: &mut Encoding) {
        let Encoding {
            content_type: _,
            headers,
            style: _,
            explode: _,
            allow_reserved: _,
            extensions,
        } = enc;
        retain_extensions_only(extensions);
        if let Some(map) = headers {
            for h in map.values_mut() {
                strip_ref_or(h, strip_header);
            }
        }
    }

    fn strip_responses(responses: &mut Responses) {
        let Responses {
            default,
            responses,
            extensions,
        } = responses;
        retain_extensions_only(extensions);
        if let Some(def) = default {
            strip_ref_or(def, strip_response);
        }
        if let Some(map) = responses {
            for r in map.values_mut() {
                strip_ref_or(r, strip_response);
            }
        }
    }

    fn strip_response(resp: &mut Response) {
        let Response {
            description: _,
            headers,
            content,
            links,
            extensions,
        } = resp;
        retain_extensions_only(extensions);
        if let Some(map) = headers {
            for h in map.values_mut() {
                strip_ref_or(h, strip_header);
            }
        }
        if let Some(map) = content {
            for mt in map.values_mut() {
                strip_media_type(mt);
            }
        }
        if let Some(map) = links {
            for l in map.values_mut() {
                strip_ref_or(l, strip_link);
            }
        }
    }

    fn strip_security_scheme(scheme: &mut SecurityScheme) {
        match scheme {
            SecurityScheme::HTTP(s) => {
                let HttpSecurityScheme {
                    scheme: _,
                    bearer_format: _,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
            SecurityScheme::ApiKey(s) => {
                let ApiKeySecurityScheme {
                    name: _,
                    location: _,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
            SecurityScheme::OAuth2(s) => {
                let OAuth2SecurityScheme {
                    flows,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
                let OAuth2Flows {
                    implicit,
                    password,
                    client_credentials,
                    authorization_code,
                    extensions,
                } = flows;
                retain_extensions_only(extensions);
                if let Some(ImplicitOAuth2Flow {
                    authorization_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = implicit
                {
                    retain_extensions_only(extensions);
                }
                if let Some(PasswordOAuth2Flow {
                    token_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = password
                {
                    retain_extensions_only(extensions);
                }
                if let Some(ClientCredentialsOAuth2Flow {
                    token_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = client_credentials
                {
                    retain_extensions_only(extensions);
                }
                if let Some(AuthorizationCodeOAuth2Flow {
                    authorization_url: _,
                    token_url: _,
                    refresh_url: _,
                    scopes: _,
                    extensions,
                }) = authorization_code
                {
                    retain_extensions_only(extensions);
                }
            }
            SecurityScheme::OpenIdConnect(s) => {
                let OpenIdConnectSecurityScheme {
                    open_id_connect_url: _,
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
            SecurityScheme::MutualTLS(s) => {
                let MutualTLSSecurityScheme {
                    description: _,
                    extensions,
                } = s.as_mut();
                retain_extensions_only(extensions);
            }
        }
    }

    fn strip_schema(schema: &mut Schema) {
        match schema {
            Schema::Bool(_) | Schema::Empty(_) => {}
            Schema::AllOf(s) => {
                let AllOfSchema {
                    all_of,
                    external_docs,
                    example: _,
                    examples: _,
                    discriminator,
                    extensions: _,
                } = s.as_mut();
                for sub in all_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
            }
            Schema::AnyOf(s) => {
                let AnyOfSchema {
                    any_of,
                    discriminator,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                } = s.as_mut();
                for sub in any_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::OneOf(s) => {
                let OneOfSchema {
                    one_of,
                    discriminator,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                } = s.as_mut();
                for sub in one_of {
                    strip_ref_or(sub, strip_schema);
                }
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Not(s) => {
                let NotSchema {
                    not,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                } = s.as_mut();
                strip_ref_or(not, strip_schema);
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Multi(s) => {
                let MultiSchema {
                    schema_types: _,
                    title: _,
                    description: _,
                    default: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    discriminator,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                } = s.as_mut();
                if let Some(d) = discriminator {
                    strip_discriminator(d);
                }
                if let Some(d) = external_docs {
                    strip_external_docs(d);
                }
            }
            Schema::Single(s) => match s.as_mut() {
                SingleSchema::String(StringSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    x_enum_descriptions: _,
                    max_length: _,
                    min_length: _,
                    pattern: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Integer(IntegerSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    x_enum_descriptions: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Number(NumberSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    format: _,
                    default: _,
                    enum_values: _,
                    x_enum_descriptions: _,
                    minimum: _,
                    exclusive_minimum: _,
                    maximum: _,
                    exclusive_maximum: _,
                    multiple_of: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Boolean(BooleanSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    default: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Array(ArraySchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    items,
                    default: _,
                    max_items: _,
                    min_items: _,
                    unique_items: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(BoolOr::Item(it)) = items {
                        strip_ref_or(it, strip_schema);
                    }
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Null(NullSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
                SingleSchema::Object(ObjectSchema {
                    schema_type: _,
                    title: _,
                    description: _,
                    properties,
                    pattern_properties,
                    default: _,
                    max_properties: _,
                    min_properties: _,
                    additional_properties,
                    unevaluated_properties,
                    property_names,
                    x_tags: _,
                    x_discriminator_value: _,
                    discriminator,
                    required: _,
                    read_only: _,
                    write_only: _,
                    deprecated: _,
                    xml,
                    external_docs,
                    example: _,
                    examples: _,
                    extensions: _,
                }) => {
                    if let Some(props) = properties {
                        for p in props.values_mut() {
                            strip_ref_or(p, strip_schema);
                        }
                    }
                    if let Some(props) = pattern_properties {
                        for p in props.values_mut() {
                            strip_ref_or(p, strip_schema);
                        }
                    }
                    if let Some(BoolOr::Item(ap)) = additional_properties {
                        strip_ref_or(ap, strip_schema);
                    }
                    if let Some(BoolOr::Item(up)) = unevaluated_properties {
                        strip_ref_or(up, strip_schema);
                    }
                    if let Some(pn) = property_names {
                        strip_ref_or(pn, strip_schema);
                    }
                    if let Some(d) = discriminator {
                        strip_discriminator(d);
                    }
                    if let Some(x) = xml {
                        strip_xml(x);
                    }
                    if let Some(d) = external_docs {
                        strip_external_docs(d);
                    }
                }
            },
        }
    }
}
