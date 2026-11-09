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

/// The statement tags of a template, `{% ... %}`, as their inner text,
/// read the way minijinja's default syntax reads them: comments
/// (`{# ... #}`) and `{% raw %}` sections are skipped, and a `%}` inside a
/// quoted string does not end a tag.
fn statements(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(comment) = after.strip_prefix('#') {
            rest = match comment.find("#}") {
                Some(end) => &comment[end + 2..],
                None => "",
            };
            continue;
        }
        let (close, inner_start) = match after.chars().next() {
            Some('%') => ("%}", &after[1..]),
            Some('{') => ("}}", &after[1..]),
            _ => {
                rest = after;
                continue;
            }
        };
        let (inner, remainder) = until_close(inner_start, close);
        rest = remainder;
        if close != "%}" {
            continue;
        }
        let tag = inner.trim_matches(['-', '+', '~']).trim();
        if tag == "raw" {
            // Everything up to the matching `endraw` is text.
            rest = skip_raw(rest);
            continue;
        }
        out.push(tag.to_owned());
    }
    out
}

/// The text of a tag up to `close`, honouring quoted strings, and what
/// follows it.
fn until_close<'s>(text: &'s str, close: &str) -> (&'s str, &'s str) {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => quote = Some(c),
            None if text[i..].starts_with(close) => {
                return (&text[..i], &text[i + close.len()..]);
            }
            None => {}
        }
    }
    (text, "")
}

fn skip_raw(text: &str) -> &str {
    let mut rest = text;
    while let Some(open) = rest.find("{%") {
        let (inner, remainder) = until_close(&rest[open + 2..], "%}");
        if inner.trim_matches(['-', '+', '~']).trim() == "endraw" {
            return remainder;
        }
        rest = remainder;
    }
    ""
}

/// The word after `keyword` in a statement, if the statement is one.
fn argument<'s>(statement: &'s str, keyword: &str) -> Option<&'s str> {
    statement
        .strip_prefix(keyword)
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map(str::trim)
}

/// The names a template declares with `{% block NAME %}`, in order.
fn blocks(source: &str) -> Vec<String> {
    statements(source)
        .iter()
        .filter_map(|s| argument(s, "block"))
        .filter_map(|rest| {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// The blocks a template fills at its top level: a block nested inside
/// another is a new declaration a descendant may fill, not a fill.
fn top_level_blocks(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    for statement in statements(source) {
        if let Some(rest) = argument(&statement, "block") {
            if depth == 0 {
                out.push(
                    rest.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect(),
                );
            }
            depth += 1;
        } else if statement == "endblock" || argument(&statement, "endblock").is_some() {
            depth = depth.saturating_sub(1);
        }
    }
    out
}

/// The template a template extends. It must be named by one string
/// literal, or its blocks could not be checked against it.
fn extends(source: &str) -> Result<Option<String>, String> {
    let statements = statements(source);
    let Some(rest) = statements.iter().find_map(|s| argument(s, "extends")) else {
        return Ok(None);
    };
    let literal = rest
        .chars()
        .next()
        .filter(|c| *c == '"' || *c == '\'')
        .and_then(|quote| {
            let end = rest[1..].find(quote)?;
            rest[end + 2..]
                .trim()
                .is_empty()
                .then(|| rest[1..=end].to_owned())
        });
    literal.map(Some).ok_or_else(|| {
        format!("`extends {rest}` must name its template as one string literal, so its blocks can be checked")
    })
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
        let Some(mut parent) = extends(source).map_err(|e| error(format!("{name}: {e}")))? else {
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
            match extends(parent_source).map_err(|e| error(format!("{parent}: {e}")))? {
                Some(next) => parent = next,
                None => break,
            }
        }
        for block in top_level_blocks(source) {
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
    // An override that writes the marker line itself gets it once; a line
    // that only starts with the same text is someone else's.
    let body = rendered
        .strip_prefix(MARKER)
        .filter(|rest| rest.is_empty() || rest.starts_with(['\n', '\r']))
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
            extends(source).unwrap().as_deref(),
            Some("builtin/rust/struct.rs.jinja")
        );
        assert_eq!(blocks(source), ["derives", "attrs"]);
        assert_eq!(extends("{% include \"x\" %}"), Ok(None));
        assert!(extends("{% extends p %}").is_err());
        assert!(extends("{% extends \"a\" if c else \"b\" %}").is_err());
        assert_eq!(
            top_level_blocks(
                "{% block a %}{% block b %}{% endblock %}{% endblock a %}{% block c %}{% endblock %}"
            ),
            ["a", "c"]
        );
        // Comments, raw sections and quoted `%}` are not tags.
        let source = "{# {% extends 'a' %}{% block old %} #}{% extends \"b\" %}{% raw %}{% block raw %}{% endraw %}{{ '%}' }}{% block kept %}{% endblock %}{% set x = '%}{% block fake %}' %}";
        assert_eq!(extends(source).unwrap().as_deref(), Some("b"));
        assert_eq!(blocks(source), ["kept"]);
    }

    #[test]
    fn every_default_template_parses_and_declares_its_blocks() {
        let env = environment(None).unwrap();
        for (path, source) in embedded().unwrap() {
            env.get_template(&path).unwrap();
            env.get_template(&format!("builtin/{path}")).unwrap();
            assert_eq!(extends(&source), Ok(None), "{path} extends nothing");
        }
    }
}
