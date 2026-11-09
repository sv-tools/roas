//! `roas openapi generate` — generate Rust types from an OpenAPI
//! description, powered by the `roas-codegen` crate.
//!
//! The description is read as raw bytes, so JSON input keeps exact
//! numbers, then version-detected, normalized to 3.2, validated with
//! external references skipped, lowered and rendered. Files are written
//! under a manifest the target owns: see `roas_codegen::write` for the
//! ownership rules. `--check` writes nothing and fails when the output
//! directory is out of date, which is what CI wants.

use anyhow::{Context, Result, anyhow, bail};
use clap::ValueEnum;
use roas::validation::Options;
use roas_codegen::{
    ConfigFile, Input, Severity, SourceDocument, Target, WriteOptions, check, generate, validate,
    write,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use url::Url;

use crate::versioned::path_looks_like_yaml;
use crate::{InputFormat, InputSource, resolve_input_source};

/// The language to generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum GenerateTarget {
    Rust,
    Go,
}

impl From<GenerateTarget> for Target {
    fn from(target: GenerateTarget) -> Self {
        match target {
            GenerateTarget::Rust => Target::Rust,
            GenerateTarget::Go => Target::Go,
        }
    }
}

#[derive(clap::Args)]
pub(crate) struct GenerateArgs {
    /// Path to the description (JSON or YAML). Pass `-`, or omit and pipe
    /// it, to read from stdin.
    pub(crate) file: Option<PathBuf>,

    /// Directory the generated files go into. Created if missing.
    #[arg(long, short)]
    pub(crate) out: PathBuf,

    /// Configuration file (TOML). Relative paths inside it resolve against
    /// the file's own directory. Flags below override its values.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,

    /// The language to generate. Required here or in the configuration.
    #[arg(long, value_enum)]
    pub(crate) target: Option<GenerateTarget>,

    /// Names this generation target in the output directory's manifest, so
    /// several targets can share one directory. Defaults to the input file's
    /// name, or `api` for stdin.
    #[arg(long)]
    pub(crate) label: Option<String>,

    /// Override format detection. By default, file paths use the extension
    /// (`.yaml`/`.yml` → YAML, otherwise JSON) and stdin defaults to JSON.
    #[arg(long, value_enum)]
    pub(crate) format: Option<InputFormat>,

    /// Skip a specific validation check before generating, as
    /// `roas openapi validate --ignore` does. Repeatable.
    #[arg(long, value_enum)]
    pub(crate) ignore: Vec<Options>,

    /// Apply code-bearing extensions from the description (`x-rust-attrs`).
    /// Off by default: a description is often written by someone else.
    #[arg(long)]
    pub(crate) allow_codegen_extensions: bool,

    /// Treat every warning as an error.
    #[arg(long)]
    pub(crate) strict: bool,

    /// Write the files that could be generated even when some types could
    /// not. By default an error leaves the output directory untouched.
    #[arg(long)]
    pub(crate) allow_partial: bool,

    /// Overwrite existing files no generation target owns. Off by default,
    /// since such a file is probably hand-written.
    #[arg(long)]
    pub(crate) force: bool,

    /// Write nothing; fail if the output directory differs from what would
    /// be generated.
    #[arg(long)]
    pub(crate) check: bool,
}

/// Read the description as bytes, so JSON keeps exact numbers, and give
/// it the URI every schema id is built from.
fn read_description(source: &InputSource, format: Option<InputFormat>) -> Result<(Input, Url)> {
    let (bytes, uri, yaml) = match source {
        InputSource::File(path) => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let absolute = std::path::absolute(path)
                .with_context(|| format!("resolving {}", path.display()))?;
            let uri = Url::from_file_path(&absolute).map_err(|()| {
                anyhow!("{} cannot be expressed as a file URL", absolute.display())
            })?;
            let yaml = match format {
                Some(f) => f == InputFormat::Yaml,
                None => path_looks_like_yaml(path),
            };
            (bytes, uri, yaml)
        }
        InputSource::Stdin => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .context("reading from stdin")?;
            (
                bytes,
                Url::parse("stdin:///").expect("a valid URL"),
                format == Some(InputFormat::Yaml),
            )
        }
    };
    let input = if yaml {
        Input::Yaml(bytes)
    } else {
        Input::Json(bytes)
    };
    Ok((input, uri))
}

