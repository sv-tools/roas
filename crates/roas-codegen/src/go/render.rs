//! Rendering the Go file through the templates.

use crate::generate::GenerateError;
use crate::go::view::FileView;
use std::path::Path;

pub(crate) fn render_file(
    file: &FileView,
    template_dir: Option<&Path>,
) -> Result<String, GenerateError> {
    crate::templates::render("go/file.go.jinja", file, template_dir, "\n")
}
