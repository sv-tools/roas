//! The intermediate representation: every hard decision made, so a
//! backend renders and never infers.
//!
//! Language-neutral but codegen-oriented. Names are *suggested* —
//! casing and reserved words are the backend's — unions are
//! pre-classified, cycles are marked, nullability is two axes, and
//! constraints ride along as data.

use crate::diagnostic::{Diagnostic, SchemaId};
use crate::front::{Constraints, FloatWidth, IntWidth};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// A type in a position: a field, an item, a branch, a newtype's inner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeRef {
    /// A generated type, or an extern standing in for one.
    Named(SchemaId),
    /// Indirection a cycle makes mandatory.
    Boxed(Box<TypeRef>),
    /// `null` permitted, in a position where absent does not exist.
    Nullable(Box<TypeRef>),
    String,
    Integer(IntWidth),
    Number(FloatWidth),
    Boolean,
    Null,
    /// Any JSON value.
    Any,
    Array(Box<TypeRef>),
    Map(Box<TypeRef>),
    /// A user-provided type, keyed by the `format` it substitutes.
    Extern(String),
}

impl TypeRef {
    /// The named type this position holds directly — through `Option`
    /// and `Box`, but not through an array or a map, which already
    /// provide indirection.
    pub(crate) fn direct_named(&self) -> Option<&SchemaId> {
        match self {
            TypeRef::Named(id) => Some(id),
            TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => inner.direct_named(),
            _ => None,
        }
    }

    /// Every named type reachable from this position.
    pub(crate) fn named(&self, out: &mut Vec<SchemaId>) {
        match self {
            TypeRef::Named(id) => out.push(id.clone()),
            TypeRef::Boxed(inner)
            | TypeRef::Nullable(inner)
            | TypeRef::Array(inner)
            | TypeRef::Map(inner) => inner.named(out),
            _ => {}
        }
    }

    /// Wrap the direct named type in a `Box`.
    pub(crate) fn boxed(self) -> TypeRef {
        match self {
            TypeRef::Nullable(inner) => TypeRef::Nullable(Box::new(inner.boxed())),
            TypeRef::Boxed(_) => self,
            other => TypeRef::Boxed(Box::new(other)),
        }
    }
}

/// One member of an object.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    /// The property name as written.
    pub raw_name: String,
    pub ty: TypeRef,
    pub required: bool,
    pub nullable: bool,
    pub description: Option<String>,
    pub default: Option<Value>,
    pub deprecated: bool,
    pub read_only: bool,
    pub write_only: bool,
    pub constraints: Constraints,
    /// `x-` extensions on the property schema.
    pub extensions: BTreeMap<String, Value>,
    /// Where the property is written: under the struct's own schema, or
    /// under the `allOf` part it was merged from.
    pub pointer: String,
}

/// How a struct treats members its fields do not name.
#[derive(Debug, Clone)]
pub(crate) enum Additional {
    Denied,
    CatchAll(TypeRef),
}

#[derive(Debug, Clone)]
pub(crate) struct StructDef {
    pub fields: Vec<Field>,
    pub additional: Additional,
    /// Beside `additionalProperties: false`, the patterns of
    /// `patternProperties`, translated: a member no property names must
    /// match one, or it is refused at decode. `None` when any member is
    /// let in, including when a pattern cannot be translated.
    pub only_patterns: Option<Vec<String>>,
    /// `patternProperties`: a member no property names is checked against
    /// the schema of every pattern it matches. Such members are held raw.
    pub patterns: Vec<PatternDef>,
    /// Beside `patternProperties`, what `additionalProperties` says of a
    /// member matching no pattern.
    pub others: Others,
}

/// Beside `patternProperties`, what `additionalProperties` says of a
/// member matching no pattern.
#[derive(Debug, Clone)]
pub(crate) enum Others {
    /// Anything goes, or there are no patterns.
    Any,
    /// `false`: no such member.
    Denied,
    /// A schema it is checked against.
    Schema(Box<Branch>),
}

