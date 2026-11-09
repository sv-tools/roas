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

/// What the block check needs from one template, read by minijinja's own
/// parser so it agrees with the engine by construction.
#[derive(Debug, Default, PartialEq)]
struct Shape {
    /// The template it extends, if any.
    extends: Option<String>,
    /// Blocks not nested in another block: what an extending template
    /// fills.
    top: Vec<String>,
    /// Every block, nested ones included: what a descendant may fill.
    all: Vec<String>,
}

fn shape(name: &str, source: &str) -> Result<Shape, String> {
    use minijinja::machinery::{self, WhitespaceConfig, ast};
    let parsed = machinery::parse(
        source,
        name,
        // The default syntax, whichever shape minijinja's features give it.
        Default::default(),
        WhitespaceConfig::default(),
    )
    .map_err(|e| format!("{e:#}"))?;
    let mut shape = Shape::default();
    let mut parents = Vec::new();
    fn walk<'a>(
        statements: &'a [ast::Stmt<'a>],
        depth: usize,
        shape: &mut Shape,
        parents: &mut Vec<&'a ast::Expr<'a>>,
    ) {
        for statement in statements {
            match statement {
                ast::Stmt::Template(t) => walk(&t.children, depth, shape, parents),
                ast::Stmt::Extends(e) => parents.push(&e.name),
                ast::Stmt::Block(b) => {
                    if depth == 0 {
                        shape.top.push(b.name.to_owned());
                    }
                    shape.all.push(b.name.to_owned());
                    walk(&b.body, depth + 1, shape, parents);
                }
                ast::Stmt::ForLoop(f) => {
                    walk(&f.body, depth, shape, parents);
                    walk(&f.else_body, depth, shape, parents);
                }
                ast::Stmt::IfCond(i) => {
                    walk(&i.true_body, depth, shape, parents);
                    walk(&i.false_body, depth, shape, parents);
                }
                ast::Stmt::WithBlock(w) => walk(&w.body, depth, shape, parents),
                ast::Stmt::SetBlock(s) => walk(&s.body, depth, shape, parents),
                ast::Stmt::AutoEscape(a) => walk(&a.body, depth, shape, parents),
                ast::Stmt::FilterBlock(f) => walk(&f.body, depth, shape, parents),
                ast::Stmt::Macro(m) => walk(&m.body, depth, shape, parents),
                ast::Stmt::CallBlock(c) => walk(&c.macro_decl.body, depth, shape, parents),
                _ => {}
            }
        }
    }
    walk(std::slice::from_ref(&parsed), 0, &mut shape, &mut parents);
    match parents.as_slice() {
        [] => {}
        [ast::Expr::Const(c)] if c.value.as_str().is_some() => {
            shape.extends = c.value.as_str().map(str::to_owned);
        }
        // Each `extends` could pick another parent, and only one is checked.
        [_, _, ..] => {
            return Err("has more than one `extends`; one parent, named once, is what the block check can follow".into());
        }
        [_] => {
            return Err(
                "`extends` must name its template as one string literal, so its blocks can be checked"
                    .into(),
            );
        }
    }
    Ok(shape)
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
        let child = shape(name, source).map_err(|e| error(format!("{name}: {e}")))?;
        let Some(mut parent) = child.extends else {
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
            let parent_shape =
                shape(&parent, parent_source).map_err(|e| error(format!("{parent}: {e}")))?;
            declared.extend(parent_shape.all);
            match parent_shape.extends {
                Some(next) => parent = next,
                None => break,
            }
        }
        for block in child.top {
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
    Ok(format!("{MARKER}\n{separator}{rendered}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(source: &str) -> Result<Shape, String> {
        shape("t", source)
    }

    #[test]
    fn templates_are_read_as_minijinja_reads_them() {
        let s = read("{%- extends 'builtin/rust/struct.rs.jinja' %}{% block derives -%}x{% endblock %}{%+ block attrs %}{% endblock attrs %}").unwrap();
        assert_eq!(s.extends.as_deref(), Some("builtin/rust/struct.rs.jinja"));
        assert_eq!(s.top, ["derives", "attrs"]);
        assert_eq!(read("{% include \"x\" %}").unwrap().extends, None);
        assert!(read("{% extends p %}").is_err());
        assert!(read("{% extends \"a\" if c else \"b\" %}").is_err());
        assert!(
            read("{% if c %}{% extends \"a\" %}{% else %}{% extends \"b\" %}{% endif %}").is_err()
        );
        let nested = read(
            "{% block a %}{% block b %}{% endblock %}{% endblock a %}{% block c %}{% endblock %}",
        )
        .unwrap();
        assert_eq!(nested.top, ["a", "c"]);
        assert_eq!(nested.all, ["a", "b", "c"]);
        // Comments, raw sections, quoted and nested delimiters are text.
        let s = read("{# {% extends 'a' %}{% block old %} #}{% extends \"b\" %}{% raw %}{% it's %}{%~ endraw %}{% block raw %}{% endraw %}{{ {'a': {}} ~ '{#' }}{% block kept %}{% endblock %}{% set x = '%}{% block fake %}' %}").unwrap();
        assert_eq!(s.extends.as_deref(), Some("b"));
        assert_eq!(s.top, ["kept"]);
        assert_eq!(
            read("{%extends\"c\"%}").unwrap().extends.as_deref(),
            Some("c")
        );
        assert_eq!(
            read(r#"{% extends "builtin\/rust\/x.jinja" %}"#)
                .unwrap()
                .extends
                .as_deref(),
            Some("builtin/rust/x.jinja")
        );
    }

    #[test]
    fn every_default_template_parses_and_declares_its_blocks() {
        let env = environment(None).unwrap();
        for (path, source) in embedded().unwrap() {
            env.get_template(&path).unwrap();
            env.get_template(&format!("builtin/{path}")).unwrap();
            assert_eq!(
                shape(&path, &source).unwrap().extends,
                None,
                "{path} extends nothing"
            );
        }
    }
}
