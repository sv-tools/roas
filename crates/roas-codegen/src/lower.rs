//! Front-end representation → IR: hoisting, naming, composition
//! classes, nullability, cycles, and every diagnostic the schema earns.

use crate::config::Config;
use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
use crate::front::{
    Additional as FrontAdditional, FloatWidth, Front, FrontSchema, IntWidth, Kind, RefTarget, Slot,
};
use crate::ir::{
    Additional, Branch, Field, Ir, OpenUnion, StructDef, TaggedUnion, TypeDef, TypeKind, TypeRef,
    Variant,
};
use roas::v3_2::discriminator::Discriminator;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Dialects whose keywords mean what this generator assumes.
const KNOWN_DIALECTS: &[&str] = &[
    "https://json-schema.org/draft/2020-12/schema",
    "https://spec.openapis.org/oas/3.1/dialect/base",
    "https://spec.openapis.org/oas/3.2/dialect/base",
    "https://spec.openapis.org/oas/3.1/dialect/2024-11-10",
    "https://spec.openapis.org/oas/3.2/dialect/2025-09-17",
];

pub(crate) fn is_known_dialect(dialect: &str) -> bool {
    KNOWN_DIALECTS.contains(&dialect.trim_end_matches('#'))
}

/// What a position resolved to: the type, and whether `null` is
/// permitted there — which is the schema's business, not the type's.
struct Lowered {
    ty: TypeRef,
    nullable: bool,
}

pub(crate) struct Lowerer<'a> {
    front: &'a Front,
    config: &'a Config,
    ir: Ir,
    /// Component pointer ids, so a `$ref` resolves without lowering.
    component_ids: BTreeMap<String, SchemaId>,
    /// The named types being lowered, innermost last; an error inside
    /// one marks it.
    owners: Vec<SchemaId>,
}

pub(crate) fn lower(front: &Front, config: &Config) -> Ir {
    let mut lowerer = Lowerer {
        front,
        config,
        ir: Ir::default(),
        owners: Vec::new(),
        component_ids: front
            .components
            .iter()
            .map(|(name, slot)| (name.clone(), slot.id().clone()))
            .collect(),
    };
    lowerer.ir.components = lowerer.component_ids.clone();
    for (name, slot) in &front.components {
        lowerer.component(name, slot);
    }
    lowerer.mark_cycles();
    lowerer.suppress();
    lowerer.ir
}

impl<'a> Lowerer<'a> {
    fn diagnose(
        &mut self,
        severity: Severity,
        kind: DiagnosticKind,
        at: &SchemaId,
        pointer: Option<&str>,
        message: String,
    ) {
        self.ir.diagnostics.push(Diagnostic {
            severity,
            kind,
            schema_id: at.clone(),
            pointer: pointer
                .map(str::to_owned)
                .unwrap_or_else(|| at.pointer.clone()),
            message,
        });
    }

    fn error(&mut self, kind: DiagnosticKind, at: &SchemaId, message: String) {
        self.diagnose(Severity::Error, kind, at, None, message);
        self.mark_owner();
    }

    /// The innermost named type being lowered holds an error and is
    /// not emitted.
    fn mark_owner(&mut self) {
        if let Some(owner) = self.owners.last().cloned()
            && let Some(def) = self.ir.types.get_mut(&owner)
        {
            def.has_error = true;
        }
    }

    fn warn(&mut self, kind: DiagnosticKind, at: &SchemaId, message: String) {
        self.diagnose(Severity::Warning, kind, at, None, message);
    }

    fn register(&mut self, def: TypeDef) {
        let id = def.id.clone();
        if !self.ir.types.contains_key(&id) {
            self.ir.order.push(id.clone());
        }
        self.ir.types.insert(id, def);
    }

    // ── components ──────────────────────────────────────────────────

    fn component(&mut self, name: &str, slot: &Slot) {
        let id = slot.id().clone();
        if self.config.substitutions.contains_key(name) {
            let (title, description) = match slot {
                Slot::Inline(schema) => (schema.title.clone(), schema.description.clone()),
                Slot::Ref { description, .. } => (None, description.clone()),
            };
            self.register(TypeDef {
                id,
                suggested_name: vec![name.to_owned()],
                component: Some(name.to_owned()),
                title,
                description,
                deprecated: false,
                nullable: false,
                constraints: Default::default(),
                extensions: BTreeMap::new(),
                kind: TypeKind::Extern,
                has_error: false,
            });
            return;
        }
        match slot {
            Slot::Inline(schema) => {
                let name_parts = vec![name.to_owned()];
                self.hoist(schema, name_parts, Some(name.to_owned()));
            }
            Slot::Ref {
                id,
                target,
                siblings,
                description,
            } => {
                let lowered = self.reference(id, target, siblings.as_deref(), &[name.to_owned()]);
                let kind = match &lowered.ty {
                    TypeRef::Named(_) if siblings.is_none() => TypeKind::Alias(lowered.ty.clone()),
                    _ => TypeKind::Newtype(lowered.ty.clone()),
                };
                let has_error = matches!(target, RefTarget::Unsupported(_));
                self.register(TypeDef {
                    id: id.clone(),
                    suggested_name: vec![name.to_owned()],
                    component: Some(name.to_owned()),
                    title: None,
                    description: description.clone(),
                    deprecated: false,
                    nullable: lowered.nullable,
                    constraints: Default::default(),
                    extensions: BTreeMap::new(),
                    kind,
                    has_error,
                });
            }
        }
    }

    // ── positions ───────────────────────────────────────────────────

    /// Lower a slot in some position, hoisting a named type when the
    /// schema needs one and returning what the position holds.
    fn position(&mut self, slot: &Slot, hint: &[String]) -> Lowered {
        match slot {
            Slot::Ref {
                id,
                target,
                siblings,
                ..
            } => self.reference(id, target, siblings.as_deref(), hint),
            Slot::Inline(schema) => self.inline(schema, hint),
        }
    }

