//! The template environment: the embedded defaults, and a user's
//! directory overriding them by name.
//!
//! Every default is addressable as `builtin/<lang>/<file>`, which a user
//! template can never shadow, and as `<lang>/<file>`, which a user
//! template of the same name replaces. The defaults include one another
//! by the plain name, so an override takes effect wherever its template
//! is used, and a user template extends `builtin/<same name>` to change
//! only some blocks.

use crate::generate::GenerateError;
use crate::rust::naming;
use include_dir::{Dir, include_dir};
use minijinja::{Environment, Value};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

static TEMPLATES: Dir = include_dir!("$CARGO_MANIFEST_DIR/templates");

/// The version of the view model and block contract templates are
/// written against. A user's template directory declares it.
pub const TEMPLATE_API: u32 = 1;

/// The file in a template directory that declares its `template_api`.
pub const TEMPLATE_MANIFEST: &str = "roas-templates.toml";

const BUILTIN: &str = "builtin/";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    template_api: u32,
}

fn error(message: impl Into<String>) -> GenerateError {
    GenerateError::Template(message.into())
}

fn embedded() -> Result<BTreeMap<String, String>, GenerateError> {
    let mut files = Vec::new();
    collect(&TEMPLATES, &mut files);
    let mut out = BTreeMap::new();
    for file in files {
        let path = file.path().to_string_lossy().replace('\\', "/");
        let source = file
            .contents_utf8()
            .ok_or_else(|| error(format!("{path} is not UTF-8")))?;
        out.insert(path, source.to_owned());
    }
    Ok(out)
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

/// Every `.jinja` file below `dir`, by its path relative to it, in a
/// fixed order.
fn user_templates(dir: &Path) -> Result<BTreeMap<String, String>, GenerateError> {
    let manifest_path = dir.join(TEMPLATE_MANIFEST);
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|e| {
        error(format!(
            "template directory {} needs a {TEMPLATE_MANIFEST} declaring `template_api = {TEMPLATE_API}`: {e}",
            dir.display()
        ))
    })?;
    let manifest: Manifest = toml::from_str(&manifest)
        .map_err(|e| error(format!("{}: {e}", manifest_path.display())))?;
    if manifest.template_api != TEMPLATE_API {
        return Err(error(format!(
            "{} declares `template_api = {}`, but this generator renders version {TEMPLATE_API}",
            manifest_path.display(),
            manifest.template_api
        )));
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) -> Result<(), GenerateError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        error(format!(
            "cannot read template directory {}: {e}",
            dir.display()
        ))
    })?;
    let mut entries: Vec<_> = entries.collect::<Result<_, _>>().map_err(|e| {
        error(format!(
            "cannot read template directory {}: {e}",
            dir.display()
        ))
    })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(error(format!(
                "{} is not a UTF-8 name",
                entry.path().display()
            )));
        };
        let path = format!("{prefix}{name}");
        let kind = std::fs::metadata(entry.path())
            .map_err(|e| error(format!("cannot read {}: {e}", entry.path().display())))?;
        if kind.is_dir() {
            if path == "builtin" {
                return Err(error(format!(
                    "{} is reserved: `builtin/` always names the embedded defaults",
                    entry.path().display()
                )));
            }
            walk(&entry.path(), &format!("{path}/"), out)?;
        } else if name.ends_with(".jinja") {
            let source = std::fs::read_to_string(entry.path())
                .map_err(|e| error(format!("cannot read {}: {e}", entry.path().display())))?;
            out.insert(path, source);
        }
    }
    Ok(())
}

/// The names a template declares with `{% block NAME %}`, in order.
fn blocks(source: &str) -> Vec<String> {
    tags(source, "block")
}

/// The template a template extends, when it names one literally.
fn extends(source: &str) -> Option<String> {
    let at = tag_at(source, "extends")?;
    let rest = source[at..].trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = rest[1..].find(quote)?;
    Some(rest[1..=end].to_owned())
}

