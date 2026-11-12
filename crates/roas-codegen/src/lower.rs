//! Front-end representation → IR: hoisting, naming, composition
//! classes, nullability, cycles, and every diagnostic the schema earns.

use crate::config::Config;
use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity, source_of};
use crate::front::{
    Additional as FrontAdditional, FloatWidth, Front, FrontSchema, IntWidth, Kind, RefTarget, Slot,
};
use crate::ir::{
    Additional, Branch, Field, Ir, OpenUnion, StructDef, TaggedUnion, TypeDef, TypeKind, TypeRef,
    UnionNote, Variant,
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
    /// Discriminator properties a tagged union provably consumes: their
    /// `const` is represented, by the union.
    proven_tags: BTreeSet<SchemaId>,
}

pub(crate) fn lower(front: &Front, config: &Config) -> Ir {
    let mut lowerer = Lowerer {
        front,
        config,
        ir: Ir::default(),
        owners: Vec::new(),
        proven_tags: BTreeSet::new(),
        component_ids: front
            .components
            .iter()
            .map(|(name, slot)| (name.clone(), slot.id().clone()))
            .collect(),
    };
    lowerer.ir.components = lowerer.component_ids.clone();
    let mut tags = BTreeSet::new();
    for slot in front.components.values() {
        lowerer.collect_proven_tags(slot, &mut tags);
    }
    lowerer.proven_tags = tags;
    for (name, slot) in &front.components {
        lowerer.component(name, slot);
    }
    lowerer.polymorphic();
    lowerer.mark_cycles();
    lowerer.ir.suppress();
    lowerer.mark_opted_out_branches();
    lowerer.ir
}