    fn reference(
        &mut self,
        id: &SchemaId,
        target: &RefTarget,
        siblings: Option<&FrontSchema>,
        hint: &[String],
    ) -> Lowered {
        let target_id = match target {
            RefTarget::Component(name) => match self.component_ids.get(name) {
                Some(target_id) => target_id.clone(),
                None => {
                    self.error(
                        DiagnosticKind::UnsupportedReferenceTarget {
                            reference: format!("#/components/schemas/{name}"),
                        },
                        id,
                        format!("`$ref` names the component schema `{name}`, which does not exist"),
                    );
                    return Lowered {
                        ty: TypeRef::Any,
                        nullable: false,
                    };
                }
            },
            RefTarget::Unsupported(reference) => {
                // An external reference and a plain-name anchor were
                // already reported before lowering; a local pointer to
                // something other than a component schema was not.
                if reference.starts_with("#/") {
                    self.error(
                        DiagnosticKind::UnsupportedReferenceTarget { reference: reference.clone() },
                        id,
                        format!("`$ref: {reference}` is not a component schema reference; only `#/components/schemas/<name>` is followed"),
                    );
                } else {
                    self.mark_owner();
                }
                return Lowered {
                    ty: TypeRef::Any,
                    nullable: false,
                };
            }
        };
        let nullable = self.component_nullable(target);
        let Some(siblings) = siblings else {
            return Lowered {
                ty: TypeRef::Named(target_id),
                nullable,
            };
        };
        // `$ref` with asserting siblings is an intersection of the
        // target and the siblings: the same shape as `allOf`, and it
        // lowers the same way — an open holder with two projections,
        // since the target's shape is not inspected here.
        let mut sibling_hint = hint.to_vec();
        sibling_hint.push("constraints".to_owned());
        let projection = self.position(&Slot::Inline(Box::new(siblings.clone())), &sibling_hint);
        self.warn(
            DiagnosticKind::OpenComposition { keyword: "$ref".to_owned() },
            id,
            "`$ref` with sibling keywords is the intersection of both; it is generated as an open holder with a projection for each".to_owned(),
        );
        let mut target_hint = hint.to_vec();
        target_hint.push("target".to_owned());
        self.register(TypeDef {
            id: id.clone(),
            suggested_name: hint.to_vec(),
            component: None,
            title: siblings.title.clone(),
            description: siblings.description.clone(),
            deprecated: siblings.deprecated,
            nullable: nullable || siblings.nullable,
            constraints: siblings.constraints.clone(),
            extensions: siblings.extensions.clone(),
            kind: TypeKind::ComposedOpen(vec![
                Branch {
                    suggested_name: target_hint,
                    ty: TypeRef::Named(target_id),
                },
                Branch {
                    suggested_name: sibling_hint,
                    ty: projection.ty,
                },
            ]),
            has_error: false,
        });
        Lowered {
            ty: TypeRef::Named(id.clone()),
            nullable: nullable || siblings.nullable,
        }
    }

    /// Whether a component permits `null`, following bare references.
    fn component_nullable(&self, target: &RefTarget) -> bool {
        let mut seen = BTreeSet::new();
        let mut current = target;
        loop {
            let RefTarget::Component(name) = current else {
                return false;
            };
            if !seen.insert(name.clone()) {
                return false;
            }
            match self.front.components.get(name) {
                Some(Slot::Inline(schema)) => return schema.nullable,
                Some(Slot::Ref {
                    target, siblings, ..
                }) => {
                    if let Some(siblings) = siblings
                        && siblings.nullable
                    {
                        return true;
                    }
                    current = target;
                }
                None => return false,
            }
        }
    }

    /// Whether a schema is generated as a type of its own rather than
    /// spelled in place.
    fn needs_name(schema: &FrontSchema) -> bool {
        match &schema.kind {
            Kind::String {
                enum_values: Some(_),
                ..
            } => true,
            Kind::Object {
                properties,
                required,
                additional,
                typeless,
            } => {
                let keyworded = !properties.is_empty()
                    || !required.is_empty()
                    || !matches!(additional, FrontAdditional::Any);
                if *typeless {
                    keyworded
                } else {
                    !properties.is_empty()
                        || !required.is_empty()
                        || matches!(additional, FrontAdditional::Denied)
                }
            }
            Kind::AllOf { .. }
            | Kind::AnyOf { .. }
            | Kind::OneOf { .. }
            | Kind::TypeUnion { .. } => true,
            _ => false,
        }
    }

