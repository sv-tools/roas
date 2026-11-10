//! The normalized front-end representation: what a schema *says*,
//! with the OpenAPI model's shape translated away.
//!
//! Lowering works from this rather than from `roas::v3_2::Schema`
//! directly, so a second description language — AsyncAPI — needs a
//! second translation into it, not a second lowering.

use crate::diagnostic::{SchemaId, push_token, source_of, synthetic};
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
    /// Keywords neither the type nor a generated check enforces: a numeric
    /// `enum` and a `const` the type does not pin.
    pub unenforced: Vec<&'static str>,
    /// The inline `items` or `additionalProperties` schema says what no
    /// generated type carries for it: it is named here.
    pub inner_unchecked: Option<&'static str>,
    /// The schema's keywords the generator does not handle at all.
    pub unsupported: Vec<String>,
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
    /// The source is OpenAPI 2 or 3.0, whose `nullable` admits null even
    /// past an `enum`: those models cannot list null in one.
    pub legacy_nullable: bool,
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

    /// The keywords an OpenAPI 2.0 or 3.0 source wrote on this schema that
    /// its model dropped on the way to 3.2: unread, they are reported as
    /// unsupported rather than lost without a word.
    fn lost_in_conversion(&self, schema: &Schema, pointer: &str) -> Vec<String> {
        if !self.legacy_nullable {
            return Vec::new();
        }
        let source = (self.source_pointer)(&source_of(pointer));
        let Some(raw) = self.raw.pointer(&source).and_then(Value::as_object) else {
            return Vec::new();
        };
        let converted = serde_json::to_value(schema).unwrap_or(Value::Null);
        let kept = converted.as_object();
        let has = |key: &str| kept.is_some_and(|k| k.contains_key(key));
        raw.keys()
            .filter(|key| {
                let key = key.as_str();
                // Rewritten rather than lost: `nullable` into the type
                // list, `example` into `examples`, a boolean exclusive
                // bound into a numeric one. A 2.0 `discriminator` name is
                // dropped but asserts nothing; a `type` beside a
                // composition is dropped, and lost unless the branches
                // already say it.
                let rewritten = match key {
                    "nullable" | "x-nullable" | "example" | "discriminator" => true,
                    "type" => self.type_implied(raw),
                    "minimum" => has("exclusiveMinimum"),
                    "maximum" => has("exclusiveMaximum"),
                    // A boolean flag: `false` says what the inclusive bound
                    // already does, and `true` without a bound nothing.
                    "exclusiveMinimum" | "exclusiveMaximum" => raw[key].is_boolean(),
                    // The model reads `default: null` as no default.
                    "default" => raw[key].is_null(),
                    _ => false,
                };
                // An annotation asserts nothing: dropped, it changes no
                // instance's validity.
                let annotation = key.starts_with("x-")
                    || (COMMON_KEYWORDS.contains(&key) && key != "type")
                    || SILENT_KEYWORDS.contains(&key)
                    || matches!(key, "default" | "examples" | "$id" | "$anchor" | "$defs");
                !rewritten && !annotation && !has(key)
            })
            .cloned()
            .collect()
    }

    /// Whether a legacy composition's `type` is already said by its
    /// branches: by one of an `allOf`, by every one of an `anyOf` or a
    /// `oneOf`, directly or through a local `$ref`.
    fn type_implied(&self, raw: &serde_json::Map<String, Value>) -> bool {
        let Some(want) = raw.get("type") else {
            return true;
        };
        let typed = |branch: &Value| {
            let mut current = branch;
            for _ in 0..16 {
                if let Some(t) = current.get("type") {
                    return t == want;
                }
                match current
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| r.strip_prefix('#'))
                    .and_then(|p| self.raw.pointer(p))
                {
                    Some(next) => current = next,
                    None => return false,
                }
            }
            false
        };
        let branches = |key: &str| raw.get(key).and_then(Value::as_array);
        branches("allOf").is_some_and(|b| b.iter().any(typed))
            || branches("anyOf").is_some_and(|b| b.iter().all(typed))
            || branches("oneOf").is_some_and(|b| b.iter().all(typed))
    }

    fn type_written(&self, pointer: &str) -> bool {
        let source = (self.source_pointer)(&source_of(pointer));
        self.raw
            .pointer(&source)
            .and_then(Value::as_object)
            .is_some_and(|object| object.contains_key("type"))
    }

    fn schema(&self, schema: &Schema, pointer: String) -> FrontSchema {
        let lost = self.lost_in_conversion(schema, &pointer);
        let mut front = self.schema_kind(schema, pointer);
        for keyword in lost {
            if !front.unsupported.contains(&keyword) {
                front.unsupported.push(keyword);
            }
        }
        let numeric_enum = matches!(
            &front.kind,
            Kind::Integer {
                enum_values: Some(_),
                ..
            } | Kind::Number {
                enum_values: Some(_),
                ..
            }
        );
        // A required member with no property of its own lands in the
        // catch-all, where decoding does not ask for it.
        let undeclared = matches!(
            &front.kind,
            Kind::Object { properties, required, .. }
                if required.iter().any(|r| !properties.contains_key(r))
        );
        for (present, keyword) in [
            (numeric_enum, "enum"),
            (front.const_value.is_some(), "const"),
            (undeclared, "required"),
        ] {
            if present && !front.constraints.unenforced.contains(&keyword) {
                front.constraints.unenforced.push(keyword);
            }
        }
        front.constraints.inner_unchecked = inner_unchecked(&front);
        front.constraints.unsupported = front.unsupported.clone();
        front
    }

    fn schema_kind(&self, schema: &Schema, pointer: String) -> FrontSchema {
        match schema {
            Schema::Bool(true) | Schema::Empty(_) => self.leaf(pointer, Kind::Any),
            Schema::Bool(false) => self.leaf(pointer, Kind::Never),
            Schema::Not(not) => {
                let mut front = self.leaf(pointer, Kind::Not);
                front.unsupported = catch_all_keywords(&not.extensions, &mut front.extensions);
                front.const_value = const_of(&not.extensions);
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
        front.const_value = const_of(&all.extensions);
        front
    }

    fn any_of(&self, any: &AnyOfSchema, pointer: String) -> FrontSchema {
        let branches = self.branches(&any.any_of, &pointer, "anyOf");
        let mut front = self.leaf(pointer, Kind::AnyOf { branches });
        front.unsupported = catch_all_keywords(&any.extensions, &mut front.extensions);
        front.dialect = dialect_of(&any.extensions);
        front.const_value = const_of(&any.extensions);
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
        front.const_value = const_of(&one.extensions);
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
        let read_as = |schema_type: &SchemaType,
                       restrict: bool,
                       object: &serde_json::Map<String, Value>|
         -> Member {
            let mut map = serde_json::Map::new();
            let applicable = keywords_for(schema_type);
            for (key, value) in object {
                if key == "type" {
                    continue;
                }
                // `enum` and `const` assert on every member, whether its
                // type's schema models them or not.
                if restrict
                    && !applicable.contains(&key.as_str())
                    && !COMMON_KEYWORDS.contains(&key.as_str())
                    && !matches!(key.as_str(), "enum" | "const")
                    && !key.starts_with("x-")
                {
                    continue;
                }
                // The values the list shares among its members: each
                // member keeps those of its own type. An `enum` or `const`
                // with none admits no value of this type.
                let value = match (key.as_str(), value) {
                    ("enum", Value::Array(values)) => {
                        let own: Vec<Value> = values
                            .iter()
                            .filter(|v| fits(schema_type, v))
                            .cloned()
                            .collect();
                        if own.is_empty() {
                            return Member::Nothing;
                        }
                        Value::Array(own)
                    }
                    ("const", v) if !fits(schema_type, v) => return Member::Nothing,
                    ("default", v) if !fits(schema_type, v) => continue,
                    _ => value.clone(),
                };
                map.insert(key.clone(), value);
            }
            map.insert("type".to_owned(), Value::String(schema_type.to_string()));
            let Ok(schema) = serde_json::from_value::<Schema>(Value::Object(map)) else {
                return Member::Unread;
            };
            // Several members are several schemas, each with an identity of
            // its own; a lone member is the schema itself.
            let at = if restrict {
                synthetic(&pointer, &schema_type.to_string())
            } else {
                pointer.clone()
            };
            let mut member = self.schema(&schema, at);
            // The member's `type` was written, as part of the list.
            if let Kind::Object { typeless, .. } = &mut member.kind {
                *typeless = false;
            }
            Member::Read(Box::new(member))
        };
        let read = |schema_type: &SchemaType, restrict: bool| -> Member {
            match read_as(schema_type, restrict, &object) {
                // A `format` another member type gives is an annotation
                // for this one.
                Member::Unread if object.contains_key("format") => {
                    let mut without = object.clone();
                    without.remove("format");
                    read_as(schema_type, restrict, &without)
                }
                other => other,
            }
        };
        // Only the list's null is left: read as `type: null`, so what else
        // the list says still stands.
        let null_admitted = nullable && (self.legacy_nullable || shared_admits_null(&object));
        let null_only = || -> FrontSchema {
            // The `enum` and `const` were found to admit null;
            // what applies to other types does not apply here.
            let null_keywords = keywords_for(&SchemaType::Null);
            let only: serde_json::Map<String, Value> = object
                .iter()
                .filter(|(key, _)| {
                    let key = key.as_str();
                    let other_type = [
                        SchemaType::String,
                        SchemaType::Number,
                        SchemaType::Integer,
                        SchemaType::Object,
                        SchemaType::Array,
                        SchemaType::Boolean,
                    ]
                    .iter()
                    .any(|t| keywords_for(t).contains(&key));
                    !matches!(key, "enum" | "const")
                        && (null_keywords.contains(&key) || !other_type)
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            match read_as(&SchemaType::Null, false, &only) {
                Member::Read(member) => *member,
                _ => self.leaf(pointer.clone(), Kind::Null),
            }
        };
        let mut front = match members.as_slice() {
            [] if null_admitted => null_only(),
            [] => self.leaf(pointer.clone(), Kind::Never),
            [one] => match read(one, false) {
                Member::Read(member) => *member,
                Member::Nothing if null_admitted => null_only(),
                Member::Nothing => self.leaf(pointer.clone(), Kind::Never),
                // Not lost without a word: the schema is reported as
                // one the generator does not handle.
                Member::Unread => {
                    let mut front = self.leaf(pointer.clone(), Kind::Any);
                    front.unsupported.push("type".to_owned());
                    front
                }
            },
            many => {
                let branches: Vec<Slot> = many
                    .iter()
                    .filter_map(|t| match read(t, true) {
                        Member::Read(member) => Some(member),
                        Member::Nothing => None,
                        // Kept as a branch that proves nothing, so a value
                        // of its type is never taken to match no member.
                        Member::Unread => {
                            let mut member =
                                self.leaf(synthetic(&pointer, &t.to_string()), Kind::Any);
                            member.unsupported.push("type".to_owned());
                            member.constraints.unsupported = member.unsupported.clone();
                            Some(Box::new(member))
                        }
                    })
                    .map(Slot::Inline)
                    .collect();
                if branches.is_empty() && null_admitted {
                    let mut front = null_only();
                    front.nullable = true;
                    return front;
                }
                let mut front = self.leaf(pointer.clone(), Kind::TypeUnion { branches });
                // A keyword that applies to none of the member types is read
                // by no branch; report it on the union rather than lose it.
                front.unsupported = object
                    .keys()
                    .filter(|key| {
                        key.as_str() != "type"
                            && !key.starts_with("x-")
                            && !COMMON_KEYWORDS.contains(&key.as_str())
                            && !SILENT_KEYWORDS.contains(&key.as_str())
                            && !many.iter().any(|t| keywords_for(t).contains(&key.as_str()))
                    })
                    .cloned()
                    .collect();
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
        front.nullable = nullable && (self.legacy_nullable || shared_admits_null(&object));
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
        // A string `const` is a one-value enum: representing it that way
        // makes the generated type enforce it, which also keeps the pin
        // of a discriminated branch in the branch's own type.
        if let Some(Value::String(only)) = &front.const_value
            && let Kind::String { enum_values, .. } = &mut front.kind
            && enum_values.as_ref().is_none_or(|v| v.contains(only))
        {
            *enum_values = Some(vec![only.clone()]);
            front.const_value = None;
        }
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
/// The keyword whose inline schema says what no check carries: an inline
/// item or member value has no generated type of its own to check it.
fn inner_unchecked(schema: &FrontSchema) -> Option<&'static str> {
    let (keyword, inner) = match &schema.kind {
        Kind::Array {
            items: Some(Slot::Inline(inner)),
        } => ("items", inner),
        Kind::Object {
            additional: Additional::Schema(Slot::Inline(inner)),
            ..
        } => ("additionalProperties", inner),
        _ => return None,
    };
    // Generated as a type of its own, it checks itself: the lowerer's own
    // rule.
    let named = crate::lower::Lowerer::needs_name(inner);
    let says = !crate::checks::present_keywords(&inner.constraints).is_empty()
        || !inner.unsupported.is_empty()
        || inner.constraints.inner_unchecked.is_some();
    (!named && says).then_some(keyword)
}

/// Whether a type list's shared `enum` and `const` allow null: without
/// it, they leave it out whatever the list says.
fn shared_admits_null(object: &serde_json::Map<String, Value>) -> bool {
    let admits = |key: &str| match object.get(key) {
        Some(Value::Array(values)) if key == "enum" => values.iter().any(Value::is_null),
        Some(value) if key == "const" => value.is_null(),
        _ => true,
    };
    admits("enum") && admits("const")
}

/// A member of a type list, read as a schema of its own.
enum Member {
    Read(Box<FrontSchema>),
    /// Its `enum` or `const` admits no value of its type.
    Nothing,
    /// Its keywords do not make a schema of its type.
    Unread,
}

/// Whether `value` is an instance of `schema_type`.
fn fits(schema_type: &SchemaType, value: &Value) -> bool {
    match schema_type {
        SchemaType::String => value.is_string(),
        SchemaType::Number => value.is_number(),
        SchemaType::Integer => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        SchemaType::Object => value.is_object(),
        SchemaType::Array => value.is_array(),
        SchemaType::Boolean => value.is_boolean(),
        SchemaType::Null => value.is_null(),
        SchemaType::Custom(_) => true,
    }
}

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
            legacy_nullable: false,
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
