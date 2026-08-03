//! AsyncAPI v3.0 `Schema` and `Multi Format Schema` objects.
//!
//! Per [Schema Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#schemaObject)
//! and [Multi Format Schema Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#multiFormatSchemaObject).
//!
//! [`Schema`] models the default dialect — JSON Schema draft-07 plus
//! AsyncAPI's `discriminator` / `externalDocs` / `deprecated` additions.
//! A payload in another dialect (Avro, OpenAPI, RAML) is carried by
//! [`MultiFormatSchema`], whose `schema` stays raw JSON because its shape
//! is defined by whatever `schemaFormat` names.

use crate::common::reference::RefOr;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Every `schemaFormat` value AsyncAPI 3.0 accepts.
///
/// The list is closed in the specification's JSON Schema; 3.1 adds its
/// own `version=3.1.0` entries on top of these.
pub const SUPPORTED_SCHEMA_FORMATS: &[&str] = &[
    "application/vnd.aai.asyncapi;version=2.0.0",
    "application/vnd.aai.asyncapi;version=2.1.0",
    "application/vnd.aai.asyncapi;version=2.2.0",
    "application/vnd.aai.asyncapi;version=2.3.0",
    "application/vnd.aai.asyncapi;version=2.4.0",
    "application/vnd.aai.asyncapi;version=2.5.0",
    "application/vnd.aai.asyncapi;version=2.6.0",
    "application/vnd.aai.asyncapi;version=3.0.0",
    "application/vnd.aai.asyncapi+json;version=2.0.0",
    "application/vnd.aai.asyncapi+json;version=2.1.0",
    "application/vnd.aai.asyncapi+json;version=2.2.0",
    "application/vnd.aai.asyncapi+json;version=2.3.0",
    "application/vnd.aai.asyncapi+json;version=2.4.0",
    "application/vnd.aai.asyncapi+json;version=2.5.0",
    "application/vnd.aai.asyncapi+json;version=2.6.0",
    "application/vnd.aai.asyncapi+json;version=3.0.0",
    "application/vnd.aai.asyncapi+yaml;version=2.0.0",
    "application/vnd.aai.asyncapi+yaml;version=2.1.0",
    "application/vnd.aai.asyncapi+yaml;version=2.2.0",
    "application/vnd.aai.asyncapi+yaml;version=2.3.0",
    "application/vnd.aai.asyncapi+yaml;version=2.4.0",
    "application/vnd.aai.asyncapi+yaml;version=2.5.0",
    "application/vnd.aai.asyncapi+yaml;version=2.6.0",
    "application/vnd.aai.asyncapi+yaml;version=3.0.0",
    "application/vnd.oai.openapi;version=3.0.0",
    "application/vnd.oai.openapi+json;version=3.0.0",
    "application/vnd.oai.openapi+yaml;version=3.0.0",
    "application/vnd.apache.avro;version=1.9.0",
    "application/vnd.apache.avro+json;version=1.9.0",
    "application/vnd.apache.avro+yaml;version=1.9.0",
    "application/raml+yaml;version=1.0",
    "application/schema+json;version=draft-07",
    "application/schema+yaml;version=draft-07",
];

/// Whether `format` is a `schemaFormat` AsyncAPI 3.0 accepts.
#[must_use]
pub fn is_supported_schema_format(format: &str) -> bool {
    SUPPORTED_SCHEMA_FORMATS.contains(&format)
}

/// A schema in a named format, for payloads that are not plain AsyncAPI
/// Schema Objects.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MultiFormatSchema {
    /// The format of the `schema` value, as a media type. Defaults to
    /// the AsyncAPI Schema Object dialect of this document's version.
    #[serde(rename = "schemaFormat", skip_serializing_if = "Option::is_none")]
    pub schema_format: Option<String>,

    /// **Required** The schema definition, in whatever `schemaFormat`
    /// names. Left as raw JSON: only the default dialect is typed.
    pub schema: serde_json::Value,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for MultiFormatSchema {
    fn validate_with_context(&self, ctx: &mut Context) {
        if let Some(format) = &self.schema_format {
            if format.is_empty() {
                ctx.error_field("schemaFormat", "must not be empty");
            } else if !is_supported_schema_format(format) {
                ctx.error_field(
                    "schemaFormat",
                    format!("`{format}` is not a schema format supported by AsyncAPI 3.0"),
                );
            }
        }
        if self.schema.is_null() {
            ctx.error_field("schema", "must not be null");
        }
    }
}

