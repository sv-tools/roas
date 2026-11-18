//! The template environment: the embedded defaults, and a user's
//! directory overriding them by name.
//!
//! Every default is addressable as `builtin/<lang>/<file>`, which a user
//! template can never shadow, and as `<lang>/<file>`, which a user
//! template of the same name replaces. The defaults include one another
//! by the plain name, so an override takes effect wherever its template
//! is used, and a user template extends `builtin/<same name>` to change
//! only some blocks.

use crate::generate::{GenerateError, GeneratedFile};
use crate::rust::naming;
use include_dir::{Dir, include_dir};
use minijinja::{Environment, Value};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
        // Hidden entries are editors' and tools' (an Emacs lock is a
        // dangling `.#name.jinja` link), never templates.
        if name.as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        let Some(name) = name.to_str() else {
            return Err(error(format!(
                "{} is not a UTF-8 name",
                entry.path().display()
            )));
        };
        let path = format!("{prefix}{name}");
        let link = entry
            .file_type()
            .map_err(|e| error(format!("cannot read {}: {e}", entry.path().display())))?
            .is_symlink();
        let kind = match std::fs::metadata(entry.path()) {
            Ok(kind) => kind,
            // A link to nothing holds no template.
            Err(_) if link => continue,
            Err(e) => {
                return Err(error(format!(
                    "cannot read {}: {e}",
                    entry.path().display()
                )));
            }
        };
        if kind.is_dir() {
            // A linked directory is not followed: it could lead back up.
            if link {
                continue;
            }
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
    /// Every `include`, in order.
    includes: Vec<Include>,
}