    fn inline(&mut self, schema: &FrontSchema, hint: &[String]) -> Lowered {
        if Self::needs_name(schema) {
            return self.hoist_lowered(schema, hint);
        }
        self.report_keywords(schema, None);
        let nullable = schema.nullable;
        let ty = match &schema.kind {
            Kind::Any => TypeRef::Any,
            Kind::Never => {
                self.error(
                    DiagnosticKind::Ungeneratable {
                        keyword: "false".into(),
                    },
                    &schema.id,
                    "the schema `false` admits no value, so no type can represent it".into(),
                );
                TypeRef::Any
            }
            Kind::Not => {
                self.error(
                    DiagnosticKind::Ungeneratable {
                        keyword: "not".into(),
                    },
                    &schema.id,
                    "`not` describes what a value is not, which no type can represent".into(),
                );
                TypeRef::Any
            }
            Kind::String {
                format,
                enum_values,
            } => {
                if enum_values.is_some() {
                    return self.hoist_lowered(schema, hint);
                }
                match format {
                    Some(format) if self.config.substitutions.contains_key(format) => {
                        TypeRef::Extern(format.clone())
                    }
                    _ => TypeRef::String,
                }
            }
            Kind::Integer { width, enum_values } => {
                self.number_diagnostics(schema, true);
                if enum_values.is_some() {
                    self.warn(DiagnosticKind::NumericEnum, &schema.id, "`enum` over integers is not represented in the type; the values are listed in its documentation".into());
                }
                TypeRef::Integer(width.unwrap_or(IntWidth::Int64))
            }
            Kind::Number { width, enum_values } => {
                self.number_diagnostics(schema, false);
                if enum_values.is_some() {
                    self.warn(DiagnosticKind::NumericEnum, &schema.id, "`enum` over numbers is not represented in the type; the values are listed in its documentation".into());
                }
                TypeRef::Number(width.unwrap_or(FloatWidth::Double))
            }
            Kind::Boolean => TypeRef::Boolean,
            Kind::Null => TypeRef::Null,
            Kind::Array { items } => {
                let mut item_hint = hint.to_vec();
                item_hint.push("item".to_owned());
                let item = match items {
                    Some(slot) => self.position(slot, &item_hint),
                    None => Lowered {
                        ty: TypeRef::Any,
                        nullable: false,
                    },
                };
                TypeRef::Array(Box::new(wrap_nullable(item)))
            }
            Kind::Object {
                properties,
                required,
                additional,
                typeless,
            } => {
                let keyworded = !properties.is_empty()
                    || !required.is_empty()
                    || !matches!(additional, FrontAdditional::Any);
                if *typeless && !keyworded {
                    TypeRef::Any
                } else if properties.is_empty() && required.is_empty() && !*typeless {
                    match additional {
                        FrontAdditional::Any => TypeRef::Map(Box::new(TypeRef::Any)),
                        FrontAdditional::Schema(slot) => {
                            let mut value_hint = hint.to_vec();
                            value_hint.push("value".to_owned());
                            let value = self.position(slot, &value_hint);
                            TypeRef::Map(Box::new(wrap_nullable(value)))
                        }
                        FrontAdditional::Denied => return self.hoist_lowered(schema, hint),
                    }
                } else {
                    return self.hoist_lowered(schema, hint);
                }
            }
            Kind::AllOf { .. }
            | Kind::AnyOf { .. }
            | Kind::OneOf { .. }
            | Kind::TypeUnion { .. } => {
                return self.hoist_lowered(schema, hint);
            }
        };
        Lowered { ty, nullable }
    }

    fn hoist_lowered(&mut self, schema: &FrontSchema, hint: &[String]) -> Lowered {
        let name = match &schema.title {
            Some(title) if !title.trim().is_empty() => vec![title.clone()],
            _ => hint.to_vec(),
        };
        self.hoist(schema, name, None);
        Lowered {
            ty: TypeRef::Named(schema.id.clone()),
            nullable: schema.nullable,
        }
    }

    // ── named types ─────────────────────────────────────────────────

    /// Give a schema a named type of its own.
    fn hoist(&mut self, schema: &FrontSchema, name: Vec<String>, component: Option<String>) {
        if self.ir.types.contains_key(&schema.id) {
            return;
        }
        // Reserve the id first so a cycle back to it resolves.
        self.register(TypeDef {
            id: schema.id.clone(),
            suggested_name: name.clone(),
            component: component.clone(),
            title: schema.title.clone(),
            description: schema.description.clone(),
            deprecated: schema.deprecated,
            nullable: schema.nullable,
            constraints: schema.constraints.clone(),
            extensions: schema.extensions.clone(),
            kind: TypeKind::Extern,
            has_error: false,
        });
        self.owners.push(schema.id.clone());
        let named = Self::needs_name(schema);
        if named {
            self.report_keywords(schema, None);
        }
        let kind = match &schema.kind {
            Kind::Object { typeless: true, .. } if named => {
                self.warn(DiagnosticKind::TypelessObject, &schema.id, "no `type` is declared, so every instance type is permitted; the object keywords are generated as a projection of an open holder".into());
                let mut object_hint = name.clone();
                object_hint.push("object".to_owned());
                let projection = self.object_projection(schema, &object_hint);
                TypeKind::ComposedOpen(vec![Branch {
                    suggested_name: object_hint,
                    ty: TypeRef::Named(projection),
                }])
            }
            Kind::Object { .. } if named => TypeKind::Struct(self.object(schema, &name)),
            Kind::String {
                enum_values: Some(values),
                ..
            } => TypeKind::StringEnum(values.clone()),
            Kind::AllOf { branches } => self.all_of(schema, branches, &name),
            Kind::OneOf {
                branches,
                discriminator,
            } => self.one_of(schema, branches, discriminator.as_ref(), &name),
            Kind::AnyOf { branches } => TypeKind::UnionOpen(OpenUnion {
                branches: self.branches(branches, &name, "variant"),
                exclusive: false,
            }),
            Kind::TypeUnion { branches } => TypeKind::UnionOpen(OpenUnion {
                branches: self.type_branches(branches, &name),
                exclusive: true,
            }),
            _ => {
                // A scalar, array, map or any reaches here only as a
                // component: a newtype over what it would be in place.
                TypeKind::Newtype(self.inline(schema, &name).ty)
            }
        };
        self.owners.pop();
        if let Some(def) = self.ir.types.get_mut(&schema.id) {
            def.kind = kind;
        }
    }

    /// A typeless schema's object keywords as a struct of their own.
    fn object_projection(&mut self, schema: &FrontSchema, hint: &[String]) -> SchemaId {
        let mut id = schema.id.clone();
        id.pointer.push_str("/(object)");
        let def = TypeDef {
            id: id.clone(),
            suggested_name: hint.to_vec(),
            component: None,
            title: None,
            description: None,
            deprecated: false,
            nullable: false,
            constraints: schema.constraints.clone(),
            extensions: BTreeMap::new(),
            kind: TypeKind::Extern,
            has_error: false,
        };
        self.register(def);
        self.owners.push(id.clone());
        let object = self.object(schema, hint);
        self.owners.pop();
        if let Some(def) = self.ir.types.get_mut(&id) {
            def.kind = TypeKind::Struct(object);
        }
        id
    }

