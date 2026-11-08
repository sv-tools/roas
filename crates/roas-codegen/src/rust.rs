//! The Rust backend: allocates names, spells types, builds the view
//! model, renders, validates and formats.

mod attrs;
mod format;
mod naming;
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
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_module(&built.file)?;
    let path = "types.rs";
    format::validate(&rendered, path)?;
    let contents = format::format(&rendered);
    Ok(Output {
        files: vec![GeneratedFile {
            path: path.into(),
            contents,
        }],
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
    })
}
