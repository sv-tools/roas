//! User template overrides: by name, by extending `builtin/`, with block
//! names checked against what is extended, and the context and the
//! pre-format text reachable for writing them.

use roas::validation::{IGNORE_UNUSED, Options};
use roas_codegen::{
    ConfigFile, GenerateError, Input, SourceDocument, TEMPLATE_API, Target, generate, render,
    template_context, validate,
};
use std::path::{Path, PathBuf};
use url::Url;

fn document() -> roas_codegen::ValidatedSourceDocument {
    let raw = serde_json::json!({
        "openapi": "3.2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {
            "Pet": {"type": "object", "required": ["name"], "properties": {
                "name": {"type": "string"},
                "status": {"type": "string", "enum": ["available", "sold"]}
            }}
        }}
    });
    let bytes = serde_json::to_vec(&raw).unwrap();
    let doc = SourceDocument::parse(Input::Json(bytes), Url::parse("file:///t.json").unwrap())
        .expect("parses");
    validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).expect("valid")
}

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str, files: &[(&str, &str)]) -> Dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "roas-codegen-templates-{tag}-{}-{n}",
            std::process::id()
        ));
        for (path, contents) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const MANIFEST: (&str, &str) = ("roas-templates.toml", "template_api = 1\n");

fn config(target: Target, dir: Option<&Path>) -> roas_codegen::Config {
    let mut file = ConfigFile::default();
    file.target = Some(target);
    file.template_dir = dir.map(Path::to_path_buf);
    file.build().unwrap()
}

#[test]
fn an_override_extends_the_builtin_and_changes_one_block() {
    let dir = Dir::new(
        "rust-derives",
        &[
            MANIFEST,
            (
                "rust/enum.rs.jinja",
                "{% extends \"builtin/rust/enum.rs.jinja\" %}{% block derives %}#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]\n{% endblock %}",
            ),
        ],
    );
    let generation = generate(&document(), &config(Target::Rust, Some(&dir.0))).unwrap();
    let code = &generation.files[0].contents;
    assert!(code.contains("Clone, Copy, PartialEq"), "{code}");
    // Everything else is the default.
    assert!(code.contains("pub struct Pet {"), "{code}");
}

#[test]
fn an_override_of_an_included_template_applies_where_it_is_used() {
    let dir = Dir::new(
        "go-field",
        &[
            MANIFEST,
            (
                "go/field.go.jinja",
                "{% extends \"builtin/go/field.go.jinja\" %}{% block doc %}\t// {{ f.raw_name }} on the wire.\n{{ super() }}{% endblock %}",
            ),
        ],
    );
    let generation = match generate(&document(), &config(Target::Go, Some(&dir.0))) {
        Err(GenerateError::Toolchain(_)) => return, // no gofmt here
        other => other.unwrap(),
    };
    let code = &generation.files[0].contents;
    assert!(code.contains("// name on the wire."), "{code}");
    assert!(code.contains("// status on the wire."), "{code}");
}

#[test]
fn a_new_partial_can_be_included_by_an_override() {
    let dir = Dir::new(
        "partial",
        &[
            MANIFEST,
            ("rust/note.jinja", "// Reviewed by hand.\n"),
            (
                "rust/module.rs.jinja",
                "{% extends \"builtin/rust/module.rs.jinja\" %}{% block footer %}{% include \"rust/note.jinja\" %}{% endblock %}",
            ),
        ],
    );
    let generation = generate(&document(), &config(Target::Rust, Some(&dir.0))).unwrap();
    assert!(
        generation.files[0]
            .contents
            .ends_with("// Reviewed by hand.\n"),
        "{}",
        generation.files[0].contents
    );
}

#[test]
fn a_block_no_parent_declares_is_refused() {
    let dir = Dir::new(
        "typo",
        &[
            MANIFEST,
            (
                "rust/struct.rs.jinja",
                "{% extends \"builtin/rust/struct.rs.jinja\" %}{% block derivs %}x{% endblock %}",
            ),
        ],
    );
    let error = generate(&document(), &config(Target::Rust, Some(&dir.0))).unwrap_err();
    let GenerateError::Template(message) = &error else {
        panic!("{error}")
    };
    assert!(message.contains("`derivs`"), "{message}");
    assert!(message.contains("derives"), "{message}");
}