/// Either a plain [`Schema`] in the default dialect, or a
/// [`MultiFormatSchema`] naming its own format.
///
/// Deserialization dispatches on the presence of the discriminating
/// `schemaFormat` key, so a malformed schema reports its real error
/// rather than an untagged-enum catch-all.
/// The `Schema` arm is boxed: a full JSON Schema is an order of
/// magnitude larger than a multi-format wrapper, and this type appears
/// in every message header and payload.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SchemaOrMultiFormat {
    MultiFormat(MultiFormatSchema),
    Schema(Box<Schema>),
}

impl Default for SchemaOrMultiFormat {
    fn default() -> Self {
        Self::Schema(Box::default())
    }
}

impl<'de> Deserialize<'de> for SchemaOrMultiFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("schemaFormat").is_some() {
            serde_json::from_value(value)
                .map(SchemaOrMultiFormat::MultiFormat)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(SchemaOrMultiFormat::Schema)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl ValidateWithContext for SchemaOrMultiFormat {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            SchemaOrMultiFormat::MultiFormat(m) => m.validate_with_context(ctx),
            SchemaOrMultiFormat::Schema(s) => s.validate_with_context(ctx),
        }
    }
}

/// `additionalProperties` / `items`-style fields that accept a boolean
/// shorthand as well as a schema.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum BoolOrSchema {
    Bool(bool),
    Schema(Box<RefOr<Schema>>),
}

impl ValidateWithContext for BoolOrSchema {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            BoolOrSchema::Bool(_) => {}
            BoolOrSchema::Schema(s) => s.validate_with_context(ctx),
        }
    }
}

/// The `type` keyword, which draft-07 allows to be a single name or a
/// list of them.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SchemaType {
    Single(String),
    Multiple(Vec<String>),
}

