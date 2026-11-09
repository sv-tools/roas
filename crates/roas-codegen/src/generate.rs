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
#[non_exhaustive]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

impl GeneratedFile {
    pub fn new(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        GeneratedFile {
            path: path.into(),
            contents: contents.into(),
        }
    }
}

/// What the generated code depends on, computed from what was emitted.
/// Nothing generated depends on `roas`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Dependencies {
    pub rust: Vec<RustDependency>,
}

/// The result of a generation: files in memory, everything the
/// generator had to say, and the dependency list.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Generation {
    /// The language the files are in; [`write`](crate::write()) names the
    /// manifest after it.
    pub target: Target,
    pub files: Vec<GeneratedFile>,
    pub diagnostics: Vec<Diagnostic>,
    pub dependencies: Dependencies,
}

impl Generation {
    /// A generation made of files alone, for callers that produce their
    /// own and want [`write`](crate::write()) to manage them.
    pub fn new(target: Target, files: Vec<GeneratedFile>) -> Self {
        Generation {
            target,
            files,
            diagnostics: Vec::new(),
            dependencies: Dependencies::default(),
        }
    }

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
#[non_exhaustive]
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

/// How the generated header names its source: the document's file name,
/// never its full location. Output must be identical in every checkout,
/// and an absolute path would also leak the machine it was generated on.
fn source_name(uri: &url::Url) -> String {
    uri.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .map(|name| {
            // A file name is free text: a decoded newline would end the
            // doc comment it is quoted in and turn the rest into code.
            percent_decode(name)
                .chars()
                .flat_map(|c| {
                    // Bidi controls reorder what a reader sees, and rustc
                    // refuses them in comments.
                    if c.is_control()
                        || matches!(
                            c,
                            '\u{061C}'
                                | '\u{200E}'
                                | '\u{200F}'
                                | '\u{202A}'..='\u{202E}'
                                | '\u{2066}'..='\u{2069}'
                        )
                    {
                        c.escape_default().collect::<Vec<_>>()
                    } else {
                        vec![c]
                    }
                })
                .collect()
        })
        .unwrap_or_else(|| match uri.scheme() {
            "stdin" => "standard input".to_owned(),
            _ => uri.host_str().unwrap_or("the description").to_owned(),
        })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
    // Settings the configuration accepts but this release cannot honour
    // are refused, never silently ignored.
    if config.validation {
        return Err(GenerateError::Unsupported(
            "`validation = true` (generated validation)".into(),
        ));
    }
    if config.template_dir.is_some() {
        return Err(GenerateError::Unsupported(
            "`template_dir` (user template overrides)".into(),
        ));
    }
    // `Config`'s fields are public, so what `ConfigFile::build` checked may
    // have been changed since; the module names the file written.
    if !crate::config::is_module_name(&config.rust.module) {
        return Err(GenerateError::Config(format!(
            "`rust.module = {:?}` must be a lowercase Rust module name",
            config.rust.module
        )));
    }
    if config.rust.crate_name.is_some() {
        return Err(GenerateError::Unsupported(
            "`rust.crate_name` (generating a crate)".into(),
        ));
    }
    if !matches!(
        config.rust.edition.as_str(),
        "2015" | "2018" | "2021" | "2024"
    ) {
        return Err(GenerateError::Config(format!(
            "`rust.edition = {:?}` is not a Rust edition",
            config.rust.edition
        )));
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
            target: config.target,
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
        Target::Rust => rust::emit(&ir, config, &source_name(document.uri()))?,
        Target::Go => return Err(GenerateError::Unsupported("the Go backend".into())),
    };
    diagnostics.extend(output.diagnostics);
    if !output.formatted {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::Unformatted,
            schema_id: SchemaId::new(document.uri().clone(), ""),
            pointer: String::new(),
            message: "rustfmt did not run or failed, so the output is not formatted; it differs from a formatted run".into(),
        });
    }
    diagnostics.sort_by(|a, b| {
        a.pointer
            .cmp(&b.pointer)
            .then(a.severity.cmp(&b.severity))
            .then(a.message.cmp(&b.message))
    });
    diagnostics.dedup();
    Ok(Generation {
        target: config.target,
        files: output.files,
        diagnostics,
        dependencies: Dependencies {
            rust: output.dependencies,
        },
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn bidi_controls_are_escaped_in_the_source_name() {
        let uri = url::Url::parse("file:///specs/a%E2%80%AEb.json").unwrap();
        let name = super::source_name(&uri);
        assert!(!name.contains('\u{202E}'), "{name}");
        assert!(name.contains("\\u{202e}"), "{name}");
    }
}