#[test]
fn the_directory_must_declare_its_template_api() {
    let missing = Dir::new("missing", &[]);
    let error = generate(&document(), &config(Target::Rust, Some(&missing.0))).unwrap_err();
    assert!(error.to_string().contains("roas-templates.toml"), "{error}");
    let wrong = Dir::new("wrong", &[("roas-templates.toml", "template_api = 99\n")]);
    let error = generate(&document(), &config(Target::Rust, Some(&wrong.0))).unwrap_err();
    assert!(error.to_string().contains("template_api = 99"), "{error}");
    let unknown = Dir::new(
        "unknown",
        &[("roas-templates.toml", "template_api = 1\nextra = true\n")],
    );
    assert!(generate(&document(), &config(Target::Rust, Some(&unknown.0))).is_err());
}

#[test]
fn builtin_is_reserved_and_extends_must_resolve() {
    let reserved = Dir::new(
        "reserved",
        &[MANIFEST, ("builtin/rust/struct.rs.jinja", "x")],
    );
    let error = generate(&document(), &config(Target::Rust, Some(&reserved.0))).unwrap_err();
    assert!(error.to_string().contains("reserved"), "{error}");
    let dangling = Dir::new(
        "dangling",
        &[
            MANIFEST,
            ("rust/struct.rs.jinja", "{% extends \"rust/nope.jinja\" %}"),
        ],
    );
    let error = generate(&document(), &config(Target::Rust, Some(&dangling.0))).unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
    let circle = Dir::new(
        "circle",
        &[
            MANIFEST,
            (
                "rust/struct.rs.jinja",
                "{% extends \"rust/struct.rs.jinja\" %}",
            ),
        ],
    );
    let error = generate(&document(), &config(Target::Rust, Some(&circle.0))).unwrap_err();
    assert!(error.to_string().contains("circle"), "{error}");
}

#[test]
fn a_template_producing_bad_code_is_caught_and_its_text_reachable() {
    let dir = Dir::new(
        "broken",
        &[
            MANIFEST,
            (
                "rust/struct.rs.jinja",
                "{% extends \"builtin/rust/struct.rs.jinja\" %}{% block trailing %}fn {{ t.name }}({% endblock %}",
            ),
        ],
    );
    let config = config(Target::Rust, Some(&dir.0));
    let error = generate(&document(), &config).unwrap_err();
    assert!(
        matches!(error, GenerateError::TemplateOutput { .. }),
        "{error}"
    );
    let files = render(&document(), &config).unwrap();
    assert_eq!(files[0].path, Path::new("types.rs"));
    assert!(
        files[0].contents.contains("fn Pet("),
        "{}",
        files[0].contents
    );
}

#[test]
fn the_context_is_what_the_templates_see() {
    for target in [Target::Rust, Target::Go] {
        let context = template_context(&document(), &config(target, None))
            .unwrap()
            .context;
        assert_eq!(context["template_api"], TEMPLATE_API);
        let types = context["types"].as_array().unwrap();
        let pet = types.iter().find(|t| t["name"] == "Pet").unwrap();
        assert_eq!(pet["kind"], "struct");
        assert_eq!(pet["fields"][0]["raw_name"], "name");
    }
}

#[test]
fn the_generated_marker_survives_a_full_override() {
    let dir = Dir::new(
        "marker",
        &[MANIFEST, ("rust/module.rs.jinja", "pub struct Only;\n")],
    );
    let generation = generate(&document(), &config(Target::Rust, Some(&dir.0))).unwrap();
    assert_eq!(
        generation.files[0].contents,
        "// Code generated by roas. DO NOT EDIT.\npub struct Only;\n"
    );
}

