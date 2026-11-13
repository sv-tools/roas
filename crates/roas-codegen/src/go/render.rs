//! Rendering the Go package through the templates.

use crate::config::{Config, Layout};
use crate::generate::GenerateError;
use crate::go::imports::{SpecName, qualifiers, spec_name};
use crate::go::view::{Built, FileView};
use crate::templates::Renderer;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Every file of the package, as the templates render them: one, or under
/// `layout = "file_per_type"` the package file and a file per type.
pub(crate) fn render(
    built: &Built,
    config: &Config,
) -> Result<Vec<(PathBuf, String)>, GenerateError> {
    let renderer = Renderer::new(config.template_dir.as_deref())?;
    let file = &built.file;
    let package_path = PathBuf::from(format!("{}.go", config.go.file));
    if config.layout == Layout::OneFilePerModule {
        let text = renderer.render("go/file.go.jinja", file, "\n")?;
        return Ok(vec![(package_path, text)]);
    }
    // The extra imports some file took.
    let mut placed = BTreeSet::new();
    let mut files = vec![(
        package_path,
        render_used(
            &renderer,
            "go/package.go.jinja",
            file.clone(),
            built,
            true,
            &mut placed,
        )?,
    )];
    for t in &file.types {
        let stem = t
            .file
            .as_deref()
            .expect("every type has a file under file_per_type");
        let view = FileView {
            types: vec![t.clone()],
            ..file.clone()
        };
        files.push((
            PathBuf::from(format!("{stem}.go")),
            render_used(
                &renderer,
                "go/type.go.jinja",
                view,
                built,
                false,
                &mut placed,
            )?,
        ));
    }
    // One no file names by the name its path suggests is likely used
    // under another: the package's own name, which only an alias says.
    if let Some(spec) = file
        .extra_imports
        .iter()
        .find(|spec| !placed.contains(*spec))
    {
        return Err(GenerateError::Config(format!(
            "`extra_imports` entry {spec:?} is used by no generated file under its name; under `layout = \"file_per_type\"` an import goes to the files using it"
        )));
    }
    Ok(files)
}

/// Render with every import, then again with only those the text uses:
/// Go refuses an unused one. A blank or dot import shows no use, and goes
/// to the package file.
fn render_used(
    renderer: &Renderer,
    entry: &str,
    mut view: FileView,
    built: &Built,
    package: bool,
    placed: &mut BTreeSet<String>,
) -> Result<String, GenerateError> {
    let used = qualifiers(&renderer.render(entry, &view, "\n")?);
    let uses = |names: &BTreeSet<String>| names.iter().any(|n| used.contains(n));
    view.imports = view
        .imports
        .into_iter()
        .zip(&built.import_names)
        .filter(|(_, names)| uses(names))
        .map(|(spec, _)| spec)
        .collect();
    view.extra_imports.retain(|spec| match spec_name(spec) {
        SpecName::Named(name) => used.contains(&name),
        SpecName::Unnamed => package,
        // Refused before rendering.
        SpecName::Uncertain => false,
    });
    placed.extend(view.extra_imports.iter().cloned());
    renderer.render(entry, &view, "\n")
}
