//! Rendering the Rust module through the templates.

use crate::config::{Config, Layout};
use crate::generate::GenerateError;
use crate::rust::view::FileView;
use crate::templates::Renderer;
use std::path::PathBuf;

/// Every file of the module, as the templates render them: one, or under
/// `layout = "file_per_type"` the module and a file per type it includes.
pub(crate) fn render(
    file: &FileView,
    config: &Config,
) -> Result<Vec<(PathBuf, String)>, GenerateError> {
    let renderer = Renderer::new(config.template_dir.as_deref())?;
    let module = &config.rust.module;
    if config.layout == Layout::OneFilePerModule {
        let text = renderer.render("rust/module.rs.jinja", file, "")?;
        return Ok(vec![(PathBuf::from(format!("{module}.rs")), text)]);
    }
    let mut files = vec![(
        PathBuf::from(format!("{module}.rs")),
        renderer.render("rust/mod.rs.jinja", file, "")?,
    )];
    // The module includes each file: its imports, helpers and types are
    // in scope there already, and an import here would be a second one.
    // Cloned once, not with every type in each file.
    let shell = FileView {
        imports: Vec::new(),
        extra_imports: Vec::new(),
        types: Vec::new(),
        ..file.clone()
    };
    for t in &file.types {
        let stem = t
            .file
            .as_deref()
            .expect("every type has a file under file_per_type");
        let view = FileView {
            types: vec![t.clone()],
            ..shell.clone()
        };
        files.push((
            PathBuf::from(module).join(format!("{stem}.rs")),
            renderer.render("rust/type.rs.jinja", &view, "")?,
        ));
    }
    Ok(files)
}