/// One pattern of `patternProperties` and the schema of the members it
/// matches.
#[derive(Debug, Clone)]
pub(crate) struct PatternDef {
    /// The pattern as written.
    pub source: String,
    /// The pattern translated for both engines, or why it cannot be.
    pub regex: Result<String, String>,
    /// The schema a matching member is checked against.
    pub schema: PatternSchema,
}

impl PatternDef {
    /// The schema a matching member is read as, when it is read at all.
    pub(crate) fn branch(&self) -> Option<&Branch> {
        match &self.schema {
            PatternSchema::Branch(b) => Some(b.as_ref()),
            _ => None,
        }
    }

    /// Whether the pattern's schema lets any member be: `{}` or `true`,
    /// written in place or behind references.
    pub(crate) fn accepts_anything(&self, ir: &Ir) -> bool {
        self.branch().is_some_and(|b| accepts_anything(ir, b))
    }

    /// Whether the declared property `name` matches the pattern: `None`
    /// for no, `Some(Err)` when that cannot be known.
    pub(crate) fn matches_name(&self, name: &str) -> Option<Result<(), String>> {
        let regex = match &self.regex {
            Ok(regex) => regex,
            Err(reason) => return Some(Err(reason.clone())),
        };
        match regex::Regex::new(regex) {
            Ok(re) => re.is_match(name).then_some(Ok(())),
            Err(_) => Some(Err("the pattern is too large to compile".to_owned())),
        }
    }
}

/// Whether `branch` lets any value be: `{}` or `true`, written in place or
/// behind references.
pub(crate) fn accepts_anything(ir: &Ir, branch: &Branch) -> bool {
    fn any(ir: &Ir, ty: &TypeRef, depth: usize) -> bool {
        match ty {
            TypeRef::Any => true,
            TypeRef::Nullable(inner) | TypeRef::Boxed(inner) => any(ir, inner, depth),
            TypeRef::Named(id) if depth < 64 => ir.types.get(id).is_some_and(|def| {
                def.constraints == Constraints::default()
                    && matches!(&def.kind, TypeKind::Newtype(t) | TypeKind::Alias(t) if any(ir, t, depth + 1))
            }),
            _ => false,
        }
    }
    any(ir, &branch.ty, 0) && branch.constraints == Constraints::default() && !branch.unchecked
}

impl StructDef {
    /// Beside `patternProperties`, whether a member matching no pattern
    /// may be anything.
    pub(crate) fn others_accept_anything(&self, ir: &Ir) -> bool {
        match &self.others {
            Others::Any => true,
            Others::Denied => false,
            Others::Schema(b) => accepts_anything(ir, b),
        }
    }
}

/// What a pattern's schema says of the members it matches.
#[derive(Debug, Clone)]
pub(crate) enum PatternSchema {
    /// A schema each is read as and checked against.
    Branch(Box<Branch>),
    /// `false`: no such member.
    Never,
    /// A schema no type can represent (`not`): each is unevaluated.
    Unchecked(String),
}

