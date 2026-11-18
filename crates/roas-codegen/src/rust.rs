//! The Rust backend: allocates names, spells types, builds the view
//! model, renders, validates and formats.

mod attrs;
mod format;
pub(crate) mod naming;
pub(crate) mod render;
mod validation;
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
    Ok(render::render(&built.file, config)?
        .into_iter()
        .map(|(path, text)| GeneratedFile::new(path, text))
        .collect())
}

pub(crate) fn emit(ir: &Ir, config: &Config, source: &str) -> Result<Output, GenerateError> {
    let built = view::build(ir, config, source)?;
    let mut files = Vec::new();
    let mut unformatted = None;
    for (path, rendered) in render::render(&built.file, config)? {
        format::validate(&rendered, &path.to_string_lossy())?;
        // Once `rustfmt` fails, it is not run on the files after: the
        // output is unformatted either way, and says why once.
        let contents = if unformatted.is_some() {
            rendered
        } else {
            match format::format(&rendered, &config.rust.edition) {
                Ok(formatted) => formatted,
                Err(reason) => {
                    unformatted = Some(reason);
                    rendered
                }
            }
        };
        files.push(GeneratedFile::new(path, contents));
    }
    Ok(Output {
        files,
        dependencies: built.dependencies,
        diagnostics: built.diagnostics,
        unformatted,
    })
}
