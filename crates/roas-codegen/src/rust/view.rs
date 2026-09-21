//! The view model: what templates see. Every type is already spelled
//! for Rust, every name is final, every attribute is decided. A
//! template lays this out and nothing more.

use crate::config::{Config, FieldConfig, TypeConfig};
use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
use crate::front::{Constraints, FloatWidth, IntWidth};
use crate::generate::GenerateError;
use crate::ir::{Additional, Ir, TypeDef, TypeKind, TypeRef};
use crate::rust::attrs::{check_container_attrs, check_field_attrs};
use crate::rust::naming::{Allocator, legal, pascal, snake};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct SupportView {
    pub double_option: bool,
    pub reject_null: bool,
    pub require_present: bool,
    pub int64: bool,
    pub int32: bool,
    pub raw_holder: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FieldView {
    pub name: String,
    pub raw_name: String,
    pub ty: String,
    pub doc: Vec<String>,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VariantView {
    pub name: String,
    pub value: String,
    /// The payload type, for a tagged union; empty for a string enum.
    pub ty: String,
    pub doc: Vec<String>,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BranchView {
    /// Accessor name, e.g. `as_cat`.
    pub accessor: String,
    /// Constructor name, e.g. `from_cat`.
    pub constructor: String,
    pub ty: String,
    pub doc: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypeView {
    /// `struct`, `enum`, `union_tagged`, `union_open`, `composed_open`,
    /// `newtype`, `alias`.
    pub kind: &'static str,
    pub name: String,
    pub raw_name: String,
    pub doc: Vec<String>,
    pub derives: Vec<String>,
    pub attrs: Vec<String>,
    pub fields: Vec<FieldView>,
    pub variants: Vec<VariantView>,
    pub branches: Vec<BranchView>,
    /// Inner type of a newtype or alias.
    pub inner: String,
    /// Discriminator property of a tagged union.
    pub tag: String,
    /// `oneOf` rather than `anyOf`, for an open union's documentation.
    pub exclusive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileView {
    pub source: String,
    pub header: String,
    pub imports: Vec<String>,
    pub extra_imports: Vec<String>,
    pub support: SupportView,
    pub types: Vec<TypeView>,
}

/// A Rust dependency the generated code needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RustDependency {
    pub name: String,
    pub features: Vec<String>,
}

/// The constraints a schema declares, as documentation until generated
/// validation lands; the type itself does not enforce them.
fn constraint_notes(c: &Constraints) -> Vec<String> {
    let mut notes = Vec::new();
    let mut push = |keyword: &str, value: String| notes.push(format!("`{keyword}`: {value}."));
    if let Some(v) = c.min_length {
        push("minLength", v.to_string());
    }
    if let Some(v) = c.max_length {
        push("maxLength", v.to_string());
    }
    if let Some(v) = &c.pattern {
        push("pattern", format!("`{v}`"));
    }
    if let Some(v) = &c.minimum {
        push("minimum", v.to_string());
    }
    if let Some(v) = &c.exclusive_minimum {
        push("exclusiveMinimum", v.to_string());
    }
    if let Some(v) = &c.maximum {
        push("maximum", v.to_string());
    }
    if let Some(v) = &c.exclusive_maximum {
        push("exclusiveMaximum", v.to_string());
    }
    if let Some(v) = &c.multiple_of {
        push("multipleOf", v.to_string());
    }
    if let Some(v) = c.min_items {
        push("minItems", v.to_string());
    }
    if let Some(v) = c.max_items {
        push("maxItems", v.to_string());
    }
    if c.unique_items {
        push("uniqueItems", "true".into());
    }
    if let Some(v) = c.min_properties {
        push("minProperties", v.to_string());
    }
    if let Some(v) = c.max_properties {
        push("maxProperties", v.to_string());
    }
    notes
}

pub(crate) struct Built {
    pub file: FileView,
    pub dependencies: Vec<RustDependency>,
    pub diagnostics: Vec<Diagnostic>,
}

struct Builder<'a> {
    ir: &'a Ir,
    config: &'a Config,
    names: BTreeMap<SchemaId, String>,
    support: SupportView,
    externs: BTreeMap<String, String>,
    diagnostics: Vec<Diagnostic>,
}

pub(crate) fn build(ir: &Ir, config: &Config, source: &str) -> Result<Built, GenerateError> {
    let mut builder = Builder {
        ir,
        config,
        names: BTreeMap::new(),
        support: SupportView::default(),
        externs: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    builder.check_selectors()?;
    builder.allocate_type_names();
    let mut types = Vec::new();
    for id in &ir.order {
        if ir.suppressed.contains(id) {
            continue;
        }
        let def = &ir.types[id];
        if let Some(view) = builder.type_view(def)? {
            types.push(view);
        }
    }
    let mut dependencies = vec![
        RustDependency {
            name: "serde".into(),
            features: vec!["derive".into()],
        },
        RustDependency {
            name: "serde_json".into(),
            features: vec!["raw_value".into()],
        },
    ];
    for import in builder.externs.values() {
        let dependency = RustDependency {
            name: import.clone(),
            features: Vec::new(),
        };
        if !dependencies.contains(&dependency) {
            dependencies.push(dependency);
        }
    }
    dependencies.sort();
    let imports = vec!["use serde::{Deserialize, Serialize};".to_owned()];
    Ok(Built {
        file: FileView {
            source: source.to_owned(),
            header: config.header.clone().unwrap_or_default(),
            imports,
            extra_imports: config.extra_imports.clone(),
            support: builder.support,
            types,
        },
        dependencies,
        diagnostics: builder.diagnostics,
    })
}

impl Builder<'_> {
    /// Every `types.<Name>` and `fields.<prop>` in the configuration
    /// must name something in the description.
    fn check_selectors(&self) -> Result<(), GenerateError> {
        for (name, type_config) in &self.config.types {
            let Some(id) = self.ir.components.get(name) else {
                return Err(GenerateError::UnknownSelector {
                    selector: format!("types.{name}"),
                    candidates: self.ir.components.keys().cloned().collect(),
                });
            };
            if type_config.fields.is_empty() {
                continue;
            }
            let def = &self.ir.types[id];
            let fields: Vec<String> = match &def.kind {
                TypeKind::Struct(s) => s.fields.iter().map(|f| f.raw_name.clone()).collect(),
                _ => Vec::new(),
            };
            for field in type_config.fields.keys() {
                if !fields.contains(field) {
                    return Err(GenerateError::UnknownSelector {
                        selector: format!("types.{name}.fields.{field}"),
                        candidates: fields.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn allocate_type_names(&mut self) {
        let mut allocator = Allocator::default();
        for reserved in ["Int64", "Int32", "Nullable"] {
            allocator.reserve(reserved);
        }
        for id in &self.ir.order {
            let def = &self.ir.types[id];
            if matches!(def.kind, TypeKind::Extern) {
                continue;
            }
            let base = match def
                .component
                .as_ref()
                .and_then(|c| self.config.types.get(c))
                .and_then(|t| t.rename.clone())
            {
                Some(rename) => rename,
                None => pascal(&def.suggested_name),
            };
            let name = allocator.allocate(legal(base));
            self.names.insert(id.clone(), name);
        }
    }

    fn type_config(&self, def: &TypeDef) -> Option<&TypeConfig> {
        def.component
            .as_ref()
            .and_then(|c| self.config.types.get(c))
    }

    fn field_config<'c>(&'c self, def: &TypeDef, raw_name: &str) -> Option<&'c FieldConfig> {
        self.type_config(def).and_then(|t| t.fields.get(raw_name))
    }

    /// Spell a type for Rust.
    fn spell(&mut self, ty: &TypeRef) -> Result<String, GenerateError> {
        Ok(match ty {
            TypeRef::Named(id) => match &self.ir.types[id].kind {
                TypeKind::Extern => {
                    let component = self.ir.types[id].component.clone().unwrap_or_default();
                    self.extern_spelling(&component)?
                }
                _ => self.names[id].clone(),
            },
            TypeRef::Boxed(inner) => format!("Box<{}>", self.spell(inner)?),
            TypeRef::Nullable(inner) => format!("Option<{}>", self.spell(inner)?),
            TypeRef::String => "String".into(),
            TypeRef::Integer(IntWidth::Int64) => {
                self.support.int64 = true;
                "Int64".into()
            }
            TypeRef::Integer(IntWidth::Int32) => {
                self.support.int32 = true;
                "Int32".into()
            }
            TypeRef::Number(FloatWidth::Double) => "f64".into(),
            TypeRef::Number(FloatWidth::Float) => "f32".into(),
            TypeRef::Boolean => "bool".into(),
            TypeRef::Null => "()".into(),
            TypeRef::Any => "serde_json::Value".into(),
            TypeRef::Array(inner) => format!("Vec<{}>", self.spell(inner)?),
            TypeRef::Map(inner) => {
                format!("std::collections::BTreeMap<String, {}>", self.spell(inner)?)
            }
            TypeRef::Extern(key) => self.extern_spelling(key)?,
        })
    }

    fn extern_spelling(&mut self, key: &str) -> Result<String, GenerateError> {
        let spelling = self
            .config
            .substitutions
            .get(key)
            .and_then(|s| s.rust.as_ref())
            .ok_or_else(|| {
                GenerateError::Config(format!("substitution `{key}` has no `rust` spelling"))
            })?;
        if let Some(import) = &spelling.import {
            self.externs.insert(key.to_owned(), import.clone());
        }
        Ok(spelling.name.clone())
    }

    fn doc(
        description: Option<&str>,
        default: Option<&serde_json::Value>,
        read_only: bool,
        write_only: bool,
        constraints: &Constraints,
    ) -> Vec<String> {
        let mut lines: Vec<String> = description
            .map(|d| d.lines().map(|l| l.trim_end().to_owned()).collect())
            .unwrap_or_default();
        let mut notes = Vec::new();
        if let Some(default) = default {
            notes.push(format!("Default: `{default}`."));
        }
        if read_only {
            notes.push("Read only.".to_owned());
        }
        if write_only {
            notes.push("Write only.".to_owned());
        }
        notes.extend(constraint_notes(constraints));
        if !notes.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(notes);
        }
        lines
    }

    fn codegen_attrs(
        &mut self,
        extensions: &BTreeMap<String, serde_json::Value>,
        at: &SchemaId,
    ) -> Vec<String> {
        let Some(value) = extensions.get("x-rust-attrs") else {
            return Vec::new();
        };
        if !self.config.allow_codegen_extensions {
            self.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                kind: DiagnosticKind::IgnoredExtension { keyword: "x-rust-attrs".into() },
                schema_id: at.clone(),
                pointer: format!("{}/x-rust-attrs", at.pointer),
                message: "`x-rust-attrs` is code from the description and is not applied unless `allow_codegen_extensions` is set".into(),
            });
            return Vec::new();
        }
        match value {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn type_view(&mut self, def: &TypeDef) -> Result<Option<TypeView>, GenerateError> {
        let name = match self.names.get(&def.id) {
            Some(name) => name.clone(),
            None => return Ok(None),
        };
        let raw_name = def
            .component
            .clone()
            .unwrap_or_else(|| def.id.pointer.clone());
        let mut doc = Vec::new();
        if let Some(title) = def
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty() && def.component.as_deref() != Some(t))
        {
            doc.push(title.trim().to_owned());
            doc.push(String::new());
        }
        doc.extend(Self::doc(
            def.description.as_deref(),
            None,
            false,
            false,
            &def.constraints,
        ));
        if def.nullable {
            doc.push(
                "`null` is a valid instance; a position holding this type is an `Option`."
                    .to_owned(),
            );
        }
        while doc.last().is_some_and(String::is_empty) {
            doc.pop();
        }
        let mut attrs = Vec::new();
        if def.deprecated {
            attrs.push("#[deprecated]".to_owned());
        }
        let user_attrs: Vec<String> = self
            .type_config(def)
            .map(|t| t.attrs.clone())
            .unwrap_or_default();
        check_container_attrs(&user_attrs, &raw_name)?;
        attrs.extend(user_attrs);
        attrs.extend(self.codegen_attrs(&def.extensions, &def.id));
        let mut derives: Vec<String> = ["Debug", "Clone", "PartialEq"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        derives.extend(self.config.rust.derives.iter().cloned());
        let mut view = TypeView {
            kind: "struct",
            name,
            raw_name,
            doc,
            derives,
            attrs,
            fields: Vec::new(),
            variants: Vec::new(),
            branches: Vec::new(),
            inner: String::new(),
            tag: String::new(),
            exclusive: false,
        };
        let result = self.fill_kind(def, &mut view)?;
        if !result {
            return Ok(None);
        }
        let mut seen = std::collections::BTreeSet::new();
        view.derives.retain(|d| seen.insert(d.clone()));
        Ok(Some(view))
    }

    /// Fill the kind-specific parts of a view; `false` when the type is
    /// not emitted at all.
    fn fill_kind(&mut self, def: &TypeDef, view: &mut TypeView) -> Result<bool, GenerateError> {
        match &def.kind {
            TypeKind::Extern => return Ok(false),
            TypeKind::Struct(s) => {
                view.kind = "struct";
                view.derives
                    .extend(["Serialize".to_owned(), "Deserialize".to_owned()]);
                let mut allocator = Allocator::default();
                for field in &s.fields {
                    let rename = self
                        .field_config(def, &field.raw_name)
                        .and_then(|f| f.rename.clone());
                    let base =
                        rename.unwrap_or_else(|| snake(std::slice::from_ref(&field.raw_name)));
                    let name = allocator.allocate(legal(base));
                    let inner = self.spell(&field.ty)?;
                    let (ty, mut attrs) = match (field.required, field.nullable) {
                        (true, false) => (inner, vec![]),
                        (true, true) => {
                            self.support.require_present = true;
                            (
                                format!("Option<{inner}>"),
                                vec![
                                    "#[serde(deserialize_with = \"require_present::deserialize\")]"
                                        .to_owned(),
                                ],
                            )
                        }
                        (false, false) => {
                            self.support.reject_null = true;
                            (
                                format!("Option<{inner}>"),
                                vec!["#[serde(default, skip_serializing_if = \"Option::is_none\", deserialize_with = \"reject_null::deserialize\")]".to_owned()],
                            )
                        }
                        (false, true) => {
                            self.support.double_option = true;
                            (
                                format!("Option<Option<{inner}>>"),
                                vec!["#[serde(default, skip_serializing_if = \"Option::is_none\", deserialize_with = \"double_option::deserialize\")]".to_owned()],
                            )
                        }
                    };
                    if name.trim_start_matches("r#") != field.raw_name {
                        attrs.insert(0, format!("#[serde(rename = {:?})]", field.raw_name));
                    }
                    if field.deprecated {
                        attrs.push("#[deprecated]".to_owned());
                    }
                    let user_attrs: Vec<String> = self
                        .field_config(def, &field.raw_name)
                        .map(|f| f.attrs.clone())
                        .unwrap_or_default();
                    check_field_attrs(
                        &user_attrs,
                        &format!("{}.{}", view.raw_name, field.raw_name),
                    )?;
                    attrs.extend(user_attrs);
                    let mut at = def.id.clone();
                    at.pointer = format!("{}/properties/{}", def.id.pointer, field.raw_name);
                    attrs.extend(self.codegen_attrs(&field.extensions, &at));
                    view.fields.push(FieldView {
                        name,
                        raw_name: field.raw_name.clone(),
                        ty,
                        doc: Self::doc(
                            field.description.as_deref(),
                            field.default.as_ref(),
                            field.read_only,
                            field.write_only,
                            &field.constraints,
                        ),
                        attrs,
                    });
                }
                match &s.additional {
                    Additional::Denied => {
                        view.attrs.push("#[serde(deny_unknown_fields)]".to_owned())
                    }
                    Additional::CatchAll(ty) => {
                        let inner = self.spell(ty)?;
                        let name = allocator.allocate("additional_properties".to_owned());
                        view.fields.push(FieldView {
                            name,
                            raw_name: "additionalProperties".to_owned(),
                            ty: format!("std::collections::BTreeMap<String, {inner}>"),
                            doc: vec!["Members not named above.".to_owned()],
                            attrs: vec!["#[serde(flatten)]".to_owned()],
                        });
                    }
                }
            }
            TypeKind::StringEnum(values) => {
                view.kind = "enum";
                view.derives.extend([
                    "Eq".to_owned(),
                    "Hash".to_owned(),
                    "Serialize".to_owned(),
                    "Deserialize".to_owned(),
                ]);
                let mut allocator = Allocator::default();
                for value in values {
                    let base = if value.trim().is_empty() {
                        "Empty".to_owned()
                    } else {
                        pascal(std::slice::from_ref(value))
                    };
                    let name = allocator.allocate(legal(base));
                    view.variants.push(VariantView {
                        name,
                        value: value.clone(),
                        ty: String::new(),
                        doc: Vec::new(),
                        attrs: vec![format!("#[serde(rename = {value:?})]")],
                    });
                }
            }
            TypeKind::UnionTagged(u) => {
                view.kind = "union_tagged";
                view.derives.push("Serialize".to_owned());
                view.tag = u.tag.clone();
                let mut allocator = Allocator::default();
                for variant in &u.variants {
                    let name = allocator.allocate(legal(pascal(&variant.suggested_name)));
                    let ty = self.spell(&variant.ty)?;
                    view.variants.push(VariantView {
                        name,
                        value: variant.value.clone(),
                        ty,
                        doc: vec![format!("`{}: {:?}`", u.tag, variant.value)],
                        attrs: Vec::new(),
                    });
                }
            }
            TypeKind::UnionOpen(u) => {
                view.kind = "union_open";
                // The raw value has no `PartialEq`; the template compares text.
                view.derives.retain(|d| d != "PartialEq");
                view.derives
                    .extend(["Serialize".to_owned(), "Deserialize".to_owned()]);
                view.exclusive = u.exclusive;
                self.support.raw_holder = true;
                view.branches = self.branches(&u.branches)?;
            }
            TypeKind::ComposedOpen(branches) => {
                view.kind = "composed_open";
                // The raw value has no `PartialEq`; the template compares text.
                view.derives.retain(|d| d != "PartialEq");
                view.derives
                    .extend(["Serialize".to_owned(), "Deserialize".to_owned()]);
                self.support.raw_holder = true;
                view.branches = self.branches(branches)?;
            }
            TypeKind::Newtype(inner) => {
                view.kind = "newtype";
                view.derives
                    .extend(["Serialize".to_owned(), "Deserialize".to_owned()]);
                view.attrs.push("#[serde(transparent)]".to_owned());
                view.inner = self.spell(inner)?;
            }
            TypeKind::Alias(inner) => {
                view.kind = "alias";
                view.derives.clear();
                view.inner = self.spell(inner)?;
            }
        }
        Ok(true)
    }

    fn branches(
        &mut self,
        branches: &[crate::ir::Branch],
    ) -> Result<Vec<BranchView>, GenerateError> {
        let mut allocator = Allocator::default();
        let mut out = Vec::new();
        for branch in branches {
            let base = snake(&branch.suggested_name);
            let name = allocator.allocate(base);
            let ty = self.spell(&branch.ty)?;
            out.push(BranchView {
                accessor: format!("as_{name}"),
                constructor: format!("from_{name}"),
                doc: vec![format!("The value read as `{ty}`, when it is one.")],
                ty,
            });
        }
        Ok(out)
    }
}
