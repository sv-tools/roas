//! The view model: what templates see. Every type is already spelled
//! for Rust, every name is final, every attribute is decided. A
//! template lays this out and nothing more.

use crate::config::{Config, FieldConfig, TypeConfig};
use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
use crate::front::{Constraints, FloatWidth, IntWidth};
use crate::generate::GenerateError;
use crate::ir::{Additional, Ir, TypeDef, TypeKind, TypeRef};
use crate::rust::attrs::{cfg_gates, check_container_attrs, check_field_attrs, serde_aliases};
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
    /// A closed struct reads through `ObjectOnly`.
    pub object_only: bool,
    /// A string enum reads through `StringOnly`.
    pub string_only: bool,
    /// A member beside `patternProperties` is held as `RawJson`.
    pub raw_json: bool,
    /// A struct with a catch-all reads through `roas_catch_all`.
    pub catch_all: bool,
    /// Generated validation is on: the report types and `Validate`.
    pub validation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FieldView {
    pub name: String,
    pub raw_name: String,
    pub ty: String,
    pub doc: Vec<String>,
    pub attrs: Vec<String>,
    /// The field's conditional-compilation gates, which the validation
    /// reading it repeats.
    #[serde(skip)]
    pub cfg: Vec<String>,
}

/// A struct's catch-all, decoded as the members stream by.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CatchAllView {
    /// The catch-all field.
    pub field: String,
    /// The type of one of its values.
    pub ty: String,
    /// The members routed to the derived code, one entry per field.
    pub declared: Vec<DeclaredView>,
    /// A Rust function from a member name no field reads to whether it is
    /// let in: beside `additionalProperties: false`, only one matching a
    /// pattern of `patternProperties`.
    pub allowed: String,
}

/// The names one declared field is read by.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DeclaredView {
    /// The field's conditional-compilation gates.
    pub cfg: Vec<String>,
    /// A Rust pattern: the quoted name and aliases, joined by `|`.
    pub names: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VariantView {
    pub name: String,
    pub value: String,
    /// The payload type, for a tagged union; empty for a string enum.
    pub ty: String,
    pub doc: Vec<String>,
    pub attrs: Vec<String>,
    /// The variant's conditional-compilation gates, repeated on every
    /// match arm naming it.
    pub cfg: Vec<String>,
    /// The wire value as a Rust string literal.
    pub literal: String,
    /// The value is made up: no tag selects the variant.
    pub synthetic: bool,
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
    /// For a tagged union, the index of the variant an unknown or missing
    /// tag is read as; `None` refuses such a value.
    pub fallback: Option<usize>,
    /// For a union read by a parent's discriminator, the index of the
    /// parent's variant: what the fallback falls back to when compiled out.
    pub parent_variant: Option<usize>,
    /// The discriminator property as a Rust string literal.
    pub tag_literal: String,
    /// The tag as it reads in a doc comment.
    pub tag_doc: String,
    /// `oneOf` rather than `anyOf`, for an open union's documentation.
    pub exclusive: bool,
    /// The statements of `Validate::validate_into`, with validation on;
    /// `None` for no implementation (an alias shares its target's).
    pub validate: Option<Vec<String>>,
    /// The statements of `Validate::validate_null`; empty for none.
    pub validate_null: Vec<String>,
    /// A struct read from a JSON object only: serde's derive would also
    /// read it from an array, so it decodes through a guard.
    pub object_only: bool,
    /// A string enum read from a JSON string only: serde's derive would
    /// also read `{"a": null}` as `"a"`.
    pub string_only: bool,
    /// A struct with a catch-all: the members it does not declare decode
    /// into the catch-all as they stream by.
    pub catch_all: Option<CatchAllView>,
    /// The Rust spelling of each `patternProperties` schema, in order.
    #[serde(skip)]
    pub pattern_tys: Vec<Option<String>>,
    /// The Rust spelling of the `additionalProperties` schema beside them.
    #[serde(skip)]
    pub others_ty: Option<String>,
    /// The type's conditional-compilation gates (`#[cfg(...)]` among its
    /// configured attributes), repeated on every impl generated beside it.
    pub cfg: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileView {
    /// The contract version this context follows.
    pub template_api: u32,
    pub source: String,
    pub header: String,
    pub imports: Vec<String>,
    pub extra_imports: Vec<String>,
    pub support: SupportView,
    pub types: Vec<TypeView>,
}

/// A Rust dependency the generated code needs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct RustDependency {
    pub name: String,
    pub features: Vec<String>,
}

