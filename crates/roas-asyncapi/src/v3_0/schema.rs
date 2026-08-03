//! AsyncAPI v3.0 `Schema` and `Multi Format Schema` objects.
//!
//! Per [Schema Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#schemaObject)
//! and [Multi Format Schema Object](https://www.asyncapi.com/docs/reference/specification/v3.0.0#multiFormatSchemaObject).
//!
//! [`Schema`] models the default dialect — JSON Schema draft-07 plus
//! AsyncAPI's `discriminator` / `externalDocs` / `deprecated` additions.
//! A payload in another dialect (Avro, OpenAPI, RAML) is carried by
//! [`MultiFormatSchema`], whose `schema` stays raw JSON because its
//! shape is defined by whatever `schemaFormat` names.
//!
//! The two are told apart exactly as the specification's `anySchema`
//! does — by the presence of a `schema` property, *not* by
//! `schemaFormat`, which is optional and defaults to this document's
//! own dialect.

use crate::common::reference::RefOr;
use crate::v3_0::external_documentation::ExternalDocumentation;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// The `schemaFormat` values AsyncAPI 3.0 documents as "formats tooling
/// MUST support".
///
/// This is advisory, not a closed set: the schema types `schemaFormat`
/// as `anyOf: [string, <this enum>]`, so a custom dialect is legal and
/// its `schema` simply stays raw JSON. Use
/// [`is_supported_schema_format`] to ask whether a format is one this
/// list names.
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

/// Whether `format` is one of the [`SUPPORTED_SCHEMA_FORMATS`].
///
/// A `false` answer does not make a document invalid — see that
/// constant.
#[must_use]
pub fn is_supported_schema_format(format: &str) -> bool {
    SUPPORTED_SCHEMA_FORMATS.contains(&format)
}

/// Keep an explicit `null` distinct from an absent property. Plain
/// `Option<Value>` collapses both to `None`, which would drop a valid
/// `"default": null` or `"const": null`.
fn deserialize_present_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// A schema in a named format, for payloads that are not plain AsyncAPI
/// Schema Objects.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct MultiFormatSchema {
    /// The format of the `schema` value, as a media type. When absent
    /// it defaults to this document's own AsyncAPI Schema dialect,
    /// which makes the object equivalent to a Schema Object.
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
        // `schemaFormat` accepts any media type, so only an empty one
        // is a mistake — an unrecognized dialect is legal and simply
        // leaves `schema` unparsed.
        if let Some(format) = &self.schema_format {
            ctx.require_non_empty("schemaFormat", format);
        }
    }
}

/// Either a [`Schema`] in the default dialect, a [`MultiFormatSchema`]
/// naming its own format, or a boolean schema.
///
/// Deserialization mirrors the specification's `anySchema`: a boolean
/// is a boolean schema, an object carrying a `schema` property is a
/// Multi Format Schema, and anything else is a Schema Object. The
/// `Schema` arm is boxed because a full JSON Schema is an order of
/// magnitude larger than the other two, and this type appears in every
/// message header and payload.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SchemaOrMultiFormat {
    /// A JSON Schema boolean: `true` accepts anything, `false` nothing.
    Bool(bool),
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
        if let serde_json::Value::Bool(b) = value {
            return Ok(SchemaOrMultiFormat::Bool(b));
        }
        if value.get("schema").is_some() {
            return serde_json::from_value(value)
                .map(SchemaOrMultiFormat::MultiFormat)
                .map_err(serde::de::Error::custom);
        }
        serde_json::from_value(value)
            .map(|schema| SchemaOrMultiFormat::Schema(Box::new(schema)))
            .map_err(serde::de::Error::custom)
    }
}

impl ValidateWithContext for SchemaOrMultiFormat {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            SchemaOrMultiFormat::Bool(_) => {}
            SchemaOrMultiFormat::MultiFormat(m) => m.validate_with_context(ctx),
            SchemaOrMultiFormat::Schema(s) => s.validate_with_context(ctx),
        }
    }
}

/// A subschema: JSON Schema allows `true` / `false` wherever a schema
/// is expected, so every child position accepts either.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SubSchema {
    Bool(bool),
    Schema(Box<RefOr<Schema>>),
}

