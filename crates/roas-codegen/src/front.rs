//! The normalized front-end representation: what a schema *says*,
//! with the OpenAPI model's shape translated away.
//!
//! Lowering works from this rather than from `roas::v3_2::Schema`
//! directly, so a second description language — AsyncAPI — needs a
//! second translation into it, not a second lowering.

use crate::diagnostic::{SchemaId, push_token};
use roas::common::bool_or::BoolOr;
use roas::common::formats::{IntegerFormat, NumberFormat, SchemaType, StringFormat};
use roas::common::reference::RefOr;
use roas::v3_2::discriminator::Discriminator;
use roas::v3_2::schema::{
    AllOfSchema, AnyOfSchema, ArraySchema, BooleanSchema, IntegerSchema, MultiSchema, NullSchema,
    NumberSchema, ObjectSchema, OneOfSchema, Schema, SchemaRef, SingleSchema, StringSchema,
};
use serde_json::Value;
use std::collections::BTreeMap;
use url::Url;

/// Where a `$ref` points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefTarget {
    /// `#/components/schemas/<name>`: the one kind of reference the
    /// first release follows.
    Component(String),
    /// Anything else. Reported at lowering; the slot lowers to nothing.
    Unsupported(String),
}

/// One schema position: either a reference or an inline schema.
#[derive(Debug, Clone)]
pub(crate) enum Slot {
    Ref {
        id: SchemaId,
        target: RefTarget,
        /// The reference's sibling keywords, as a schema of their own,
        /// when they assert anything beyond annotations. `$ref` with
        /// siblings is the intersection of both.
        siblings: Option<Box<FrontSchema>>,
        /// Annotation-only siblings: a description or summary override.
        description: Option<String>,
    },
    Inline(Box<FrontSchema>),
}

impl Slot {
    pub(crate) fn id(&self) -> &SchemaId {
        match self {
            Slot::Ref { id, .. } => id,
            Slot::Inline(schema) => &schema.id,
        }
    }
}