impl<'a> Lowerer<'a> {
    /// A parent schema with a `discriminator` and the components that
    /// build on it with `allOf`: every value position holding the parent
    /// holds a union of them instead, selected by the discriminator — by
    /// `mapping`, else by component name; any other tag is the parent (or
    /// the `defaultMapping`). The discriminator here is a hint JSON Schema
    /// validation does not use; the union trusts it.
    fn polymorphic(&mut self) {
        let components = &self.front.components;
        let mut unions: BTreeMap<SchemaId, SchemaId> = BTreeMap::new();
        for (parent, slot) in components {
            let Slot::Inline(schema) = slot else { continue };
            let (Some(discriminator), Kind::Object { properties, .. }) =
                (&schema.discriminator, &schema.kind)
            else {
                continue;
            };
            // A substituted parent is the user's type wherever it is held.
            if matches!(
                self.ir
                    .types
                    .get(&self.component_ids[parent])
                    .map(|d| &d.kind),
                Some(TypeKind::Extern)
            ) {
                continue;
            }
            // The components building on the parent, directly or through
            // one another: an `allOf` with a `$ref` to it or to one of them.
            let builds_on = |slot: &Slot, bases: &BTreeSet<&str>| {
                match slot {
                Slot::Inline(s) => match &s.kind {
                    Kind::AllOf { branches } => branches.iter().any(|b| {
                        matches!(b, Slot::Ref { target: RefTarget::Component(n), .. } if bases.contains(n.as_str()))
                    }),
                    _ => false,
                },
                _ => false,
            }
            };
            let mut family: BTreeSet<&str> = BTreeSet::from([parent.as_str()]);
            loop {
                let before = family.len();
                for (name, slot) in components {
                    if !family.contains(name.as_str()) && builds_on(slot, &family) {
                        family.insert(name.as_str());
                    }
                }
                if family.len() == before {
                    break;
                }
            }
            let subtypes: Vec<&String> = components
                .keys()
                .filter(|name| *name != parent && family.contains(name.as_str()))
                .collect();
            if subtypes.is_empty() {
                continue;
            }
            // The tag must be a string property of the parent, written in
            // place or behind references.
            let tag_is_string = {
                let mut slot = properties.get(&discriminator.property_name);
                let mut seen = BTreeSet::new();
                loop {
                    match slot {
                        Some(Slot::Inline(tag)) => break matches!(tag.kind, Kind::String { .. }),
                        Some(Slot::Ref {
                            target: RefTarget::Component(name),
                            siblings: None,
                            ..
                        }) if seen.insert(name.clone()) => slot = components.get(name),
                        _ => break false,
                    }
                }
            };
            if !tag_is_string {
                self.warn(
                    DiagnosticKind::UnsupportedKeyword { keyword: "discriminator".into() },
                    &schema.id,
                    format!(
                        "the discriminator names `{}`, which is not a string property of this schema; the schemas building on it are not read by it",
                        discriminator.property_name
                    ),
                );
                continue;
            }
            let members: BTreeSet<&str> = subtypes
                .iter()
                .map(|s| s.as_str())
                .chain(std::iter::once(parent.as_str()))
                .collect();
            let target = |reference: &str| -> Option<String> {
                let name = reference
                    .strip_prefix("#/components/schemas/")
                    .unwrap_or(reference);
                members.contains(name).then(|| name.to_owned())
            };
            let fallback_name = match discriminator.default_mapping.as_deref() {
                Some(reference) => target(reference).unwrap_or_else(|| {
                    // Warned about once it is settled the parent is generated.
                    self.ir.union_notes.push((
                        schema.id.clone(),
                        UnionNote::OffFamilyDefault {
                            reference: reference.to_owned(),
                        },
                    ));
                    parent.clone()
                }),
                None => parent.clone(),
            };
            let mut variants: Vec<(String, String)> = Vec::new();
            // Tags `mapping` gives to no member: read as the fallback, so
            // no member takes them by name.
            let mut taken = BTreeSet::new();
            for (value, reference) in discriminator.mapping.iter().flatten() {
                match target(reference) {
                    Some(name) => variants.push((value.clone(), name)),
                    None => {
                        taken.insert(value.clone());
                        // Warned about once the fallback is settled.
                        self.ir.union_notes.push((
                            schema.id.clone(),
                            UnionNote::OffFamily {
                                tag: value.clone(),
                                reference: reference.clone(),
                            },
                        ));
                    }
                }
            }
            // By name, unless `mapping` uses the name as a tag: `mapping`
            // overrides a value, not a schema, so a schema it gives another
            // tag is still selected by its name too.
            let mut aliases: BTreeMap<usize, Vec<String>> = BTreeMap::new();
            for name in subtypes
                .iter()
                .map(|s| s.as_str())
                .chain(std::iter::once(parent.as_str()))
            {
                if taken.contains(name) || variants.iter().any(|(value, _)| value == name) {
                    continue;
                }
                match variants.iter().position(|(_, n)| n == name) {
                    Some(i) => aliases.entry(i).or_default().push(name.to_owned()),
                    None => variants.push((name.to_owned(), name.to_owned())),
                }
            }
            // The parent, and what an unknown tag is read as, are always
            // variants: when a name is a tag `mapping` gives to another
            // schema, under a tag of its own, which any unknown tag reads
            // it as anyway. The parent is what a tag is read as once the
            // schema it names is not generated or compiled out.
            let mut synthetic = BTreeSet::new();
            for name in [parent.clone(), fallback_name.clone()] {
                if !variants.iter().any(|(_, n)| *n == name) {
                    let mut value = name.clone();
                    while taken.contains(&value)
                        || variants.iter().any(|(v, _)| *v == value)
                        || aliases.values().flatten().any(|a| *a == value)
                    {
                        value.push('~');
                    }
                    if value != name {
                        synthetic.insert(value.clone());
                    }
                    variants.push((value, name));
                }
            }
            let fallback = variants.iter().position(|(_, n)| *n == fallback_name);
            let parent_id = self.component_ids[parent].clone();
            let Some(parent_def) = self.ir.types.get(&parent_id) else {
                continue;
            };

            let union_id = SchemaId {
                pointer: crate::diagnostic::synthetic(&parent_id.pointer, "union"),
                ..parent_id.clone()
            };
            // Named after the parent as it is generated.
            let base = self
                .config
                .types
                .get(parent)
                .and_then(|t| t.rename.clone())
                .unwrap_or_else(|| parent.clone());
            let def = TypeDef {
                id: union_id.clone(),
                suggested_name: vec![base, "union".to_owned()],
                component: None,
                title: None,
                description: Some(format!(
                    "A `{parent}` or a schema building on it, selected by `{}`.",
                    discriminator.property_name
                )),
                deprecated: parent_def.deprecated,
                nullable: false,
                constraints: Default::default(),
                extensions: BTreeMap::new(),
                kind: TypeKind::UnionTagged(TaggedUnion {
                    tag: discriminator.property_name.clone(),
                    variants: variants
                        .iter()
                        .enumerate()
                        .map(|(i, (value, name))| Variant {
                            value: value.clone(),
                            ty: TypeRef::Named(self.component_ids[name].clone()),
                            suggested_name: vec![name.clone()],
                            synthetic: synthetic.contains(value),
                            aliases: aliases.remove(&i).unwrap_or_default(),
                        })
                        .collect(),
                    fallback,
                    parent: Some(parent_id.clone()),
                }),
                has_error: false,
            };
            self.register(def);
            unions.insert(parent_id, union_id);
        }
        if unions.is_empty() {
            return;
        }
        // Every value position holding a parent holds its union; an
        // `allOf` part stays the parent, which a schema building on it is
        // made of, and a union's own parent variant stays the parent.
        fn rewrite(
            ty: &mut TypeRef,
            unions: &BTreeMap<SchemaId, SchemaId>,
            keep: Option<&SchemaId>,
        ) {
            match ty {
                TypeRef::Named(id) if Some(&*id) != keep => {
                    if let Some(union) = unions.get(id) {
                        *id = union.clone();
                    }
                }
                TypeRef::Boxed(inner)
                | TypeRef::Nullable(inner)
                | TypeRef::Array(inner)
                | TypeRef::Map(inner) => rewrite(inner, unions, keep),
                _ => {}
            }
        }
        let parent_of: BTreeMap<SchemaId, SchemaId> =
            unions.iter().map(|(p, u)| (u.clone(), p.clone())).collect();
        for def in self.ir.types.values_mut() {
            let keep = parent_of.get(&def.id);
            match &mut def.kind {
                TypeKind::Struct(s) => {
                    for field in &mut s.fields {
                        rewrite(&mut field.ty, &unions, None);
                    }
                    if let crate::ir::Additional::CatchAll(ty) = &mut s.additional {
                        rewrite(ty, &unions, None);
                    }
                    for pattern in &mut s.patterns {
                        if let crate::ir::PatternSchema::Branch(b) = &mut pattern.schema {
                            rewrite(&mut b.ty, &unions, None);
                        }
                    }
                    if let crate::ir::Others::Schema(b) = &mut s.others {
                        rewrite(&mut b.ty, &unions, None);
                    }
                }
                TypeKind::UnionTagged(u) => {
                    for variant in &mut u.variants {
                        rewrite(&mut variant.ty, &unions, keep);
                    }
                }
                TypeKind::UnionOpen(u) => {
                    for branch in &mut u.branches {
                        rewrite(&mut branch.ty, &unions, None);
                    }
                }
                TypeKind::Newtype(ty) | TypeKind::Alias(ty) => rewrite(ty, &unions, None),
                TypeKind::ComposedOpen(_) | TypeKind::StringEnum(_) | TypeKind::Extern => {}
            }
        }
    }

