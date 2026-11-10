//! The Go backend: allocates names, spells types, builds the view model,
//! renders, and formats with `gofmt`, which is also the syntax check.

mod format;
pub(crate) mod naming;
mod render;
mod view;

pub use view::GoDependency;

use crate::config::Config;
use crate::diagnostic::Diagnostic;
use crate::generate::{GenerateError, GeneratedFile};
use crate::ir::Ir;

pub(crate) struct Output {
    pub files: Vec<GeneratedFile>,
    pub dependencies: Vec<GoDependency>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn context(
    ir: &Ir,
    config: &Config,
    source: &str,
) -> Result<(serde_json::Value, Vec<Diagnostic>), GenerateError> {
    let built = view::build(ir, config, source)?;
    let context =
        serde_json::to_value(&built.file).map_err(|e| GenerateError::Template(e.to_string()))?;
    Ok((context, built.diagnostics))
}

pub(crate) fn render(
    ir: &Ir,
    config: &Config,
    source: &str,
) -> Result<Vec<GeneratedFile>, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_file(&built.file, config.template_dir.as_deref())?;
    Ok(vec![GeneratedFile::new(
        format!("{}.go", config.go.file),
        rendered,
    )])
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let rendered = render::render_file(&built.file, config.template_dir.as_deref())?;
    let path = format!("{}.go", config.go.file);
    let contents = format::format(&rendered, &path)?;
    Ok(Output {
        files: vec![GeneratedFile::new(path, contents)],
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
    })
}