/// How an object treats members its `properties` do not name.
#[derive(Debug, Clone)]
pub(crate) enum Additional {
    /// Omitted, or `true`: any JSON value.
    Any,
    /// `false`: unknown members are rejected.
    Denied,
    /// A schema every unknown member must match.
    Schema(Slot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntWidth {
    Int32,
    Int64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloatWidth {
    Float,
    Double,
}

/// The constraints a schema declares that the type cannot express.
/// First-class so validation, when it lands, is a rendering decision.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Constraints {
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<String>,
    pub minimum: Option<serde_json::Number>,
    pub maximum: Option<serde_json::Number>,
    pub exclusive_minimum: Option<serde_json::Number>,
    pub exclusive_maximum: Option<serde_json::Number>,
    pub multiple_of: Option<serde_json::Number>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,
    /// `propertyNames` is declared; the schema itself is not modelled.
    pub property_names: bool,
    /// `patternProperties` patterns, in declaration order.
    pub pattern_properties: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// `{}` or `true`: any JSON value.
    Any,
    /// `false`: no value is valid.
    Never,
    /// `not`: ungeneratable.
    Not,
    String {
        format: Option<String>,
        enum_values: Option<Vec<String>>,
    },
    Integer {
        width: Option<IntWidth>,
        enum_values: Option<Vec<i64>>,
    },
    Number {
        width: Option<FloatWidth>,
        enum_values: Option<Vec<serde_json::Number>>,
    },
    Boolean,
    Null,
    Array {
        /// `None` when `items` is omitted or `true`: any value.
        items: Option<Slot>,
    },
    Object {
        properties: BTreeMap<String, Slot>,
        required: Vec<String>,
        additional: Additional,
        /// No `type` was written. `properties` only constrains objects,
        /// so a typeless schema still permits every other instance.
        typeless: bool,
    },
    AllOf {
        branches: Vec<Slot>,
    },
    AnyOf {
        branches: Vec<Slot>,
    },
    OneOf {
        branches: Vec<Slot>,
        discriminator: Option<Discriminator>,
    },
    /// `type: [a, b, …]` with several non-null members: a union with no
    /// discriminator and no branch schemas of its own. Each branch is
    /// the same schema read under one type.
    TypeUnion {
        branches: Vec<Slot>,
    },
}

/// One schema, translated.
#[derive(Debug, Clone)]
pub(crate) struct FrontSchema {
    pub id: SchemaId,
    pub kind: Kind,
    /// `null` is among the permitted types.
    pub nullable: bool,
    pub title: Option<String>,
    pub description: Option<String>,
    pub default: Option<Value>,
    pub deprecated: bool,
    pub read_only: bool,
    pub write_only: bool,
    pub constraints: Constraints,
    /// Keywords that reached the model's catch-all and are not `x-`
    /// extensions. Every one is reported by lowering.
    pub unsupported: Vec<String>,
    /// `x-` extensions, verbatim.
    pub extensions: BTreeMap<String, Value>,
    /// `$schema`, when the schema declares its own dialect.
    pub dialect: Option<String>,
    /// `const`, which the model keeps only in its catch-all. Consumed
    /// by the discriminator proof; reported as unsupported otherwise.
    pub const_value: Option<Value>,
}

/// The translation of one document's components.
pub(crate) struct Front {
    pub components: BTreeMap<String, Slot>,
}

/// Keywords the catch-all may hold that mean something at every type
/// and need no report: annotations, and the reference machinery the
/// restriction report already covers.
const SILENT_KEYWORDS: &[&str] = &[
    "$comment",
    "$id",
    "$anchor",
    "$schema",
    "$defs",
    "definitions",
    "discriminator",
    "nullable",
    "const",
];

/// Keywords that make sense only under a given type. Used to read a
/// `type: [a, b]` schema once per member type without reporting `b`'s
/// keywords as unsupported under `a`.
fn keywords_for(schema_type: &SchemaType) -> &'static [&'static str] {
    match schema_type {
        SchemaType::String => &[
            "maxLength",
            "minLength",
            "pattern",
            "format",
            "enum",
            "default",
            "contentEncoding",
            "contentMediaType",
            "contentSchema",
        ],
        SchemaType::Integer | SchemaType::Number => &[
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
            "format",
            "enum",
            "default",
        ],
        SchemaType::Array => &[
            "items",
            "prefixItems",
            "minItems",
            "maxItems",
            "uniqueItems",
            "contains",
            "minContains",
            "maxContains",
            "unevaluatedItems",
            "default",
        ],
        SchemaType::Object => &[
            "properties",
            "patternProperties",
            "additionalProperties",
            "required",
            "minProperties",
            "maxProperties",
            "propertyNames",
            "unevaluatedProperties",
            "dependentRequired",
            "dependentSchemas",
            "default",
        ],
        SchemaType::Boolean | SchemaType::Null | SchemaType::Custom(_) => &["default", "enum"],
    }
}

const COMMON_KEYWORDS: &[&str] = &[
    "title",
    "description",
    "deprecated",
    "readOnly",
    "writeOnly",
    "examples",
    "example",
    "xml",
    "externalDocs",
    "type",
    "$comment",
    "$schema",
    "$id",
    "$anchor",
    "not",
    "allOf",
    "anyOf",
    "oneOf",
    "if",
    "then",
    "else",
    "const",
];

pub(crate) struct Translator<'a> {
    pub uri: &'a Url,
    /// The source as given, for the one thing the model cannot say:
    /// whether `type` was written.
    pub raw: &'a Value,
    /// Maps a 3.2 pointer to the source document's pointer.
    pub source_pointer: fn(&str) -> String,
}