#[test]
fn a_nested_block_is_a_new_declaration_and_extends_must_be_literal() {
    let nested = Dir::new(
        "nested",
        &[
            MANIFEST,
            (
                "rust/module.rs.jinja",
                "{% extends \"builtin/rust/module.rs.jinja\" %}{% block footer %}{% block extra %}// extra\n{% endblock %}{% endblock %}",
            ),
        ],
    );
    let generation = generate(&document(), &config(Target::Rust, Some(&nested.0))).unwrap();
    assert!(generation.files[0].contents.ends_with("// extra\n"));
    let variable = Dir::new(
        "variable",
        &[
            MANIFEST,
            (
                "rust/struct.rs.jinja",
                "{% set p = \"builtin/rust/struct.rs.jinja\" %}{% extends p %}{% block derivez %}x{% endblock %}",
            ),
        ],
    );
    let error = generate(&document(), &config(Target::Rust, Some(&variable.0))).unwrap_err();
    assert!(error.to_string().contains("one string literal"), "{error}");
}

#[test]
fn a_header_starting_like_the_marker_is_kept_whole() {
    let mut file = ConfigFile::default();
    file.target = Some(Target::Rust);
    file.header = Some("// Code generated by roas. DO NOT EDIT. Source: api.yaml".into());
    let generation = generate(&document(), &file.build().unwrap()).unwrap();
    assert!(
        generation.files[0]
            .contents
            .contains("// Code generated by roas. DO NOT EDIT. Source: api.yaml\n"),
        "{}",
        generation.files[0].contents
    );
}

#[test]
fn nothing_to_generate_is_said_rather_than_rendered_empty() {
    let raw = serde_json::json!({
        "openapi": "3.2.0",
        "jsonSchemaDialect": "https://example.com/dialect",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {"Pet": {"type": "object"}}}
    });
    let doc = SourceDocument::parse(
        Input::Json(serde_json::to_vec(&raw).unwrap()),
        Url::parse("file:///t.json").unwrap(),
    )
    .unwrap();
    let doc = validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).unwrap();
    let config = config(Target::Rust, None);
    for error in [
        template_context(&doc, &config).unwrap_err(),
        render(&doc, &config).unwrap_err(),
    ] {
        assert!(
            matches!(&error, GenerateError::NothingToGenerate(d) if !d.is_empty()),
            "{error}"
        );
        assert!(error.to_string().contains("dialect"), "{error}");
    }
}

#[test]
fn a_marker_line_in_the_header_is_left_alone_without_overrides() {
    let mut file = ConfigFile::default();
    file.target = Some(Target::Rust);
    file.header = Some("// Code generated by roas. DO NOT EDIT.\n\n// extra".into());
    let generation = generate(&document(), &file.build().unwrap()).unwrap();
    assert!(
        generation.files[0].contents.starts_with(
            "// Code generated by roas. DO NOT EDIT.\n// Code generated by roas. DO NOT EDIT.\n\n// extra\n"
        ),
        "{}",
        generation.files[0].contents
    );
}

#[test]
fn a_go_imports_override_applies_with_no_generated_imports() {
    let raw = serde_json::json!({
        "openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {}
    });
    let doc = SourceDocument::parse(
        Input::Json(serde_json::to_vec(&raw).unwrap()),
        Url::parse("file:///t.json").unwrap(),
    )
    .unwrap();
    let doc = validate(doc, IGNORE_UNUSED | Options::IgnoreMissingTags).unwrap();
    let dir = Dir::new(
        "go-imports",
        &[
            MANIFEST,
            (
                "go/file.go.jinja",
                "{% extends \"builtin/go/file.go.jinja\" %}{% block imports %}{{ super() }}\t\"time\"\n{% endblock %}{% block footer %}var Started = time.Now()\n{% endblock %}",
            ),
        ],
    );
    let generation = match generate(&doc, &config(Target::Go, Some(&dir.0))) {
        Err(GenerateError::Toolchain(_)) => return,
        other => other.unwrap(),
    };
    let code = &generation.files[0].contents;
    assert!(code.contains("import (\n\t\"time\"\n)"), "{code}");
    // Without an override, a file with nothing to import has no import.
    let plain = generate(&doc, &config(Target::Go, None)).unwrap();
    assert!(
        !plain.files[0].contents.contains("import"),
        "{}",
        plain.files[0].contents
    );
}