impl Default for SubSchema {
    fn default() -> Self {
        Self::Schema(Box::new(RefOr::Item(Schema::default())))
    }
}

impl ValidateWithContext for SubSchema {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            SubSchema::Bool(_) => {}
            SubSchema::Schema(s) => s.validate_with_context(ctx),
        }
    }
}

/// `items` is a single schema (every element) or a tuple of schemas
/// (positional), per draft-07.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Items {
    Tuple(Vec<SubSchema>),
    Single(Box<SubSchema>),
}

impl Items {
    /// Validate under `items`, indexing each entry of the tuple form so
    /// diagnostics read `…items[1]` rather than `…items`.
    fn validate_under_items(&self, ctx: &mut Context) {
        match self {
            Items::Single(schema) => {
                ctx.in_field("items", |ctx| schema.validate_with_context(ctx));
            }
            Items::Tuple(schemas) => {
                // The tuple form is a `schemaArray`: `minItems: 1`.
                if schemas.is_empty() {
                    ctx.error_field("items", "must contain at least one subschema");
                }
                for (i, schema) in schemas.iter().enumerate() {
                    ctx.in_index("items", i, |ctx| schema.validate_with_context(ctx));
                }
            }
        }
    }
}

/// A `dependencies` entry: either a schema that applies when the
/// property is present, or the property names it requires.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Dependency {
    Required(Vec<String>),
    Schema(SubSchema),
}

impl ValidateWithContext for Dependency {
    fn validate_with_context(&self, ctx: &mut Context) {
        match self {
            // The property-name form is a `stringArray`, so entries
            // must be unique. The dependency is already the current
            // path, so the index goes in the message.
            Dependency::Required(names) => {
                for (i, name) in names.iter().enumerate() {
                    if names[..i].contains(name) {
                        ctx.error(format!("duplicate entry `{name}` at index {i}"));
                    }
                }
            }
            Dependency::Schema(schema) => schema.validate_with_context(ctx),
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
/// additions (`discriminator`, `externalDocs`, `deprecated`).
///
/// Every draft-07 keyword is modeled, so a schema round-trips
/// unchanged; keywords from other dialects belong in a
/// [`MultiFormatSchema`] instead.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Schema {
    // ---- identification ----
    #[serde(rename = "$id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The `$schema` dialect declaration.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub dialect: Option<String>,

    #[serde(rename = "$comment", skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,

    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<SchemaType>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// An explicit `null` is preserved as `Some(Value::Null)`; absent
    /// is `None`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<serde_json::Value>,

    /// `minItems: 1` in the meta-schema, so an empty-but-present
    /// `enum` is an error rather than an absent one — hence the
    /// `Option`.
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<serde_json::Value>>,

    /// An explicit `null` is preserved as `Some(Value::Null)`; absent
    /// is `None`.
    #[serde(
        rename = "const",
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub const_value: Option<serde_json::Value>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<serde_json::Value>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub definitions: BTreeMap<String, SubSchema>,

    // ---- objects ----
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, SubSchema>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,

    #[serde(
        rename = "additionalProperties",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_properties: Option<SubSchema>,

    #[serde(
        rename = "patternProperties",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub pattern_properties: BTreeMap<String, SubSchema>,

    #[serde(rename = "propertyNames", skip_serializing_if = "Option::is_none")]
    pub property_names: Option<Box<SubSchema>>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,

    #[serde(rename = "minProperties", skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<u64>,

    #[serde(rename = "maxProperties", skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<u64>,

    // ---- arrays ----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Items>,

    #[serde(rename = "additionalItems", skip_serializing_if = "Option::is_none")]
    pub additional_items: Option<Box<SubSchema>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub contains: Option<Box<SubSchema>>,

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

    #[serde(rename = "contentMediaType", skip_serializing_if = "Option::is_none")]
    pub content_media_type: Option<String>,

    #[serde(rename = "contentEncoding", skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,

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
    #[serde(rename = "allOf", skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<SubSchema>>,

    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<SubSchema>>,

    #[serde(rename = "oneOf", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<SubSchema>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<SubSchema>>,

    #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
    pub if_schema: Option<Box<SubSchema>>,

    #[serde(rename = "then", skip_serializing_if = "Option::is_none")]
    pub then_schema: Option<Box<SubSchema>>,

    #[serde(rename = "else", skip_serializing_if = "Option::is_none")]
    pub else_schema: Option<Box<SubSchema>>,

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

/// The draft-07 `simpleTypes` enumeration.
const SIMPLE_TYPES: &[&str] = &[
    "array", "boolean", "integer", "null", "number", "object", "string",
];

/// JSON instance equality, per
/// [draft-07 §4.2.3](https://json-schema.org/draft-07/json-schema-core#rfc.section.4.2.3).
///
/// Numbers are equal when they have the same *mathematical* value, so
/// `1` and `1.0` are the same instance. `serde_json`'s `PartialEq`
/// compares the stored representation instead and would call them
/// different, which is why `uniqueItems` cannot use it.
fn json_instance_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
                return a == b;
            }
            if let (Some(a), Some(b)) = (a.as_u64(), b.as_u64()) {
                return a == b;
            }
            match (a.as_f64(), b.as_f64()) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_instance_eq(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| json_instance_eq(a, b)))
        }
        _ => a == b,
    }
}