    fn object(&mut self, schema: &FrontSchema, name: &[String]) -> StructDef {
        let Kind::Object {
            properties,
            required,
            additional,
            ..
        } = &schema.kind
        else {
            unreachable!("object() on a non-object");
        };
        let mut fields = Vec::new();
        for (raw_name, slot) in properties {
            let mut hint = name.to_vec();
            hint.push(raw_name.clone());
            let lowered = self.position(slot, &hint);
            let (description, default, deprecated, read_only, write_only, constraints, extensions) =
                match slot {
                    Slot::Inline(s) => (
                        s.description.clone(),
                        s.default.clone(),
                        s.deprecated,
                        s.read_only,
                        s.write_only,
                        s.constraints.clone(),
                        s.extensions.clone(),
                    ),
                    Slot::Ref {
                        description,
                        siblings,
                        ..
                    } => (
                        description
                            .clone()
                            .or_else(|| siblings.as_ref().and_then(|s| s.description.clone())),
                        None,
                        false,
                        false,
                        false,
                        Default::default(),
                        BTreeMap::new(),
                    ),
                };
            fields.push(Field {
                raw_name: raw_name.clone(),
                ty: lowered.ty,
                required: required.contains(raw_name),
                nullable: lowered.nullable,
                description,
                default,
                deprecated,
                read_only,
                write_only,
                constraints,
                extensions,
            });
        }
        for missing in required.iter().filter(|r| !properties.contains_key(*r)) {
            self.warn(
                DiagnosticKind::UnsupportedKeyword { keyword: "required".into() },
                &schema.id,
                format!("`required` names `{missing}`, which `properties` does not declare; the requirement is not enforced"),
            );
        }
        let additional = match additional {
            FrontAdditional::Denied => Additional::Denied,
            FrontAdditional::Any => Additional::CatchAll(TypeRef::Any),
            FrontAdditional::Schema(slot) => {
                let mut hint = name.to_vec();
                hint.push("value".to_owned());
                let value = self.position(slot, &hint);
                Additional::CatchAll(wrap_nullable(value))
            }
        };
        if !schema.constraints.pattern_properties.is_empty() {
            self.warn(
                DiagnosticKind::UnsupportedKeyword { keyword: "patternProperties".into() },
                &schema.id,
                "`patternProperties` is not represented in the type; matching members land in the catch-all".into(),
            );
        }
        StructDef { fields, additional }
    }