impl Translator<'_> {
    pub(crate) fn translate(&self, spec: &roas::v3_2::spec::Spec) -> Front {
        let mut components = BTreeMap::new();
        if let Some(schemas) = spec.components.as_ref().and_then(|c| c.schemas.as_ref()) {
            for (name, slot) in schemas {
                let mut pointer = String::from("/components/schemas");
                push_token(&mut pointer, name);
                components.insert(name.clone(), self.slot(slot, pointer));
            }
        }
        Front { components }
    }

    fn id(&self, pointer: &str) -> SchemaId {
        SchemaId::new(self.uri.clone(), pointer)
    }

    fn slot(&self, slot: &RefOr<Schema, SchemaRef>, pointer: String) -> Slot {
        match slot {
            RefOr::Item(schema) => Slot::Inline(Box::new(self.schema(schema, pointer))),
            RefOr::Ref(reference) => self.reference(reference, pointer),
        }
    }

    fn reference(&self, reference: &SchemaRef, pointer: String) -> Slot {
        let target = match reference.reference.strip_prefix("#/components/schemas/") {
            Some(name) if !name.is_empty() && !name.contains('/') => {
                RefTarget::Component(unescape(name))
            }
            _ => RefTarget::Unsupported(reference.reference.clone()),
        };
        let asserting = reference.siblings.keys().any(|k| {
            !k.starts_with("x-")
                && !matches!(
                    k.as_str(),
                    "title"
                        | "description"
                        | "deprecated"
                        | "readOnly"
                        | "writeOnly"
                        | "example"
                        | "examples"
                        | "externalDocs"
                        | "xml"
                        | "$comment"
                )
        });
        let siblings = if asserting {
            // Read the siblings as a schema on their own. The plain parse
            // reads a typeless set as an object; whether `type` was
            // written is checked against the source, as everywhere.
            reference
                .siblings_schema()
                .ok()
                .flatten()
                .map(|schema| Box::new(self.schema(&schema, pointer.clone())))
        } else {
            None
        };
        Slot::Ref {
            id: self.id(&pointer),
            target,
            siblings,
            description: reference
                .description
                .clone()
                .or_else(|| reference.summary.clone()),
        }
    }

    fn type_written(&self, pointer: &str) -> bool {
        let source = (self.source_pointer)(pointer);
        self.raw
            .pointer(&source)
            .and_then(Value::as_object)
            .is_some_and(|object| object.contains_key("type"))
    }

    fn schema(&self, schema: &Schema, pointer: String) -> FrontSchema {
        match schema {
            Schema::Bool(true) | Schema::Empty(_) => self.leaf(pointer, Kind::Any),
            Schema::Bool(false) => self.leaf(pointer, Kind::Never),
            Schema::Not(not) => {
                let mut front = self.leaf(pointer, Kind::Not);
                front.unsupported = catch_all_keywords(&not.extensions, &mut front.extensions);
                front
            }
            Schema::AllOf(all) => self.all_of(all, pointer),
            Schema::AnyOf(any) => self.any_of(any, pointer),
            Schema::OneOf(one) => self.one_of(one, pointer),
            Schema::Multi(multi) => self.multi(multi, pointer),
            Schema::Single(single) => self.single(single, pointer),
        }
    }

    fn leaf(&self, pointer: String, kind: Kind) -> FrontSchema {
        FrontSchema {
            id: self.id(&pointer),
            kind,
            nullable: false,
            title: None,
            description: None,
            default: None,
            deprecated: false,
            read_only: false,
            write_only: false,
            constraints: Constraints::default(),
            unsupported: Vec::new(),
            extensions: BTreeMap::new(),
            dialect: None,
            const_value: None,
        }
    }

    fn branches(
        &self,
        slots: &[RefOr<Schema, SchemaRef>],
        pointer: &str,
        keyword: &str,
    ) -> Vec<Slot> {
        slots
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                let mut child = pointer.to_owned();
                push_token(&mut child, keyword);
                push_token(&mut child, &i.to_string());
                self.slot(slot, child)
            })
            .collect()
    }

    fn all_of(&self, all: &AllOfSchema, pointer: String) -> FrontSchema {
        let branches = self.branches(&all.all_of, &pointer, "allOf");
        let mut front = self.leaf(pointer, Kind::AllOf { branches });
        front.unsupported = catch_all_keywords(&all.extensions, &mut front.extensions);
        front.dialect = dialect_of(&all.extensions);
        front
    }

    fn any_of(&self, any: &AnyOfSchema, pointer: String) -> FrontSchema {
        let branches = self.branches(&any.any_of, &pointer, "anyOf");
        let mut front = self.leaf(pointer, Kind::AnyOf { branches });
        front.unsupported = catch_all_keywords(&any.extensions, &mut front.extensions);
        front.dialect = dialect_of(&any.extensions);
        front
    }

    fn one_of(&self, one: &OneOfSchema, pointer: String) -> FrontSchema {
        let branches = self.branches(&one.one_of, &pointer, "oneOf");
        let mut front = self.leaf(
            pointer,
            Kind::OneOf {
                branches,
                discriminator: one.discriminator.clone(),
            },
        );
        front.unsupported = catch_all_keywords(&one.extensions, &mut front.extensions);
        front.dialect = dialect_of(&one.extensions);
        front
    }

    /// `type: [...]`. The model keeps nothing but the type list and the
    /// annotations; every other keyword sits in its catch-all. So the
    /// schema is re-read once per non-null member type, with the
    /// keywords that apply to that type, and `null` becomes the
    /// nullable flag.
    fn multi(&self, multi: &MultiSchema, pointer: String) -> FrontSchema {
        let nullable = multi
            .schema_types
            .iter()
            .any(|t| matches!(t, SchemaType::Null));
        let members: Vec<&SchemaType> = multi
            .schema_types
            .iter()
            .filter(|t| !matches!(t, SchemaType::Null))
            .collect();
        let raw = serde_json::to_value(multi).unwrap_or(Value::Null);
        let object = raw.as_object().cloned().unwrap_or_default();
        let read = |schema_type: &SchemaType, restrict: bool| -> Option<FrontSchema> {
            let mut map = serde_json::Map::new();
            let applicable = keywords_for(schema_type);
            for (key, value) in &object {
                if key == "type" {
                    continue;
                }
                if restrict
                    && !applicable.contains(&key.as_str())
                    && !COMMON_KEYWORDS.contains(&key.as_str())
                    && !key.starts_with("x-")
                {
                    continue;
                }
                map.insert(key.clone(), value.clone());
            }
            map.insert("type".to_owned(), Value::String(schema_type.to_string()));
            let schema: Schema = serde_json::from_value(Value::Object(map)).ok()?;
            Some(self.schema(&schema, pointer.clone()))
        };
        let mut front = match members.as_slice() {
            [] => self.leaf(pointer.clone(), Kind::Null),
            [one] => read(one, false).unwrap_or_else(|| self.leaf(pointer.clone(), Kind::Any)),
            many => {
                let branches = many
                    .iter()
                    .filter_map(|t| read(t, true))
                    .map(|schema| Slot::Inline(Box::new(schema)))
                    .collect();
                let mut front = self.leaf(pointer.clone(), Kind::TypeUnion { branches });
                front.title = multi.title.clone();
                front.description = multi.description.clone();
                front.deprecated = multi.deprecated.unwrap_or(false);
                front.read_only = multi.read_only.unwrap_or(false);
                front.write_only = multi.write_only.unwrap_or(false);
                front
            }
        };
        // A member read on its own reports its own `type` as written.
        if let Kind::Object { typeless, .. } = &mut front.kind {
            *typeless = false;
        }
        front.nullable = nullable;
        front
    }

    fn single(&self, single: &SingleSchema, pointer: String) -> FrontSchema {
        match single {
            SingleSchema::String(s) => self.string(s, pointer),
            SingleSchema::Integer(s) => self.integer(s, pointer),
            SingleSchema::Number(s) => self.number(s, pointer),
            SingleSchema::Boolean(s) => self.boolean(s, pointer),
            SingleSchema::Null(s) => self.null(s, pointer),
            SingleSchema::Array(s) => self.array(s, pointer),
            SingleSchema::Object(s) => self.object(s, pointer),
        }
    }

    fn string(&self, s: &StringSchema, pointer: String) -> FrontSchema {
        let format = s.format.as_ref().map(|f| match f {
            StringFormat::Custom(custom) => custom.clone(),
            other => other.to_string(),
        });
        let mut front = self.leaf(
            pointer,
            Kind::String {
                format,
                enum_values: s.enum_values.clone(),
            },
        );
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.default = s.default.clone().map(Value::String);
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.constraints.min_length = s.min_length;
        front.constraints.max_length = s.max_length;
        front.constraints.pattern = s.pattern.clone();
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }

    fn integer(&self, s: &IntegerSchema, pointer: String) -> FrontSchema {
        let width = s.format.as_ref().map(|f| match f {
            IntegerFormat::Int32 => IntWidth::Int32,
            IntegerFormat::Int64 => IntWidth::Int64,
        });
        let mut front = self.leaf(
            pointer,
            Kind::Integer {
                width,
                enum_values: s.enum_values.clone(),
            },
        );
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.default = s.default.map(Value::from);
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.constraints.minimum = s.minimum.clone();
        front.constraints.maximum = s.maximum.clone();
        front.constraints.exclusive_minimum = s.exclusive_minimum.clone();
        front.constraints.exclusive_maximum = s.exclusive_maximum.clone();
        front.constraints.multiple_of = s.multiple_of.clone();
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }

    fn number(&self, s: &NumberSchema, pointer: String) -> FrontSchema {
        let width = s.format.as_ref().map(|f| match f {
            NumberFormat::Float => FloatWidth::Float,
            NumberFormat::Double => FloatWidth::Double,
        });
        let mut front = self.leaf(
            pointer,
            Kind::Number {
                width,
                enum_values: s.enum_values.clone(),
            },
        );
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.default = s.default.clone().map(Value::Number);
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.constraints.minimum = s.minimum.clone();
        front.constraints.maximum = s.maximum.clone();
        front.constraints.exclusive_minimum = s.exclusive_minimum.clone();
        front.constraints.exclusive_maximum = s.exclusive_maximum.clone();
        front.constraints.multiple_of = s.multiple_of.clone();
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }

    fn boolean(&self, s: &BooleanSchema, pointer: String) -> FrontSchema {
        let mut front = self.leaf(pointer, Kind::Boolean);
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.default = s.default.map(Value::Bool);
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }

    fn null(&self, s: &NullSchema, pointer: String) -> FrontSchema {
        let mut front = self.leaf(pointer, Kind::Null);
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }

    fn array(&self, s: &ArraySchema, pointer: String) -> FrontSchema {
        let items = match &s.items {
            None | Some(BoolOr::Bool(true)) => None,
            Some(BoolOr::Bool(false)) => {
                let mut child = pointer.clone();
                push_token(&mut child, "items");
                Some(Slot::Inline(Box::new(self.leaf(child, Kind::Never))))
            }
            Some(BoolOr::Item(slot)) => {
                let mut child = pointer.clone();
                push_token(&mut child, "items");
                Some(self.slot(slot, child))
            }
        };
        let mut front = self.leaf(pointer, Kind::Array { items });
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.default = s.default.clone().map(Value::Array);
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.constraints.min_items = s.min_items;
        front.constraints.max_items = s.max_items;
        front.constraints.unique_items = s.unique_items.unwrap_or(false);
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }

    fn object(&self, s: &ObjectSchema, pointer: String) -> FrontSchema {
        let mut properties = BTreeMap::new();
        if let Some(props) = &s.properties {
            for (name, slot) in props {
                let mut child = pointer.clone();
                push_token(&mut child, "properties");
                push_token(&mut child, name);
                properties.insert(name.clone(), self.slot(slot, child));
            }
        }
        let additional = match &s.additional_properties {
            None | Some(BoolOr::Bool(true)) => Additional::Any,
            Some(BoolOr::Bool(false)) => Additional::Denied,
            Some(BoolOr::Item(slot)) => {
                let mut child = pointer.clone();
                push_token(&mut child, "additionalProperties");
                Additional::Schema(self.slot(slot, child))
            }
        };
        let typeless = !self.type_written(&pointer);
        let mut front = self.leaf(
            pointer,
            Kind::Object {
                properties,
                required: s.required.clone().unwrap_or_default(),
                additional,
                typeless,
            },
        );
        front.title = s.title.clone();
        front.description = s.description.clone();
        front.default = s
            .default
            .clone()
            .map(|m| Value::Object(m.into_iter().collect()));
        front.deprecated = s.deprecated.unwrap_or(false);
        front.read_only = s.read_only.unwrap_or(false);
        front.write_only = s.write_only.unwrap_or(false);
        front.constraints.min_properties = s.min_properties;
        front.constraints.max_properties = s.max_properties;
        front.constraints.property_names = s.property_names.is_some();
        front.constraints.pattern_properties = s
            .pattern_properties
            .as_ref()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        front.unsupported = catch_all_keywords(&s.extensions, &mut front.extensions);
        if s.unevaluated_properties.is_some() {
            front.unsupported.push("unevaluatedProperties".to_owned());
        }
        front.dialect = dialect_of(&s.extensions);
        front.const_value = const_of(&s.extensions);
        front
    }
}