/// An AsyncAPI Schema Object — JSON Schema draft-07 plus AsyncAPI's own
/// additions.
///
/// Keywords outside this set are dropped on parse, matching how the
/// sibling crates treat unknown fields.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Schema {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<SchemaType>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<serde_json::Value>,

    #[serde(rename = "const", skip_serializing_if = "Option::is_none")]
    pub const_value: Option<serde_json::Value>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<serde_json::Value>,

    // ---- objects ----
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, RefOr<Schema>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,

    #[serde(
        rename = "additionalProperties",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_properties: Option<BoolOrSchema>,

    #[serde(
        rename = "patternProperties",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub pattern_properties: BTreeMap<String, RefOr<Schema>>,

    #[serde(rename = "propertyNames", skip_serializing_if = "Option::is_none")]
    pub property_names: Option<Box<RefOr<Schema>>>,

    #[serde(rename = "minProperties", skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<u64>,

    #[serde(rename = "maxProperties", skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<u64>,

    // ---- arrays ----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<BoolOrSchema>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub contains: Option<Box<RefOr<Schema>>>,

    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,

    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,

    #[serde(rename = "uniqueItems", skip_serializing_if = "Option::is_none")]
    pub unique_items: Option<bool>,

    // ---- strings ----
    #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,

    #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    // ---- numbers ----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,

    #[serde(rename = "exclusiveMinimum", skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<f64>,

    #[serde(rename = "exclusiveMaximum", skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<f64>,

    #[serde(rename = "multipleOf", skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,

    // ---- composition ----
    #[serde(rename = "allOf", default, skip_serializing_if = "Vec::is_empty")]
    pub all_of: Vec<RefOr<Schema>>,

    #[serde(rename = "anyOf", default, skip_serializing_if = "Vec::is_empty")]
    pub any_of: Vec<RefOr<Schema>>,

    #[serde(rename = "oneOf", default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<RefOr<Schema>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<RefOr<Schema>>>,

    #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_schema: Option<Box<RefOr<Schema>>>,

    #[serde(rename = "then", skip_serializing_if = "Option::is_none")]
    pub then_schema: Option<Box<RefOr<Schema>>>,

    #[serde(rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_schema: Option<Box<RefOr<Schema>>>,

    // ---- AsyncAPI additions ----
    /// The property name used to differentiate between other schemas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,

    #[serde(rename = "externalDocs", skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<RefOr<ExternalDocumentation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,

    #[serde(rename = "readOnly", skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,

    #[serde(rename = "writeOnly", skip_serializing_if = "Option::is_none")]
    pub write_only: Option<bool>,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Schema {
    fn validate_with_context(&self, ctx: &mut Context) {
        check_bounds(
            ctx,
            "minLength",
            self.min_length,
            "maxLength",
            self.max_length,
        );
        check_bounds(ctx, "minItems", self.min_items, "maxItems", self.max_items);
        check_bounds(
            ctx,
            "minProperties",
            self.min_properties,
            "maxProperties",
            self.max_properties,
        );
        if let (Some(min), Some(max)) = (self.minimum, self.maximum)
            && min > max
        {
            ctx.error_field("minimum", "must not be greater than `maximum`");
        }

        // A `discriminator` names a property, which must be declared and
        // required for the discriminator to be usable.
        if let Some(discriminator) = &self.discriminator {
            if discriminator.is_empty() {
                ctx.error_field("discriminator", "must not be empty");
            } else if !self.properties.is_empty()
                && !self.properties.contains_key(discriminator)
                && self.one_of.is_empty()
                && self.any_of.is_empty()
                && self.all_of.is_empty()
            {
                ctx.error_field(
                    "discriminator",
                    format!("property `{discriminator}` is not declared in `properties`"),
                );
            }
        }

        for (i, name) in self.required.iter().enumerate() {
            if name.is_empty() {
                ctx.in_index("required", i, |ctx| ctx.error("must not be empty"));
            }
        }

        for (name, schema) in &self.properties {
            ctx.in_key("properties", name, |ctx| schema.validate_with_context(ctx));
        }
        for (name, schema) in &self.pattern_properties {
            ctx.in_key("patternProperties", name, |ctx| {
                schema.validate_with_context(ctx);
            });
        }
        if let Some(items) = &self.items {
            ctx.in_field("items", |ctx| items.validate_with_context(ctx));
        }
        if let Some(additional) = &self.additional_properties {
            ctx.in_field("additionalProperties", |ctx| {
                additional.validate_with_context(ctx);
            });
        }
        for (field, list) in [
            ("allOf", &self.all_of),
            ("anyOf", &self.any_of),
            ("oneOf", &self.one_of),
        ] {
            for (i, schema) in list.iter().enumerate() {
                ctx.in_index(field, i, |ctx| schema.validate_with_context(ctx));
            }
        }
        for (field, schema) in [
            ("not", &self.not),
            ("if", &self.if_schema),
            ("then", &self.then_schema),
            ("else", &self.else_schema),
            ("contains", &self.contains),
            ("propertyNames", &self.property_names),
        ] {
            if let Some(schema) = schema {
                ctx.in_field(field, |ctx| schema.validate_with_context(ctx));
            }
        }
        if let Some(docs) = &self.external_docs {
            ctx.in_field("externalDocs", |ctx| docs.validate_with_context(ctx));
        }
    }
}

