//! Rendering through the embedded templates.

use crate::generate::GenerateError;
use crate::rust::naming;
use crate::rust::view::FileView;
use include_dir::{Dir, include_dir};
use minijinja::{Environment, Value};

static TEMPLATES: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// The default templates, addressable as `builtin/<lang>/<file>` and,
/// until user overrides land, as `<lang>/<file>` too.
pub(crate) fn environment() -> Result<Environment<'static>, GenerateError> {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    let mut files = Vec::new();
    collect(&TEMPLATES, &mut files);
    for file in files {
        let path = file.path().to_string_lossy().replace('\\', "/");
        let source = file
            .contents_utf8()
            .ok_or_else(|| GenerateError::Template(format!("{path} is not UTF-8")))?;
        env.add_template_owned(format!("builtin/{path}"), source.to_owned())
            .map_err(|e| GenerateError::Template(e.to_string()))?;
        env.add_template_owned(path, source.to_owned())
            .map_err(|e| GenerateError::Template(e.to_string()))?;
    }
    env.add_filter("pascal", |s: String| naming::pascal(&[s]));
    env.add_filter("snake", |s: String| naming::snake(&[s]));
    env.add_filter("camel", |s: String| naming::camel(&[s]));
    env.add_filter("kebab", |s: String| naming::kebab(&[s]));
    env.add_filter("screaming_snake", |s: String| naming::screaming_snake(&[s]));
    env.add_filter("doc", |s: String, prefix: Option<String>| {
        let prefix = prefix.unwrap_or_else(|| "/// ".to_owned());
        s.lines()
            .map(|l| format!("{prefix}{}", l.trim_end()))
            .collect::<Vec<_>>()
            .join("\n")
    });
    Ok(env)
}

fn collect<'d>(dir: &'d Dir<'d>, out: &mut Vec<&'d include_dir::File<'d>>) {
    for file in dir.files() {
        if file.path().extension().is_some_and(|e| e == "jinja") {
            out.push(file);
        }
    }
    for sub in dir.dirs() {
        collect(sub, out);
    }
}

pub(crate) fn render_module(file: &FileView) -> Result<String, GenerateError> {
    let env = environment()?;
    let template = env
        .get_template("rust/module.rs.jinja")
        .map_err(|e| GenerateError::Template(e.to_string()))?;
    template
        .render(Value::from_serialize(file))
        .map_err(|e| GenerateError::Template(format!("{e:#}")))
}
