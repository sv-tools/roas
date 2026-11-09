//! Rendering the Rust module through the templates.

use crate::generate::GenerateError;
use crate::rust::view::FileView;
use std::path::Path;

pub(crate) fn render_module(
    file: &FileView,
    template_dir: Option<&Path>,
) -> Result<String, GenerateError> {
    crate::templates::render("rust/module.rs.jinja", file, template_dir, "")
}