/// One `include`: the template it names, when that is one string
/// literal, and the loops around it, as written (`f in t.fields`).
#[derive(Debug, PartialEq)]
struct Include {
    name: Option<String>,
    loops: Vec<String>,
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
    struct Walk<'a, 's> {
        source: &'s str,
        shape: Shape,
        parents: Vec<&'a ast::Expr<'a>>,
        loops: Vec<String>,
    }
    fn string(expr: &ast::Expr<'_>) -> Option<String> {
        match expr {
            ast::Expr::Const(c) => c.value.as_str().map(str::to_owned),
            _ => None,
        }
    }
    fn walk<'a>(statements: &'a [ast::Stmt<'a>], depth: usize, w: &mut Walk<'a, '_>) {
        for statement in statements {
            match statement {
                ast::Stmt::Template(t) => walk(&t.children, depth, w),
                ast::Stmt::Extends(e) => w.parents.push(&e.name),
                ast::Stmt::Block(b) => {
                    if depth == 0 {
                        w.shape.top.push(b.name.to_owned());
                    }
                    w.shape.all.push(b.name.to_owned());
                    walk(&b.body, depth + 1, w);
                }
                ast::Stmt::Include(i) => w.shape.includes.push(Include {
                    name: string(&i.name),
                    loops: w.loops.clone(),
                }),
                ast::Stmt::ForLoop(f) => {
                    let end = f.filter_expr.as_ref().unwrap_or(&f.iter).span().end_offset;
                    let header = w
                        .source
                        .get(f.target.span().start_offset as usize..end as usize)
                        .unwrap_or_default();
                    w.loops
                        .push(header.split_whitespace().collect::<Vec<_>>().join(" "));
                    walk(&f.body, depth, w);
                    w.loops.pop();
                    walk(&f.else_body, depth, w);
                }
                ast::Stmt::IfCond(i) => {
                    walk(&i.true_body, depth, w);
                    walk(&i.false_body, depth, w);
                }
                ast::Stmt::WithBlock(b) => walk(&b.body, depth, w),
                ast::Stmt::SetBlock(s) => walk(&s.body, depth, w),
                ast::Stmt::AutoEscape(a) => walk(&a.body, depth, w),
                ast::Stmt::FilterBlock(f) => walk(&f.body, depth, w),
                ast::Stmt::Macro(m) => walk(&m.body, depth, w),
                ast::Stmt::CallBlock(c) => walk(&c.macro_decl.body, depth, w),
                _ => {}
            }
        }
    }
    let mut w = Walk {
        source,
        shape: Shape::default(),
        parents: Vec::new(),
        loops: Vec::new(),
    };
    walk(std::slice::from_ref(&parsed), 0, &mut w);
    let Walk {
        mut shape, parents, ..
    } = w;
    match parents.as_slice() {
        [] => {}
        [expr] if string(expr).is_some() => shape.extends = string(expr),
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

/// A template directory overriding nothing: for each default template
/// rendered from `entries` (a template and what it renders), and each one
/// they include, a template at its name extending the default and filling
/// every block it declares with the default's own content. As a template
/// directory it renders what the defaults do; it is there to show what
/// can be overridden, and to copy from.
pub(crate) fn scaffold(entries: &[(&str, String)]) -> Result<Vec<GeneratedFile>, GenerateError> {
    scaffold_of(&embedded()?, entries)
}

fn scaffold_of(
    defaults: &BTreeMap<String, String>,
    entries: &[(&str, String)],
) -> Result<Vec<GeneratedFile>, GenerateError> {
    let mut roles: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut shapes = BTreeMap::new();
    let mut queue = VecDeque::new();
    for (name, what) in entries {
        roles
            .entry((*name).to_owned())
            .or_default()
            .push(format!("Renders {what}."));
        queue.push_back((*name).to_owned());
    }
    while let Some(name) = queue.pop_front() {
        if shapes.contains_key(&name) {
            continue;
        }
        let source = defaults
            .get(&name)
            .ok_or_else(|| error(format!("there is no default template {name}")))?;
        let parsed = shape(&name, source).map_err(|e| error(format!("{name}: {e}")))?;
        for include in &parsed.includes {
            let Some(included) = &include.name else {
                return Err(error(format!(
                    "{name} includes a template named by an expression, which the scaffold cannot follow"
                )));
            };
            let role = match include.loops.as_slice() {
                [] => format!("Included by {name}."),
                loops => format!(
                    "Included by {name} inside `for {}`.",
                    loops.join("` and `for ")
                ),
            };
            let seen = roles.entry(included.clone()).or_default();
            if !seen.contains(&role) {
                seen.push(role);
            }
            queue.push_back(included.clone());
        }
        shapes.insert(name, parsed);
    }
    let mut files = vec![GeneratedFile::new(
        TEMPLATE_MANIFEST,
        format!("template_api = {TEMPLATE_API}\n"),
    )];
    for (name, roles) in roles {
        let shape = &shapes[&name];
        let mut text = String::from("{#- ");
        text.push_str(&name);
        for role in &roles {
            text.push_str("\n    ");
            text.push_str(role);
        }
        text.push_str(if roles.iter().any(|r| r.starts_with("Included")) {
            "\n    It sees the context where it is included, loop variables too."
        } else {
            "\n    `--print-context` prints the full context."
        });
        // minijinja refuses a block declared twice, so each is once.
        let blocks = &shape.all;
        text.push_str(if blocks.is_empty() {
            "\n    It declares no blocks: to change it, write the whole template here."
        } else {
            "\n    Each block renders the default through `super()`: change the ones\n    you need, and delete the rest."
        });
        // Nothing the comment quotes may end it.
        let mut text = text.replace("#}", "# }");
        text.push_str(" #}\n{%- extends \"");
        text.push_str(BUILTIN);
        text.push_str(&name);
        text.push_str("\" %}\n");
        for block in blocks {
            text.push_str("{%- block ");
            text.push_str(block);
            text.push_str(" %}{{ super() }}{% endblock %}\n");
        }
        files.push(GeneratedFile::new(name, text));
    }
    Ok(files)
}

/// The line tooling reads as "generated, do not edit" (`go vet`, linters,
/// review tools). It is not the templates' to drop: the renderer puts it
/// first, whatever an override renders.
pub(crate) const MARKER: &str = "// Code generated by roas. DO NOT EDIT.";

/// The environment built once, for rendering several files.
pub(crate) struct Renderer {
    env: Environment<'static>,
}

impl Renderer {
    pub(crate) fn new(template_dir: Option<&Path>) -> Result<Renderer, GenerateError> {
        Ok(Renderer {
            env: environment(template_dir)?,
        })
    }

    /// Render the named entry template with a backend's view model, the
    /// marker first and followed by `separator`.
    pub(crate) fn render(
        &self,
        entry: &str,
        context: &impl Serialize,
        separator: &str,
    ) -> Result<String, GenerateError> {
        let template = self
            .env
            .get_template(entry)
            .map_err(|e| error(format!("{e:#}")))?;
        let rendered = template
            .render(Value::from_serialize(context))
            .map_err(|e| error(format!("{e:#}")))?;
        Ok(format!("{MARKER}\n{separator}{rendered}"))
    }
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
    fn includes_are_read_with_the_loops_around_them() {
        let s = read("{% for t in types %}{% for f in t.fields\n  if not f.skip %}{% include 'f' %}{% endfor %}{% else %}{% include \"e\" %}{% endfor %}{% include name %}").unwrap();
        assert_eq!(
            s.includes,
            [
                Include {
                    name: Some("f".into()),
                    loops: vec!["t in types".into(), "f in t.fields if not f.skip".into()],
                },
                Include {
                    name: Some("e".into()),
                    loops: Vec::new(),
                },
                Include {
                    name: None,
                    loops: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn the_scaffold_follows_includes_and_refuses_what_it_cannot() {
        let defaults: BTreeMap<String, String> = [
            ("a", "{% for x in '#}' %}{% include 'b' %}{% endfor %}{% include 'b' %}{% include 'b' %}"),
            ("b", "{% block one %}{% block two %}{% endblock %}{% endblock %}"),
            ("c", "{% include name %}"),
            ("d", "{% include 'missing' %}"),
            ("e", "{% block %}"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let files = scaffold_of(&defaults, &[("a", "`a.rs`".into())]).unwrap();
        let names: Vec<_> = files.iter().map(|f| f.path.to_str().unwrap()).collect();
        assert_eq!(names, [TEMPLATE_MANIFEST, "a", "b"]);
        assert!(
            files[1].contents.contains("Renders `a.rs`."),
            "{}",
            files[1].contents
        );
        assert!(
            files[1].contents.contains("declares no blocks"),
            "{}",
            files[1].contents
        );
        let b = &files[2].contents;
        // Quoted text cannot end the comment, and a role is said once.
        assert!(b.contains("inside `for x in '# }'`."), "{b}");
        assert_eq!(b.matches("Included by a.").count(), 1, "{b}");
        assert!(b.ends_with("{%- extends \"builtin/b\" %}\n{%- block one %}{{ super() }}{% endblock %}\n{%- block two %}{{ super() }}{% endblock %}\n"), "{b}");
        // Every comment is closed where it should be.
        for file in &files[1..] {
            shape("t", &file.contents).unwrap();
        }
        for (entry, wanted) in [
            ("c", "named by an expression"),
            ("d", "no default template missing"),
            ("e", "e:"),
            ("nope", "no default template nope"),
        ] {
            let error = scaffold_of(&defaults, &[(entry, String::new())]).unwrap_err();
            assert!(error.to_string().contains(wanted), "{error}");
        }
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