    fn branches(&mut self, slots: &[Slot], name: &[String], word: &str) -> Vec<Branch> {
        slots
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                let mut hint = name.to_vec();
                hint.push(format!("{word} {}", i + 1));
                let suggested_name = self.branch_name(slot, &hint);
                let lowered = self.position(slot, &hint);
                Branch {
                    suggested_name,
                    ty: wrap_nullable(lowered),
                }
            })
            .collect()
    }

    fn type_branches(&mut self, slots: &[Slot], name: &[String]) -> Vec<Branch> {
        slots
            .iter()
            .map(|slot| {
                let word = match slot {
                    Slot::Inline(schema) => kind_word(&schema.kind),
                    Slot::Ref { .. } => "ref",
                };
                let mut hint = name.to_vec();
                hint.push(word.to_owned());
                let lowered = self.position(slot, &hint);
                Branch {
                    suggested_name: hint,
                    ty: wrap_nullable(lowered),
                }
            })
            .collect()
    }

    /// What to call a branch: the component it references, its title,
    /// or its position.
    fn branch_name(&self, slot: &Slot, hint: &[String]) -> Vec<String> {
        match slot {
            Slot::Ref {
                target: RefTarget::Component(name),
                ..
            } => vec![name.clone()],
            Slot::Inline(schema) => match &schema.title {
                Some(title) if !title.trim().is_empty() => vec![title.clone()],
                _ => hint.to_vec(),
            },
            Slot::Ref { .. } => hint.to_vec(),
        }
    }

    // ── composition ─────────────────────────────────────────────────

    fn all_of(&mut self, schema: &FrontSchema, branches: &[Slot], name: &[String]) -> TypeKind {
        if let Some(merged) = self.safe_all_of(branches) {
            let mut fields = Vec::new();
            let mut seen = BTreeSet::new();
            for (branch, branch_slot) in merged {
                let branch_name = match branch_slot {
                    Slot::Ref {
                        target: RefTarget::Component(component),
                        ..
                    } => vec![component.clone()],
                    _ => name.to_vec(),
                };
                let def = self.object(branch, &branch_name);
                for field in def.fields {
                    if seen.insert(field.raw_name.clone()) {
                        fields.push(field);
                    }
                }
            }
            return TypeKind::Struct(StructDef {
                fields,
                additional: Additional::CatchAll(TypeRef::Any),
            });
        }
        self.warn(
            DiagnosticKind::OpenComposition { keyword: "allOf".into() },
            &schema.id,
            "`allOf` cannot be proven safe to flatten — every branch must be an object with disjoint, self-declared properties and no composition or extra member constraints — so it is generated as an open holder with a projection per branch".into(),
        );
        TypeKind::ComposedOpen(self.branches(branches, name, "part"))
    }

    /// The branches of an `allOf` that can be merged into one struct,
    /// each resolved to its object schema — or `None`.
    fn safe_all_of<'s>(&self, branches: &'s [Slot]) -> Option<Vec<(&'s FrontSchema, &'s Slot)>>
    where
        'a: 's,
    {
        let mut resolved = Vec::new();
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for slot in branches {
            let schema = self.resolve_object(slot)?;
            let Kind::Object {
                properties,
                required,
                additional,
                typeless,
            } = &schema.kind
            else {
                return None;
            };
            if *typeless || !matches!(additional, FrontAdditional::Any) || schema.nullable {
                return None;
            }
            if !schema.constraints.pattern_properties.is_empty()
                || schema.constraints.property_names
                || schema.constraints.min_properties.is_some()
                || schema.constraints.max_properties.is_some()
                || schema
                    .unsupported
                    .iter()
                    .any(|k| k == "unevaluatedProperties")
            {
                return None;
            }
            if required.iter().any(|r| !properties.contains_key(r)) {
                return None;
            }
            for name in properties.keys() {
                if !names.insert(name.as_str()) {
                    return None;
                }
            }
            resolved.push((schema, slot));
        }
        Some(resolved)
    }

    /// A bare reference to a component, or an inline schema, as the
    /// object schema it is — or `None` when it is neither an object nor
    /// resolvable.
    fn resolve_object<'s>(&self, slot: &'s Slot) -> Option<&'s FrontSchema>
    where
        'a: 's,
    {
        let mut seen = BTreeSet::new();
        let mut current = slot;
        loop {
            match current {
                Slot::Inline(schema) => {
                    return matches!(schema.kind, Kind::Object { .. }).then_some(schema);
                }
                Slot::Ref {
                    target: RefTarget::Component(name),
                    siblings: None,
                    ..
                } => {
                    if !seen.insert(name.clone()) {
                        return None;
                    }
                    current = self.front.components.get(name)?;
                }
                Slot::Ref { .. } => return None,
            }
        }
    }

    fn one_of(
        &mut self,
        schema: &FrontSchema,
        branches: &[Slot],
        discriminator: Option<&Discriminator>,
        name: &[String],
    ) -> TypeKind {
        if let Some(discriminator) = discriminator
            && let Some(variants) = self.safe_one_of(branches, discriminator)
        {
            let mut out = Vec::new();
            for (value, slot) in variants {
                let mut hint = name.to_vec();
                hint.push(value.clone());
                let suggested_name = self.branch_name(slot, &hint);
                let lowered = self.position(slot, &hint);
                out.push(Variant {
                    value,
                    ty: lowered.ty,
                    suggested_name,
                });
            }
            return TypeKind::UnionTagged(TaggedUnion {
                tag: discriminator.property_name.clone(),
                variants: out,
            });
        }
        let reason = match discriminator {
            None => "it has no discriminator",
            Some(d) if d.default_mapping.is_some() => "its discriminator has a `defaultMapping`",
            Some(_) => {
                "the discriminator does not prove which branch a value belongs to: every branch must be a component object schema that requires the discriminator property and pins it with `const` or a single-value `enum`, and `mapping`, if present, must cover every branch"
            }
        };
        self.warn(
            DiagnosticKind::OpenComposition { keyword: "oneOf".into() },
            &schema.id,
            format!("`oneOf` cannot be generated as a tagged enum because {reason}; it is generated as an open holder with typed accessors, never first-match-wins"),
        );
        TypeKind::UnionOpen(OpenUnion {
            branches: self.branches(branches, name, "variant"),
            exclusive: true,
        })
    }

    /// The tag value of every branch, when tagged decoding provably
    /// selects the same branch validation would.
    fn safe_one_of<'s>(
        &self,
        branches: &'s [Slot],
        discriminator: &Discriminator,
    ) -> Option<Vec<(String, &'s Slot)>>
    where
        'a: 's,
    {
        if discriminator.default_mapping.is_some() {
            return None;
        }
        // mapping: value -> component name
        let mut by_component: BTreeMap<String, String> = BTreeMap::new();
        if let Some(mapping) = &discriminator.mapping {
            for (value, target) in mapping {
                let component = target
                    .strip_prefix("#/components/schemas/")
                    .unwrap_or(target);
                if by_component
                    .insert(component.to_owned(), value.clone())
                    .is_some()
                {
                    return None;
                }
            }
        }
        let mut out = Vec::new();
        let mut values = BTreeSet::new();
        for slot in branches {
            let Slot::Ref {
                target: RefTarget::Component(component),
                siblings: None,
                ..
            } = slot
            else {
                return None;
            };
            let value = match by_component.get(component) {
                Some(value) => value.clone(),
                None if discriminator.mapping.is_some() => return None,
                None => component.clone(),
            };
            let schema = self.resolve_object(slot)?;
            let Kind::Object {
                properties,
                required,
                typeless: false,
                ..
            } = &schema.kind
            else {
                return None;
            };
            if !required.contains(&discriminator.property_name) {
                return None;
            }
            let Some(Slot::Inline(tag)) = properties.get(&discriminator.property_name) else {
                return None;
            };
            let pinned = match (&tag.const_value, &tag.kind) {
                (Some(Value::String(c)), _) => c == &value,
                (
                    None,
                    Kind::String {
                        enum_values: Some(one),
                        ..
                    },
                ) if one.len() == 1 => one[0] == value,
                _ => false,
            };
            if !pinned || !values.insert(value.clone()) {
                return None;
            }
            out.push((value, slot));
        }
        Some(out)
    }

    // ── diagnostics ─────────────────────────────────────────────────

    fn report_keywords(&mut self, schema: &FrontSchema, tag_consumed: Option<&str>) {
        let _ = tag_consumed;
        for keyword in &schema.unsupported {
            self.warn(
                DiagnosticKind::UnsupportedKeyword { keyword: keyword.clone() },
                &schema.id,
                format!("`{keyword}` is not handled by the generator; it is neither represented in the type nor validated"),
            );
        }
        if schema.const_value.is_some() && !self.const_is_tag(schema) {
            self.warn(
                DiagnosticKind::UnsupportedKeyword {
                    keyword: "const".into(),
                },
                &schema.id,
                "`const` is not represented in the type; the value is not enforced".into(),
            );
        }
        if let Some(dialect) = &schema.dialect
            && !is_known_dialect(dialect)
        {
            self.error(
                DiagnosticKind::UnsupportedDialect { dialect: dialect.clone() },
                &schema.id,
                format!("`$schema: {dialect}` is not a dialect this generator understands; only the OAS dialect and Draft 2020-12 are"),
            );
        }
    }

    /// A `const` on a required string property is how a discriminated
    /// branch pins its tag; that use is represented, by the union.
    fn const_is_tag(&self, schema: &FrontSchema) -> bool {
        matches!(
            (&schema.kind, &schema.const_value),
            (Kind::String { .. }, Some(Value::String(_)))
        )
    }

    fn number_diagnostics(&mut self, schema: &FrontSchema, integer: bool) {
        let c = &schema.constraints;
        let bounded_below = c.minimum.is_some() || c.exclusive_minimum.is_some();
        let bounded_above = c.maximum.is_some() || c.exclusive_maximum.is_some();
        if integer && !(bounded_below && bounded_above) {
            self.warn(
                DiagnosticKind::LossyNumber,
                &schema.id,
                "`integer` is unbounded in JSON Schema and is mapped to a fixed-width integer; values outside its range are rejected at decode".into(),
            );
        }
        if !integer && c.multiple_of.is_some() {
            self.warn(
                DiagnosticKind::LossyNumber,
                &schema.id,
                "`multipleOf` on a floating-point number cannot be checked exactly on the mapped type".into(),
            );
        }
    }

    // ── cycles and suppression ──────────────────────────────────────

    /// Tarjan's strongly connected components over direct containment;
    /// an edge that stays inside its component needs a `Box`.
    fn mark_cycles(&mut self) {
        let ids: Vec<SchemaId> = self.ir.order.clone();
        let index_of: HashMap<SchemaId, usize> = ids
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect();
        let edges: Vec<Vec<usize>> = ids
            .iter()
            .map(|id| {
                Ir::direct_edges(&self.ir.types[id])
                    .iter()
                    .filter_map(|t| index_of.get(t).copied())
                    .collect()
            })
            .collect();
        let component = tarjan(&edges);
        // Self-loops and same-component edges get boxed.
        for (i, id) in ids.iter().enumerate() {
            let same: BTreeSet<usize> = edges[i]
                .iter()
                .copied()
                .filter(|&j| component[j] == component[i])
                .collect();
            if same.is_empty() {
                continue;
            }
            let needs_box = |ty: &TypeRef| {
                ty.direct_named()
                    .and_then(|t| index_of.get(t))
                    .is_some_and(|j| same.contains(j))
            };
            let def = self.ir.types.get_mut(id).expect("registered");
            match &mut def.kind {
                TypeKind::Struct(s) => {
                    for field in &mut s.fields {
                        if needs_box(&field.ty) {
                            field.ty = std::mem::replace(&mut field.ty, TypeRef::Any).boxed();
                        }
                    }
                }
                TypeKind::UnionTagged(u) => {
                    for variant in &mut u.variants {
                        if needs_box(&variant.ty) {
                            variant.ty = std::mem::replace(&mut variant.ty, TypeRef::Any).boxed();
                        }
                    }
                }
                TypeKind::Newtype(ty) if needs_box(ty) => {
                    *ty = std::mem::replace(ty, TypeRef::Any).boxed();
                }
                TypeKind::Alias(_) => {
                    // A type alias cannot break a cycle; the struct on the
                    // other side of it boxes its field instead, since the
                    // alias's own edge is in the same component.
                }
                _ => {}
            }
        }
    }

    /// A type with an error, and every type that holds it, is not
    /// emitted; the rest still is.
    fn suppress(&mut self) {
        let mut suppressed: BTreeSet<SchemaId> = self
            .ir
            .types
            .values()
            .filter(|d| d.has_error)
            .map(|d| d.id.clone())
            .collect();
        loop {
            let before = suppressed.len();
            for def in self.ir.types.values() {
                if suppressed.contains(&def.id) {
                    continue;
                }
                if Ir::all_edges(def).iter().any(|e| suppressed.contains(e)) {
                    suppressed.insert(def.id.clone());
                }
            }
            if suppressed.len() == before {
                break;
            }
        }
        self.ir.suppressed = suppressed;
    }
}