#[derive(Debug, Clone)]
pub(crate) struct Variant {
    /// The wire value that selects this variant.
    pub value: String,
    pub ty: TypeRef,
    pub suggested_name: Vec<String>,
    /// `value` is made up, no tag on the wire: the variant is read only
    /// as the fallback, never by its tag.
    pub synthetic: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TaggedUnion {
    /// The discriminator property.
    pub tag: String,
    pub variants: Vec<Variant>,
    /// The variant a value with an unknown or missing tag is read as: for
    /// a parent schema's discriminator, the parent itself (or the
    /// `defaultMapping`); `None` refuses such a value.
    pub fallback: Option<usize>,
    /// For a union read by a parent schema's discriminator, the parent.
    pub parent: Option<SchemaId>,
}

/// One projection of an open holder.
#[derive(Debug, Clone)]
pub(crate) struct Branch {
    pub suggested_name: Vec<String>,
    pub ty: TypeRef,
    /// Constraints an inline branch declares that its type cannot carry.
    pub constraints: Constraints,
    /// The projection applies only to objects: a typeless schema's object
    /// keywords constrain objects and say nothing about other values.
    pub object_only: bool,
    /// The branch declares something neither its type nor a generated
    /// check enforces (a numeric `enum`, a `const`, a keyword the generator
    /// does not handle): decoding it is no proof that it matches.
    pub unchecked: bool,
    /// Where the branch is written in the source, for a `oneOf`, `anyOf`
    /// or `allOf` branch; `None` for one read from the holder's own
    /// keywords (a type list member, `$ref` siblings, a typeless
    /// schema's object keywords).
    pub pointer: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct OpenUnion {
    pub branches: Vec<Branch>,
    /// `oneOf` rather than `anyOf`: exactly one branch should match,
    /// which the holder exposes but cannot prove.
    pub exclusive: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum TypeKind {
    Struct(StructDef),
    StringEnum(Vec<String>),
    UnionTagged(TaggedUnion),
    UnionOpen(OpenUnion),
    /// An intersection that could not be flattened: raw JSON with a
    /// projection per branch.
    ComposedOpen(Vec<Branch>),
    /// A named type over another type.
    Newtype(TypeRef),
    /// A component that is a bare reference to another.
    Alias(TypeRef),
    /// Provided by the user; not generated.
    Extern,
}

#[derive(Debug, Clone)]
pub(crate) struct TypeDef {
    pub id: SchemaId,
    /// Name parts, raw. The backend cases and joins them.
    pub suggested_name: Vec<String>,
    /// The component name, for a component schema.
    pub component: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    /// `null` is a valid instance of this schema; use sites carry it.
    pub nullable: bool,
    pub constraints: Constraints,
    pub extensions: BTreeMap<String, Value>,
    pub kind: TypeKind,
    /// An `Error` diagnostic was raised on this schema; it and every
    /// type holding it are not emitted.
    pub has_error: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Ir {
    pub types: BTreeMap<SchemaId, TypeDef>,
    /// Emission order: components by name, then hoisted types in the
    /// order they were reached.
    pub order: Vec<SchemaId>,
    pub components: BTreeMap<String, SchemaId>,
    pub diagnostics: Vec<Diagnostic>,
    /// Types not emitted: their own error, or a dependency's.
    pub suppressed: BTreeSet<SchemaId>,
    /// What to warn about a parent's union once every pass is done, and
    /// so what an unknown tag is finally read as.
    pub union_notes: Vec<(SchemaId, UnionNote)>,
}

#[derive(Debug, Clone)]
pub(crate) enum UnionNote {
    /// A schema left out of the union: the tag reading it, or, when that
    /// tag is made up, the schema it names as the default.
    Pruned {
        tag: String,
        default: Option<String>,
    },
    /// A `mapping` entry naming a schema off the family.
    OffFamily { tag: String, reference: String },
    /// A `defaultMapping` naming a schema off the family.
    OffFamilyDefault { reference: String },
}

impl Ir {
    /// Suppresses every type with an error, or holding one. A union read by
    /// a parent's discriminator first leaves out the schemas not generated
    /// on their own account — judged with every such union set aside — so
    /// that they do not take every holder of the parent with them; each left
    /// out is recorded in `union_notes`.
    pub(crate) fn suppress(&mut self) {
        fn closure(ir: &Ir, aside: &BTreeSet<SchemaId>) -> BTreeSet<SchemaId> {
            let mut suppressed: BTreeSet<SchemaId> = ir
                .types
                .values()
                .filter(|d| d.has_error)
                .map(|d| d.id.clone())
                .collect();
            loop {
                let before = suppressed.len();
                for def in ir.types.values() {
                    if suppressed.contains(&def.id) || aside.contains(&def.id) {
                        continue;
                    }
                    if Ir::all_edges(def).iter().any(|e| suppressed.contains(e)) {
                        suppressed.insert(def.id.clone());
                    }
                }
                if suppressed.len() == before {
                    return suppressed;
                }
            }
        }
        let unions: BTreeSet<SchemaId> = self
            .types
            .values()
            .filter(|d| {
                !d.has_error && matches!(&d.kind, TypeKind::UnionTagged(u) if u.parent.is_some())
            })
            .map(|d| d.id.clone())
            .collect();
        // A union whose parent is broken on its own account goes, and with
        // it what holds it: set aside only the others, until that settles.
        let mut aside = unions;
        let alone = loop {
            let alone = closure(self, &aside);
            let kept: BTreeSet<SchemaId> = aside
                .iter()
                .filter(|id| {
                    !matches!(self.types.get(*id).map(|d| &d.kind),
                        Some(TypeKind::UnionTagged(u)) if u.parent.as_ref().is_some_and(|p| alone.contains(p)))
                })
                .cloned()
                .collect();
            if kept.len() == aside.len() {
                break alone;
            }
            aside = kept;
        };
        let mut notes = Vec::new();
        for id in &aside {
            let Some(TypeKind::UnionTagged(u)) = self.types.get_mut(id).map(|d| &mut d.kind) else {
                continue;
            };
            let Some(parent) = u.parent.clone() else {
                continue;
            };
            // A parent not generated on its own account takes its union.
            if alone.contains(&parent) {
                continue;
            }
            let fallback = u.fallback.map(|i| u.variants[i].ty.clone());
            let mut gone = Vec::new();
            u.variants.retain(|v| {
                let left_out = v.ty.direct_named().is_some_and(|id| alone.contains(id));
                if left_out {
                    gone.push(UnionNote::Pruned {
                        tag: v.value.clone(),
                        default: v.synthetic.then(|| v.suggested_name.join("")),
                    });
                }
                !left_out
            });
            // By name: a variant in a cycle is boxed.
            u.fallback = fallback
                .and_then(|ty| u.variants.iter().position(|v| v.ty == ty))
                .or_else(|| {
                    u.variants
                        .iter()
                        .position(|v| v.ty.direct_named() == Some(&parent))
                });
            notes.extend(gone.into_iter().map(|note| (parent.clone(), note)));
        }
        self.suppressed = closure(self, &BTreeSet::new());
        self.union_notes.extend(notes);
    }

    /// Named types a definition holds directly — the edges that can
    /// close a cycle needing indirection.
    pub(crate) fn direct_edges(def: &TypeDef) -> Vec<SchemaId> {
        let mut out = Vec::new();
        match &def.kind {
            TypeKind::Struct(s) => {
                for field in &s.fields {
                    if let Some(id) = field.ty.direct_named() {
                        out.push(id.clone());
                    }
                }
                if let Additional::CatchAll(ty) = &s.additional {
                    // A map already provides indirection; nothing here.
                    let _ = ty;
                }
            }
            TypeKind::UnionTagged(u) => {
                for variant in &u.variants {
                    if let Some(id) = variant.ty.direct_named() {
                        out.push(id.clone());
                    }
                }
            }
            TypeKind::Newtype(ty) | TypeKind::Alias(ty) => {
                if let Some(id) = ty.direct_named() {
                    out.push(id.clone());
                }
            }
            // Open holders keep raw JSON and project by value.
            TypeKind::UnionOpen(_)
            | TypeKind::ComposedOpen(_)
            | TypeKind::StringEnum(_)
            | TypeKind::Extern => {}
        }
        out
    }

    /// Every named type a definition depends on, for suppression.
    pub(crate) fn all_edges(def: &TypeDef) -> Vec<SchemaId> {
        let mut out = Vec::new();
        match &def.kind {
            TypeKind::Struct(s) => {
                for field in &s.fields {
                    field.ty.named(&mut out);
                }
                if let Additional::CatchAll(ty) = &s.additional {
                    ty.named(&mut out);
                }
                // Checked against, so emitted alongside.
                for pattern in &s.patterns {
                    if let Some(b) = pattern.branch() {
                        b.ty.named(&mut out);
                    }
                }
                if let Others::Schema(others) = &s.others {
                    others.ty.named(&mut out);
                }
            }
            TypeKind::UnionTagged(u) => {
                for variant in &u.variants {
                    variant.ty.named(&mut out);
                }
            }
            TypeKind::UnionOpen(u) => {
                for branch in &u.branches {
                    branch.ty.named(&mut out);
                }
            }
            TypeKind::ComposedOpen(branches) => {
                for branch in branches {
                    branch.ty.named(&mut out);
                }
            }
            TypeKind::Newtype(ty) | TypeKind::Alias(ty) => ty.named(&mut out),
            TypeKind::StringEnum(_) | TypeKind::Extern => {}
        }
        out
    }
}