impl RustDependency {
    pub fn new(name: impl Into<String>, features: Vec<String>) -> Self {
        RustDependency {
            name: name.into(),
            features,
        }
    }
}

/// The constraints a schema declares, as documentation until generated
/// validation lands; the type itself does not enforce them.
pub(crate) fn constraint_notes(c: &Constraints) -> Vec<String> {
    let mut notes = Vec::new();
    let mut push = |keyword: &str, value: String| notes.push(format!("`{keyword}`: {value}."));
    if let Some(v) = c.min_length {
        push("minLength", v.to_string());
    }
    if let Some(v) = c.max_length {
        push("maxLength", v.to_string());
    }
    if let Some(v) = &c.pattern {
        push("pattern", code_span(&v.escape_debug().to_string()));
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
    select_check(ir, config)?;
    builder.allocate_type_names();
    // Reset: only what this build renders counts.
    super::validation::take_uses_regex();
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
    let uses_regex = super::validation::take_uses_regex();
    let mut dependencies = vec![
        RustDependency {
            name: "serde".into(),
            features: vec!["derive".into()],
        },
        RustDependency {
            name: "serde_json".into(),
            // Validation compares a decoded number with a bound read
            // exactly: the decoder must read it exactly too.
            features: if builder.config.validation {
                vec!["raw_value".into(), "float_roundtrip".into()]
            } else {
                vec!["raw_value".into()]
            },
        },
    ];
    // A checked `pattern` is matched with `regex`.
    if uses_regex {
        dependencies.push(RustDependency {
            name: "regex".into(),
            features: Vec::new(),
        });
    }
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
            template_api: crate::templates::TEMPLATE_API,
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

/// Every `types.<Name>` and `fields.<prop>` in the configuration must
/// name something in the description.
pub(crate) fn select_check(ir: &Ir, config: &Config) -> Result<(), GenerateError> {
    for (name, type_config) in &config.types {
        let Some(id) = ir.components.get(name) else {
            return Err(GenerateError::UnknownSelector {
                selector: format!("types.{name}"),
                candidates: ir.components.keys().cloned().collect(),
            });
        };
        if type_config.fields.is_empty() {
            continue;
        }
        let def = &ir.types[id];
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

impl Builder<'_> {
    fn allocate_type_names(&mut self) {
        let mut allocator = Allocator::default();
        // Names the generated code itself uses unqualified: its own
        // helpers, the prelude types it spells, and its imports.
        for reserved in [
            "Int64",
            "Int32",
            "Nullable",
            "String",
            "Option",
            "Some",
            "None",
            "Vec",
            "Box",
            "Result",
            "Ok",
            "Err",
            "Self",
            "Serialize",
            "Deserialize",
            "Default",
            "Clone",
            "Debug",
            "PartialEq",
            "Eq",
            "Hash",
            "Copy",
            "From",
            "Into",
            "double_option",
            "reject_null",
            "require_present",
            "integer_token",
        ] {
            allocator.reserve(reserved);
        }
        // Generated only beside `patternProperties`.
        if self
            .ir
            .types
            .values()
            .any(|def| matches!(&def.kind, TypeKind::Struct(s) if !s.patterns.is_empty()))
        {
            allocator.reserve("RawJson");
        }
        if self.config.validation {
            for reserved in [
                "Validate",
                "ValidationReport",
                "Finding",
                "FindingKind",
                "child_pointer",
                "wide_number",
            ] {
                allocator.reserve(reserved);
            }
        }
        // Components first: a component keeps its name whatever an inline
        // schema is titled or positioned to be called.
        let ir = self.ir;
        let order = ir
            .order
            .iter()
            .filter(|id| ir.types[*id].component.is_some());
        let rest = ir
            .order
            .iter()
            .filter(|id| ir.types[*id].component.is_none());
        for id in order.chain(rest) {
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

    /// The cfg gates a generated type carries: configured, or from the
    /// description's `x-rust-attrs` as applied.
    fn type_gates(&self, id: &SchemaId, at: &str) -> Result<Vec<String>, GenerateError> {
        let Some(def) = self.ir.types.get(id) else {
            return Ok(Vec::new());
        };
        let mut attrs: Vec<String> = self
            .type_config(def)
            .map(|t| t.attrs.clone())
            .unwrap_or_default();
        if self.config.allow_codegen_extensions {
            let described: Vec<String> = match def.extensions.get("x-rust-attrs") {
                Some(serde_json::Value::String(a)) => vec![a.clone()],
                Some(serde_json::Value::Array(items)) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                _ => Vec::new(),
            };
            attrs.extend(
                described
                    .into_iter()
                    .filter(|a| check_container_attrs(std::slice::from_ref(a), "").is_ok()),
            );
        }
        cfg_gates(&attrs, at)
    }

    fn type_config(&self, def: &TypeDef) -> Option<&TypeConfig> {
        def.component
            .as_ref()
            .and_then(|c| self.config.types.get(c))
    }

    /// Whether the struct `id` reads its members by other names than the
    /// schema does: a field compiled out by a `cfg` gate (its member then
    /// lands among the others) or read under a `serde(alias)`. A value
    /// valid for the schema may then fail to decode. A configured
    /// attribute that does not parse counts: generation fails on it anyway.
    fn reshaped(&self, id: &SchemaId) -> bool {
        let def = &self.ir.types[id];
        let TypeKind::Struct(s) = &def.kind else {
            return false;
        };
        s.fields.iter().any(|field| {
            let mut attrs: Vec<String> = self
                .field_config(def, &field.raw_name)
                .map(|f| f.attrs.clone())
                .unwrap_or_default();
            if self.config.allow_codegen_extensions {
                let described: Vec<String> = match field.extensions.get("x-rust-attrs") {
                    Some(serde_json::Value::String(a)) => vec![a.clone()],
                    Some(serde_json::Value::Array(items)) => items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                // Only the entries generation applies: a failing one is
                // dropped there with a warning.
                attrs.extend(
                    described
                        .into_iter()
                        .filter(|a| check_field_attrs(std::slice::from_ref(a), "").is_ok()),
                );
            }
            attrs.iter().any(|attr| {
                let one = std::slice::from_ref(attr);
                !matches!(cfg_gates(one, ""), Ok(g) if g.is_empty())
                    || !matches!(serde_aliases(one, ""), Ok(a) if a.is_empty())
            })
        })
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
        // Every entry becomes exactly one `///` line.
        lines
            .into_iter()
            .flat_map(|l| l.split(['\n', '\r']).map(str::to_owned).collect::<Vec<_>>())
            .collect()
    }

    fn codegen_attrs(
        &mut self,
        extensions: &BTreeMap<String, serde_json::Value>,
        at: &SchemaId,
        field: bool,
    ) -> Vec<String> {
        let Some(value) = extensions.get("x-rust-attrs") else {
            return Vec::new();
        };
        if !self.config.allow_codegen_extensions {
            self.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                kind: DiagnosticKind::IgnoredExtension { keyword: "x-rust-attrs".into() },
                schema_id: at.clone(),
                pointer: crate::diagnostic::source_of(&format!("{}/x-rust-attrs", at.pointer)),
                message: "`x-rust-attrs` is code from the description and is not applied unless `allow_codegen_extensions` is set".into(),
            });
            return Vec::new();
        }
        let attrs: Vec<String> = match value {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        };
        // Code from the description passes the same checks as code from
        // the configuration: valid Rust, and no backend-owned serde key.
        // A failing one is dropped with a warning rather than failing the
        // whole generation, since the description is not the user's file.
        let mut kept = Vec::new();
        for attr in attrs {
            let one = std::slice::from_ref(&attr);
            let checked = if field {
                check_field_attrs(one, &crate::diagnostic::source_of(&at.pointer))
            } else {
                check_container_attrs(one, &crate::diagnostic::source_of(&at.pointer))
            };
            match checked {
                Ok(()) => kept.push(attr),
                Err(error) => self.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    kind: DiagnosticKind::IgnoredExtension {
                        keyword: "x-rust-attrs".into(),
                    },
                    schema_id: at.clone(),
                    pointer: crate::diagnostic::source_of(&format!("{}/x-rust-attrs", at.pointer)),
                    message: format!("`x-rust-attrs` entry not applied: {error}"),
                }),
            }
        }
        kept
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
            doc.extend(title.trim().lines().map(|l| l.trim_end().to_owned()));
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
        attrs.extend(self.codegen_attrs(&def.extensions, &def.id, false));
        // A union read by a parent's discriminator exists only where the
        // parent does; each other variant is gated on its own.
        if let TypeKind::UnionTagged(u) = &def.kind
            && let Some(parent) = &u.parent
        {
            for gate in self.type_gates(parent, &raw_name)? {
                if !attrs.contains(&gate) {
                    attrs.push(gate);
                }
            }
        }
        let cfg = cfg_gates(&attrs, &raw_name)?;
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
            fallback: None,
            parent_variant: None,
            tag_literal: String::new(),
            tag_doc: String::new(),
            exclusive: false,
            validate: None,
            validate_null: Vec::new(),
            object_only: false,
            string_only: false,
            catch_all: None,
            pattern_tys: Vec::new(),
            others_ty: None,
            cfg,
        };
        let result = self.fill_kind(def, &mut view)?;
        if !result {
            return Ok(None);
        }
        if self.config.validation {
            self.support.validation = true;
            let opted_out = self.type_config(def).and_then(|t| t.validation) == Some(false);
            let emitted = |id: &SchemaId| self.names.contains_key(id);
            let generated = |ty: &TypeRef| crate::checks::validates(self.ir, &emitted, ty);
            let reshaped = |id: &SchemaId| self.reshaped(id);
            view.validate =
                super::validation::body(self.ir, def, &view, &generated, &reshaped, opted_out);
            if view.validate.is_some() && !opted_out {
                view.validate_null = super::validation::null_body(def);
                // An alias spelled as a newtype has its target's null.
                if let TypeKind::Alias(inner) = &def.kind
                    && generated(inner)
                {
                    view.validate_null = vec![format!(
                        "<{} as Validate>::validate_null(pointer, report);",
                        view.inner
                    )];
                }
            }
            if opted_out && view.validate.is_none() {
                return Err(crate::go::view::opt_out_on_alias(def));
            }
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
                let mut declared = Vec::new();
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
                    at.pointer = field.pointer.clone();
                    attrs.extend(self.codegen_attrs(&field.extensions, &at, true));
                    let cfg = cfg_gates(&attrs, &format!("{}.{}", view.raw_name, field.raw_name))?;
                    if matches!(s.additional, Additional::CatchAll(_)) {
                        let mut names = vec![field.raw_name.clone()];
                        names.extend(serde_aliases(
                            &attrs,
                            &format!("{}.{}", view.raw_name, field.raw_name),
                        )?);
                        declared.push(DeclaredView {
                            cfg: cfg.clone(),
                            names: names
                                .iter()
                                .map(|n| format!("{n:?}"))
                                .collect::<Vec<_>>()
                                .join(" | "),
                        });
                    }
                    view.fields.push(FieldView {
                        cfg,
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
                    // With no catch-all to flatten, serde's derive reads a
                    // sequence too: the derived code becomes inherent and a
                    // guard decodes an object only.
                    Additional::Denied => {
                        view.attrs.push("#[serde(deny_unknown_fields)]".to_owned());
                        view.attrs.push("#[serde(remote = \"Self\")]".to_owned());
                        view.object_only = true;
                        self.support.object_only = true;
                    }
                    Additional::CatchAll(ty) => {
                        // Beside `patternProperties`, a member is held as
                        // the text it was read from, to be read again as
                        // each schema it is checked against.
                        let inner = if s.patterns.is_empty() {
                            self.spell(ty)?
                        } else {
                            self.support.raw_json = true;
                            "RawJson".to_owned()
                        };
                        for pattern in &s.patterns {
                            let spelled = match pattern.branch() {
                                Some(b) => Some(self.spell(&b.ty)?),
                                None => None,
                            };
                            view.pattern_tys.push(spelled);
                        }
                        if let crate::ir::Others::Schema(b) = &s.others {
                            view.others_ty = Some(self.spell(&b.ty)?);
                        }
                        let name = allocator.allocate("additional_properties".to_owned());
                        view.fields.push(FieldView {
                            name: name.clone(),
                            raw_name: "additionalProperties".to_owned(),
                            ty: format!("std::collections::BTreeMap<String, {inner}>"),
                            doc: vec!["Members not named above.".to_owned()],
                            // Serialized inline; read by the impls in
                            // `rust/catch_all.rs.jinja`, not by `flatten`.
                            attrs: vec!["#[serde(flatten, skip_deserializing)]".to_owned()],
                            cfg: Vec::new(),
                        });
                        // serde's `flatten` buffers the members it does not
                        // know, and an integer or a raw holder cannot be read
                        // from the buffer: the derived code becomes inherent,
                        // and the declared members are handed to it while the
                        // rest decode into the catch-all as they stream by.
                        view.attrs.push("#[serde(remote = \"Self\")]".to_owned());
                        let allowed = match &s.only_patterns {
                            None => "|_: &str| true".to_owned(),
                            Some(patterns) => {
                                super::validation::mark_uses_regex();
                                let list = patterns
                                    .iter()
                                    .map(|p| format!("{p:?}"))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                // A pattern the engine cannot compile lets
                                // every member in rather than refuse one it
                                // allows.
                                format!(
                                    "|name: &str| {{ static PATTERNS: std::sync::LazyLock<Option<Vec<regex::Regex>>> = std::sync::LazyLock::new(|| [{list}].iter().map(|p| regex::Regex::new(p).ok()).collect()); PATTERNS.as_ref().is_none_or(|patterns| patterns.iter().any(|re| re.is_match(name))) }}"
                                )
                            }
                        };
                        view.catch_all = Some(CatchAllView {
                            field: name,
                            ty: inner,
                            declared,
                            allowed,
                        });
                        self.support.catch_all = true;
                    }
                }
            }
            TypeKind::StringEnum(values) => {
                view.kind = "enum";
                // serde's derive also reads a single-member object as a
                // variant: the derived code becomes inherent and a guard
                // reads a JSON string only.
                view.attrs.push("#[serde(remote = \"Self\")]".to_owned());
                view.string_only = true;
                self.support.string_only = true;
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
                        cfg: Vec::new(),
                        literal: format!("{value:?}"),
                        synthetic: false,
                    });
                }
            }
            TypeKind::UnionTagged(u) => {
                view.kind = "union_tagged";
                view.derives.push("Serialize".to_owned());
                view.tag = u.tag.clone();
                view.tag_literal = format!("{:?}", u.tag);
                view.tag_doc = code_span(&u.tag.escape_debug().to_string());
                view.fallback = u.fallback;
                view.parent_variant = u.parent.as_ref().and_then(|p| {
                    u.variants
                        .iter()
                        .position(|v| v.ty.direct_named() == Some(p))
                });
                let mut allocator = Allocator::default();
                for (index, variant) in u.variants.iter().enumerate() {
                    let name = allocator.allocate(legal(pascal(&variant.suggested_name)));
                    let ty = self.spell(&variant.ty)?;
                    // Beside a parent, a variant gated out leaves its tag to
                    // the fallback; the union itself carries the parent's
                    // gates.
                    let cfg = match variant.ty.direct_named() {
                        Some(id) if u.parent.is_some() => self
                            .type_gates(id, &view.raw_name)?
                            .into_iter()
                            .filter(|g| !view.cfg.contains(g))
                            .collect(),
                        _ => Vec::new(),
                    };
                    view.variants.push(VariantView {
                        name,
                        value: variant.value.clone(),
                        ty,
                        doc: vec![if !variant.synthetic {
                            code_span(&format!("{}: {:?}", u.tag.escape_debug(), variant.value))
                        } else if u.fallback == Some(index) {
                            format!("Read when {} names no other variant.", view.tag_doc)
                        } else {
                            "No tag selects this variant: it is read only when the fallback is not available.".to_owned()
                        }],
                        attrs: cfg.clone(),
                        cfg,
                        literal: format!("{:?}", variant.value),
                        synthetic: variant.synthetic,
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
                view.inner = self.spell(inner)?;
                if view.attrs.is_empty() {
                    view.kind = "alias";
                    view.derives.clear();
                } else {
                    // A type alias cannot carry most attributes, and a derive
                    // on one does not compile; a transparent newtype carries
                    // them all.
                    view.kind = "newtype";
                    view.derives
                        .extend(["Serialize".to_owned(), "Deserialize".to_owned()]);
                    view.attrs.push("#[serde(transparent)]".to_owned());
                }
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

/// `text` as a Markdown code span, fenced by more backticks than any run
/// in it.
fn code_span(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}