    /// A branch that holds an opted-out type anywhere inside proves no
    /// match: that type checks nothing, so decoding it says too little.
    fn mark_opted_out_branches(&mut self) {
        let opted: BTreeSet<SchemaId> = self
            .ir
            .types
            .values()
            .filter(|def| {
                def.component.as_ref().is_some_and(|name| {
                    self.config.types.get(name).and_then(|t| t.validation) == Some(false)
                })
            })
            .map(|def| def.id.clone())
            .collect();
        if opted.is_empty() {
            return;
        }
        fn reaches(
            ir: &Ir,
            opted: &BTreeSet<SchemaId>,
            ty: &TypeRef,
            seen: &mut BTreeSet<SchemaId>,
        ) -> bool {
            match ty {
                TypeRef::Boxed(inner)
                | TypeRef::Nullable(inner)
                | TypeRef::Array(inner)
                | TypeRef::Map(inner) => reaches(ir, opted, inner, seen),
                TypeRef::Named(id) => {
                    if opted.contains(id) {
                        return true;
                    }
                    if !seen.insert(id.clone()) {
                        return false;
                    }
                    let Some(def) = ir.types.get(id) else {
                        return false;
                    };
                    let mut inner: Vec<&TypeRef> = Vec::new();
                    match &def.kind {
                        TypeKind::Struct(s) => {
                            inner.extend(s.fields.iter().map(|f| &f.ty));
                            if let Additional::CatchAll(ty) = &s.additional {
                                inner.push(ty);
                            }
                        }
                        TypeKind::Newtype(ty) | TypeKind::Alias(ty) => inner.push(ty),
                        TypeKind::UnionTagged(u) => inner.extend(u.variants.iter().map(|v| &v.ty)),
                        TypeKind::UnionOpen(OpenUnion { branches, .. })
                        | TypeKind::ComposedOpen(branches) => {
                            inner.extend(branches.iter().map(|b| &b.ty))
                        }
                        TypeKind::StringEnum(_) | TypeKind::Extern => {}
                    }
                    if let TypeKind::Struct(s) = &def.kind {
                        inner.extend(s.patterns.iter().filter_map(|p| p.branch()).map(|b| &b.ty));
                        if let crate::ir::Others::Schema(b) = &s.others {
                            inner.push(&b.ty);
                        }
                    }
                    inner.into_iter().any(|ty| reaches(ir, opted, ty, seen))
                }
                _ => false,
            }
        }
        let marks: Vec<(SchemaId, usize)> = self
            .ir
            .types
            .values()
            .flat_map(|def| {
                let branches: &[Branch] = match &def.kind {
                    TypeKind::UnionOpen(OpenUnion { branches, .. })
                    | TypeKind::ComposedOpen(branches) => branches,
                    _ => &[],
                };
                branches
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| reaches(&self.ir, &opted, &b.ty, &mut BTreeSet::new()))
                    .map(|(i, _)| (def.id.clone(), i))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (id, i) in marks {
            if let Some(def) = self.ir.types.get_mut(&id) {
                match &mut def.kind {
                    TypeKind::UnionOpen(OpenUnion { branches, .. })
                    | TypeKind::ComposedOpen(branches) => branches[i].unchecked = true,
                    _ => {}
                }
            }
        }
    }

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
            // Diagnostics point into the source document, so a synthetic
            // member segment is dropped from the reported location.
            pointer: source_of(pointer.unwrap_or(&at.pointer)),
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
            // The constraints are kept so validation can say they go
            // unchecked on the user's type.
            let (title, description, constraints) = match slot {
                Slot::Inline(schema) => (
                    schema.title.clone(),
                    schema.description.clone(),
                    self.substituted_constraints(schema),
                ),
                Slot::Ref { description, .. } => {
                    (None, description.clone(), self.referenced_constraints(slot))
                }
            };
            self.register(TypeDef {
                id,
                suggested_name: vec![name.to_owned()],
                component: Some(name.to_owned()),
                title,
                description,
                deprecated: false,
                nullable: false,
                constraints,
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
                // With asserting siblings, `reference` already registered the
                // intersection holder under this id; name it after the
                // component instead of wrapping it in itself.
                if siblings.is_some()
                    && let Some(def) = self.ir.types.get_mut(id)
                {
                    def.component = Some(name.to_owned());
                    def.suggested_name = vec![name.to_owned()];
                    if def.description.is_none() {
                        def.description = description.clone();
                    }
                    return;
                }
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
        // The siblings get an id of their own: hoisted under the reference's
        // id, they would be overwritten by the intersection registered
        // there, and the projection would name the intersection itself.
        let mut own = siblings.clone();
        own.id.pointer = crate::diagnostic::synthetic(&own.id.pointer, "siblings");
        let projection = self.position(&Slot::Inline(Box::new(own)), &sibling_hint);
        // Null is one of the intersection's values only when both sides
        // allow it; a side without a `type` says nothing about null.
        let nullable = nullable && admits_null(siblings);
        let target_ty = wrap_nullable(Lowered {
            ty: TypeRef::Named(target_id),
            nullable: self.component_nullable(target),
        });
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
            nullable,
            // The sibling branch reports what goes unchecked, once.
            constraints: crate::front::Constraints {
                unenforced: Vec::new(),
                unsupported: Vec::new(),
                ..siblings.constraints.clone()
            },
            extensions: siblings.extensions.clone(),
            kind: TypeKind::ComposedOpen(vec![
                Branch {
                    suggested_name: target_hint,
                    ty: target_ty,
                    constraints: Default::default(),
                    object_only: false,
                    unchecked: false,
                    pointer: None,
                },
                Branch {
                    suggested_name: sibling_hint,
                    constraints: branch_constraints(&projection.ty, &siblings.constraints),
                    ty: wrap_nullable(projection),
                    object_only: false,
                    unchecked: schema_unchecked(siblings),
                    pointer: None,
                },
            ]),
            has_error: false,
        });
        Lowered {
            ty: TypeRef::Named(id.clone()),
            nullable,
        }
    }

