//! `roas openapi generate` — generate Rust types from an OpenAPI
//! description, powered by the `roas-codegen` crate.
//!
//! The description is read as raw bytes, so that a build with the
//! `exact-numbers` feature keeps JSON numbers exact, then version-detected, normalized to 3.2, validated with
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
    // Go joins when its backend exists; the library refuses it until then.
}

impl From<GenerateTarget> for Target {
    fn from(target: GenerateTarget) -> Self {
        match target {
            GenerateTarget::Rust => Target::Rust,
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
    /// several targets can share one directory, and names the generated
    /// file unless the configuration sets `rust.module`. Defaults to the
    /// input file's name, or `api` for stdin.
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
    /// since such a file is probably hand-written; one that already holds
    /// exactly the generated content is adopted without it.
    #[arg(long)]
    pub(crate) force: bool,

    /// Let this description take a label recorded for a different source:
    /// after moving or renaming it, or moving the output directory, since
    /// the source is recorded relative to it. Off by default, since such a
    /// label probably belongs to another description.
    #[arg(long)]
    pub(crate) replace_source: bool,

    /// Write nothing; fail if the output directory differs from what would
    /// be generated.
    #[arg(long)]
    pub(crate) check: bool,
}

/// Read the description as bytes, so JSON can keep exact numbers, and give
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

/// `path` without a description's format extension (`.json`, `.yaml`,
/// `.yml`), and with any other one kept: `api.v1` and `api.v2` are two
/// descriptions, `pets.json` and `pets.yaml` one.
fn without_format_extension(path: &Path) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext)
            if ["json", "yaml", "yml"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known)) =>
        {
            path.with_extension("")
        }
        _ => path.to_path_buf(),
    }
}

/// The default label: the input file's stem, made safe for a manifest name.
fn default_label(source: &InputSource) -> String {
    let stem = match source {
        InputSource::File(path) => without_format_extension(path)
            .file_name()
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
/// output directory, `/`-separated and without a `.json`/`.yaml`/`.yml`
/// extension. Two files
/// called `openapi.yaml` in different folders differ; converting
/// `petstore.json` to `petstore.yaml` does not. Stdin has no identity.
fn source_id(source: &InputSource, out: &Path) -> Option<String> {
    let InputSource::File(path) = source else {
        return None;
    };
    // Resolve symlinks so one file has one identity however its path is
    // spelled (`$PWD`, macOS `/tmp` → `/private/tmp`). The output directory
    // may not exist yet, so its deepest existing ancestor is resolved.
    let file = without_format_extension(&resolve(path)?);
    let out = resolve(out)?;
    let file: Vec<_> = file.components().collect();
    let out: Vec<_> = out.components().collect();
    let common = file.iter().zip(&out).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), out.len() - common).collect();
    parts.extend(file[common..].iter().map(|c| lossless(c.as_os_str())));
    Some(parts.join("/"))
}

/// A path component as text that keeps every byte, so two different
/// names never record one source: `%` and bytes that are not UTF-8 are
/// written as `%XX`.
fn lossless(name: &std::ffi::OsStr) -> String {
    if let Some(text) = name.to_str() {
        return text.replace('%', "%25");
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut out = String::new();
        for chunk in name.as_bytes().utf8_chunks() {
            out.push_str(&chunk.valid().replace('%', "%25"));
            for byte in chunk.invalid() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
        out
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::ffi::OsStrExt;
        name.encode_wide()
            .map(|unit| format!("%u{unit:04X}"))
            .collect()
    }
}

/// The canonical form of `path`, resolving its deepest existing ancestor
/// and appending the rest lexically.
fn resolve(path: &Path) -> Option<PathBuf> {
    // The filesystem resolves first, `..` included, so `link/..` means the
    // link's parent; only the part that does not exist yet is lexical.
    let absolute = std::path::absolute(path).ok()?;
    let parts: Vec<_> = absolute.components().collect();
    for split in (1..=parts.len()).rev() {
        let prefix: PathBuf = parts[..split].iter().collect();
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            let mut out = real;
            for part in &parts[split..] {
                out.push(part);
            }
            return Some(normalize(&out));
        }
    }
    None
}

