//! The view model for Go: every type spelled, every name final, every
//! literal quoted. A template lays this out and nothing more.
//!
//! The decisions are the Rust backend's, carried over: the four
//! nullability cases, integers read from their tokens, tagged unions
//! decoded by reading the tag and then the whole value, and open holders
//! keeping the raw JSON. What differs is only how Go spells them — and
//! that Go's decoder stores a zero value for a JSON null where Rust's
//! refuses it, which the generated code turns back into an error.

use crate::config::{Config, FieldConfig, Layout, TypeConfig};
use crate::diagnostic::{Diagnostic, SchemaId};
use crate::front::{FloatWidth, IntWidth};
use crate::generate::GenerateError;
use crate::go::naming::{
    comment_text, exported, is_exported_ident, is_tag_key, quote, tag_name_ok,
};
use crate::ir::{Additional, Ir, TypeDef, TypeKind, TypeRef};
use crate::rust::naming::Allocator;
use crate::rust::view::{constraint_notes, select_check};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The packages every generated file imports, by import path and the
/// name they are used under.
const BUILTIN_IMPORTS: &[(&str, &str)] = &[
    ("encoding/json/jsontext", "jsontext"),
    ("encoding/json/v2", "json"),
    ("errors", "errors"),
    ("fmt", "fmt"),
];

/// Lowercase names the templates declare, at package level or locally; an
/// import used under one of them would not compile.
/// The names generated validation adds, reserved only when it is on.
const VALIDATION_NAMES: &[&str] = &[
    "certain",
    "uncertain",
    "branch",
    "repeated",
    "childPointer",
    "wideNumber",
    "members",
    "seen",
    "item",
    "p",
    "x",
    "f",
    "zero",
];

/// The package-level names the pattern support declares, reserved only
/// when it is generated.
const PATTERN_NAMES: &[&str] = &["matchPattern", "patterns"];

/// The names the `Decimal` support declares, at package level or locally,
/// reserved only when it is generated.
const DECIMAL_NAMES: &[&str] = &[
    "decimalToken",
    "decimalDigits",
    "power",
    "all",
    "start",
    "end",
    "count",
    "leading",
    "sx",
    "sy",
    "lx",
    "ly",
    "dx",
    "dy",
    "c",
    "b",
    "shift",
    "quotient",
    "remainder",
    "prime",
    "taken",
    "m",
    "y",
    "divisor",
];

const TEMPLATE_NAMES: &[&str] = &[
    "decodeOptions",
    "noUnmarshalers",
    "isolatedNone",
    "refuseNull",
    "readTag",
    "integerToken",
    "v",
    "u",
    "d",
    "e",
    "n",
    "raw",
    "err",
    "ok",
    "zero",
    "tag",
    "typ",
    "value",
    "s",
    "token",
    "name",
    "present",
    "members",
    "member",
    "limit",
    "text",
    "negative",
    "mantissa",
    "exponent",
    "digits",
    "negativeExponent",
    "whole",
    "fraction",
    "scale",
    "ceiling",
    "magnitude",
    "digit",
    "i",
    "j",
];

