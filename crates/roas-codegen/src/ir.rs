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
}

#[derive(Debug, Clone)]
pub(crate) struct Variant {
    /// The wire value that selects this variant.
    pub value: String,
    pub ty: TypeRef,
    pub suggested_name: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TaggedUnion {
    /// The discriminator property.
    pub tag: String,
    pub variants: Vec<Variant>,
}

/// One projection of an open holder.
#[derive(Debug, Clone)]
pub(crate) struct Branch {
    pub suggested_name: Vec<String>,
    pub ty: TypeRef,
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
}

impl Ir {
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
