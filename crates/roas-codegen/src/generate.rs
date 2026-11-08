//! The generation entry point and its result.

use crate::config::{Config, Layout, Target};
use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
use crate::front::Translator;
use crate::rust::RustDependency;
use crate::source::{Fidelity, SourceVersion};
use crate::validate::ValidatedSourceDocument;
use crate::{lower, rust};
use std::path::PathBuf;
use thiserror::Error;

/// One generated file, relative to the output directory the caller
/// chooses. The library never touches disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

/// What the generated code depends on, computed from what was emitted.
/// Nothing generated depends on `roas`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dependencies {
    pub rust: Vec<RustDependency>,
}

/// The result of a generation: files in memory, everything the
/// generator had to say, and the dependency list.
#[derive(Debug, Clone)]
pub struct Generation {
    pub files: Vec<GeneratedFile>,
    pub diagnostics: Vec<Diagnostic>,
    pub dependencies: Dependencies,
}

impl Generation {
    /// True when any diagnostic is an error, meaning some type was not
    /// emitted.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Generation itself did not complete. Distinct from a diagnostic: a
/// diagnostic is an answerable fact about the description, with partial
/// output; this is about the configuration, the templates or the
/// toolchain, and no output is meaningful.
#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("configuration: {0}")]
    Config(String),
    #[error("`{selector}` matches nothing; the description has {candidates:?}")]
    UnknownSelector {
        selector: String,
        candidates: Vec<String>,
    },
    #[error("attribute {attr:?} on `{at}` is not valid Rust: {reason}")]
    InvalidAttribute {
        at: String,
        attr: String,
        reason: String,
    },
    #[error("attribute on `{at}` sets `serde({key})`, which the generator owns: {reason}")]
    ReservedAttribute {
        at: String,
        key: String,
        reason: String,
    },
    #[error("`exact_numbers` needs exact input: {0}")]
    Fidelity(String),
    #[error("template: {0}")]
    Template(String),
    #[error("generated `{path}` is not valid syntax, which is a generator bug: {reason}")]
    InvalidOutput { path: String, reason: String },
    #[error("not supported yet: {0}")]
    Unsupported(String),
}

/// Generate from a validated document.
pub fn generate(
    document: &ValidatedSourceDocument,
    config: &Config,
) -> Result<Generation, GenerateError> {
    if config.exact_numbers {
        if !cfg!(feature = "exact-numbers") {
            return Err(GenerateError::Config(
                "`exact_numbers` requires roas-codegen built with the `exact-numbers` feature"
                    .into(),
            ));
        }
        if document.fidelity() != Fidelity::Exact {
            return Err(GenerateError::Fidelity(format!(
                "{} was read with {:?} fidelity; only JSON text yields exact numbers",
                document.uri(),
                document.fidelity()
            )));
        }
        return Err(GenerateError::Unsupported(
            "`exact_numbers` decimal wrappers".into(),
        ));
    }
    if config.layout != Layout::OneFilePerModule {
        return Err(GenerateError::Unsupported(
            "`layout = \"file_per_type\"`".into(),
        ));
    }
    let mut diagnostics = document.diagnostics();

    // The root dialect decides what every keyword means.
    if let Some(dialect) = &document.spec().json_schema_dialect
        && !lower::is_known_dialect(dialect)
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: DiagnosticKind::UnsupportedDialect { dialect: dialect.clone() },
            schema_id: SchemaId::new(document.uri().clone(), ""),
            pointer: "/jsonSchemaDialect".into(),
            message: format!("`jsonSchemaDialect: {dialect}` is not a dialect this generator understands, so no schema is generated; only the OAS dialect and Draft 2020-12 are"),
        });
        return Ok(Generation {
            files: Vec::new(),
            diagnostics,
            dependencies: Dependencies::default(),
        });
    }

    let source_pointer: fn(&str) -> String = match document.version() {
        SourceVersion::V2 => |p| p.replacen("/components/schemas/", "/definitions/", 1),
        _ => |p| p.to_owned(),
    };
    let front = Translator {
        uri: document.uri(),
        raw: document.raw(),
        source_pointer,
    }
    .translate(document.spec());
    let mut ir = lower::lower(&front, config);

    // A schema the reference report refused is not generated either.
    let refused: Vec<SchemaId> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.schema_id.clone())
        .collect();
    if !refused.is_empty() {
        for id in ir.types.keys().cloned().collect::<Vec<_>>() {
            if refused.iter().any(|r| {
                r.uri == id.uri
                    && (id.pointer == r.pointer
                        || r.pointer.starts_with(&format!("{}/", id.pointer)))
            }) {
                ir.types.get_mut(&id).expect("present").has_error = true;
            }
        }
        // Re-run suppression with the extra errors.
        let mut suppressed = ir.suppressed.clone();
        loop {
            let before = suppressed.len();
            for def in ir.types.values() {
                if def.has_error
                    || crate::ir::Ir::all_edges(def)
                        .iter()
                        .any(|e| suppressed.contains(e))
                {
                    suppressed.insert(def.id.clone());
                }
            }
            if suppressed.len() == before {
                break;
            }
        }
        ir.suppressed = suppressed;
    }
    diagnostics.append(&mut ir.diagnostics);

    let output = match config.target {
        Target::Rust => rust::emit(&ir, config, document.uri().as_str())?,
        Target::Go => return Err(GenerateError::Unsupported("the Go backend".into())),
    };
    diagnostics.extend(output.diagnostics);
    diagnostics.sort_by(|a, b| {
        a.pointer
            .cmp(&b.pointer)
            .then(a.severity.cmp(&b.severity))
            .then(a.message.cmp(&b.message))
    });
    diagnostics.dedup();
    Ok(Generation {
        files: output.files,
        diagnostics,
        dependencies: Dependencies {
            rust: output.dependencies,
        },
    })
}