/// Resolve `.` and `..` lexically: `std::path::absolute` keeps them, and
/// `pets/../out` must compare equal to `out`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn show(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The writer's errors name library options; say which flag sets them.
fn hint(out: &Path, error: roas_codegen::WriteError) -> anyhow::Error {
    use roas_codegen::WriteError as E;
    match &error {
        E::Unowned(_) => anyhow!("{}: {error} (pass --force)", out.display()),
        // Their paths are relative to the output directory.
        E::OwnedByOther { .. }
        | E::ReservedPath(_)
        | E::IsADirectory(_)
        | E::OutputDirOwned { .. } => {
            anyhow!("{}: {error}", out.display())
        }
        // Always relative to the output directory; the directory itself
        // has its own variants, which name it.
        E::Symlink(_) | E::NotADirectory(_) => anyhow!("{}: {error}", out.display()),
        E::SourceMismatch {
            label,
            previous,
            current,
            ..
        } => anyhow!(
            "label {label:?} belongs to output generated from {previous:?}, not {current:?}; \
             if it is a different description, give it its own --label; \
             if the description or the output directory moved, pass --replace-source"
        ),
        _ => error.into(),
    }
}

pub(crate) fn run_generate(mut args: GenerateArgs) -> Result<()> {
    // As the library reads it, so the recorded source is computed too.
    if args.out.as_os_str().is_empty() {
        args.out = PathBuf::from(".");
    }
    let source = resolve_input_source(args.file.as_deref())?;
    // Before any work: a label that cannot name a manifest fails the run
    // anyway, and should name what was typed.
    if let Some(given) = &args.label
        && roas_codegen::manifest_name(&given.to_ascii_lowercase(), Target::Rust).is_err()
    {
        bail!("--label {given:?} must use only letters, digits, `-` and `_` (it is lowercased)");
    }
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
    let mut flags = ConfigFile::default();
    flags.target = args.target.map(Target::from);
    flags.allow_codegen_extensions = args.allow_codegen_extensions.then_some(true);
    // The generated file is named after the label unless the configuration
    // names the module, so descriptions sharing a directory do not collide.
    if file.rust.module.is_none() {
        flags.rust.module = Some(roas_codegen::rust_module_name(&label));
    }
    let config = file.merge(flags).build().map_err(|error| match error {
        roas_codegen::ConfigError::MissingTarget => anyhow!(
            "no target language: set `target = \"rust\"` in the configuration, or pass `--target rust`"
        ),
        other => other.into(),
    })?;

    let generation = generate(&document, &config)?;
    let mut errors = 0;
    let mut warnings = 0;
    for diagnostic in &generation.diagnostics {
        eprintln!("{diagnostic}");
        match diagnostic.severity {
            Severity::Error => errors += 1,
            // A kind added later counts as a warning: `--strict` fails on it.
            _ => warnings += 1,
        }
    }
    let failing = if args.strict {
        errors + warnings
    } else {
        errors
    };
    let partial_failure = || {
        anyhow!(
            "{}: {failing} {}{}; {} what could be generated",
            source.display(),
            if args.strict { "diagnostic" } else { "error" },
            if failing == 1 { "" } else { "s" },
            if args.check { "checked" } else { "wrote" }
        )
    };
    let unformatted = generation
        .diagnostics
        .iter()
        .any(|d| matches!(d.kind, roas_codegen::DiagnosticKind::Unformatted));
    if failing > 0 && !args.allow_partial {
        // rustfmt is the reason only when nothing else failed.
        let only_unformatted = unformatted && failing == 1 && errors == 0;
        let (what, hint) = if args.strict && only_unformatted {
            (
                if args.check {
                    "cannot check"
                } else {
                    "nothing written"
                },
                "rustfmt did not format the output; the warning above says why (rustfmt installs with `rustup component add rustfmt`)",
            )
        } else if args.check {
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
        if unformatted {
            bail!(
                "--check needs formatted output, which cannot match an unformatted run; rustfmt did not format it, and the warning above says why (rustfmt installs with `rustup component add rustfmt`)"
            );
        }
        let mut check_options = WriteOptions::new(label.clone());
        check_options.allow_partial = args.allow_partial;
        check_options.source_id = source_id(&source, &args.out);
        check_options.replace_source = args.replace_source;
        check_options.overwrite_unowned = args.force;
        let report =
            check(&generation, &args.out, &check_options).map_err(|e| hint(&args.out, e))?;
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
            (
                "no longer generated but edited (kept, untracked)",
                &report.stale_edited,
            ),
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
    write_options.replace_source = args.replace_source;
    write_options.allow_partial = args.allow_partial;
    write_options.source_id = source_id(&source, &args.out);
    let report = write(&generation, &args.out, &write_options).map_err(|e| hint(&args.out, e))?;
    for path in &report.kept_edited {
        eprintln!(
            "warning: {} is no longer generated but was edited, so it was kept and is no longer tracked",
            path.display()
        );
    }
    for (old, new) in &report.renamed {
        eprintln!("renamed {} to {}", old.display(), new.display());
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
            replace_source: false,
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
        assert!(error.contains("its own --label"), "{error}");
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
        assert!(error.contains("its own --label"), "{error}");
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
    fn source_ids_are_relative_and_ignore_how_the_path_is_spelled() {
        let dir = TempDir::new("ids");
        let real = dir.0.join("real");
        std::fs::create_dir_all(real.join("spec/pets")).unwrap();
        std::fs::write(real.join("spec/pets/openapi.json"), PET).unwrap();
        std::fs::create_dir_all(real.join("billing")).unwrap();
        std::fs::write(real.join("billing/openapi.json"), PET).unwrap();
        let file = InputSource::File(real.join("spec/pets/openapi.json"));
        assert_eq!(
            source_id(&file, &real.join("src/api")).as_deref(),
            Some("../../spec/pets/openapi")
        );
        // `..` is resolved, so siblings stay distinct.
        let pets = InputSource::File(real.join("spec/pets/../pets/openapi.json"));
        let billing = InputSource::File(real.join("billing/openapi.json"));
        assert_ne!(
            source_id(&pets, &real.join("out")),
            source_id(&billing, &real.join("out"))
        );
        assert_eq!(source_id(&InputSource::Stdin, Path::new("/x")), None);
        #[cfg(unix)]
        {
            // Through a symlink: the same identity.
            let link = dir.0.join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let via_link = InputSource::File(link.join("spec/pets/openapi.json"));
            assert_eq!(
                source_id(&via_link, &real.join("out")),
                source_id(&file, &link.join("out"))
            );
            // A symlink followed by `..` is resolved by the filesystem:
            // `spec/up/..` is wherever `up` points, not `spec`.
            std::os::unix::fs::symlink(real.join("billing"), real.join("spec/up")).unwrap();
            let through = InputSource::File(real.join("spec/up/../billing/openapi.json"));
            assert_eq!(
                source_id(&through, &real.join("out")),
                source_id(
                    &InputSource::File(real.join("billing/openapi.json")),
                    &real.join("out")
                )
            );
        }
    }

    #[test]
    fn replace_source_lets_a_moved_description_keep_its_label() {
        let dir = TempDir::new("moved");
        std::fs::create_dir_all(dir.0.join("spec")).unwrap();
        std::fs::create_dir_all(dir.0.join("api")).unwrap();
        let before = dir.0.join("spec/pets.json");
        std::fs::write(&before, PET).unwrap();
        let out = dir.0.join("out");
        run_generate(args(&before, &out)).unwrap();
        let after = dir.0.join("api/pets.json");
        std::fs::rename(&before, &after).unwrap();
        let error = format!("{:#}", run_generate(args(&after, &out)).unwrap_err());
        assert!(error.contains("--replace-source"), "{error}");
        let mut forced = args(&after, &out);
        forced.force = true;
        assert!(
            run_generate(forced).is_err(),
            "--force alone does not move a label"
        );
        let mut moved = args(&after, &out);
        moved.replace_source = true;
        run_generate(moved).unwrap();
        run_generate(args(&after, &out)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn source_ids_keep_every_byte() {
        use std::os::unix::ffi::OsStrExt;
        let a = std::ffi::OsStr::from_bytes(b"spec\x80");
        let b = std::ffi::OsStr::from_bytes(b"spec\x81");
        assert_ne!(lossless(a), lossless(b));
        assert_eq!(lossless(a), "spec%80");
        assert_eq!(lossless(std::ffi::OsStr::new("a%80")), "a%2580");
    }

    #[test]
    fn only_format_extensions_are_dropped() {
        assert_eq!(
            without_format_extension(Path::new("a/api.v1")),
            Path::new("a/api.v1")
        );
        assert_eq!(
            without_format_extension(Path::new("a/pets.YAML")),
            Path::new("a/pets")
        );
        assert_ne!(
            default_label(&InputSource::File("api.v1".into())),
            default_label(&InputSource::File("api.v2".into()))
        );
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

    #[test]
    fn a_bad_label_fails_before_any_work_and_names_what_was_typed() {
        let dir = TempDir::new("bad-label");
        let mut bad = args(&dir.0.join("missing.json"), &dir.0.join("out"));
        bad.label = Some("My API".into());
        let error = format!("{:#}", run_generate(bad).unwrap_err());
        assert!(error.contains("\"My API\""), "{error}");
    }
}