fn check_bounds(
    ctx: &mut Context,
    min_field: &str,
    min: Option<u64>,
    max_field: &str,
    max: Option<u64>,
) {
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        ctx.error_field(min_field, format!("must not be greater than `{max_field}`"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn parses_a_nested_object_schema_and_round_trips() {
        let value = json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "ref": { "$ref": "#/components/schemas/other" }
            },
            "additionalProperties": false,
            "x-extra": 1
        });
        let schema: Schema = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(schema.schema_type, Some(SchemaType::Single(ref t)) if t == "object"));
        assert_eq!(schema.properties.len(), 3);
        assert!(matches!(
            schema.additional_properties,
            Some(BoolOrSchema::Bool(false))
        ));
        assert_eq!(serde_json::to_value(&schema).unwrap(), value);
    }

    #[test]
    fn type_accepts_a_list_of_names() {
        let schema: Schema = serde_json::from_value(json!({ "type": ["string", "null"] })).unwrap();
        match schema.schema_type {
            Some(SchemaType::Multiple(ref t)) => assert_eq!(t, &["string", "null"]),
            other => panic!("expected multiple types, got {other:?}"),
        }
    }

    #[test]
    fn inverted_bounds_are_reported() {
        let schema: Schema = serde_json::from_value(json!({
            "minLength": 5, "maxLength": 1,
            "minItems": 3, "maxItems": 2,
            "minProperties": 4, "maxProperties": 1,
            "minimum": 10.0, "maximum": 1.0
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        for field in ["minLength", "minItems", "minProperties", "minimum"] {
            assert!(
                ctx.errors.iter().any(|e| e.contains(field)),
                "expected an error for {field}, got: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn discriminator_must_name_a_declared_property() {
        let schema: Schema = serde_json::from_value(json!({
            "type": "object",
            "discriminator": "kind",
            "properties": { "other": { "type": "string" } }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("property `kind` is not declared")),
            "got: {:?}",
            ctx.errors
        );

        // A discriminator alongside a composition keyword is fine — the
        // property lives in the composed subschemas.
        let composed: Schema = serde_json::from_value(json!({
            "discriminator": "kind",
            "properties": { "other": { "type": "string" } },
            "oneOf": [ { "$ref": "#/components/schemas/a" } ]
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        composed.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn nested_schema_errors_carry_their_path() {
        let schema: Schema = serde_json::from_value(json!({
            "properties": { "a": { "minItems": 2, "maxItems": 1 } },
            "items": { "minLength": 2, "maxLength": 1 },
            "oneOf": [ { "minimum": 5.0, "maximum": 1.0 } ],
            "not": { "discriminator": "" },
            "externalDocs": { "url": "" },
            "required": [""]
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.payload.properties.a.minItems"))
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.payload.items.minLength"))
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.payload.oneOf[0].minimum"))
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.payload.not.discriminator"))
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.externalDocs.url: must not be empty")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.required[0]: must not be empty")
        );
    }

    #[test]
    fn pattern_and_additional_properties_are_validated_as_schemas() {
        let schema: Schema = serde_json::from_value(json!({
            "patternProperties": { "^x-": { "minItems": 2, "maxItems": 1 } },
            "additionalProperties": { "minLength": 5, "maxLength": 1 }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.payload.patternProperties.^x-.minItems")),
            "got: {msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|e| e.starts_with("#.payload.additionalProperties.minLength")),
            "got: {msgs:?}"
        );
    }

    #[test]
    fn multi_format_schema_is_picked_by_schema_format_key() {
        let value = json!({
            "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
            "schema": { "type": "record", "name": "User" }
        });
        let parsed: SchemaOrMultiFormat = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(parsed, SchemaOrMultiFormat::MultiFormat(_)));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);

        let plain: SchemaOrMultiFormat =
            serde_json::from_value(json!({ "type": "string" })).unwrap();
        assert!(matches!(plain, SchemaOrMultiFormat::Schema(_)));
        assert!(matches!(
            SchemaOrMultiFormat::default(),
            SchemaOrMultiFormat::Schema(_)
        ));
    }

    #[test]
    fn unsupported_schema_format_is_reported() {
        let parsed: SchemaOrMultiFormat = serde_json::from_value(json!({
            "schemaFormat": "application/vnd.aai.asyncapi;version=9.9.9",
            "schema": { "type": "string" }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        parsed.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("is not a schema format supported by AsyncAPI 3.0")),
            "got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn every_supported_format_passes_and_empty_is_rejected() {
        for format in SUPPORTED_SCHEMA_FORMATS {
            assert!(is_supported_schema_format(format), "{format} should pass");
            let m = MultiFormatSchema {
                schema_format: Some((*format).to_owned()),
                schema: json!({ "type": "string" }),
                extensions: None,
            };
            let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
            m.validate_with_context(&mut ctx);
            assert!(ctx.errors.is_empty(), "{format}: {:?}", ctx.errors);
        }
        assert!(!is_supported_schema_format("text/plain"));

        let empty = MultiFormatSchema {
            schema_format: Some(String::new()),
            schema: json!(null),
            extensions: None,
        };
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        empty.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.schemaFormat: must not be empty")
        );
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.schema: must not be null")
        );
    }

    #[test]
    fn bool_or_schema_validates_only_the_schema_arm() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload.additionalProperties");
        BoolOrSchema::Bool(true).validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());

        let nested = BoolOrSchema::Schema(Box::new(RefOr::Item(Schema {
            discriminator: Some(String::new()),
            ..Default::default()
        })));
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload.additionalProperties");
        nested.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.additionalProperties.discriminator: must not be empty")
        );
    }
}