/// Split the model's catch-all into `x-` extensions, kept, and every
/// other keyword, returned for reporting.
fn catch_all_keywords(
    extensions: &Option<BTreeMap<String, Value>>,
    keep: &mut BTreeMap<String, Value>,
) -> Vec<String> {
    let mut unsupported = Vec::new();
    if let Some(extensions) = extensions {
        for (key, value) in extensions {
            if key.starts_with("x-") {
                keep.insert(key.clone(), value.clone());
            } else if !SILENT_KEYWORDS.contains(&key.as_str()) {
                unsupported.push(key.clone());
            }
        }
    }
    unsupported
}

fn const_of(extensions: &Option<BTreeMap<String, Value>>) -> Option<Value> {
    extensions.as_ref().and_then(|e| e.get("const")).cloned()
}

fn dialect_of(extensions: &Option<BTreeMap<String, Value>>) -> Option<String> {
    extensions
        .as_ref()
        .and_then(|e| e.get("$schema"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Undo RFC 6901 escaping in one reference token.
fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate(components: Value) -> Front {
        let raw = serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "t", "version": "1"},
            "paths": {},
            "components": {"schemas": components}
        });
        let spec: roas::v3_2::spec::Spec = serde_json::from_value(raw.clone()).unwrap();
        let uri = Url::parse("file:///t.json").unwrap();
        let translator = Translator {
            uri: &uri,
            raw: &raw,
            source_pointer: |p| p.to_owned(),
        };
        translator.translate(&spec)
    }

    fn inline(slot: &Slot) -> &FrontSchema {
        match slot {
            Slot::Inline(schema) => schema,
            Slot::Ref { .. } => panic!("expected inline"),
        }
    }

    #[test]
    fn nullable_multi_is_read_as_its_single_type() {
        let front = translate(serde_json::json!({
            "Tag": {"type": ["string", "null"], "maxLength": 5, "description": "d"}
        }));
        let tag = inline(&front.components["Tag"]);
        assert!(tag.nullable);
        assert!(matches!(tag.kind, Kind::String { .. }));
        assert_eq!(tag.constraints.max_length, Some(5));
        assert_eq!(tag.description.as_deref(), Some("d"));
        assert!(tag.unsupported.is_empty(), "{:?}", tag.unsupported);
    }

    #[test]
    fn multi_with_several_types_is_a_type_union_with_per_type_keywords() {
        let front = translate(serde_json::json!({
            "V": {"type": ["string", "integer"], "maxLength": 5, "minimum": 1}
        }));
        let v = inline(&front.components["V"]);
        let Kind::TypeUnion { branches } = &v.kind else {
            panic!("{:?}", v.kind)
        };
        assert_eq!(branches.len(), 2);
        let s = inline(&branches[0]);
        assert!(matches!(s.kind, Kind::String { .. }));
        assert_eq!(s.constraints.max_length, Some(5));
        assert!(s.unsupported.is_empty(), "{:?}", s.unsupported);
        let i = inline(&branches[1]);
        assert!(matches!(i.kind, Kind::Integer { .. }));
        assert_eq!(i.constraints.minimum, Some(serde_json::Number::from(1)));
        assert!(i.unsupported.is_empty(), "{:?}", i.unsupported);
    }

    #[test]
    fn typeless_is_detected_from_the_source() {
        let front = translate(serde_json::json!({
            "Loose": {"properties": {"a": {"type": "string"}}},
            "Tight": {"type": "object", "properties": {"a": {"type": "string"}}},
            "Anything": {"description": "no keywords"}
        }));
        let Kind::Object { typeless, .. } = inline(&front.components["Loose"]).kind else {
            panic!()
        };
        assert!(typeless);
        let Kind::Object { typeless, .. } = inline(&front.components["Tight"]).kind else {
            panic!()
        };
        assert!(!typeless);
        let Kind::Object {
            typeless,
            properties,
            ..
        } = &inline(&front.components["Anything"]).kind
        else {
            panic!()
        };
        assert!(*typeless && properties.is_empty());
    }

    #[test]
    fn unsupported_keywords_are_collected_and_extensions_kept() {
        let front = translate(serde_json::json!({
            "A": {"type": "array", "contains": {"type": "string"}, "minContains": 1, "x-mine": 1, "$comment": "c"}
        }));
        let a = inline(&front.components["A"]);
        assert_eq!(
            a.unsupported,
            vec!["contains".to_owned(), "minContains".to_owned()]
        );
        assert_eq!(a.extensions.get("x-mine"), Some(&Value::from(1)));
    }

    #[test]
    fn references_are_classified_and_siblings_kept() {
        let front = translate(serde_json::json!({
            "Pet": {"type": "object"},
            "Plain": {"$ref": "#/components/schemas/Pet", "description": "d"},
            "Narrow": {"$ref": "#/components/schemas/Pet", "maxLength": 3},
            "Elsewhere": {"$ref": "#/components/responses/X"}
        }));
        let Slot::Ref {
            target,
            siblings,
            description,
            ..
        } = &front.components["Plain"]
        else {
            panic!()
        };
        assert_eq!(*target, RefTarget::Component("Pet".into()));
        assert!(siblings.is_none());
        assert_eq!(description.as_deref(), Some("d"));
        let Slot::Ref { siblings, .. } = &front.components["Narrow"] else {
            panic!()
        };
        assert!(siblings.is_some());
        let Slot::Ref { target, .. } = &front.components["Elsewhere"] else {
            panic!()
        };
        assert!(matches!(target, RefTarget::Unsupported(_)));
    }

    #[test]
    fn additional_properties_forms() {
        let front = translate(serde_json::json!({
            "Open": {"type": "object"},
            "Closed": {"type": "object", "additionalProperties": false},
            "Typed": {"type": "object", "additionalProperties": {"type": "integer"}}
        }));
        let Kind::Object { additional, .. } = &inline(&front.components["Open"]).kind else {
            panic!()
        };
        assert!(matches!(additional, Additional::Any));
        let Kind::Object { additional, .. } = &inline(&front.components["Closed"]).kind else {
            panic!()
        };
        assert!(matches!(additional, Additional::Denied));
        let Kind::Object { additional, .. } = &inline(&front.components["Typed"]).kind else {
            panic!()
        };
        assert!(matches!(additional, Additional::Schema(_)));
    }
}
