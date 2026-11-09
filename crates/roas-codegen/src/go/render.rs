//! Rendering through the embedded templates, shared with Rust.

use crate::generate::GenerateError;
use crate::go::view::FileView;
use minijinja::Value;

pub(crate) fn render_file(file: &FileView) -> Result<String, GenerateError> {
    let env = crate::rust::render::environment()?;
    let template = env
        .get_template("go/file.go.jinja")
        .map_err(|e| GenerateError::Template(e.to_string()))?;
    template
        .render(Value::from_serialize(file))
        .map_err(|e| GenerateError::Template(format!("{e:#}")))
}