fn wrap_nullable(lowered: Lowered) -> TypeRef {
    if lowered.nullable {
        TypeRef::Nullable(Box::new(lowered.ty))
    } else {
        lowered.ty
    }
}

fn kind_word(kind: &Kind) -> &'static str {
    match kind {
        Kind::String { .. } => "string",
        Kind::Integer { .. } => "integer",
        Kind::Number { .. } => "number",
        Kind::Boolean => "boolean",
        Kind::Null => "null",
        Kind::Array { .. } => "array",
        Kind::Object { .. } => "object",
        Kind::Any => "any",
        Kind::Never => "never",
        Kind::Not => "not",
        Kind::AllOf { .. } => "all",
        Kind::AnyOf { .. } => "any of",
        Kind::OneOf { .. } => "one of",
        Kind::TypeUnion { .. } => "union",
    }
}

/// Strongly connected components; returns the component index of each
/// node, and a self-loop keeps its node in a component with itself.
fn tarjan(edges: &[Vec<usize>]) -> Vec<usize> {
    struct State<'e> {
        edges: &'e [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        component: Vec<usize>,
        components: usize,
    }
    fn visit(s: &mut State<'_>, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on_stack[v] = true;
        for &w in s.edges[v].iter() {
            match s.index[w] {
                None => {
                    visit(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(iw) if s.on_stack[w] => s.low[v] = s.low[v].min(iw),
                _ => {}
            }
        }
        if s.low[v] == s.index[v].unwrap_or(0) {
            loop {
                let w = s.stack.pop().expect("stack");
                s.on_stack[w] = false;
                s.component[w] = s.components;
                if w == v {
                    break;
                }
            }
            s.components += 1;
        }
    }
    let n = edges.len();
    let mut s = State {
        edges,
        index: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        component: vec![usize::MAX; n],
        components: 0,
    };
    for v in 0..n {
        if s.index[v].is_none() {
            visit(&mut s, v);
        }
    }
    s.component
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;
    use crate::front::Translator;
    use url::Url;

    fn lower_json(components: Value) -> Ir {
        let raw = serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "t", "version": "1"},
            "paths": {},
            "components": {"schemas": components}
        });
        let spec: roas::v3_2::spec::Spec = serde_json::from_value(raw.clone()).unwrap();
        let uri = Url::parse("file:///t.json").unwrap();
        let front = Translator {
            uri: &uri,
            raw: &raw,
            source_pointer: |p| p.to_owned(),
        }
        .translate(&spec);
        let config = ConfigFile {
            target: Some(crate::config::Target::Rust),
            ..Default::default()
        }
        .build()
        .unwrap();
        lower(&front, &config)
    }

    fn def<'a>(ir: &'a Ir, pointer: &str) -> &'a TypeDef {
        ir.types
            .iter()
            .find(|(id, _)| id.pointer == pointer)
            .map(|(_, d)| d)
            .unwrap_or_else(|| {
                panic!(
                    "no type at {pointer}; have {:?}",
                    ir.types.keys().map(|k| &k.pointer).collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn pet_lowers_to_a_struct_with_four_nullability_cases_and_a_hoisted_enum() {
        let ir = lower_json(serde_json::json!({
            "Pet": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name", "owner"],
                "properties": {
                    "name": {"type": "string", "minLength": 1},
                    "owner": {"type": ["string", "null"]},
                    "tag": {"type": ["string", "null"]},
                    "status": {"type": "string", "enum": ["available", "sold"]}
                }
            }
        }));
        let pet = def(&ir, "/components/schemas/Pet");
        let TypeKind::Struct(s) = &pet.kind else {
            panic!("{:?}", pet.kind)
        };
        assert!(matches!(s.additional, Additional::Denied));
        let by_name: BTreeMap<&str, &Field> =
            s.fields.iter().map(|f| (f.raw_name.as_str(), f)).collect();
        assert!(by_name["name"].required && !by_name["name"].nullable);
        assert_eq!(by_name["name"].constraints.min_length, Some(1));
        assert!(by_name["owner"].required && by_name["owner"].nullable);
        assert!(!by_name["tag"].required && by_name["tag"].nullable);
        assert!(!by_name["status"].required && !by_name["status"].nullable);
        let status = def(&ir, "/components/schemas/Pet/properties/status");
        assert_eq!(
            status.suggested_name,
            vec!["Pet".to_owned(), "status".to_owned()]
        );
        assert!(matches!(&status.kind, TypeKind::StringEnum(v) if v.len() == 2));
        assert_eq!(by_name["status"].ty, TypeRef::Named(status.id.clone()));
        assert!(ir.suppressed.is_empty());
        assert!(ir.diagnostics.is_empty(), "{:?}", ir.diagnostics);
    }

    #[test]
    fn maps_arrays_and_newtypes() {
        let ir = lower_json(serde_json::json!({
            "Tags": {"type": "array", "items": {"type": "string"}},
            "Counts": {"type": "object", "additionalProperties": {"type": "integer", "minimum": 0, "maximum": 10}},
            "Loose": {"type": "object"},
            "Id": {"type": "string", "format": "uuid"}
        }));
        assert!(
            matches!(&def(&ir, "/components/schemas/Tags").kind, TypeKind::Newtype(TypeRef::Array(item)) if **item == TypeRef::String)
        );
        assert!(
            matches!(&def(&ir, "/components/schemas/Counts").kind, TypeKind::Newtype(TypeRef::Map(v)) if **v == TypeRef::Integer(IntWidth::Int64))
        );
        assert!(
            matches!(&def(&ir, "/components/schemas/Loose").kind, TypeKind::Newtype(TypeRef::Map(v)) if **v == TypeRef::Any)
        );
        assert!(matches!(
            &def(&ir, "/components/schemas/Id").kind,
            TypeKind::Newtype(TypeRef::String)
        ));
        assert!(ir.diagnostics.is_empty(), "{:?}", ir.diagnostics);
    }

    #[test]
    fn unbounded_integer_is_diagnosed_once() {
        let ir = lower_json(serde_json::json!({"N": {"type": "integer"}}));
        assert_eq!(ir.diagnostics.len(), 1);
        assert!(matches!(
            ir.diagnostics[0].kind,
            DiagnosticKind::LossyNumber
        ));
    }

    #[test]
    fn safe_all_of_flattens_and_unsafe_opens() {
        let ir = lower_json(serde_json::json!({
            "Named": {"type": "object", "properties": {"name": {"type": "string"}}},
            "Aged": {"type": "object", "properties": {"age": {"type": "integer", "minimum": 0, "maximum": 200}}},
            "Person": {"allOf": [{"$ref": "#/components/schemas/Named"}, {"$ref": "#/components/schemas/Aged"}]},
            "Strict": {"allOf": [{"$ref": "#/components/schemas/Named"}, {"type": "object", "additionalProperties": false}]}
        }));
        let person = def(&ir, "/components/schemas/Person");
        let TypeKind::Struct(s) = &person.kind else {
            panic!("{:?}", person.kind)
        };
        assert_eq!(
            s.fields
                .iter()
                .map(|f| f.raw_name.as_str())
                .collect::<Vec<_>>(),
            ["name", "age"]
        );
        let strict = def(&ir, "/components/schemas/Strict");
        assert!(matches!(&strict.kind, TypeKind::ComposedOpen(b) if b.len() == 2));
        assert!(ir.diagnostics.iter().any(
            |d| matches!(&d.kind, DiagnosticKind::OpenComposition { keyword } if keyword == "allOf")
        ));
    }

    #[test]
    fn discriminated_one_of_is_tagged_only_when_pinned() {
        let branches = |pin_cat: Value, pin_dog: Value| {
            serde_json::json!({
                "Cat": {"type": "object", "required": ["petType"], "properties": {"petType": pin_cat}},
                "Dog": {"type": "object", "required": ["petType"], "properties": {"petType": pin_dog}},
                "Pet": {"oneOf": [{"$ref": "#/components/schemas/Cat"}, {"$ref": "#/components/schemas/Dog"}], "discriminator": {"propertyName": "petType"}}
            })
        };
        let ir = lower_json(branches(
            serde_json::json!({"type": "string", "const": "Cat"}),
            serde_json::json!({"type": "string", "enum": ["Dog"]}),
        ));
        let pet = def(&ir, "/components/schemas/Pet");
        let TypeKind::UnionTagged(u) = &pet.kind else {
            panic!("{:?}", pet.kind)
        };
        assert_eq!(u.tag, "petType");
        assert_eq!(
            u.variants
                .iter()
                .map(|v| v.value.as_str())
                .collect::<Vec<_>>(),
            ["Cat", "Dog"]
        );
        assert!(ir.diagnostics.is_empty(), "{:?}", ir.diagnostics);

        let ir = lower_json(branches(
            serde_json::json!({"type": "string"}),
            serde_json::json!({"type": "string"}),
        ));
        let pet = def(&ir, "/components/schemas/Pet");
        assert!(
            matches!(&pet.kind, TypeKind::UnionOpen(u) if u.exclusive && u.branches.len() == 2)
        );
        assert!(ir.diagnostics.iter().any(
            |d| matches!(&d.kind, DiagnosticKind::OpenComposition { keyword } if keyword == "oneOf")
        ));
    }

    #[test]
    fn cycles_are_boxed_and_lists_are_not() {
        let ir = lower_json(serde_json::json!({
            "Node": {"type": "object", "properties": {
                "next": {"$ref": "#/components/schemas/Node"},
                "children": {"type": "array", "items": {"$ref": "#/components/schemas/Node"}}
            }}
        }));
        let node = def(&ir, "/components/schemas/Node");
        let TypeKind::Struct(s) = &node.kind else {
            panic!()
        };
        let next = s.fields.iter().find(|f| f.raw_name == "next").unwrap();
        assert!(
            matches!(&next.ty, TypeRef::Boxed(inner) if matches!(**inner, TypeRef::Named(_))),
            "{:?}",
            next.ty
        );
        let children = s.fields.iter().find(|f| f.raw_name == "children").unwrap();
        assert!(
            matches!(&children.ty, TypeRef::Array(inner) if matches!(**inner, TypeRef::Named(_)))
        );
    }

    #[test]
    fn errors_suppress_the_type_and_its_holders() {
        let ir = lower_json(serde_json::json!({
            "Bad": {"not": {"type": "string"}},
            "Holder": {"type": "object", "properties": {"bad": {"$ref": "#/components/schemas/Bad"}}},
            "Fine": {"type": "object", "properties": {"x": {"type": "boolean"}}},
            "Elsewhere": {"$ref": "#/components/responses/X"}
        }));
        let pointers: BTreeSet<&str> = ir.suppressed.iter().map(|id| id.pointer.as_str()).collect();
        assert_eq!(
            pointers,
            BTreeSet::from([
                "/components/schemas/Bad",
                "/components/schemas/Holder",
                "/components/schemas/Elsewhere"
            ])
        );
        assert!(
            ir.diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count()
                == 2
        );
    }

    #[test]
    fn typeless_object_keywords_become_a_projection() {
        let ir = lower_json(serde_json::json!({
            "Loose": {"properties": {"a": {"type": "string"}}}
        }));
        let loose = def(&ir, "/components/schemas/Loose");
        let TypeKind::ComposedOpen(branches) = &loose.kind else {
            panic!("{:?}", loose.kind)
        };
        assert_eq!(branches.len(), 1);
        let projection = def(&ir, "/components/schemas/Loose/(object)");
        assert!(matches!(&projection.kind, TypeKind::Struct(s) if s.fields.len() == 1));
        assert!(
            ir.diagnostics
                .iter()
                .any(|d| matches!(d.kind, DiagnosticKind::TypelessObject))
        );
    }

    #[test]
    fn type_union_and_any_of_open() {
        let ir = lower_json(serde_json::json!({
            "Either": {"type": ["string", "integer"], "minimum": 0, "maximum": 1},
            "Some": {"anyOf": [{"type": "string"}, {"type": "boolean"}]}
        }));
        assert!(
            matches!(&def(&ir, "/components/schemas/Either").kind, TypeKind::UnionOpen(u) if u.exclusive && u.branches.len() == 2)
        );
        assert!(
            matches!(&def(&ir, "/components/schemas/Some").kind, TypeKind::UnionOpen(u) if !u.exclusive && u.branches.len() == 2)
        );
    }

    #[test]
    fn substitutions_become_externs() {
        let raw = serde_json::json!({
            "openapi": "3.2.0",
            "info": {"title": "t", "version": "1"},
            "paths": {},
            "components": {"schemas": {
                "Money": {"type": "object", "properties": {"amount": {"type": "string"}}},
                "Order": {"type": "object", "properties": {"total": {"$ref": "#/components/schemas/Money"}, "when": {"type": "string", "format": "date-time"}}}
            }}
        });
        let spec: roas::v3_2::spec::Spec = serde_json::from_value(raw.clone()).unwrap();
        let uri = Url::parse("file:///t.json").unwrap();
        let front = Translator {
            uri: &uri,
            raw: &raw,
            source_pointer: |p| p.to_owned(),
        }
        .translate(&spec);
        let config = ConfigFile::from_toml(
            "target = \"rust\"\n[substitutions.Money.rust]\nname = \"my::Money\"\n[substitutions.date-time.rust]\nname = \"chrono::DateTime<chrono::Utc>\"\nimport = \"chrono\"\n",
            std::path::Path::new("/"),
        )
        .unwrap()
        .build()
        .unwrap();
        let ir = lower(&front, &config);
        assert!(matches!(
            def(&ir, "/components/schemas/Money").kind,
            TypeKind::Extern
        ));
        let order = def(&ir, "/components/schemas/Order");
        let TypeKind::Struct(s) = &order.kind else {
            panic!()
        };
        assert!(
            matches!(&s.fields.iter().find(|f| f.raw_name == "when").unwrap().ty, TypeRef::Extern(k) if k == "date-time")
        );
    }
}