/// The first word after each `{% <keyword>` tag.
fn tags(source: &str, keyword: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find("{%") {
        rest = &rest[open + 2..];
        let body = rest.trim_start_matches(['-', '+']).trim_start();
        if let Some(after) = body.strip_prefix(keyword)
            && after.starts_with(char::is_whitespace)
        {
            let name: String = after
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

/// Where the argument of the first `{% <keyword>` tag starts.
fn tag_at(source: &str, keyword: &str) -> Option<usize> {
    let mut offset = 0;
    while let Some(open) = source[offset..].find("{%") {
        let start = offset + open + 2;
        let body = &source[start..];
        let trimmed = body.trim_start_matches(['-', '+']).trim_start();
        let skipped = body.len() - trimmed.len();
        if let Some(after) = trimmed.strip_prefix(keyword)
            && after.starts_with(char::is_whitespace)
        {
            return Some(start + skipped + keyword.len());
        }
        offset = start;
    }
    None
}

/// A template that extends another may only fill blocks its ancestors
/// declare: minijinja ignores any other block without a word, so a typo
/// or a block renamed upstream would silently stop applying.
fn check_blocks(
    user: &BTreeMap<String, String>,
    defaults: &BTreeMap<String, String>,
) -> Result<(), GenerateError> {
    let resolve = |name: &str| -> Option<&String> {
        match name.strip_prefix(BUILTIN) {
            Some(rest) => defaults.get(rest),
            None => user.get(name).or_else(|| defaults.get(name)),
        }
    };
    for (name, source) in user {
        let Some(mut parent) = extends(source) else {
            continue;
        };
        let mut declared = BTreeSet::new();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(parent.clone()) {
                return Err(error(format!(
                    "{name}: `extends` goes round in a circle through {parent}"
                )));
            }
            let Some(parent_source) = resolve(&parent) else {
                return Err(error(format!(
                    "{name} extends {parent:?}, which does not exist"
                )));
            };
            declared.extend(blocks(parent_source));
            match extends(parent_source) {
                Some(next) => parent = next,
                None => break,
            }
        }
        for block in blocks(source) {
            if !declared.contains(&block) {
                return Err(error(format!(
                    "{name} fills block `{block}`, which no template it extends declares, so it would never apply; it declares {}",
                    declared
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    Ok(())
}

/// The environment: defaults under `builtin/` and under their own names,
/// the user's templates replacing or adding to the latter.
pub(crate) fn environment(
    template_dir: Option<&Path>,
) -> Result<Environment<'static>, GenerateError> {
    let defaults = embedded()?;
    let user = match template_dir {
        Some(dir) => user_templates(dir)?,
        None => BTreeMap::new(),
    };
    check_blocks(&user, &defaults)?;
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    let add = |env: &mut Environment<'static>, name: String, source: String| {
        env.add_template_owned(name.clone(), source)
            .map_err(|e| error(format!("{name}: {e:#}")))
    };
    for (path, source) in &defaults {
        add(&mut env, format!("{BUILTIN}{path}"), source.clone())?;
        if !user.contains_key(path) {
            add(&mut env, path.clone(), source.clone())?;
        }
    }
    for (path, source) in user {
        add(&mut env, path, source)?;
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

/// The line tooling reads as "generated, do not edit" (`go vet`, linters,
/// review tools). It is not the templates' to drop: the renderer puts it
/// first, whatever an override renders.
pub(crate) const MARKER: &str = "// Code generated by roas. DO NOT EDIT.";

/// Render the named entry template with a backend's view model, the
/// marker first and followed by `separator`.
pub(crate) fn render(
    entry: &str,
    context: &impl Serialize,
    template_dir: Option<&Path>,
    separator: &str,
) -> Result<String, GenerateError> {
    let env = environment(template_dir)?;
    let template = env
        .get_template(entry)
        .map_err(|e| error(format!("{e:#}")))?;
    let rendered = template
        .render(Value::from_serialize(context))
        .map_err(|e| error(format!("{e:#}")))?;
    // An override that writes the marker itself gets it once.
    let body = rendered
        .strip_prefix(MARKER)
        .map(|rest| rest.trim_start_matches(['\n', '\r']))
        .unwrap_or(&rendered);
    Ok(format!("{MARKER}\n{separator}{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_extends_tags_are_read() {
        let source = "{%- extends 'builtin/rust/struct.rs.jinja' %}{% block derives -%}x{% endblock %}{%+ block attrs %}{% endblock attrs %}";
        assert_eq!(
            extends(source).as_deref(),
            Some("builtin/rust/struct.rs.jinja")
        );
        assert_eq!(blocks(source), ["derives", "attrs"]);
        assert_eq!(extends("{% include \"x\" %}"), None);
    }

    #[test]
    fn every_default_template_parses_and_declares_its_blocks() {
        let env = environment(None).unwrap();
        for (path, source) in embedded().unwrap() {
            env.get_template(&path).unwrap();
            env.get_template(&format!("builtin/{path}")).unwrap();
            assert!(extends(&source).is_none(), "{path} extends nothing");
        }
    }
}
