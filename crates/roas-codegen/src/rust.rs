//! The Rust backend: allocates names, spells types, builds the view
//! model, renders, validates and formats.

mod attrs;
mod format;
pub(crate) mod naming;
mod render;
mod view;

pub use view::RustDependency;

use crate::config::Config;
use crate::diagnostic::Diagnostic;
use crate::generate::{GenerateError, GeneratedFile};
use crate::ir::Ir;

pub(crate) struct Output {
    pub files: Vec<GeneratedFile>,
    pub dependencies: Vec<RustDependency>,
    pub diagnostics: Vec<Diagnostic>,
    /// `rustfmt` ran and formatted the output.
    pub formatted: bool,
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_module(&built.file)?;
    let path = format!("{}.rs", config.rust.module);
    format::validate(&rendered, &path)?;
    let (contents, formatted) = match format::format(&rendered, &config.rust.edition) {
        Some(formatted) => (formatted, true),
        None => (rendered, false),
    };
    Ok(Output {
        files: vec![GeneratedFile::new(path, contents)],
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
        formatted,
    })
}