#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct SupportView {
    pub nullable: bool,
    pub int64: bool,
    pub int32: bool,
    /// A `number` held exactly, under `exact_numbers`: `Decimal`.
    pub decimal: bool,
    /// `exact_numbers` is on: the support reads numbers as decimals may.
    pub exact_numbers: bool,
    pub null: bool,
    pub tagged: bool,
    /// A self-writing struct has a catch-all, written in key order.
    pub sorted_keys: bool,
    /// Types a JSON null would silently set to their zero value, which
    /// the generated decoders refuse.
    pub refuse_null: Vec<String>,
    /// Substituted types likewise, whose own decoding then runs without
    /// the generated rules, as the substitution decides the rest itself.
    pub refuse_null_isolated: Vec<String>,
    /// Substituted types that hold null themselves: decoded without the
    /// generated rules, null included.
    pub isolated: Vec<String>,
    /// Generated validation is on: the report types and `Validator`.
    pub validation: bool,
    /// A pattern is matched, by validation or at decode: `matchPattern`.
    pub patterns: bool,
    /// A struct lets in only members matching a pattern: `memberMatches`.
    pub member_patterns: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FieldView {
    pub name: String,
    pub raw_name: String,
    pub ty: String,
    /// The whole struct tag, as a Go string literal.
    pub tag: String,
    pub doc: Vec<String>,
    /// `raw_name` as a Go string literal.
    pub literal: String,
    /// The decoder refuses a `null` for this member.
    pub non_null: bool,
    /// The member's slot among the required ones, if it is required.
    pub required_index: Option<usize>,
    /// The embedded map collecting members no field names.
    pub catch_all: bool,
    /// For a member left out when absent, the Go condition under which
    /// it is written; empty when it always is.
    pub present: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VariantView {
    /// The constant, for a string enum; the variant's own name, for a
    /// tagged union.
    pub name: String,
    pub value: String,
    /// `value` as a Go string literal.
    pub literal: String,
    /// The payload type, for a tagged union; empty for a string enum.
    pub ty: String,
    pub accessor: String,
    pub constructor: String,
    /// The value is made up: no tag selects the variant.
    pub synthetic: bool,
    /// Every wire value selecting the variant, as a `case` list.
    pub cases: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BranchView {
    pub accessor: String,
    pub constructor: String,
    pub ty: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypeView {
    /// `struct`, `enum`, `union_tagged`, `union_open`, `composed_open`,
    /// `newtype`, `alias`.
    pub kind: &'static str,
    pub name: String,
    /// `name` as a Go string literal, for error messages.
    pub name_literal: String,
    pub raw_name: String,
    pub doc: Vec<String>,
    pub fields: Vec<FieldView>,
    pub variants: Vec<VariantView>,
    pub branches: Vec<BranchView>,
    /// Inner type of a newtype or alias.
    pub inner: String,
    /// Discriminator property of a tagged union, and as a literal.
    pub tag: String,
    /// For a tagged union, the index of the variant an unknown or missing
    /// tag is read as; `None` refuses such a value.
    pub fallback: Option<usize>,
    pub tag_literal: String,
    /// `oneOf` rather than `anyOf`, for an open union's documentation.
    pub exclusive: bool,
    /// How many of a struct's members are required.
    pub required_count: usize,
    /// The value type of a struct's catch-all map; empty when members it
    /// does not name are refused.
    pub extra_ty: String,
    /// The catch-all field's name.
    pub extra_name: String,
    /// Beside `additionalProperties: false`, the quoted patterns a member
    /// no field reads must match.
    pub member_patterns: Vec<String>,
    /// The Go spelling of each `patternProperties` schema, in order.
    #[serde(skip)]
    pub pattern_tys: Vec<Option<String>>,
    /// The Go spelling of the `additionalProperties` schema beside them.
    #[serde(skip)]
    pub others_ty: Option<String>,
    /// A member name no struct tag can carry: the struct writes itself.
    pub marshal: bool,
    /// The raw constructor of a composed holder.
    pub from_raw: String,
    /// The statements of `validateInto`, with validation on; `None` for
    /// no methods of its own (an alias).
    pub validate: Option<Vec<String>>,
    /// The statements of `validateNull`; empty for none.
    pub validate_null: Vec<String>,
    /// Under `layout = "file_per_type"`, the stem of the file the type is
    /// written to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileView {
    /// The contract version this context follows.
    pub template_api: u32,
    pub source: String,
    pub header: String,
    pub package: String,
    /// Import specs, each a quoted path.
    pub imports: Vec<String>,
    pub extra_imports: Vec<String>,
    pub support: SupportView,
    pub types: Vec<TypeView>,
}

/// A Go module the generated code imports from, beyond the standard
/// library.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct GoDependency {
    /// The import path, as the substitution gives it.
    pub import: String,
}

impl GoDependency {
    pub fn new(import: impl Into<String>) -> Self {
        GoDependency {
            import: import.into(),
        }
    }
}

pub(crate) struct Built {
    pub file: FileView,
    /// For each of `file.imports`, the names a file using it qualifies
    /// with: what decides, under `layout = "file_per_type"`, which files
    /// import it.
    pub import_names: Vec<BTreeSet<String>>,
    pub dependencies: Vec<GoDependency>,
    pub diagnostics: Vec<Diagnostic>,
}

struct Builder<'a> {
    /// Imports the validation checks need.
    needs: super::validation::Needs,
    ir: &'a Ir,
    config: &'a Config,
    names: BTreeMap<SchemaId, String>,
    /// Package scope: types, constants and constructor functions.
    package_names: Allocator,
    suppressed: BTreeSet<SchemaId>,
    support: SupportView,
    refuse_null: BTreeSet<String>,
    refuse_null_isolated: BTreeSet<String>,
    isolated: BTreeSet<String>,
    externs: BTreeSet<String>,
    /// For each substitution import, the qualifiers its spellings use.
    extern_names: BTreeMap<String, BTreeSet<String>>,
    diagnostics: Vec<Diagnostic>,
}

/// The package name an import path is used under, as far as the path
/// says: its last element, skipping a `/vN` major-version suffix.
pub(crate) fn package_of(import: &str) -> &str {
    let mut elements = import.rsplit('/');
    let last = elements.next().unwrap_or(import);
    let major =
        last.len() > 1 && last.starts_with('v') && last[1..].bytes().all(|b| b.is_ascii_digit());
    match elements.next() {
        Some(previous) if major => previous,
        _ => last,
    }
}

pub(crate) fn build(ir: &Ir, config: &Config, source: &str) -> Result<Built, GenerateError> {
    let mut builder = Builder {
        needs: Default::default(),
        ir,
        config,
        names: BTreeMap::new(),
        package_names: Allocator::default(),
        suppressed: ir.suppressed.clone(),
        support: SupportView {
            exact_numbers: config.exact_numbers,
            ..SupportView::default()
        },
        refuse_null: BTreeSet::new(),
        refuse_null_isolated: BTreeSet::new(),
        isolated: BTreeSet::new(),
        externs: BTreeSet::new(),
        extern_names: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    select_check(ir, config)?;
    builder.allocate_type_names()?;
    let mut types = Vec::new();
    for id in &ir.order {
        if builder.suppressed.contains(id) {
            continue;
        }
        if let Some(view) = builder.type_view(&ir.types[id])? {
            types.push(view);
        }
    }
    builder.support.refuse_null = builder.refuse_null.iter().cloned().collect();
    // A substitution spelled like a generated type (`string`, say) is
    // refused like that type; the first registered function would win.
    builder.support.refuse_null_isolated = builder
        .refuse_null_isolated
        .difference(&builder.refuse_null)
        .cloned()
        .collect();
    builder.support.isolated = builder.isolated.iter().cloned().collect();
    if config.layout == Layout::FilePerType {
        assign_files(&mut types, config);
    }
    let mut imports = Vec::new();
    let mut import_names = Vec::new();
    let mut dependencies = Vec::new();
    if !types.is_empty() {
        let mut builtin = BUILTIN_IMPORTS.to_vec();
        if builder.support.sorted_keys || builder.needs.sorted {
            builtin.extend([("maps", "maps"), ("slices", "slices")]);
        } else if builder.needs.slices {
            builtin.push(("slices", "slices"));
        }
        if builder.needs.math {
            builtin.push(("math", "math"));
        }
        if builder.support.decimal {
            builtin.extend([("math/big", "big"), ("strconv", "strconv")]);
        }
        if builder.needs.utf8 {
            builtin.push(("unicode/utf8", "utf8"));
        }
        if builder.needs.pattern {
            builtin.extend([("regexp", "regexp"), ("sync", "sync")]);
            builder.support.patterns = true;
        }
        imports.extend(builtin.iter().map(|(path, _)| quote(path)));
        import_names.extend(
            builtin
                .iter()
                .map(|(_, name)| BTreeSet::from([(*name).to_owned()])),
        );
        let mut used: BTreeMap<&str, &str> =
            builtin.iter().map(|(path, name)| (*name, *path)).collect();
        // A package named like a name the generated code declares would be
        // shadowed by it, or clash with it.
        for local in TEMPLATE_NAMES {
            used.insert(local, "a name the generated code declares");
        }
        if builder.needs.pattern {
            for local in PATTERN_NAMES {
                used.insert(local, "a name the generated code declares");
            }
        }
        if builder.support.member_patterns {
            used.insert("memberMatches", "a name the generated code declares");
        }
        if builder.support.decimal {
            for local in DECIMAL_NAMES {
                used.insert(local, "a name the generated code declares");
            }
        }
        if config.validation {
            for local in VALIDATION_NAMES {
                used.insert(local, "a name the generated validation declares");
            }
        }
        // A file whose only `r.` is validation's report would import a
        // package named `r`, which goes by its uses under `file_per_type`.
        if config.validation && config.layout == Layout::FilePerType {
            used.insert("r", "a name the generated validation declares");
        }
        for import in &builder.externs {
            if builtin.iter().any(|(path, _)| path == import) {
                continue;
            }
            let name = package_of(import);
            if let Some(path) = used.insert(name, import) {
                return Err(GenerateError::Config(format!(
                    "substitution import {import:?} is used as `{name}`, which the generated code already imports as {path:?}"
                )));
            }
            imports.push(quote(import));
            let mut names = builder
                .extern_names
                .get(import)
                .cloned()
                .unwrap_or_default();
            names.insert(name.to_owned());
            import_names.push(names);
            // The standard library is no dependency to declare; a module
            // path has a dot in its first element.
            if import
                .split('/')
                .next()
                .is_some_and(|first| first.contains('.'))
            {
                dependencies.push(GoDependency::new(import.clone()));
            }
        }
        // Under `file_per_type` an extra import goes to the files naming it,
        // and a name the generated code also declares, or imports, would
        // send it to files that only use that.
        if config.layout == Layout::FilePerType {
            for spec in &config.extra_imports {
                let names = super::imports::spec_names(spec).unwrap_or_default();
                if let Some((name, what)) = names.iter().find_map(|n| {
                    used.get(n.as_str()).map(|what| (n, *what)).or_else(|| {
                        // Or the name a substitution's spelling uses.
                        builder
                            .extern_names
                            .iter()
                            .find(|(_, names)| names.contains(n))
                            .map(|(import, _)| (n, import.as_str()))
                    })
                }) {
                    return Err(GenerateError::Config(format!(
                        "`extra_imports` entry {spec:?} is used as `{name}`, which is taken by {what}; under `layout = \"file_per_type\"` give it another name"
                    )));
                }
            }
        }
    }
    Ok(Built {
        file: FileView {
            template_api: crate::templates::TEMPLATE_API,
            source: comment_text(source),
            header: config.header.clone().unwrap_or_default(),
            package: config.go.package.clone(),
            imports,
            extra_imports: config.extra_imports.clone(),
            support: builder.support,
            types,
        },
        import_names,
        dependencies,
        diagnostics: builder.diagnostics,
    })
}

/// Names the file of every type under `layout = "file_per_type"`: its
/// name in snake case, never the package file's, one `go build` would
/// constrain, or one a file system resolves elsewhere.
fn assign_files(types: &mut [TypeView], config: &Config) {
    let mut taken = BTreeSet::from([config.go.file.clone()]);
    for t in types {
        let base =
            super::naming::go_file_name(&crate::rust::naming::snake(std::slice::from_ref(&t.name)));
        let file = (1u32..)
            .map(|i| {
                if i == 1 {
                    base.clone()
                } else {
                    format!("{base}{i}")
                }
            })
            .find(|candidate| !taken.contains(candidate) && super::naming::is_file_stem(candidate))
            .expect("a free file name");
        taken.insert(file.clone());
        t.file = Some(file);
    }
}

impl Builder<'_> {
    fn allocate_type_names(&mut self) -> Result<(), GenerateError> {
        // The helpers every file may declare.
        for reserved in ["Nullable", "Int64", "Int32", "Null"] {
            self.package_names.reserve(reserved);
        }
        if self.config.exact_numbers {
            for reserved in ["Decimal", "DecimalMaxDigits", "ParseDecimal"] {
                self.package_names.reserve(reserved);
            }
        }
        if self.config.validation {
            for reserved in [
                "ValidationReport",
                "Finding",
                "FindingKind",
                "Violation",
                "Unevaluated",
                "Validator",
            ] {
                self.package_names.reserve(reserved);
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
            if matches!(def.kind, TypeKind::Extern) || self.suppressed.contains(id) {
                continue;
            }
            let base = match self.type_config(def).and_then(|t| t.rename.clone()) {
                Some(rename) => {
                    if !is_exported_ident(&rename) {
                        return Err(GenerateError::Config(format!(
                            "`types.{}.rename = {rename:?}` must be an exported Go identifier: an ASCII capital, then letters, digits or `_`",
                            def.component.as_deref().unwrap_or_default()
                        )));
                    }
                    rename
                }
                None => exported(&def.suggested_name),
            };
            let name = self.package_names.allocate(base);
            self.names.insert(id.clone(), name);
        }
        Ok(())
    }

    fn type_config(&self, def: &TypeDef) -> Option<&TypeConfig> {
        def.component
            .as_ref()
            .and_then(|c| self.config.types.get(c))
    }

    fn field_config<'c>(&'c self, def: &TypeDef, raw_name: &str) -> Option<&'c FieldConfig> {
        self.type_config(def).and_then(|t| t.fields.get(raw_name))
    }

    /// Spell a type for Go.
    fn spell(&mut self, ty: &TypeRef) -> Result<String, GenerateError> {
        let spelled = match ty {
            TypeRef::Named(id) => {
                let def = &self.ir.types[id];
                if let TypeKind::Extern = def.kind {
                    let component = def.component.clone().unwrap_or_default();
                    let spelled = self.extern_spelling(&component)?;
                    self.refused_extern(&spelled)
                } else {
                    self.names[id].clone()
                }
            }
            // Go needs a pointer where a value would contain itself.
            TypeRef::Boxed(inner) => format!("*{}", self.spell(inner)?),
            TypeRef::Nullable(inner) => format!("*{}", self.spell(unboxed(inner))?),
            TypeRef::String => self.refused("string"),
            TypeRef::Integer(IntWidth::Int64) => {
                self.support.int64 = true;
                "Int64".into()
            }
            TypeRef::Integer(IntWidth::Int32) => {
                self.support.int32 = true;
                "Int32".into()
            }
            TypeRef::Number(FloatWidth::Double) => self.refused("float64"),
            TypeRef::Number(FloatWidth::Float) => self.refused("float32"),
            // Its own decoder refuses null.
            TypeRef::Decimal => {
                self.support.decimal = true;
                "Decimal".into()
            }
            TypeRef::Boolean => self.refused("bool"),
            TypeRef::Null => {
                self.support.null = true;
                "Null".into()
            }
            TypeRef::Any => "jsontext.Value".into(),
            TypeRef::Array(inner) => {
                let spelled = format!("[]{}", self.spell(unboxed(inner))?);
                self.refused(&spelled)
            }
            TypeRef::Map(inner) => {
                let spelled = format!("map[string]{}", self.spell(unboxed(inner))?);
                self.refused(&spelled)
            }
            TypeRef::Extern(key) => {
                let spelled = self.extern_spelling(key)?;
                self.refused_extern(&spelled)
            }
        };
        Ok(spelled)
    }

    /// A type that stores its zero value for a JSON null, so the
    /// generated decoders refuse null for it explicitly.
    /// A substituted type, refused null like any other — unless it is
    /// spelled like a type the generator itself holds null in: a pointer,
    /// `jsontext.Value`, or an interface. The refusal is per Go type, so it
    /// would reach those positions too; such a substitution decides about
    /// null itself, as its Rust spelling does.
    fn refused_extern(&mut self, spelled: &str) -> String {
        if !null_holding_spelling(spelled) {
            self.refuse_null_isolated.insert(spelled.to_owned());
        } else if spelled != "jsontext.Value" {
            // Raw JSON has nothing inside to reach.
            self.isolated.insert(spelled.to_owned());
        }
        spelled.to_owned()
    }

    fn refused(&mut self, spelled: &str) -> String {
        self.refuse_null.insert(spelled.to_owned());
        spelled.to_owned()
    }

    fn extern_spelling(&mut self, key: &str) -> Result<String, GenerateError> {
        let spelling = self
            .config
            .substitutions
            .get(key)
            .and_then(|s| s.go.as_ref())
            .ok_or_else(|| {
                GenerateError::Config(format!("substitution `{key}` has no `go` spelling"))
            })?;
        if let Some(import) = &spelling.import {
            self.externs.insert(import.clone());
            self.extern_names
                .entry(import.clone())
                .or_default()
                .extend(super::imports::qualifiers(&spelling.name));
        }
        Ok(spelling.name.clone())
    }

    fn doc(
        description: Option<&str>,
        default: Option<&serde_json::Value>,
        read_only: bool,
        write_only: bool,
        constraints: &crate::front::Constraints,
        deprecated: bool,
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
        for paragraph in [notes, deprecation(deprecated)] {
            if !paragraph.is_empty() {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.extend(paragraph);
            }
        }
        lines
            .into_iter()
            .flat_map(|l| l.split(['\n', '\r']).map(comment_text).collect::<Vec<_>>())
            .collect()
    }

    fn type_view(&mut self, def: &TypeDef) -> Result<Option<TypeView>, GenerateError> {
        let Some(name) = self.names.get(&def.id).cloned() else {
            return Ok(None);
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
            doc.extend(title.trim().lines().map(|l| comment_text(l.trim_end())));
            doc.push(String::new());
        }
        doc.extend(Self::doc(
            def.description.as_deref(),
            None,
            false,
            false,
            &def.constraints,
            false,
        ));
        if def.nullable {
            doc.push(
                "`null` is a valid instance; a position holding this type is a pointer or a `Nullable`."
                    .to_owned(),
            );
        }
        while doc.last().is_some_and(String::is_empty) {
            doc.pop();
        }
        let deprecated = deprecation(def.deprecated);
        if !deprecated.is_empty() {
            if !doc.is_empty() {
                doc.push(String::new());
            }
            doc.extend(deprecated);
        }
        let mut view = TypeView {
            kind: "struct",
            name_literal: quote(&name),
            name,
            raw_name,
            doc,
            fields: Vec::new(),
            variants: Vec::new(),
            branches: Vec::new(),
            inner: String::new(),
            tag: String::new(),
            fallback: None,
            tag_literal: String::new(),
            exclusive: false,
            required_count: 0,
            extra_ty: String::new(),
            extra_name: String::new(),
            member_patterns: Vec::new(),
            pattern_tys: Vec::new(),
            others_ty: None,
            marshal: false,
            from_raw: String::new(),
            validate: None,
            validate_null: Vec::new(),
            file: None,
        };
        if !self.fill_kind(def, &mut view)? {
            return Ok(None);
        }
        if self.config.validation {
            self.support.validation = true;
            // The validation support's number scan uses `math`.
            self.needs.math = true;
            let opted_out = self.type_config(def).and_then(|t| t.validation) == Some(false);
            let validates = |ty: &TypeRef| self.validates(ty);
            let nil_able = |ty: &TypeRef| match ty {
                TypeRef::Named(id) => matches!(
                    self.alias_target(id),
                    Some(TypeRef::Boxed(_) | TypeRef::Nullable(_))
                ),
                _ => false,
            };
            let names = &self.names;
            let ir = self.ir;
            // Through aliases, as `holds_null` sees them.
            let raw_holder = |ty: &TypeRef| {
                let mut id = ty.direct_named()?;
                for _ in 0..=ir.types.len() {
                    match &ir.types.get(id)?.kind {
                        // A JSON-value wrapper holds `null` itself.
                        TypeKind::Newtype(TypeRef::Any) => {
                            let name = names.get(id)?;
                            return Some(format!("{name}(jsontext.Value(\"null\"))"));
                        }
                        TypeKind::UnionOpen(_) | TypeKind::ComposedOpen(_) => {
                            let name = names.get(id)?;
                            return Some(format!("{name}{{raw: jsontext.Value(\"null\")}}"));
                        }
                        TypeKind::Alias(inner) | TypeKind::Newtype(inner) => {
                            id = inner.direct_named()?
                        }
                        _ => return None,
                    }
                }
                None
            };
            let holder_id = def.id.clone();
            let loops = |ty: &TypeRef| crate::checks::loops_back(ir, &holder_id, ty);
            // Through aliases, which share their target's methods.
            let null_name = |ty: &TypeRef| {
                let mut id = ty.direct_named()?;
                for _ in 0..=ir.types.len() {
                    let def = ir.types.get(id)?;
                    match &def.kind {
                        TypeKind::Alias(inner) => id = inner.direct_named()?,
                        // An opted-out type has no `validateNull`.
                        _ if !crate::checks::has_null_hook(def)
                            || self.type_config(def).and_then(|t| t.validation) == Some(false) =>
                        {
                            return None;
                        }
                        _ => return names.get(&def.id).cloned(),
                    }
                }
                None
            };
            let mut cx = super::validation::Context {
                validates: &validates,
                nil_able: &nil_able,
                raw_holder: &raw_holder,
                loops: &loops,
                null_name: &null_name,
                // A newtype over a substitution is an alias in Go, with no
                // validator of its own.
                extern_checks: &|ty: &TypeRef| {
                    crate::checks::extern_checks(ir, ty, !self.validates(ty))
                },
                ir,
                needs: self.needs,
            };
            view.validate = super::validation::body(def, &view, opted_out, &mut cx);
            if view.validate.is_some() && !opted_out {
                view.validate_null = super::validation::null_body(def, &view.name, &mut cx);
            }
            if opted_out && view.validate.is_none() {
                return Err(opt_out_on_alias(def));
            }
            self.needs = cx.needs;
        }
        Ok(Some(view))
    }

    /// Fill the kind-specific parts of a view; `false` when the type is
    /// not emitted at all.
    fn fill_kind(&mut self, def: &TypeDef, view: &mut TypeView) -> Result<bool, GenerateError> {
        match &def.kind {
            TypeKind::Extern => return Ok(false),
            TypeKind::Struct(s) => {
                view.kind = "struct";
                let mut fields = Allocator::default();
                // The methods a generated struct has.
                fields.reserve("UnmarshalJSONFrom");
                if self.config.validation {
                    fields.reserve("Validate");
                }
                fields.reserve("MarshalJSONTo");
                let mut required_count = 0;
                for field in &s.fields {
                    let config = self.field_config(def, &field.raw_name);
                    let rename = config.and_then(|f| f.rename.clone());
                    let go_tags = config.map(|f| f.go_tags.clone()).unwrap_or_default();
                    let at = format!("types.{}.fields.{}", view.raw_name, field.raw_name);
                    let base = match rename {
                        Some(rename) if !is_exported_ident(&rename) => {
                            return Err(GenerateError::Config(format!(
                                "`{at}.rename = {rename:?}` must be an exported Go identifier: an ASCII capital, then letters, digits or `_`"
                            )));
                        }
                        Some(rename) => rename,
                        None => exported(std::slice::from_ref(&field.raw_name)),
                    };
                    let name = fields.allocate(base);
                    // A type that holds `null` as a value — `{}`, `type: null`,
                    // an open holder, or a reference to one — reads it as
                    // that value, as in Rust; an optional one keeps it apart
                    // from absence, so its member is nullable in Go's terms.
                    let holds_null = self.holds_null(&field.ty);
                    let (ty, omitzero) = match (field.required, field.nullable || holds_null) {
                        (true, false) => (self.spell(&field.ty)?, false),
                        (true, true) => {
                            self.support.nullable = true;
                            (
                                format!("Nullable[{}]", self.spell(unboxed(&field.ty))?),
                                false,
                            )
                        }
                        // `omitzero` asks a pointer's pointee `IsZero` when it
                        // has one — `time.Time` does — so a present zero value
                        // would be dropped; `Nullable` asks only for absence.
                        (false, false) if self.substituted(&field.ty) => {
                            self.support.nullable = true;
                            (
                                format!("Nullable[{}]", self.spell(unboxed(&field.ty))?),
                                true,
                            )
                        }
                        (false, false) => {
                            let inner = self.spell(&field.ty)?;
                            if inner.starts_with('*') {
                                (inner, true)
                            } else {
                                (format!("*{inner}"), true)
                            }
                        }
                        (false, true) => {
                            self.support.nullable = true;
                            (
                                format!("Nullable[{}]", self.spell(unboxed(&field.ty))?),
                                true,
                            )
                        }
                    };
                    let required_index = field.required.then(|| {
                        required_count += 1;
                        required_count - 1
                    });
                    let tag = struct_tag(&field.raw_name, omitzero, &go_tags, &at)?;
                    if !tag_name_ok(&field.raw_name) {
                        view.marshal = true;
                        self.support.sorted_keys |= matches!(s.additional, Additional::CatchAll(_));
                    }
                    let present = match (omitzero, ty.starts_with("Nullable[")) {
                        (false, _) => String::new(),
                        (true, true) => format!("v.{name}.Set"),
                        (true, false) => format!("v.{name} != nil"),
                    };
                    view.fields.push(FieldView {
                        present,
                        name,
                        raw_name: field.raw_name.clone(),
                        ty,
                        tag,
                        literal: quote(&field.raw_name),
                        non_null: !field.nullable && !holds_null,
                        required_index,
                        catch_all: false,
                        doc: Self::doc(
                            field.description.as_deref(),
                            field.default.as_ref(),
                            field.read_only,
                            field.write_only,
                            &field.constraints,
                            field.deprecated,
                        ),
                    });
                }
                view.required_count = required_count;
                if let Some(patterns) = &s.only_patterns {
                    self.needs.pattern = true;
                    self.support.member_patterns = true;
                    view.member_patterns = patterns.iter().map(|p| quote(p)).collect();
                }
                for pattern in &s.patterns {
                    let spelled = match pattern.branch() {
                        Some(b) => Some(self.spell(unboxed(&b.ty))?),
                        None => None,
                    };
                    view.pattern_tys.push(spelled);
                }
                if let crate::ir::Others::Schema(b) = &s.others {
                    view.others_ty = Some(self.spell(unboxed(&b.ty))?);
                }
                if let Additional::CatchAll(ty) = &s.additional {
                    let inner = self.spell(unboxed(ty))?;
                    let name = fields.allocate("AdditionalProperties".to_owned());
                    view.extra_name = name.clone();
                    view.extra_ty = inner.clone();
                    view.fields.push(FieldView {
                        name,
                        raw_name: "additionalProperties".to_owned(),
                        ty: format!("map[string]{inner}"),
                        tag: "`json:\",embed\"`".to_owned(),
                        doc: vec!["Members not named above.".to_owned()],
                        literal: String::new(),
                        non_null: false,
                        required_index: None,
                        catch_all: true,
                        present: String::new(),
                    });
                }
            }
            TypeKind::StringEnum(values) => {
                view.kind = "enum";
                let mut seen = BTreeSet::new();
                // A repeated value is one member: a second constant would make
                // a duplicate `case`, which Go refuses.
                for value in values.iter().filter(|v| seen.insert(v.as_str())) {
                    let base = if value.trim().is_empty() {
                        "Empty".to_owned()
                    } else {
                        exported(std::slice::from_ref(value))
                    };
                    let name = self.package_names.allocate(format!("{}{base}", view.name));
                    view.variants.push(VariantView {
                        name,
                        literal: quote(value),
                        value: value.clone(),
                        ty: String::new(),
                        accessor: String::new(),
                        constructor: String::new(),
                        synthetic: false,
                        cases: String::new(),
                    });
                }
            }
            TypeKind::UnionTagged(u) => {
                view.kind = "union_tagged";
                self.support.tagged = true;
                view.tag = u.tag.clone();
                view.tag_literal = quote(&u.tag);
                view.fallback = u.fallback;
                let mut methods = Allocator::default();
                for reserved in ["Tag", "Value", "MarshalJSONTo", "UnmarshalJSONFrom"] {
                    methods.reserve(reserved);
                }
                if self.config.validation {
                    methods.reserve("Validate");
                }
                for variant in &u.variants {
                    let base = exported(&variant.suggested_name);
                    let accessor = methods.allocate(format!("As{base}"));
                    let constructor = self
                        .package_names
                        .allocate(format!("{}From{base}", view.name));
                    view.variants.push(VariantView {
                        name: base,
                        literal: quote(&variant.value),
                        value: variant.value.clone(),
                        ty: self.spell(&variant.ty)?,
                        accessor,
                        constructor,
                        synthetic: variant.synthetic,
                        cases: std::iter::once(&variant.value)
                            .chain(&variant.aliases)
                            .map(|tag| quote(tag))
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
            }
            TypeKind::UnionOpen(u) => {
                view.kind = "union_open";
                view.exclusive = u.exclusive;
                view.branches = self.branches(&view.name, &u.branches)?;
            }
            TypeKind::ComposedOpen(branches) => {
                view.kind = "composed_open";
                view.from_raw = self.package_names.allocate(format!("{}FromRaw", view.name));
                view.branches = self.branches(&view.name, branches)?;
            }
            TypeKind::Newtype(inner) | TypeKind::Alias(inner) => {
                view.inner = self.spell(inner)?;
                // Methods cannot be declared on a pointer type, nor on an
                // interface, which a substituted type may be: the generator
                // cannot tell. An alias keeps the methods of the type it
                // names, so a substitution keeps its own JSON behaviour.
                let substituted = match unboxed(inner) {
                    TypeRef::Extern(_) => true,
                    TypeRef::Named(id) => matches!(self.ir.types[id].kind, TypeKind::Extern),
                    _ => false,
                };
                view.kind = if matches!(def.kind, TypeKind::Alias(_))
                    || view.inner.starts_with('*')
                    || substituted
                {
                    "alias"
                } else {
                    "newtype"
                };
            }
        }
        Ok(true)
    }

    /// Whether the type's own decoder takes `null` as one of its values:
    /// any JSON, `null` itself, or a holder of raw JSON.
    fn holds_null(&self, ty: &TypeRef) -> bool {
        let mut ty = ty;
        // A chain of aliases is at most every type long; one closing a cycle
        // generates nothing to hold anything.
        for _ in 0..=self.ir.types.len() {
            ty = match ty {
                TypeRef::Any | TypeRef::Null => return true,
                TypeRef::Boxed(inner) => inner,
                TypeRef::Extern(key) => return self.extern_holds_null(key),
                TypeRef::Named(id) => match &self.ir.types[id].kind {
                    TypeKind::Extern => {
                        let component = self.ir.types[id].component.as_deref();
                        return self.extern_holds_null(component.unwrap_or_default());
                    }
                    TypeKind::UnionOpen(_) | TypeKind::ComposedOpen(_) => return true,
                    TypeKind::Newtype(inner) | TypeKind::Alias(inner) => inner,
                    _ => return false,
                },
                _ => return false,
            };
        }
        false
    }

    /// Whether the type is a substitution, directly or through aliases and
    /// newtypes over one (emitted as aliases).
    /// The type a named type is a Go alias of, if it is emitted as one:
    /// it then has no methods of its own.
    fn alias_target(&self, id: &SchemaId) -> Option<&TypeRef> {
        match &self.ir.types.get(id)?.kind {
            TypeKind::Alias(inner) => Some(inner),
            TypeKind::Newtype(inner)
                if matches!(inner, TypeRef::Boxed(_) | TypeRef::Nullable(_))
                    || self.substituted(inner) =>
            {
                Some(inner)
            }
            _ => None,
        }
    }

    /// Whether `ty` holds a generated type with a `validateInto` of its own,
    /// seen through the aliases Go emits.
    fn validates(&self, ty: &TypeRef) -> bool {
        let mut ids = Vec::new();
        ty.named(&mut ids);
        ids.iter().any(|id| self.validates_named(id, 0))
    }

    fn validates_named(&self, id: &SchemaId, depth: usize) -> bool {
        if depth > self.ir.types.len()
            || !self.names.contains_key(id)
            || matches!(self.ir.types[id].kind, TypeKind::Extern)
        {
            return false;
        }
        match self.alias_target(id) {
            Some(inner) => inner
                .direct_named()
                .is_some_and(|target| self.validates_named(target, depth + 1)),
            None => true,
        }
    }

    fn substituted(&self, ty: &TypeRef) -> bool {
        let mut ty = ty;
        for _ in 0..=self.ir.types.len() {
            ty = match ty {
                TypeRef::Extern(_) => return true,
                TypeRef::Boxed(inner) => inner,
                TypeRef::Named(id) => match &self.ir.types[id].kind {
                    TypeKind::Extern => return true,
                    TypeKind::Newtype(inner) | TypeKind::Alias(inner) => inner,
                    _ => return false,
                },
                _ => return false,
            };
        }
        false
    }

    /// A substituted type takes null as a value when it is spelled like
    /// one that does.
    fn extern_holds_null(&self, key: &str) -> bool {
        self.config
            .substitutions
            .get(key)
            .and_then(|s| s.go.as_ref())
            .is_some_and(|go| null_holding_spelling(&go.name))
    }

    fn branches(
        &mut self,
        owner: &str,
        branches: &[crate::ir::Branch],
    ) -> Result<Vec<BranchView>, GenerateError> {
        let mut methods = Allocator::default();
        for reserved in ["Raw", "MarshalJSONTo", "UnmarshalJSONFrom"] {
            methods.reserve(reserved);
        }
        if self.config.validation {
            methods.reserve("Validate");
        }
        let mut out = Vec::new();
        for branch in branches {
            let base = exported(&branch.suggested_name);
            out.push(BranchView {
                accessor: methods.allocate(format!("As{base}")),
                constructor: self.package_names.allocate(format!("{owner}From{base}")),
                ty: self.spell(&branch.ty)?,
            });
        }
        Ok(out)
    }
}

/// A Go spelling the generator itself holds null in — a pointer,
/// `jsontext.Value` — or an interface, which takes null as nil.
fn null_holding_spelling(spelled: &str) -> bool {
    spelled.starts_with('*')
        || spelled == "jsontext.Value"
        || spelled == "any"
        || spelled.starts_with("interface")
}

/// A type that is an alias shares its target's validation, so a per-type
/// opt-out on it cannot be honoured: said rather than ignored.
pub(crate) fn opt_out_on_alias(def: &TypeDef) -> GenerateError {
    let name = def.component.as_deref().unwrap_or_default();
    GenerateError::Config(format!(
        "`types.{name}.validation = false` cannot apply: `{name}` is an alias, which validates as the type it names; opt that type out instead"
    ))
}

/// The value type a pointer or a container already gives indirection to.
fn unboxed(ty: &TypeRef) -> &TypeRef {
    match ty {
        TypeRef::Boxed(inner) => inner,
        other => other,
    }
}

fn deprecation(deprecated: bool) -> Vec<String> {
    if deprecated {
        vec!["Deprecated: the description marks this deprecated.".to_owned()]
    } else {
        Vec::new()
    }
}

/// The struct tag for one member: the backend's `json` key, then the
/// configured keys, as one Go string literal.
fn struct_tag(
    raw_name: &str,
    omitzero: bool,
    extra: &BTreeMap<String, String>,
    at: &str,
) -> Result<String, GenerateError> {
    let mut json = raw_name.to_owned();
    if !tag_name_ok(raw_name) {
        // The struct's own MarshalJSONTo writes it; decoding goes by name.
        json = "-".to_owned();
    } else if omitzero {
        json.push_str(",omitzero");
    } else if raw_name == "-" {
        // `json:"-"` alone skips the field, and a trailing comma is
        // malformed, so `-` takes an option that changes nothing: strict
        // matching is the default.
        json.push_str(",case:strict");
    }
    let mut parts = vec![format!("json:{}", quote(&json))];
    for (key, value) in extra {
        if key == "json" {
            return Err(GenerateError::Config(format!(
                "`{at}.go_tags` sets `json`, which the generator owns: it carries the member name and the absent-versus-null contract"
            )));
        }
        if !is_tag_key(key) {
            return Err(GenerateError::Config(format!(
                "`{at}.go_tags` key {key:?} is not a struct-tag key: it must be printable ASCII without space, quote or colon"
            )));
        }
        parts.push(format!("{key}:{}", quote(value)));
    }
    let tag = parts.join(" ");
    Ok(if tag.contains('`') {
        quote(&tag)
    } else {
        format!("`{tag}`")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_tags() {
        let none = BTreeMap::new();
        assert_eq!(
            struct_tag("name", false, &none, "x").unwrap(),
            "`json:\"name\"`"
        );
        assert_eq!(
            struct_tag("-", false, &none, "x").unwrap(),
            "`json:\"-,case:strict\"`"
        );
        assert_eq!(
            struct_tag("a b", true, &none, "x").unwrap(),
            "`json:\"a b,omitzero\"`"
        );
        let extra = BTreeMap::from([("db".to_owned(), "x`y".to_owned())]);
        assert_eq!(
            struct_tag("n", false, &extra, "x").unwrap(),
            "\"json:\\\"n\\\" db:\\\"x`y\\\"\""
        );
        let json = BTreeMap::from([("json".to_owned(), "x".to_owned())]);
        assert!(struct_tag("n", false, &json, "x").is_err());
        let bad = BTreeMap::from([("a b".to_owned(), "x".to_owned())]);
        assert!(struct_tag("n", false, &bad, "x").is_err());
    }

    #[test]
    fn import_package_names() {
        assert_eq!(package_of("example.com/x/uuid"), "uuid");
        assert_eq!(package_of("encoding/json/v2"), "json");
        assert_eq!(package_of("time"), "time");
        assert_eq!(package_of("example.com/v"), "v");
    }
}