/// The default label: the input file's stem, made safe for a manifest name.
fn default_label(source: &InputSource) -> String {
    let stem = match source {
        InputSource::File(path) => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        InputSource::Stdin => String::new(),
    };
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-').to_owned();
    if safe.is_empty() {
        "api".to_owned()
    } else {
        safe
    }
}

/// What a label was generated from: the description's path relative to the
/// output directory, `/`-separated and without its extension. Two files
/// called `openapi.yaml` in different folders differ; converting
/// `petstore.json` to `petstore.yaml` does not. Stdin has no identity.
fn source_id(source: &InputSource, out: &Path) -> Option<String> {
    let InputSource::File(path) = source else {
        return None;
    };
    let file = std::path::absolute(path.with_extension("")).ok()?;
    let out = std::path::absolute(out).ok()?;
    let file: Vec<_> = file.components().collect();
    let out: Vec<_> = out.components().collect();
    let common = file.iter().zip(&out).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), out.len() - common).collect();
    parts.extend(
        file[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Some(parts.join("/"))
}

fn show(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn run_generate(args: GenerateArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (input, uri) = read_description(&source, args.format)?;
    let document = SourceDocument::parse(input, uri)
        .with_context(|| format!("{}: cannot be read", source.display()))?;

    // Lint-only checks say nothing about the generated types, so they never
    // block generation; `--ignore` relaxes further.
    let mut options = roas_codegen::generation_options();
    for ignore in &args.ignore {
        options |= *ignore;
    }
    let document = match validate(document, options) {
        Ok(document) => document,
        Err(error) => {
            for e in &error.errors {
                eprintln!("- {e}");
            }
            bail!(
                "{}: validation failed ({} error{}); nothing generated",
                source.display(),
                error.errors.len(),
                if error.errors.len() == 1 { "" } else { "s" }
            );
        }
    };

    let file = match &args.config {
        Some(path) => ConfigFile::from_path(path)?,
        None => ConfigFile::default(),
    };
    // Labels name manifest files, which must not differ only in case.
    let label = args
        .label
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| default_label(&source));
    let mut flags = ConfigFile {
        target: args.target.map(Target::from),
        allow_codegen_extensions: args.allow_codegen_extensions.then_some(true),
        ..Default::default()
    };
    // The generated file is named after the label unless the configuration
    // names the module, so descriptions sharing a directory do not collide.
    if file.rust.module.is_none() {
        flags.rust.module = Some(roas_codegen::rust_module_name(&label));
    }
    let config = file.merge(flags).build()?;
    let target = config.target;

    let generation = generate(&document, &config)?;
    let mut errors = 0;
    let mut warnings = 0;
    for diagnostic in &generation.diagnostics {
        eprintln!("{diagnostic}");
        match diagnostic.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
        }
    }
    let failing = if args.strict {
        errors + warnings
    } else {
        errors
    };
    let partial_failure = || {
        anyhow!(
            "{}: {failing} {}{}; wrote what could be generated",
            source.display(),
            if args.strict { "diagnostic" } else { "error" },
            if failing == 1 { "" } else { "s" }
        )
    };
    if failing > 0 && !args.allow_partial {
        let (what, hint) = if args.check {
            (
                "cannot check",
                "pass --allow-partial to check the partial output",
            )
        } else {
            (
                "nothing written",
                "pass --allow-partial to write what was generated",
            )
        };
        bail!(
            "{}: {failing} {}{}; {what} ({hint})",
            source.display(),
            if args.strict { "diagnostic" } else { "error" },
            if failing == 1 { "" } else { "s" }
        );
    }

    if args.check {
        if generation
            .diagnostics
            .iter()
            .any(|d| matches!(d.kind, roas_codegen::DiagnosticKind::Unformatted))
        {
            bail!(
                "--check needs rustfmt: without it the output is unformatted and cannot match a formatted run"
            );
        }
        let mut check_options = WriteOptions::new(label.clone());
        check_options.allow_partial = args.allow_partial;
        check_options.source_id = source_id(&source, &args.out);
        let report = check(&generation, &args.out, target, &check_options)?;
        if report.is_clean() {
            eprintln!("{}: up to date", args.out.display());
            return if failing > 0 {
                Err(partial_failure())
            } else {
                Ok(())
            };
        }
        for (what, paths) in [
            ("changed", &report.changed),
            ("missing", &report.missing),
            ("stale", &report.stale),
        ] {
            if !paths.is_empty() {
                eprintln!("{what}: {}", show(paths));
            }
        }
        if report.manifest_outdated {
            eprintln!("manifest out of date");
        }
        bail!(
            "{}: generated files are out of date; run without --check to regenerate",
            args.out.display()
        );
    }

    let mut write_options = WriteOptions::new(label);
    write_options.overwrite_unowned = args.force;
    write_options.allow_partial = args.allow_partial;
    write_options.source_id = source_id(&source, &args.out);
    let report = write(&generation, &args.out, target, &write_options)?;
    for path in &report.kept_edited {
        eprintln!(
            "warning: {} is no longer generated but was edited, so it was kept and is no longer tracked",
            path.display()
        );
    }
    eprintln!(
        "{}: {} written, {} unchanged, {} removed",
        args.out.display(),
        report.written.len(),
        report.unchanged.len(),
        report.removed.len()
    );
    if !generation.dependencies.rust.is_empty() {
        let deps: Vec<String> = generation
            .dependencies
            .rust
            .iter()
            .map(|d| {
                if d.features.is_empty() {
                    d.name.clone()
                } else {
                    format!("{} ({})", d.name, d.features.join(", "))
                }
            })
            .collect();
        eprintln!("the generated code needs: {}", deps.join(", "));
    }
    if failing > 0 {
        return Err(partial_failure());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "roas-cli-generate-{tag}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const PET: &str = r##"{
        "openapi": "3.2.0",
        "info": {"title": "t", "version": "1"},
        "paths": {},
        "components": {"schemas": {
            "Pet": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}
        }}
    }"##;

    fn args(file: &Path, out: &Path) -> GenerateArgs {
        GenerateArgs {
            file: Some(file.to_path_buf()),
            out: out.to_path_buf(),
            config: None,
            target: Some(GenerateTarget::Rust),
            label: None,
            format: None,
            ignore: vec![Options::IgnoreUnusedSchemas],
            allow_codegen_extensions: false,
            strict: false,
            allow_partial: false,
            force: false,
            check: false,
        }
    }

    #[test]
    fn generates_then_checks_clean_then_detects_drift() {
        let dir = TempDir::new("cycle");
        let spec = dir.0.join("pet store.json");
        std::fs::write(&spec, PET).unwrap();
        let out = dir.0.join("out");
        run_generate(args(&spec, &out)).unwrap();
        let code = std::fs::read_to_string(out.join("pet_store.rs")).unwrap();
        assert!(code.contains("pub struct Pet {"), "{code}");
        assert!(out.join(".roas-codegen-pet-store-rust.json").exists());

        let mut check_args = args(&spec, &out);
        check_args.check = true;
        run_generate(check_args).unwrap();

        std::fs::write(out.join("pet_store.rs"), "edited").unwrap();
        let mut check_args = args(&spec, &out);
        check_args.check = true;
        let error = run_generate(check_args).unwrap_err().to_string();
        assert!(error.contains("out of date"), "{error}");
    }

    #[test]
    fn errors_leave_the_directory_untouched_unless_partial_is_allowed() {
        let dir = TempDir::new("partial");
        let spec = dir.0.join("api.json");
        std::fs::write(
            &spec,
            r##"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {},
                "components": {"schemas": {"Bad": {"not": {"type": "string"}}, "Fine": {"type": "object", "properties": {"a": {"type": "string"}}}}}}"##,
        )
        .unwrap();
        let out = dir.0.join("out");
        let error = run_generate(args(&spec, &out)).unwrap_err().to_string();
        assert!(error.contains("nothing written"), "{error}");
        assert!(!out.exists());
        let mut partial = args(&spec, &out);
        partial.allow_partial = true;
        let error = run_generate(partial).unwrap_err().to_string();
        assert!(error.contains("wrote what could be generated"), "{error}");
        assert!(
            std::fs::read_to_string(out.join("api.rs"))
                .unwrap()
                .contains("pub struct Fine {")
        );
    }

    #[test]
    fn check_after_a_partial_write_is_clean() {
        let dir = TempDir::new("partial-check");
        let spec = dir.0.join("api.json");
        std::fs::write(
            &spec,
            r##"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {},
                "components": {"schemas": {"Bad": {"not": {"type": "string"}}, "Fine": {"type": "object", "properties": {"a": {"type": "string"}}}}}}"##,
        )
        .unwrap();
        let out = dir.0.join("out");
        let mut partial = args(&spec, &out);
        partial.allow_partial = true;
        assert!(run_generate(partial).is_err());
        let mut check = args(&spec, &out);
        check.allow_partial = true;
        check.check = true;
        // Up to date, but the errors still fail the run.
        let error = run_generate(check).unwrap_err().to_string();
        assert!(error.contains("1 error"), "{error}");
    }

    #[test]
    fn strict_promotes_warnings() {
        let dir = TempDir::new("strict");
        let spec = dir.0.join("api.json");
        std::fs::write(
            &spec,
            r##"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {},
                "components": {"schemas": {"N": {"type": "integer"}}}}"##,
        )
        .unwrap();
        let out = dir.0.join("out");
        let mut strict = args(&spec, &out);
        strict.strict = true;
        let error = run_generate(strict).unwrap_err().to_string();
        assert!(error.contains("1 diagnostic"), "{error}");
        run_generate(args(&spec, &out)).unwrap();
    }

    #[test]
    fn config_file_supplies_target_and_refuses_unowned_files() {
        let dir = TempDir::new("config");
        let spec = dir.0.join("api.yaml");
        std::fs::write(
            &spec,
            "openapi: 3.2.0\ninfo: {title: t, version: '1'}\npaths: {}\ncomponents:\n  schemas:\n    Pet: {type: object, properties: {name: {type: string}}}\n",
        )
        .unwrap();
        let config = dir.0.join("codegen.toml");
        std::fs::write(
            &config,
            "target = \"rust\"\n[rust]\nmodule = \"types\"\n[types.Pet]\nrename = \"Animal\"\n",
        )
        .unwrap();
        let out = dir.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("types.rs"), "hand written").unwrap();
        let mut from_config = args(&spec, &out);
        from_config.target = None;
        from_config.config = Some(config.clone());
        let error = run_generate(from_config).unwrap_err().to_string();
        assert!(error.contains("not written by this generator"), "{error}");
        let mut forced = args(&spec, &out);
        forced.target = None;
        forced.config = Some(config);
        forced.force = true;
        run_generate(forced).unwrap();
        assert!(
            std::fs::read_to_string(out.join("types.rs"))
                .unwrap()
                .contains("pub struct Animal {")
        );
    }

    #[test]
    fn invalid_descriptions_and_missing_targets_are_reported() {
        let dir = TempDir::new("invalid");
        let spec = dir.0.join("api.json");
        std::fs::write(
            &spec,
            r##"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {},
                "components": {"schemas": {"A": {"$ref": "#/components/schemas/Missing"}}}}"##,
        )
        .unwrap();
        let out = dir.0.join("out");
        let error = run_generate(args(&spec, &out)).unwrap_err().to_string();
        assert!(error.contains("validation failed"), "{error}");

        std::fs::write(&spec, PET).unwrap();
        let mut no_target = args(&spec, &out);
        no_target.target = None;
        let error = format!("{:#}", run_generate(no_target).unwrap_err());
        assert!(error.contains("target"), "{error}");
    }

    #[test]
    fn two_descriptions_share_one_directory() {
        let dir = TempDir::new("shared");
        let pets = dir.0.join("pets.json");
        let billing = dir.0.join("billing.json");
        std::fs::write(&pets, PET).unwrap();
        std::fs::write(&billing, PET).unwrap();
        let out = dir.0.join("out");
        run_generate(args(&pets, &out)).unwrap();
        run_generate(args(&billing, &out)).unwrap();
        assert!(out.join("pets.rs").exists() && out.join("billing.rs").exists());
        assert!(out.join(".roas-codegen-pets-rust.json").exists());
        assert!(out.join(".roas-codegen-billing-rust.json").exists());
    }

    #[test]
    fn module_names_come_from_labels() {
        assert_eq!(
            roas_codegen::rust_module_name("pet-store-v2"),
            "pet_store_v2"
        );
        assert_eq!(roas_codegen::rust_module_name("2024-api"), "m_2024_api");
        assert_eq!(roas_codegen::rust_module_name("--"), "api");
        assert_eq!(roas_codegen::rust_module_name("type"), "type_types");
    }

    #[test]
    fn lint_only_checks_do_not_block_generation() {
        let dir = TempDir::new("lint");
        let spec = dir.0.join("api.json");
        std::fs::write(
            &spec,
            r##"{"openapi": "3.2.0", "info": {"title": "", "version": "1"},
                "paths": {"/p": {"get": {"tags": ["pets"], "responses": {"200": {"description": "ok"}}}}},
                "components": {"schemas": {"Extra": {"type": "object", "properties": {"a": {"type": "string"}}}}}}"##,
        )
        .unwrap();
        let out = dir.0.join("out");
        let mut plain = args(&spec, &out);
        plain.ignore.clear();
        run_generate(plain).unwrap();
        assert!(
            std::fs::read_to_string(out.join("api.rs"))
                .unwrap()
                .contains("pub struct Extra {")
        );
    }

    #[test]
    fn a_label_shared_by_two_descriptions_is_refused() {
        let dir = TempDir::new("stems");
        std::fs::create_dir_all(dir.0.join("pets")).unwrap();
        std::fs::create_dir_all(dir.0.join("billing")).unwrap();
        let pets = dir.0.join("pets/pets.json");
        let billing = dir.0.join("billing/billing.json");
        std::fs::write(&pets, PET).unwrap();
        std::fs::write(&billing, PET.replace("Pet", "Invoice")).unwrap();
        let out = dir.0.join("out");
        let mut first = args(&pets, &out);
        first.label = Some("api".into());
        run_generate(first).unwrap();
        let mut second = args(&billing, &out);
        second.label = Some("api".into());
        let error = format!("{:#}", run_generate(second).unwrap_err());
        assert!(error.contains("its own label"), "{error}");
        assert!(
            std::fs::read_to_string(out.join("api.rs"))
                .unwrap()
                .contains("pub struct Pet {")
        );
    }

    #[test]
    fn same_file_name_in_another_folder_is_refused_and_renames_are_not() {
        let dir = TempDir::new("siblings");
        std::fs::create_dir_all(dir.0.join("pets")).unwrap();
        std::fs::create_dir_all(dir.0.join("billing")).unwrap();
        let pets = dir.0.join("pets/openapi.json");
        let billing = dir.0.join("billing/openapi.json");
        std::fs::write(&pets, PET).unwrap();
        std::fs::write(&billing, PET.replace("Pet", "Invoice")).unwrap();
        let out = dir.0.join("out");
        run_generate(args(&pets, &out)).unwrap();
        let error = format!("{:#}", run_generate(args(&billing, &out)).unwrap_err());
        assert!(error.contains("its own label"), "{error}");
        assert!(
            std::fs::read_to_string(out.join("openapi.rs"))
                .unwrap()
                .contains("pub struct Pet {")
        );
        // The same description moved from JSON to YAML keeps its label.
        let yaml = dir.0.join("pets/openapi.yaml");
        std::fs::write(&yaml, "openapi: 3.2.0\ninfo: {title: t, version: '1'}\npaths: {}\ncomponents:\n  schemas:\n    Pet: {type: object, required: [name], properties: {name: {type: string}}}\n").unwrap();
        std::fs::remove_file(&pets).unwrap();
        run_generate(args(&yaml, &out)).unwrap();
    }

    #[test]
    fn check_mode_errors_say_cannot_check() {
        let dir = TempDir::new("check-errors");
        let spec = dir.0.join("api.json");
        std::fs::write(
            &spec,
            r##"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {},
                "components": {"schemas": {"Bad": {"not": {"type": "string"}}}}}"##,
        )
        .unwrap();
        let mut check = args(&spec, &dir.0.join("out"));
        check.check = true;
        let error = run_generate(check).unwrap_err().to_string();
        assert!(error.contains("cannot check"), "{error}");
    }

    #[test]
    fn source_ids_are_relative_to_the_output_directory() {
        let id = source_id(
            &InputSource::File("/repo/specs/pets/openapi.yaml".into()),
            Path::new("/repo/src/api"),
        );
        assert_eq!(id.as_deref(), Some("../../specs/pets/openapi"));
        assert_eq!(source_id(&InputSource::Stdin, Path::new("/x")), None);
    }

    #[test]
    fn default_label_is_a_safe_file_stem() {
        assert_eq!(
            default_label(&InputSource::File("dir/Pet Store.v2.json".into())),
            "pet-store-v2"
        );
        assert_eq!(default_label(&InputSource::File("...json".into())), "api");
        assert_eq!(default_label(&InputSource::Stdin), "api");
    }
}
