//! The Go backend: allocates names, spells types, builds the view model,
//! renders, and formats with `gofmt`, which is also the syntax check.

mod format;
mod imports;
pub(crate) mod naming;
mod render;
mod validation;
pub(crate) mod view;

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
    Ok(render::render(&built, config)?
        .into_iter()
        .map(|(path, text)| GeneratedFile::new(path, text))
        .collect())
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let mut files = Vec::new();
    for (path, rendered) in render::render(&built, config)? {
        let contents = format::format(&rendered, &path.to_string_lossy())?;
        files.push(GeneratedFile::new(path, contents));
    }
    Ok(Output {
        files,
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
    })
}
