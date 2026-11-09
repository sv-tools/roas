//! The Rust backend: allocates names, spells types, builds the view
//! model, renders, validates and formats.

mod attrs;
mod format;
pub(crate) mod naming;
pub(crate) mod render;
pub(crate) mod view;

pub use view::RustDependency;

use crate::config::Config;
use crate::diagnostic::Diagnostic;
use crate::generate::{GenerateError, GeneratedFile};
use crate::ir::Ir;

pub(crate) struct Output {
    pub files: Vec<GeneratedFile>,
    pub dependencies: Vec<RustDependency>,
    pub diagnostics: Vec<Diagnostic>,
    /// Why `rustfmt` did not format the output, when it did not.
    pub unformatted: Option<String>,
}

pub(crate) fn context(
    ir: &Ir,
    config: &Config,
    source: &str,
) -> Result<serde_json::Value, GenerateError> {
    let built = view::build(ir, config, source)?;
    serde_json::to_value(&built.file).map_err(|e| GenerateError::Template(e.to_string()))
}

pub(crate) fn render(
    ir: &Ir,
    config: &Config,
    source: &str,
) -> Result<Vec<GeneratedFile>, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_module(&built.file, config.template_dir.as_deref())?;
    Ok(vec![GeneratedFile::new(
        format!("{}.rs", config.rust.module),
        rendered,
    )])
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_module(&built.file, config.template_dir.as_deref())?;
    let path = format!("{}.rs", config.rust.module);
    format::validate(&rendered, &path)?;
    let (contents, unformatted) = match format::format(&rendered, &config.rust.edition) {
        Ok(formatted) => (formatted, None),
        Err(reason) => (rendered, Some(reason)),
    };
    Ok(Output {
        files: vec![GeneratedFile::new(path, contents)],
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
        unformatted,
    })
}