    /// Whether a component permits `null`, following bare references.
    /// Whether a branch declares what nothing enforces, so decoding it
    /// proves no match. A reference is followed: the component's own type
    /// does not enforce a numeric `enum` or a `const` either.
    fn slot_unchecked(&self, slot: &Slot) -> bool {
        let mut seen = BTreeSet::new();
        let mut current = slot;
        loop {
            let schema = match current {
                Slot::Inline(schema) => schema,
                Slot::Ref {
                    target: RefTarget::Component(name),
                    siblings,
                    ..
                } => {
                    if siblings.as_ref().is_some_and(|s| schema_unchecked(s)) {
                        return true;
                    }
                    // An opted-out type checks nothing: decoding it proves
                    // no match.
                    if self.config.types.get(name).and_then(|t| t.validation) == Some(false) {
                        return true;
                    }
                    if !seen.insert(name.clone()) {
                        return false;
                    }
                    match self.front.components.get(name) {
                        Some(next) => {
                            current = next;
                            continue;
                        }
                        None => return false,
                    }
                }
                Slot::Ref { .. } => return false,
            };
            return schema_unchecked(schema);
        }
    }

    /// What a substituted schema declares, its string `enum` (or `const`
    /// pin) included: the kind that carried it is replaced by the user's
    /// type. So is what its members, items and branches declare, which
    /// is marked as one unchecked whole.
    fn substituted_constraints(&self, schema: &FrontSchema) -> crate::front::Constraints {
        let mut c = schema.constraints.clone();
        let string_enum = matches!(
            &schema.kind,
            Kind::String {
                enum_values: Some(_),
                ..
            }
        );
        if string_enum && !c.unenforced.contains(&"enum") {
            c.unenforced.push("enum");
        }
        if self.asserts_inside(schema, &mut BTreeSet::new()) {
            c.unsupported
                .push("the substituted schema's members".to_owned());
        }
        c
    }

    /// Whether anything inside `schema` — a member, an item, a value, a
    /// branch, through references — asserts what decoding does not.
    fn asserts_inside(&self, schema: &FrontSchema, seen: &mut BTreeSet<String>) -> bool {
        let slots: Vec<&Slot> = match &schema.kind {
            Kind::Array { items } => items.iter().collect(),
            Kind::Object {
                properties,
                required,
                additional,
                ..
            } => {
                // Which members must be present, or may not, is the
                // object's own assertion.
                if !required.is_empty() || matches!(additional, FrontAdditional::Denied) {
                    return true;
                }
                let mut out: Vec<&Slot> = properties.values().collect();
                if let FrontAdditional::Schema(slot) = additional {
                    out.push(slot);
                }
                out
            }
            Kind::AllOf { branches }
            | Kind::AnyOf { branches }
            | Kind::OneOf { branches, .. }
            | Kind::TypeUnion { branches } => branches.iter().collect(),
            _ => Vec::new(),
        };
        slots.into_iter().any(|slot| self.slot_asserts(slot, seen))
    }

    /// Whether a slot, followed through references, asserts what decoding
    /// does not, itself or anywhere inside.
    fn slot_asserts(&self, slot: &Slot, seen: &mut BTreeSet<String>) -> bool {
        match slot {
            Slot::Inline(inner) => {
                !crate::checks::present_keywords(&inner.constraints).is_empty()
                    || !inner.unsupported.is_empty()
                    || matches!(
                        &inner.kind,
                        Kind::String {
                            enum_values: Some(_),
                            ..
                        } | Kind::Never
                            | Kind::Not
                    )
                    || self.asserts_inside(inner, seen)
            }
            Slot::Ref {
                target: RefTarget::Component(name),
                siblings,
                ..
            } => {
                if siblings.is_some() {
                    return true;
                }
                if !seen.insert(name.clone()) {
                    return false;
                }
                self.front
                    .components
                    .get(name)
                    .is_some_and(|next| self.slot_asserts(next, seen))
            }
            Slot::Ref { .. } => false,
        }
    }

