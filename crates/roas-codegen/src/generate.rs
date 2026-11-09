//! The generation entry point and its result.

use crate::config::{Config, Layout, Target};
use crate::diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
use crate::front::Translator;
use crate::go::GoDependency;
use crate::rust::RustDependency;
use crate::source::{Fidelity, SourceVersion};
use crate::validate::ValidatedSourceDocument;
use crate::{go, lower, rust};
use std::path::PathBuf;
use thiserror::Error;

/// One generated file, relative to the output directory the caller
/// chooses. Generating writes no output files; [`write`](crate::write())
/// does.
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
    /// Modules a Go file imports from beyond the standard library.
    pub go: Vec<GoDependency>,
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
    /// A tool the target needs is missing or failed: generating Go needs
    /// `gofmt`, which is also its syntax check.
    #[error("toolchain: {0}")]
    Toolchain(String),
}

/// The first segment of every relative path in a Rust type, such as
/// `chrono` twice in `Option<chrono::NaiveDate>` and `chrono::Utc`: the
/// names a module of the same name would hide. A path starting `::` names
/// a crate no module can hide, so it is skipped.
fn first_path_segments(ty: &str) -> impl Iterator<Item = &str> {
    ty.match_indices("::").filter_map(move |(at, _)| {
        let before = &ty[..at];
        let start = before
            .char_indices()
            .rev()
            // Identifiers may hold any Unicode letter or mark; what ends
            // one in a type is punctuation or space.
            .find(|(_, c)| c.is_whitespace() || "<>,()[]&*;:'!=+".contains(*c))
            // Past that whole character, however many bytes it takes.
            .map_or(0, |(i, c)| i + c.len_utf8());
        let segment = &before[start..];
        let rooted = before[..start].ends_with(':');
        (!segment.is_empty() && !rooted).then_some(segment)
    })
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

/// What `ConfigFile::build` checked may have been changed since, as
/// `Config`'s fields are public; and some checks need the whole of it.
fn check_rust(config: &Config) -> Result<(), GenerateError> {
    // The module names the file written.
    if !crate::config::is_module_name(&config.rust.module) {
        return Err(GenerateError::Config(format!(
            "`rust.module = {:?}` must be a lowercase Rust module name",
            config.rust.module
        )));
    }
    // Nor may it share its name with a crate a substitution imports: the
    // module would hide that crate in the file that declares it.
    // Imported, or named by any path in the type (generic arguments too).
    if let Some((key, _)) = config.substitutions.iter().find(|(_, s)| {
        s.rust.as_ref().is_some_and(|r| {
            r.import
                .as_deref()
                .is_some_and(|import| import.replace('-', "_") == config.rust.module)
                || first_path_segments(&r.name).any(|first| first == config.rust.module)
        })
    }) {
        return Err(GenerateError::Config(format!(
            "`rust.module = {:?}` is the crate substitution `{key}` imports or names; choose another module name or label",
            config.rust.module
        )));
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
    Ok(())
}

fn check_go(config: &Config) -> Result<(), GenerateError> {
    if !go::naming::is_package_name(&config.go.package) {
        return Err(GenerateError::Config(format!(
            "`go.package = {:?}` must be a lowercase Go package name",
            config.go.package
        )));
    }
    if !go::naming::is_file_stem(&config.go.file) {
        return Err(GenerateError::Config(format!(
            "`go.file = {:?}` must be a lowercase file stem `go build` reads as plain source",
            config.go.file
        )));
    }
    Ok(())
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
    match config.target {
        Target::Rust => check_rust(config)?,
        Target::Go => check_go(config)?,
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

    let source = source_name(document.uri());
    let (files, dependencies, unformatted) = match config.target {
        Target::Rust => {
            let output = rust::emit(&ir, config, &source)?;
            diagnostics.extend(output.diagnostics);
            let dependencies = Dependencies {
                rust: output.dependencies,
                go: Vec::new(),
            };
            (output.files, dependencies, output.unformatted)
        }
        Target::Go => {
            let output = go::emit(&ir, config, &source)?;
            diagnostics.extend(output.diagnostics);
            let dependencies = Dependencies {
                rust: Vec::new(),
                go: output.dependencies,
            };
            (output.files, dependencies, None)
        }
    };
    if let Some(reason) = unformatted {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::Unformatted,
            schema_id: SchemaId::new(document.uri().clone(), ""),
            pointer: String::new(),
            message: format!(
                "the output is not formatted, so it differs from a formatted run: {reason}"
            ),
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
        files,
        diagnostics,
        dependencies,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_relative_path_in_a_type_names_its_crate() {
        let firsts: Vec<_> =
            super::first_path_segments("Option<chrono::NaiveDate, ::other::X>").collect();
        assert_eq!(firsts, vec!["chrono"]);
        let firsts: Vec<_> = super::first_path_segments("a::b::C").collect();
        assert_eq!(firsts, vec!["a"]);
        // A combining mark is no alphanumeric, and takes two bytes.
        let firsts: Vec<_> = super::first_path_segments("cafe\u{301}::Date").collect();
        assert_eq!(firsts.len(), 1);
    }

    #[test]
    fn bidi_controls_are_escaped_in_the_source_name() {
        let uri = url::Url::parse("file:///specs/a%E2%80%AEb.json").unwrap();
        let name = super::source_name(&uri);
        assert!(!name.contains('\u{202E}'), "{name}");
        assert!(name.contains("\\u{202e}"), "{name}");
    }
}