/// Report the index of every entry that repeats an earlier one, per the
/// meta-schema's `uniqueItems` on `stringArray`.
fn check_unique_strings(ctx: &mut Context, field: &str, values: &[String]) {
    for (i, value) in values.iter().enumerate() {
        if values[..i].contains(value) {
            ctx.in_index(field, i, |ctx| ctx.error("duplicate entry"));
        }
    }
}

impl ValidateWithContext for Schema {
    fn validate_with_context(&self, ctx: &mut Context) {
        // ---- keyword constraints from the draft-07 meta-schema ----
        match &self.schema_type {
            Some(SchemaType::Single(name)) if !SIMPLE_TYPES.contains(&name.as_str()) => {
                ctx.error_field("type", format!("`{name}` is not a JSON Schema type"));
            }
            Some(SchemaType::Single(_)) => {}
            Some(SchemaType::Multiple(names)) => {
                if names.is_empty() {
                    ctx.error_field("type", "must contain at least one type");
                }
                for (i, name) in names.iter().enumerate() {
                    if !SIMPLE_TYPES.contains(&name.as_str()) {
                        ctx.in_index("type", i, |ctx| {
                            ctx.error(format!("`{name}` is not a JSON Schema type"));
                        });
                    } else if names[..i].contains(name) {
                        ctx.in_index("type", i, |ctx| ctx.error("duplicate type"));
                    }
                }
            }
            None => {}
        }

        if let Some(values) = &self.enum_values {
            if values.is_empty() {
                ctx.error_field("enum", "must contain at least one value");
            }
            for (i, value) in values.iter().enumerate() {
                if values[..i].iter().any(|seen| json_instance_eq(seen, value)) {
                    ctx.in_index("enum", i, |ctx| ctx.error("duplicate value"));
                }
            }
        }

        if let Some(multiple_of) = self.multiple_of
            && multiple_of <= 0.0
        {
            ctx.error_field("multipleOf", "must be greater than zero");
        }

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

        // "The property name used MUST be defined at this schema and it
        // MUST be in the required property list." Composition keywords
        // put those declarations in subschemas this validator does not
        // resolve, so the check only runs when there are none.
        if let Some(discriminator) = &self.discriminator {
            if discriminator.is_empty() {
                ctx.error_field("discriminator", "must not be empty");
            } else if self.one_of.is_none() && self.any_of.is_none() && self.all_of.is_none() {
                if !self.properties.contains_key(discriminator) {
                    ctx.error_field(
                        "discriminator",
                        format!("property `{discriminator}` is not declared in `properties`"),
                    );
                }
                if !self.required.contains(discriminator) {
                    ctx.error_field(
                        "discriminator",
                        format!("property `{discriminator}` must be listed in `required`"),
                    );
                }
            }
        }

        // `required` is a `stringArray`: entries must be unique. An
        // empty property name is legal — JSON objects may have one.
        check_unique_strings(ctx, "required", &self.required);

        for (field, map) in [
            ("properties", &self.properties),
            ("patternProperties", &self.pattern_properties),
            ("definitions", &self.definitions),
        ] {
            for (name, schema) in map {
                ctx.in_key(field, name, |ctx| schema.validate_with_context(ctx));
            }
        }
        for (name, dependency) in &self.dependencies {
            ctx.in_key("dependencies", name, |ctx| {
                dependency.validate_with_context(ctx);
            });
        }
        if let Some(items) = &self.items {
            items.validate_under_items(ctx);
        }
        for (field, list) in [
            ("allOf", self.all_of.as_ref()),
            ("anyOf", self.any_of.as_ref()),
            ("oneOf", self.one_of.as_ref()),
        ] {
            let Some(list) = list else { continue };
            if list.is_empty() {
                ctx.error_field(field, "must contain at least one subschema");
            }
            for (i, schema) in list.iter().enumerate() {
                ctx.in_index(field, i, |ctx| schema.validate_with_context(ctx));
            }
        }
        for (field, schema) in [
            ("additionalProperties", self.additional_properties.as_ref()),
            ("propertyNames", self.property_names.as_deref()),
            ("additionalItems", self.additional_items.as_deref()),
            ("contains", self.contains.as_deref()),
            ("not", self.not.as_deref()),
            ("if", self.if_schema.as_deref()),
            ("then", self.then_schema.as_deref()),
            ("else", self.else_schema.as_deref()),
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
                "ref": { "$ref": "#/components/schemas/other" },
                "anything": true
            },
            "additionalProperties": false,
            "x-extra": 1
        });
        let schema: Schema = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(schema.schema_type, Some(SchemaType::Single(ref t)) if t == "object"));
        assert_eq!(schema.properties.len(), 4);
        assert!(matches!(
            schema.properties["anything"],
            SubSchema::Bool(true)
        ));
        assert!(matches!(
            schema.additional_properties,
            Some(SubSchema::Bool(false))
        ));
        assert_eq!(serde_json::to_value(&schema).unwrap(), value);
    }

    #[test]
    fn every_draft_07_keyword_round_trips() {
        let value = json!({
            "$id": "https://example.com/user.json",
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$comment": "internal note",
            "definitions": { "positiveInt": { "type": "integer", "minimum": 0.0 } },
            "dependencies": {
                "creditCard": ["billingAddress"],
                "shipping": { "required": ["address"] }
            },
            "items": [ { "type": "string" }, { "type": "integer" } ],
            "additionalItems": false,
            "contentMediaType": "application/json",
            "contentEncoding": "base64",
            "propertyNames": { "pattern": "^[a-z]+$" },
            "contains": { "type": "string" },
            "if": { "required": ["a"] },
            "then": { "required": ["b"] },
            "else": true,
            "not": { "type": "null" },
            "allOf": [ { "type": "object" } ],
            "anyOf": [ true ],
            "oneOf": [ { "type": "string" } ],
            "uniqueItems": true,
            "multipleOf": 2.0,
            "exclusiveMinimum": 1.0,
            "exclusiveMaximum": 9.0,
            "readOnly": true,
            "writeOnly": false,
            "deprecated": true,
            "discriminator": "kind",
            "externalDocs": { "url": "https://example.com" }
        });
        let schema: Schema = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(schema.id.as_deref(), Some("https://example.com/user.json"));
        assert_eq!(schema.definitions.len(), 1);
        assert!(matches!(
            schema.dependencies["creditCard"],
            Dependency::Required(_)
        ));
        assert!(matches!(
            schema.dependencies["shipping"],
            Dependency::Schema(_)
        ));
        assert!(matches!(schema.items, Some(Items::Tuple(ref t)) if t.len() == 2));
        assert_eq!(serde_json::to_value(&schema).unwrap(), value);
    }

    #[test]
    fn explicit_null_default_and_const_survive_a_round_trip() {
        let value = json!({ "default": null, "const": null });
        let schema: Schema = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(schema.default, Some(serde_json::Value::Null));
        assert_eq!(schema.const_value, Some(serde_json::Value::Null));
        assert_eq!(serde_json::to_value(&schema).unwrap(), value);

        // An absent keyword stays absent.
        let bare: Schema = serde_json::from_value(json!({})).unwrap();
        assert_eq!(bare.default, None);
        assert_eq!(bare.const_value, None);
        assert_eq!(serde_json::to_value(&bare).unwrap(), json!({}));
    }

    #[test]
    fn single_form_items_validate_under_items_without_an_index() {
        let schema: Schema =
            serde_json::from_value(json!({ "items": { "minItems": 2, "maxItems": 1 } })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("#.payload.items.minItems")),
            "got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn single_and_tuple_items_are_distinguished() {
        let single: Schema =
            serde_json::from_value(json!({ "items": { "type": "string" } })).unwrap();
        assert!(matches!(single.items, Some(Items::Single(_))));
        assert_eq!(
            serde_json::to_value(&single).unwrap(),
            json!({ "items": { "type": "string" } })
        );

        let tuple: Schema =
            serde_json::from_value(json!({ "items": [ { "type": "string" }, false ] })).unwrap();
        match tuple.items {
            Some(Items::Tuple(ref t)) => {
                assert_eq!(t.len(), 2);
                assert!(matches!(t[1], SubSchema::Bool(false)));
            }
            ref other => panic!("expected a tuple, got {other:?}"),
        }
    }

    #[test]
    fn meta_schema_keyword_constraints_are_enforced() {
        // `type: []`, `oneOf: []`, `enum: []` and `multipleOf: 0` are
        // all rejected by the meta-schema.
        let schema: Schema = serde_json::from_value(json!({
            "type": [],
            "oneOf": [],
            "enum": [],
            "multipleOf": 0.0
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.type: must contain at least one type")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.oneOf: must contain at least one subschema")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.enum: must contain at least one value")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.multipleOf: must be greater than zero")
        );

        // …and the empty arrays survive serialization rather than being
        // silently dropped.
        assert_eq!(
            serde_json::to_value(&schema).unwrap(),
            json!({ "type": [], "oneOf": [], "enum": [], "multipleOf": 0.0 })
        );
    }

    #[test]
    fn type_names_must_be_json_schema_types_and_unique() {
        let schema: Schema =
            serde_json::from_value(json!({ "type": ["string", "strng", "string"] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.type[1]: `strng` is not a JSON Schema type")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.type[2]: duplicate type")
        );

        let single: Schema = serde_json::from_value(json!({ "type": "objekt" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        single.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.type: `objekt` is not a JSON Schema type")
        );

        // Every simple type is accepted.
        for name in SIMPLE_TYPES {
            let ok: Schema = serde_json::from_value(json!({ "type": name })).unwrap();
            let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
            ok.validate_with_context(&mut ctx);
            assert!(ctx.errors.is_empty(), "{name}: {:?}", ctx.errors);
        }
    }

    #[test]
    fn enum_values_must_be_unique() {
        let schema: Schema = serde_json::from_value(json!({ "enum": ["a", "b", "a"] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.enum[2]: duplicate value"),
            "got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn enum_uniqueness_uses_json_instance_equality() {
        // Draft-07 compares numbers by mathematical value, so `1` and
        // `1.0` are the same instance even though `serde_json`'s
        // `PartialEq` calls them different.
        for values in [
            json!([1, 1.0]),
            json!([1.5, 1.50]),
            json!([0, -0.0]),
            json!([{ "a": 1 }, { "a": 1.0 }]),
            json!([[1, 2], [1.0, 2.0]]),
        ] {
            let schema: Schema = serde_json::from_value(json!({ "enum": values })).unwrap();
            let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
            schema.validate_with_context(&mut ctx);
            assert!(
                ctx.errors
                    .iter()
                    .any(|e| e == "#.payload.enum[1]: duplicate value"),
                "{values} should be a duplicate, got: {:?}",
                ctx.errors
            );
        }

        // …and genuinely distinct values still pass.
        for values in [
            json!([1, 2]),
            json!([1, "1"]),
            json!([{ "a": 1 }, { "a": 2 }]),
            json!([{ "a": 1 }, { "b": 1 }]),
            json!([[1, 2], [2, 1]]),
            json!([null, false]),
        ] {
            let schema: Schema = serde_json::from_value(json!({ "enum": values })).unwrap();
            let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
            schema.validate_with_context(&mut ctx);
            assert!(ctx.errors.is_empty(), "{values}: {:?}", ctx.errors);
        }
    }

    #[test]
    fn discriminator_must_be_declared_and_required() {
        // Named but nothing declared at all.
        let bare: Schema = serde_json::from_value(json!({ "discriminator": "kind" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        bare.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert!(
            msgs.iter()
                .any(|e| e.contains("is not declared in `properties`"))
        );
        assert!(
            msgs.iter()
                .any(|e| e.contains("must be listed in `required`"))
        );

        // Declared, but not required.
        let optional: Schema = serde_json::from_value(json!({
            "discriminator": "kind",
            "properties": { "kind": { "type": "string" } }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        optional.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e
                    == "#.payload.discriminator: property `kind` must be listed in `required`"),
            "got: {:?}",
            ctx.errors
        );

        // Declared and required.
        let ok: Schema = serde_json::from_value(json!({
            "discriminator": "kind",
            "required": ["kind"],
            "properties": { "kind": { "type": "string" } }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        ok.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
    }

    #[test]
    fn string_arrays_allow_empty_names_but_not_duplicates() {
        // `stringArray` constrains uniqueness, not non-emptiness — an
        // empty property name is a legal JSON object key.
        let schema: Schema = serde_json::from_value(json!({ "required": ["", "a", "a"] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        assert_eq!(msgs, vec!["#.payload.required[2]: duplicate entry"]);

        // The `dependencies` property-name form is a `stringArray` too.
        let dependent: Schema =
            serde_json::from_value(json!({ "dependencies": { "card": ["a", "a"] } })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        dependent.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.dependencies.card: duplicate entry `a` at index 1"),
            "got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn an_empty_items_tuple_is_rejected() {
        let schema: Schema = serde_json::from_value(json!({ "items": [] })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        assert!(
            ctx.errors
                .iter()
                .any(|e| e == "#.payload.items: must contain at least one subschema"),
            "got: {:?}",
            ctx.errors
        );
        // …and it round-trips rather than being dropped.
        assert_eq!(
            serde_json::to_value(&schema).unwrap(),
            json!({ "items": [] })
        );
    }

    #[test]
    fn positive_multiple_of_and_populated_compositions_pass() {
        let schema: Schema = serde_json::from_value(json!({
            "multipleOf": 2.5,
            "allOf": [ { "type": "object" } ],
            "anyOf": [ true ],
            "oneOf": [ { "type": "string" } ],
            "enum": ["a"],
            "type": ["string", "null"]
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
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
            "patternProperties": { "^x-": { "minLength": 2, "maxLength": 1 } },
            "definitions": { "d": { "minimum": 5.0, "maximum": 1.0 } },
            "dependencies": { "dep": { "minItems": 2, "maxItems": 1 } },
            "items": [ { "minItems": 2, "maxItems": 1 } ],
            "additionalItems": { "discriminator": "" },
            "additionalProperties": { "minLength": 5, "maxLength": 1 },
            "oneOf": [ { "minimum": 5.0, "maximum": 1.0 } ],
            "not": { "discriminator": "" },
            "externalDocs": { "url": "" },
            "required": ["a", "a"]
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        schema.validate_with_context(&mut ctx);
        let msgs: Vec<_> = ctx.errors.iter().map(ToString::to_string).collect();
        for expected in [
            "#.payload.properties.a.minItems",
            "#.payload.patternProperties.^x-.minLength",
            "#.payload.definitions.d.minimum",
            "#.payload.dependencies.dep.minItems",
            "#.payload.items[0].minItems",
            "#.payload.additionalItems.discriminator",
            "#.payload.additionalProperties.minLength",
            "#.payload.oneOf[0].minimum",
            "#.payload.not.discriminator",
        ] {
            assert!(
                msgs.iter().any(|e| e.starts_with(expected)),
                "expected {expected}, got: {msgs:?}"
            );
        }
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.externalDocs.url: must not be empty")
        );
        assert!(
            msgs.iter()
                .any(|e| e == "#.payload.required[1]: duplicate entry")
        );
    }

    #[test]
    fn multi_format_schema_is_picked_by_the_schema_key() {
        // With a `schemaFormat`…
        let value = json!({
            "schemaFormat": "application/vnd.apache.avro;version=1.9.0",
            "schema": { "type": "record", "name": "User" }
        });
        let parsed: SchemaOrMultiFormat = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(parsed, SchemaOrMultiFormat::MultiFormat(_)));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);

        // …and without one: `schemaFormat` is optional and defaults to
        // this document's dialect, so `schema` alone still selects the
        // Multi Format Schema Object rather than dropping the payload.
        let bare = json!({ "schema": { "type": "string" } });
        let parsed: SchemaOrMultiFormat = serde_json::from_value(bare.clone()).unwrap();
        match &parsed {
            SchemaOrMultiFormat::MultiFormat(m) => {
                assert!(m.schema_format.is_none());
                assert_eq!(m.schema, json!({ "type": "string" }));
            }
            other => panic!("expected a multi-format schema, got {other:?}"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), bare);
    }

    #[test]
    fn plain_and_boolean_schemas_are_recognized() {
        let plain: SchemaOrMultiFormat =
            serde_json::from_value(json!({ "type": "string" })).unwrap();
        assert!(matches!(plain, SchemaOrMultiFormat::Schema(_)));
        assert!(matches!(
            SchemaOrMultiFormat::default(),
            SchemaOrMultiFormat::Schema(_)
        ));

        for (value, expected) in [(json!(true), true), (json!(false), false)] {
            let parsed: SchemaOrMultiFormat = serde_json::from_value(value.clone()).unwrap();
            assert!(matches!(parsed, SchemaOrMultiFormat::Bool(b) if b == expected));
            assert_eq!(serde_json::to_value(&parsed).unwrap(), value);

            let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
            parsed.validate_with_context(&mut ctx);
            assert!(ctx.errors.is_empty());
        }
    }

    #[test]
    fn a_schema_property_named_schema_still_parses_as_multi_format() {
        // `schema` is not a JSON Schema keyword, so the specification's
        // `anySchema` treats its presence as the discriminator.
        let parsed: SchemaOrMultiFormat =
            serde_json::from_value(json!({ "schema": true })).unwrap();
        assert!(matches!(parsed, SchemaOrMultiFormat::MultiFormat(_)));
    }

    #[test]
    fn custom_schema_formats_are_accepted() {
        // The documented list is what tooling MUST support, not a
        // closed set: a custom dialect is legal and keeps its schema as
        // raw JSON.
        let parsed: SchemaOrMultiFormat = serde_json::from_value(json!({
            "schemaFormat": "application/vnd.example.custom+json;version=1.0",
            "schema": { "shape": "whatever the dialect says" }
        }))
        .unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload");
        parsed.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty(), "got: {:?}", ctx.errors);
        assert!(!is_supported_schema_format(
            "application/vnd.example.custom+json;version=1.0"
        ));
    }

    #[test]
    fn every_documented_format_is_recognized_and_empty_is_rejected() {
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
    }

    #[test]
    fn sub_schema_validates_only_the_schema_arm() {
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload.additionalProperties");
        SubSchema::Bool(true).validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());

        let nested = SubSchema::Schema(Box::new(RefOr::Item(Schema {
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
        assert!(matches!(SubSchema::default(), SubSchema::Schema(_)));
    }

    #[test]
    fn dependency_required_form_needs_no_validation() {
        let dependency: Dependency = serde_json::from_value(json!(["a", "b"])).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.payload.dependencies.d");
        dependency.validate_with_context(&mut ctx);
        assert!(ctx.errors.is_empty());
    }
}