    /// The constraints a reference stands for, through a chain of
    /// references; siblings that assert are kept as unsupported, since
    /// they are not merged.
    fn referenced_constraints(&self, slot: &Slot) -> crate::front::Constraints {
        let mut seen = BTreeSet::new();
        let mut current = slot;
        let mut extra = Vec::new();
        loop {
            match current {
                Slot::Inline(schema) => {
                    let mut c = self.substituted_constraints(schema);
                    for keyword in extra {
                        if !c.unsupported.contains(&keyword) {
                            c.unsupported.push(keyword);
                        }
                    }
                    return c;
                }
                Slot::Ref {
                    target: RefTarget::Component(name),
                    siblings,
                    ..
                } => {
                    if siblings.is_some() {
                        extra.push("$ref siblings".to_owned());
                    }
                    if !seen.insert(name.clone()) {
                        return Default::default();
                    }
                    match self.front.components.get(name) {
                        Some(next) => current = next,
                        None => return Default::default(),
                    }
                }
                Slot::Ref { .. } => return Default::default(),
            }
        }
    }

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
                    // An intersection: null only when both sides allow it.
                    if siblings.as_ref().is_some_and(|s| !admits_null(s)) {
                        return false;
                    }
                    current = target;
                }
                None => return false,
            }
        }
    }

    /// Whether a schema is generated as a type of its own rather than
    /// spelled in place.
    pub(crate) fn needs_name(schema: &FrontSchema) -> bool {
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
                    || !matches!(additional, FrontAdditional::Any)
                    || !schema.patterns.is_empty();
                if *typeless {
                    keyworded
                } else {
                    // Pattern members are checked by a generated struct.
                    !properties.is_empty()
                        || !required.is_empty()
                        || matches!(additional, FrontAdditional::Denied)
                        || !schema.patterns.is_empty()
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
                    let checked = if self.config.validation {
                        " and checked by generated validation"
                    } else {
                        ""
                    };
                    self.warn(DiagnosticKind::NumericEnum, &schema.id, format!("`enum` over integers is not represented in the type, which stays an integer; the values are listed in its documentation{checked}"));
                }
                TypeRef::Integer(width.unwrap_or(IntWidth::Int64))
            }
            Kind::Number { width, enum_values } => {
                self.number_diagnostics(schema, false);
                if enum_values.is_some() {
                    let checked = if self.config.validation {
                        " and checked by generated validation"
                    } else {
                        ""
                    };
                    self.warn(DiagnosticKind::NumericEnum, &schema.id, format!("`enum` over numbers is not represented in the type, which stays a number; the values are listed in its documentation{checked}"));
                }
                if self.config.exact_numbers {
                    TypeRef::Decimal
                } else {
                    TypeRef::Number(width.unwrap_or(FloatWidth::Double))
                }
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
                    || !matches!(additional, FrontAdditional::Any)
                    || !schema.patterns.is_empty();
                if *typeless && !keyworded {
                    TypeRef::Any
                } else if properties.is_empty()
                    && required.is_empty()
                    && !*typeless
                    // Pattern members are checked by a generated struct.
                    && schema.patterns.is_empty()
                {
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
                    constraints: Default::default(),
                    object_only: true,
                    unchecked: false,
                    pointer: None,
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
            // `type: [integer, number]` accepts what any member accepts,
            // overlaps included: it is `anyOf`, not `oneOf`.
            Kind::TypeUnion { branches } => TypeKind::UnionOpen(OpenUnion {
                branches: self.type_branches(branches, &name),
                exclusive: false,
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
        id.pointer = crate::diagnostic::synthetic(&id.pointer, "object");
        let def = TypeDef {
            id: id.clone(),
            suggested_name: hint.to_vec(),
            component: None,
            title: None,
            description: None,
            deprecated: false,
            nullable: false,
            // The holder reports what goes unchecked, for every value.
            constraints: crate::front::Constraints {
                unenforced: Vec::new(),
                unsupported: Vec::new(),
                ..schema.constraints.clone()
            },
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
            let pointer = match slot {
                Slot::Inline(s) => s.id.pointer.clone(),
                Slot::Ref { id, .. } => id.pointer.clone(),
            };
            fields.push(Field {
                pointer,
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
        // Beside `additionalProperties: false`, a member must be a property
        // or match a pattern: the decoder can tell, when every pattern
        // means the same to it as to JSON Schema.
        let only_patterns = (matches!(additional, FrontAdditional::Denied)
            && !schema.constraints.pattern_properties.is_empty())
        .then(|| {
            schema
                .constraints
                .pattern_properties
                .iter()
                .map(|p| crate::pattern::translate(p).ok())
                .collect::<Option<Vec<_>>>()
        })
        .flatten();
        // Each pattern's schema, and beside them the other members' schema,
        // judge the members held raw in the catch-all.
        let mut patterns = Vec::new();
        for (i, (source, slot)) in schema.patterns.iter().enumerate() {
            let mut hint = name.to_vec();
            hint.push(format!("pattern {}", i + 1));
            patterns.push(crate::ir::PatternDef {
                source: source.clone(),
                regex: crate::pattern::translate(source),
                schema: match slot {
                    // A schema no type represents still says what it does of
                    // the members: `false` refuses them; `not` goes unchecked.
                    Slot::Inline(schema) if matches!(schema.kind, Kind::Never) => {
                        crate::ir::PatternSchema::Never
                    }
                    Slot::Inline(schema) if matches!(schema.kind, Kind::Not) => {
                        self.warn(
                            DiagnosticKind::UnsupportedKeyword { keyword: "not".into() },
                            &schema.id,
                            "`not` describes what a member is not, which is not checked; matching members are reported as unevaluated".into(),
                        );
                        crate::ir::PatternSchema::Unchecked(
                            "its schema is a `not`, which is not checked".into(),
                        )
                    }
                    _ => crate::ir::PatternSchema::Branch(Box::new(self.slot_branch(slot, &hint))),
                },
            });
        }
        let others = match additional {
            _ if patterns.is_empty() => crate::ir::Others::Any,
            FrontAdditional::Schema(slot) => {
                let mut hint = name.to_vec();
                hint.push("value".to_owned());
                crate::ir::Others::Schema(Box::new(self.slot_branch(slot, &hint)))
            }
            FrontAdditional::Denied => crate::ir::Others::Denied,
            FrontAdditional::Any => crate::ir::Others::Any,
        };
        let additional = match additional {
            // Members the patterns allow are not unknown: refusing them
            // would refuse valid values. They land in the catch-all.
            // Nor are they the other members `additionalProperties` speaks
            // of: typed by it, they would be refused or misjudged.
            FrontAdditional::Denied | FrontAdditional::Schema(_)
                if !schema.constraints.pattern_properties.is_empty() =>
            {
                Additional::CatchAll(TypeRef::Any)
            }
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
            let message = if only_patterns.is_some() {
                "`patternProperties` schemas are not represented in the type: matching members are held as raw JSON in the catch-all, checked against their patterns' schemas by generated validation, and a member matching no pattern is refused at decode"
            } else if matches!(additional, Additional::CatchAll(_))
                && matches!(
                    &schema.kind,
                    Kind::Object {
                        additional: FrontAdditional::Denied,
                        ..
                    }
                )
            {
                "`patternProperties` schemas are not represented in the type: matching members are held as raw JSON in the catch-all, checked against their patterns' schemas by generated validation, and since a pattern cannot be translated, a member matching none is not refused at decode"
            } else {
                "`patternProperties` schemas are not represented in the type: matching members are held as raw JSON in the catch-all, checked against their patterns' schemas by generated validation"
            };
            self.warn(
                DiagnosticKind::UnsupportedKeyword {
                    keyword: "patternProperties".into(),
                },
                &schema.id,
                message.into(),
            );
        }
        StructDef {
            fields,
            additional,
            only_patterns,
            patterns,
            others,
        }
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
                let ty = wrap_nullable(lowered);
                Branch {
                    suggested_name,
                    constraints: slot_constraints(slot, &ty),
                    unchecked: self.slot_unchecked(slot),
                    ty,
                    object_only: false,
                    pointer: Some(match slot {
                        Slot::Inline(schema) => schema.id.pointer.clone(),
                        Slot::Ref { id, .. } => id.pointer.clone(),
                    }),
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
                let ty = wrap_nullable(lowered);
                Branch {
                    suggested_name: hint,
                    constraints: slot_constraints(slot, &ty),
                    unchecked: self.slot_unchecked(slot),
                    ty,
                    object_only: false,
                    pointer: None,
                }
            })
            .collect()
    }

    /// What to call a branch: the component it references, its title,
    /// or its position.
    /// A schema in a slot as a branch checked against a raw value.
    fn slot_branch(&mut self, slot: &Slot, hint: &[String]) -> Branch {
        let lowered = self.position(slot, hint);
        let ty = wrap_nullable(lowered);
        Branch {
            suggested_name: self.branch_name(slot, hint),
            constraints: slot_constraints(slot, &ty),
            unchecked: self.slot_unchecked(slot),
            ty,
            object_only: false,
            pointer: Some(match slot {
                Slot::Inline(schema) => schema.id.pointer.clone(),
                Slot::Ref { id, .. } => id.pointer.clone(),
            }),
        }
    }

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
                only_patterns: None,
                patterns: Vec::new(),
                others: crate::ir::Others::Any,
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
                || !schema.unsupported.is_empty()
                || schema.const_value.is_some()
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
                    synthetic: false,
                    aliases: Vec::new(),
                });
            }
            return TypeKind::UnionTagged(TaggedUnion {
                tag: discriminator.property_name.clone(),
                variants: out,
                fallback: None,
                parent: None,
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
            // A nullable branch admits `null`, which carries no tag.
            if schema.nullable || self.component_nullable(&RefTarget::Component(component.clone()))
            {
                return None;
            }
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
                if self.config.validation {
                    format!("`{keyword}` is not handled by the generator; it is not represented in the type, and generated validation reports it as unevaluated")
                } else {
                    format!("`{keyword}` is not handled by the generator; it is neither represented in the type nor validated")
                },
            );
        }
        if schema.const_value.is_some() && !self.const_is_tag(schema) {
            let message = if schema.constraints.const_number.is_some() && self.config.validation {
                "`const` is not represented in the type, which stays a number; generated validation checks it"
            } else {
                "`const` is not represented in the type; the value is not enforced"
            };
            self.warn(
                DiagnosticKind::UnsupportedKeyword {
                    keyword: "const".into(),
                },
                &schema.id,
                message.into(),
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

    /// A `const` that pins the tag of a branch in a proven tagged union
    /// is represented, by the union. Any other `const` is not enforced.
    fn const_is_tag(&self, schema: &FrontSchema) -> bool {
        self.proven_tags.contains(&schema.id)
    }

    /// Walk every schema reachable from `slot` and record the tag
    /// property of each `oneOf` whose discriminator is proven.
    fn collect_proven_tags(&self, slot: &Slot, out: &mut BTreeSet<SchemaId>) {
        let schema = match slot {
            Slot::Inline(schema) => schema,
            Slot::Ref { siblings, .. } => match siblings {
                Some(siblings) => siblings,
                None => return,
            },
        };
        match &schema.kind {
            Kind::OneOf {
                branches,
                discriminator,
            } => {
                if let Some(discriminator) = discriminator
                    && let Some(proven) = self.safe_one_of(branches, discriminator)
                {
                    for (_, branch) in proven {
                        if let Some(object) = self.resolve_object(branch)
                            && let Kind::Object { properties, .. } = &object.kind
                            && let Some(Slot::Inline(tag)) =
                                properties.get(&discriminator.property_name)
                        {
                            out.insert(tag.id.clone());
                        }
                    }
                }
                for branch in branches {
                    self.collect_proven_tags(branch, out);
                }
            }
            Kind::AllOf { branches } | Kind::AnyOf { branches } | Kind::TypeUnion { branches } => {
                for branch in branches {
                    self.collect_proven_tags(branch, out);
                }
            }
            Kind::Array { items: Some(items) } => self.collect_proven_tags(items, out),
            Kind::Object {
                properties,
                additional,
                ..
            } => {
                for property in properties.values() {
                    self.collect_proven_tags(property, out);
                }
                if let FrontAdditional::Schema(value) = additional {
                    self.collect_proven_tags(value, out);
                }
            }
            _ => {}
        }
    }

    fn number_diagnostics(&mut self, schema: &FrontSchema, integer: bool) {
        let c = &schema.constraints;
        // Held exactly, a number loses nothing to its type; only a literal
        // the decimal cannot hold is an error, since no check on it can be
        // generated.
        if !integer && self.config.exact_numbers {
            let literals = [
                ("minimum", &c.minimum),
                ("exclusiveMinimum", &c.exclusive_minimum),
                ("maximum", &c.maximum),
                ("exclusiveMaximum", &c.exclusive_maximum),
                ("multipleOf", &c.multiple_of),
                ("const", &c.const_number),
            ]
            .into_iter()
            .filter_map(|(keyword, n)| n.as_ref().map(|n| (keyword, n)))
            .chain(c.enum_numbers.iter().flatten().map(|n| ("enum", n)));
            let unheld: Vec<String> = literals
                .filter(|(_, n)| crate::checks::dec(n).is_none())
                .map(|(keyword, n)| format!("`{keyword}` {n}"))
                .collect();
            if !unheld.is_empty() {
                self.error(
                    DiagnosticKind::LossyNumber,
                    &schema.id,
                    format!(
                        "{} has more than {} significant digits, or an exponent past the 32-bit range, which an exact decimal cannot hold",
                        unheld.join(", "),
                        crate::checks::MAX_DIGITS
                    ),
                );
            }
            return;
        }
        if integer {
            let (lo, hi): (i128, i128) = match &schema.kind {
                Kind::Integer {
                    width: Some(IntWidth::Int32),
                    ..
                } => (i32::MIN.into(), i32::MAX.into()),
                _ => (i64::MIN.into(), i64::MAX.into()),
            };
            // A bound fits when it lies inside the mapped type's range,
            // compared exactly: integers as `i128`, and a fractional bound
            // against the range edges, both of which (`-2^63`, `2^63`,
            // `±2^31`) are exact in `f64`.
            let fits = |n: &Option<serde_json::Number>, below: bool| {
                let Some(n) = n.as_ref() else { return false };
                let exact = n
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from));
                match (exact, n.as_f64()) {
                    (Some(v), _) => {
                        if below {
                            v >= lo
                        } else {
                            v <= hi
                        }
                    }
                    (None, Some(v)) => {
                        if below {
                            v >= lo as f64
                        } else {
                            v < (hi + 1) as f64
                        }
                    }
                    (None, None) => false,
                }
            };
            let bounded_below = fits(&c.minimum, true) || fits(&c.exclusive_minimum, true);
            let bounded_above = fits(&c.maximum, false) || fits(&c.exclusive_maximum, false);
            if !(bounded_below && bounded_above) {
                self.warn(
                    DiagnosticKind::LossyNumber,
                    &schema.id,
                    "`integer` is unbounded in JSON Schema, or bounded beyond the mapped fixed-width integer; values outside that range are rejected at decode".into(),
                );
            }
        }
        // Said only where validation hears it: a new warning would fail a
        // `--strict` generation that passed before.
        if let Kind::Number {
            width: Some(FloatWidth::Float),
            ..
        } = &schema.kind
            && self.config.validation
        {
            let within = |n: &Option<serde_json::Number>| {
                n.as_ref()
                    .and_then(serde_json::Number::as_f64)
                    .is_some_and(|v| v.abs() <= f64::from(f32::MAX))
            };
            let below = within(&c.minimum) || within(&c.exclusive_minimum);
            let above = within(&c.maximum) || within(&c.exclusive_maximum);
            if !(below && above) {
                self.warn(
                    DiagnosticKind::LossyNumber,
                    &schema.id,
                    "`format: float` maps to a 32-bit float; a value past its range is rejected at decode (read as an infinity in Rust without serde_json's `float_roundtrip`)".into(),
                );
            }
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
        // A cycle made only of aliases has no type to box: `type A = B;
        // type B = A;` cannot compile. Refuse it.
        let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, &c) in component.iter().enumerate() {
            members.entry(c).or_default().push(i);
        }
        for nodes in members.values() {
            let cyclic = nodes.len() > 1 || edges[nodes[0]].contains(&nodes[0]);
            let aliases_only = nodes
                .iter()
                .all(|&i| matches!(self.ir.types[&ids[i]].kind, TypeKind::Alias(_)));
            if cyclic && aliases_only {
                for &i in nodes {
                    let id = ids[i].clone();
                    self.diagnose(
                        Severity::Error,
                        DiagnosticKind::ReferenceCycle,
                        &id,
                        None,
                        "this schema is a `$ref` that leads back to itself through other bare references; no type can be generated for it".into(),
                    );
                    if let Some(def) = self.ir.types.get_mut(&id) {
                        def.has_error = true;
                    }
                }
            }
        }
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
}

/// The warning a note on a parent's union makes, `read_as` the schema an
/// unknown tag is finally read as.
pub(crate) fn union_note(parent: &SchemaId, note: &UnionNote, read_as: Option<&str>) -> Diagnostic {
    let read_as = read_as.map_or("this schema".to_owned(), |name| format!("`{name}`"));
    Diagnostic {
        severity: Severity::Warning,
        kind: DiagnosticKind::UnsupportedKeyword {
            keyword: "discriminator".into(),
        },
        schema_id: parent.clone(),
        pointer: source_of(&parent.pointer),
        message: match note {
            UnionNote::Pruned {
                default: Some(name),
                ..
            } => format!(
                "the discriminator's default `{name}` is not generated; an unknown or missing value is read as {read_as}"
            ),
            UnionNote::Pruned { tag, default: None } => format!(
                "the schema the discriminator reads `{tag}` as is not generated; such a value is read as {read_as}"
            ),
            UnionNote::OffFamilyDefault { reference } => format!(
                "the discriminator's default is `{reference}`, which is neither this schema nor one building on it; an unknown or missing value is read as {read_as}"
            ),
            UnionNote::OffFamily { tag, reference } => format!(
                "the discriminator maps `{tag}` to `{reference}`, which is neither this schema nor one building on it; the value is read as {read_as}"
            ),
        },
    }
}

/// A schema saying what neither its type nor a generated check enforces.
fn schema_unchecked(schema: &FrontSchema) -> bool {
    // A number's `enum` and `const` are checked; any other `const` is not.
    (schema.const_value.is_some() && schema.constraints.const_number.is_none())
        || !schema.unsupported.is_empty()
}

/// What an inline branch constrains that no named type carries for it.
fn slot_constraints(slot: &Slot, ty: &TypeRef) -> crate::front::Constraints {
    match slot {
        Slot::Inline(schema) => branch_constraints(ty, &schema.constraints),
        Slot::Ref { .. } => Default::default(),
    }
}

/// A named type checks its own constraints; anything else keeps them on
/// the branch.
fn branch_constraints(
    ty: &TypeRef,
    constraints: &crate::front::Constraints,
) -> crate::front::Constraints {
    if ty.direct_named().is_some() {
        Default::default()
    } else {
        constraints.clone()
    }
}

/// Whether `$ref` siblings are known to allow null: they say so, or say
/// no `type` and nothing else about it.
fn admits_null(siblings: &FrontSchema) -> bool {
    // What the generator does not read may leave null out: unproven, the
    // null is validated rather than passed.
    if !siblings.unsupported.is_empty() {
        return false;
    }
    // An `enum` or `const` names the values allowed, null among them or not.
    let listed = match &siblings.kind {
        Kind::String {
            enum_values: Some(_),
            ..
        }
        | Kind::Integer {
            enum_values: Some(_),
            ..
        }
        | Kind::Number {
            enum_values: Some(_),
            ..
        } => Some(false),
        _ => siblings.const_value.as_ref().map(Value::is_null),
    };
    listed.unwrap_or(
        siblings.nullable
            || matches!(
                &siblings.kind,
                Kind::Any | Kind::Object { typeless: true, .. }
            ),
    )
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
            legacy_nullable: false,
            report_lost: false,
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
    fn alias_only_cycles_are_refused() {
        let ir = lower_json(serde_json::json!({
            "A": {"$ref": "#/components/schemas/B"},
            "B": {"$ref": "#/components/schemas/A"},
            "Me": {"$ref": "#/components/schemas/Me"},
            "Fine": {"type": "string"}
        }));
        let suppressed: BTreeSet<&str> =
            ir.suppressed.iter().map(|id| id.pointer.as_str()).collect();
        assert_eq!(
            suppressed,
            BTreeSet::from([
                "/components/schemas/A",
                "/components/schemas/B",
                "/components/schemas/Me"
            ])
        );
        assert_eq!(
            ir.diagnostics
                .iter()
                .filter(|d| matches!(d.kind, DiagnosticKind::ReferenceCycle))
                .count(),
            3
        );
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
        let projection = def(&ir, "/components/schemas/Loose/~object");
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
            matches!(&def(&ir, "/components/schemas/Either").kind, TypeKind::UnionOpen(u) if !u.exclusive && u.branches.len() == 2)
        );
        // A type list accepts what any member does; each member keeps the
        // constraints that apply to it, for validation.
        let TypeKind::UnionOpen(either) = &def(&ir, "/components/schemas/Either").kind else {
            unreachable!()
        };
        assert!(
            either
                .branches
                .iter()
                .any(|b| b.constraints.minimum.is_some())
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
            legacy_nullable: false,
            report_lost: false,
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
