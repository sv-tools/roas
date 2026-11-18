//! `roas openapi generate` — generate Rust or Go types from an OpenAPI
//! description, powered by the `roas-codegen` crate.
//!
//! The description is read as raw bytes, so that a build with the
//! `exact-numbers` feature keeps JSON numbers exact, then version-detected, normalized to 3.2, validated with
//! external references skipped, lowered and rendered. `--apply` overlays
//! apply before any of that, and the result is written back in the format
//! it was read in. Files are written
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
    /// Needs `gofmt` to generate, and Go 1.27 or later to build.
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
    /// several targets can share one directory, and names the generated
    /// file unless the configuration sets `rust.module` or `go.file`. Defaults to the
    /// input file's name, or `api` for stdin or a pipe.
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

    /// Path to an OpenAPI Overlay document to apply to the description
    /// before it is validated and generated, as `roas openapi convert
    /// --apply` does. Repeat the flag to apply several overlays in order.
    #[arg(long)]
    pub(crate) apply: Vec<PathBuf>,

    /// Per-call overlay apply option (repeatable). Requires at least one
    /// `--apply`.
    #[arg(long = "apply-option", value_enum, requires = "apply")]
    pub(crate) apply_options: Vec<roas_overlay::apply::ApplyOptions>,

    /// Generate validation: every type gets a `validate` that checks the
    /// constraints its type cannot express. Overrides `validation` in the
    /// configuration.
    #[arg(long)]
    pub(crate) validate: bool,

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

    /// A directory of templates overriding the built-in ones by name; it
    /// holds a `roas-templates.toml` declaring `template_api = 1`.
    /// Overrides `template_dir` in the configuration.
    #[arg(long)]
    pub(crate) template_dir: Option<PathBuf>,

    /// Print the JSON context the templates receive to stdout, and write
    /// nothing: the reference for writing templates.
    #[arg(long, conflicts_with_all = ["check", "dump_rendered"])]
    pub(crate) print_context: bool,

    /// Also write the files as the templates render them, before the syntax
    /// check and the formatter, into this existing directory, which must be
    /// apart from the output directory: what to look at when a template
    /// produces code the formatter refuses.
    #[arg(long, conflicts_with = "check")]
    pub(crate) dump_rendered: Option<PathBuf>,

    /// Write a template directory overriding nothing into this existing
    /// directory, apart from the output directory, and generate nothing:
    /// for each built-in template this target, layout and validation render, a file
    /// at the path an override takes, extending the built-in and listing
    /// every block it declares. Existing files are never overwritten.
    #[arg(long, conflicts_with_all = ["check", "print_context", "dump_rendered"])]
    pub(crate) dump_scaffold: Option<PathBuf>,
}

/// Read the description as bytes, so JSON can keep exact numbers, and give
/// it the URI every schema id is built from.
fn read_description(source: &InputSource, format: Option<InputFormat>) -> Result<(Input, Url)> {
    let (bytes, uri, yaml) = match source {
        InputSource::File(path) => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            // As stored on disk, so the generated header names the file the
            // same way however its path was typed (case on macOS/Windows),
            // just as the recorded source does.
            // A pipe (`<(...)`, `/dev/stdin`) has no stored name: it is
            // named like stdin.
            let uri = if is_regular_file(path) {
                let absolute =
                    stored_path(path).with_context(|| format!("resolving {}", path.display()))?;
                Url::from_file_path(&absolute).map_err(|()| {
                    anyhow!("{} cannot be expressed as a file URL", absolute.display())
                })?
            } else {
                Url::parse("stdin:///").expect("a valid URL")
            };
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

/// Apply the `--apply` overlays to the description, in the format it was
/// read in, so JSON keeps exact numbers and YAML keeps its fidelity.
fn overlay_description(
    source: &InputSource,
    input: Input,
    overlays: &[PathBuf],
    options: &[roas_overlay::apply::ApplyOptions],
) -> Result<Input> {
    if overlays.is_empty() {
        return Ok(input);
    }
    let options = options.iter().copied().collect();
    let (bytes, yaml) = match input {
        Input::Json(bytes) => (bytes, false),
        Input::Yaml(bytes) => (bytes, true),
        _ => unreachable!("descriptions are read as text"),
    };
    let mut value = std::str::from_utf8(&bytes)
        .context("not UTF-8")
        .and_then(|text| crate::versioned::parse_value(text, yaml))
        .with_context(|| format!("{}: cannot be read", source.display()))?;
    let all_json = crate::overlay::apply_overlays(&mut value, overlays, options)?;
    // Exact only as JSON throughout: a YAML overlay's numbers went through
    // `f64`, and a YAML description is no more exact for being overlaid.
    // Otherwise the value goes on as it is, of no known fidelity, which
    // `exact_numbers` refuses, and with no text to write it back to.
    Ok(if !yaml && all_json {
        Input::Json(serde_json::to_vec(&value)?)
    } else {
        Input::Value(value)
    })
}

/// The description's path with its directories resolved and its own name
/// as stored: a symlinked description (a "current" link) is named by the
/// link, not its target, so repointing the link keeps the label, source
/// and header; and the name has the case on disk, however it was typed.
fn stored_path(path: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let parent = std::fs::canonicalize(parent)?;
    let typed = parent.join(name);
    let entry = std::fs::symlink_metadata(&typed)?;
    // The name as typed when it is stored that way; otherwise the one entry
    // that is the same file (another case), never another hard link.
    // A directory that can be searched but not listed: the typed name is
    // all there is to go on.
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return Ok(typed);
    };
    let entries: Vec<_> = entries.flatten().collect();
    if entries.iter().any(|e| e.file_name() == name) {
        return Ok(typed);
    }
    let stored = entries
        .iter()
        .find(|e| {
            if cfg!(unix) {
                // The same file, under this name in another case, or as its
                // only name; never a hard link under another name.
                same_entry(&entry, &e.path())
                    && (single_link(&entry)
                        || e.file_name()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&name.to_string_lossy()))
            } else {
                // No inode to compare: Windows folds case, so the entry
                // spelled the same ignoring case is this file.
                e.file_name().eq_ignore_ascii_case(name)
            }
        })
        .map(|e| e.file_name());
    Ok(parent.join(stored.unwrap_or_else(|| name.to_owned())))
}

/// Whether the file has one name only: a hard link is a name of its own.
fn single_link(entry: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        entry.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        let _ = entry;
        true
    }
}

/// Whether `other` is the directory entry `entry` describes.
fn same_entry(entry: &std::fs::Metadata, other: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(other)
            .is_ok_and(|m| m.dev() == entry.dev() && m.ino() == entry.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (entry, other);
        false
    }
}

/// Whether `path` names a lasting file, as opposed to a pipe, a device or
/// a file descriptor (`<(...)`, `/dev/stdin`, `/proc/self/fd/0`), which is
/// read like stdin: label `api`, no recorded source.
fn is_regular_file(path: &Path) -> bool {
    !is_pipe(path) && std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// Whether `path` reads through a file descriptor or another non-file.
fn is_pipe(path: &Path) -> bool {
    // A file descriptor by name, even when it is redirected from a file.
    let parts: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    let through_fd = path == Path::new("/dev/stdin")
        || path.starts_with("/dev/fd")
        || (path.starts_with("/proc") && parts.get(3).is_some_and(|p| *p == "fd"));
    through_fd || std::fs::metadata(path).is_ok_and(|m| !m.is_file())
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
        // As stored, like the header and source: two spellings of one file
        // (case, Unicode normalization) give one label.
        InputSource::File(path) if !is_pipe(path) => {
            let stored = stored_path(path).unwrap_or_else(|_| path.clone());
            without_format_extension(&stored)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        }
        InputSource::File(_) => String::new(),
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
/// `petstore.json` to `petstore.yaml` does not. Stdin and pipes have no identity.
fn source_id(source: &InputSource, out: &Path) -> Option<String> {
    let InputSource::File(path) = source else {
        return None;
    };
    // A pipe has no lasting identity: like stdin, it records none.
    if !is_regular_file(path) {
        return None;
    }
    // Resolve symlinks so one file has one identity however its path is
    // spelled (`$PWD`, macOS `/tmp` → `/private/tmp`). The output directory
    // may not exist yet, so its deepest existing ancestor is resolved.
    let file = without_format_extension(&stored_path(path).ok()?);
    let out = resolve(out)?;
    let file: Vec<_> = file.components().collect();
    let out: Vec<_> = out.components().collect();
    let common = file.iter().zip(&out).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), out.len() - common).collect();
    parts.extend(file[common..].iter().map(|c| lossless(c.as_os_str())));
    // Never empty: `petstore.json` beside `-o petstore` is a source too, and
    // an empty one would switch the check off.
    if parts.is_empty() {
        return Some(".".to_owned());
    }
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
            // A `..` through a directory that did not exist yet was
            // resolved by text; what remains may still pass through links,
            // so it is resolved again, now without any `..`.
            let normalized = normalize(&out);
            return if normalized == out {
                Some(normalized)
            } else {
                resolve(&normalized)
            };
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
        | E::DirectorySpelling { .. }
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

/// Whether two existing directories are one, by identity rather than by
/// name: case and Unicode normalization make names an unreliable test.
fn same_dir(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!(
            (std::fs::metadata(a), std::fs::metadata(b)),
            (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino()
        )
    }
    #[cfg(not(unix))]
    {
        matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
    }
}

/// Whether the dump directory `flag` names, which must exist, is the
/// output directory, holds it, or sits inside it. Compared by identity
/// along every existing ancestor, so no spelling of either path can hide
/// the overlap.
fn overlaps(flag: &str, dump: &Path, out: &Path) -> Result<bool> {
    if !dump.is_dir() {
        bail!("{flag} {} must be an existing directory", dump.display());
    }
    let resolved =
        |path: &Path| resolve(path).ok_or_else(|| anyhow!("cannot resolve {}", path.display()));
    let (dump, out) = (resolved(dump)?, resolved(out)?);
    // The output directory, or the part of it that exists, inside the dump.
    if out.ancestors().any(|a| same_dir(a, &dump)) {
        return Ok(true);
    }
    // The dump inside the output directory, which then exists.
    Ok(out.is_dir() && dump.ancestors().any(|a| same_dir(a, &out)))
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
        bail!(
            "--label {given:?} must be non-empty and use only ASCII letters, digits, `-` and `_` (it is lowercased)"
        );
    }
    let (input, uri) = read_description(&source, args.format)?;
    // Before parsing, so validation, generation and every dump see the
    // overlaid description.
    let input = overlay_description(&source, input, &args.apply, &args.apply_options)?;
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
    flags.validation = args.validate.then_some(true);
    if let Some(dir) = &args.template_dir {
        flags.template_dir =
            Some(std::path::absolute(dir).with_context(|| format!("resolving {}", dir.display()))?);
    }
    // The generated file is named after the label unless the configuration
    // names it, so descriptions sharing a directory do not collide.
    if file.rust.module.is_none() {
        flags.rust.module = Some(roas_codegen::rust_module_name(&label));
    }
    if file.go.file.is_none() {
        flags.go.file = Some(roas_codegen::go_file_name(&label));
    }
    let config = file.merge(flags).build().map_err(|error| match error {
        roas_codegen::ConfigError::MissingTarget => anyhow!(
            "no target language: set `target = \"rust\"` or `\"go\"` in the configuration, or pass `--target rust` or `--target go`"
        ),
        other => other.into(),
    })?;

    if let Some(dir) = &args.dump_scaffold {
        // Not where generated files and their manifests live either.
        if overlaps("--dump-scaffold", dir, &args.out)? {
            bail!(
                "--dump-scaffold {} overlaps the output directory {}; dump somewhere else",
                dir.display(),
                args.out.display()
            );
        }
        let files = roas_codegen::scaffold(&config)?;
        // A scaffold is for editing, so a file already there may be
        // someone's work, and a link inside the directory could lead
        // anywhere: every path is checked before anything is written.
        for file in &files {
            scaffold_slot(dir, &file.path)?;
        }
        // The manifest last: a run that fails part way leaves no
        // `roas-templates.toml` making the directory a template one.
        let (manifest, templates): (Vec<_>, Vec<_>) = files
            .into_iter()
            .partition(|file| file.path == std::path::Path::new("roas-templates.toml"));
        for file in templates.into_iter().chain(manifest) {
            let path = dir.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, file.contents.as_bytes()))
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("scaffolded {}", path.display());
        }
        return Ok(());
    }
    if args.print_context {
        let context = roas_codegen::template_context(&document, &config)?;
        println!("{}", serde_json::to_string_pretty(&context.context)?);
        // What was suppressed is missing from the context: say so, and
        // fail as a generation would.
        let mut failing = 0;
        for diagnostic in &context.diagnostics {
            eprintln!("{diagnostic}");
            if args.strict || diagnostic.severity == Severity::Error {
                failing += 1;
            }
        }
        if failing > 0 {
            bail!(
                "{}: {failing} {}{}; the context leaves out what could not be generated",
                source.display(),
                if args.strict { "diagnostic" } else { "error" },
                if failing == 1 { "" } else { "s" }
            );
        }
        return Ok(());
    }
    if let Some(dir) = &args.dump_rendered {
        // The dump is raw text with no ownership rules: it must not land
        // where generated files and their manifests live.
        if overlaps("--dump-rendered", dir, &args.out)? {
            bail!(
                "--dump-rendered {} overlaps the output directory {}; dump somewhere else",
                dir.display(),
                args.out.display()
            );
        }
        for file in roas_codegen::render(&document, &config)? {
            let path = dir.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            // Replaced, never written through: a link left at that name
            // (to a generated file, say) is removed rather than followed.
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("replacing {}", path.display())),
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, file.contents.as_bytes()))
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("rendered {}", path.display());
        }
    }
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
        .chain(generation.dependencies.go.iter().map(|d| d.import.clone()))
        .collect();
    if !deps.is_empty() {
        eprintln!("the generated code needs: {}", deps.join(", "));
    }
    if failing > 0 {
        return Err(partial_failure());
    }
    Ok(())
}

/// Whether the last part of `path` is an entry of its directory spelled
/// exactly so.
fn spelled_exactly(path: &std::path::Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return true;
    };
    std::fs::read_dir(parent).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == name)
    })
}

/// Refuses a scaffold file at `relative` under `dir` unless the path is
/// free: every directory on the way is a real one, not a link or a file,
/// and nothing is at the end.
fn scaffold_slot(dir: &std::path::Path, relative: &std::path::Path) -> anyhow::Result<()> {
    let mut path = dir.to_path_buf();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        path.push(part);
        let last = parts.peek().is_none();
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("--dump-scaffold: reading {}", path.display()));
            }
            Ok(_) if last => bail!(
                "--dump-scaffold: {} already exists; dump into an empty directory",
                path.display()
            ),
            Ok(meta) if meta.file_type().is_symlink() => bail!(
                "--dump-scaffold: {} is a symbolic link; dump into a directory without one",
                path.display()
            ),
            Ok(meta) if !meta.is_dir() => bail!(
                "--dump-scaffold: {} is not a directory; dump somewhere else",
                path.display()
            ),
            // On a case-insensitive disk another spelling resolves here,
            // and templates are named by the spelling on disk: written
            // under it, the overrides would override nothing.
            Ok(_) if !spelled_exactly(&path) => bail!(
                "--dump-scaffold: {} exists under another spelling; dump somewhere else",
                path.display()
            ),
            Ok(_) => {}
        }
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

    /// Whether `rustfmt` runs here: without it the output is unformatted
    /// and carries a warning, which these tests do not expect.
    fn have_rustfmt() -> bool {
        std::process::Command::new("rustfmt")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn args(file: &Path, out: &Path) -> GenerateArgs {
        GenerateArgs {
            file: Some(file.to_path_buf()),
            out: out.to_path_buf(),
            config: None,
            target: Some(GenerateTarget::Rust),
            label: None,
            format: None,
            ignore: vec![Options::IgnoreUnusedSchemas],
            apply: vec![],
            apply_options: vec![],
            allow_codegen_extensions: false,
            validate: false,
            strict: false,
            allow_partial: false,
            force: false,
            replace_source: false,
            check: false,
            template_dir: None,
            print_context: false,
            dump_rendered: None,
            dump_scaffold: None,
        }
    }

    #[test]
    fn generates_then_checks_clean_then_detects_drift() {
        if !have_rustfmt() {
            return; // the output and --check depend on it
        }
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
        if !have_rustfmt() {
            return; // the output and --check depend on it
        }
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
        if !have_rustfmt() {
            return; // the output and --check depend on it
        }
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

    /// Whether `gofmt` runs here: generating Go needs it.
    fn have_gofmt() -> bool {
        std::process::Command::new("gofmt")
            .arg("-l")
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn go_is_generated_beside_rust_and_checks_clean() {
        if !have_gofmt() || !have_rustfmt() {
            assert!(
                std::env::var_os("ROAS_CODEGEN_REQUIRE_GO").is_none(),
                "ROAS_CODEGEN_REQUIRE_GO is set but gofmt or rustfmt is missing"
            );
            return;
        }
        let dir = TempDir::new("go");
        let spec = dir.0.join("pet-store_linux.json");
        std::fs::write(&spec, PET).unwrap();
        let out = dir.0.join("out");
        let mut go = args(&spec, &out);
        go.target = Some(GenerateTarget::Go);
        run_generate(go).unwrap();
        // `_linux.go` would build only on Linux, so the stem avoids it.
        let code = std::fs::read_to_string(out.join("pet_store_linux_types.go")).unwrap();
        assert!(
            code.starts_with("// Code generated by roas. DO NOT EDIT.\n"),
            "{code}"
        );
        assert!(code.contains("package api\n"), "{code}");
        assert!(code.contains("type Pet struct {"), "{code}");
        assert!(out.join(".roas-codegen-pet-store_linux-go.json").exists());
        let mut check_args = args(&spec, &out);
        check_args.target = Some(GenerateTarget::Go);
        check_args.check = true;
        run_generate(check_args).unwrap();
        // The Rust target for the same description shares the directory.
        run_generate(args(&spec, &out)).unwrap();
        assert!(out.join("pet_store_linux.rs").exists());
    }

    #[test]
    fn templates_context_and_rendered_text_are_reachable() {
        if !have_rustfmt() {
            return;
        }
        let dir = TempDir::new("templates");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let templates = dir.0.join("tpl");
        std::fs::create_dir_all(templates.join("rust")).unwrap();
        std::fs::write(templates.join("roas-templates.toml"), "template_api = 1\n").unwrap();
        std::fs::write(
            templates.join("rust/module.rs.jinja"),
            "{% extends \"builtin/rust/module.rs.jinja\" %}{% block footer %}// custom footer\n{% endblock %}",
        )
        .unwrap();
        // --print-context writes nothing.
        let out = dir.0.join("out");
        let mut print = args(&spec, &out);
        print.print_context = true;
        run_generate(print).unwrap();
        assert!(!out.exists());
        // A suppressed type fails it, as a generation would.
        let bad = dir.0.join("bad.json");
        std::fs::write(
            &bad,
            r##"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {},
                "components": {"schemas": {"Bad": {"not": {"type": "string"}}}}}"##,
        )
        .unwrap();
        let mut print = args(&bad, &out);
        print.print_context = true;
        let error = run_generate(print).unwrap_err().to_string();
        assert!(error.contains("leaves out"), "{error}");
        assert!(!out.exists());
        // --template-dir applies, and --dump-rendered keeps the raw text.
        let mut custom = args(&spec, &out);
        custom.template_dir = Some(templates.clone());
        std::fs::create_dir_all(dir.0.join("rendered")).unwrap();
        custom.dump_rendered = Some(dir.0.join("rendered"));
        run_generate(custom).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(code.ends_with("// custom footer\n"), "{code}");
        let raw = std::fs::read_to_string(dir.0.join("rendered/api.rs")).unwrap();
        assert!(raw.contains("// custom footer"), "{raw}");
        // A dump into the output directory, or around it, is refused before
        // anything is written there.
        std::fs::write(out.join("api.rs"), "hand written").unwrap();
        std::fs::create_dir_all(out.join("raw")).unwrap();
        for dump in [out.clone(), out.join("raw"), dir.0.clone()] {
            let mut overlapping = args(&spec, &out);
            overlapping.dump_rendered = Some(dump);
            let error = run_generate(overlapping).unwrap_err().to_string();
            assert!(error.contains("overlaps the output directory"), "{error}");
        }
        // Nor through a `..` past a directory that does not exist yet.
        let mut sneaky = args(&spec, &out);
        std::fs::create_dir_all(dir.0.join("missing")).unwrap();
        sneaky.dump_rendered = Some(dir.0.join("missing/../out"));
        let error = run_generate(sneaky).unwrap_err().to_string();
        assert!(error.contains("overlaps the output directory"), "{error}");
        // Nor by spelling the output directory in another case.
        let mut cased = args(&spec, &out);
        cased.dump_rendered = Some(dir.0.join("OUT/raw"));
        let error = run_generate(cased).unwrap_err().to_string();
        // A case-sensitive disk has no `OUT`; a case-insensitive one finds
        // the output directory under that name.
        assert!(
            error.contains("overlaps the output directory") || error.contains("existing directory"),
            "{error}"
        );
        // A dump directory that does not exist is refused, never created.
        let mut fresh = args(&spec, &out);
        fresh.dump_rendered = Some(dir.0.join("nowhere"));
        let error = run_generate(fresh).unwrap_err().to_string();
        assert!(error.contains("existing directory"), "{error}");
        assert!(!dir.0.join("nowhere").exists());
        // Every refusal came before anything was written.
        assert_eq!(
            std::fs::read_to_string(out.join("api.rs")).unwrap(),
            "hand written"
        );
        // A link left in the dump directory is replaced, not followed.
        #[cfg(unix)]
        {
            let raw = dir.0.join("raw");
            std::fs::create_dir_all(&raw).unwrap();
            let sentinel = dir.0.join("sentinel.rs");
            std::fs::write(&sentinel, "keep me").unwrap();
            std::os::unix::fs::symlink(&sentinel, raw.join("api.rs")).unwrap();
            let mut linked = args(&spec, &out);
            linked.dump_rendered = Some(raw.clone());
            run_generate(linked).unwrap();
            assert!(
                !std::fs::symlink_metadata(raw.join("api.rs"))
                    .unwrap()
                    .is_symlink()
            );
            assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep me");
        }
    }

    #[test]
    fn dump_scaffold_writes_a_template_dir_that_changes_nothing() {
        let dir = TempDir::new("scaffold");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let out = dir.0.join("out");
        let scaffold = dir.0.join("tpl");
        std::fs::create_dir_all(&scaffold).unwrap();
        let mut dump = args(&spec, &out);
        dump.dump_scaffold = Some(scaffold.clone());
        run_generate(dump).unwrap();
        // Nothing is generated.
        assert!(!out.exists());
        let module = std::fs::read_to_string(scaffold.join("rust/module.rs.jinja")).unwrap();
        assert!(
            module.contains("{%- extends \"builtin/rust/module.rs.jinja\" %}")
                && module.contains("{%- block footer %}{{ super() }}{% endblock %}"),
            "{module}"
        );
        assert!(!scaffold.join("rust/mod.rs.jinja").exists());
        // Used as the template directory, it renders the defaults.
        run_generate(args(&spec, &out)).unwrap();
        let plain = std::fs::read_to_string(out.join("api.rs")).unwrap();
        let mut through = args(&spec, &out);
        through.template_dir = Some(scaffold.clone());
        run_generate(through).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("api.rs")).unwrap(), plain);
        // A second dump would overwrite it, and is refused whole.
        std::fs::write(scaffold.join("rust/module.rs.jinja"), "edited").unwrap();
        std::fs::remove_file(scaffold.join("roas-templates.toml")).unwrap();
        let mut again = args(&spec, &out);
        again.dump_scaffold = Some(scaffold.clone());
        let error = run_generate(again).unwrap_err().to_string();
        assert!(error.contains("already exists"), "{error}");
        assert!(!scaffold.join("roas-templates.toml").exists());
        assert_eq!(
            std::fs::read_to_string(scaffold.join("rust/module.rs.jinja")).unwrap(),
            "edited"
        );
        // The directory must exist and be apart from the output.
        for (target, wanted) in [
            (dir.0.join("nowhere"), "--dump-scaffold"),
            (out.clone(), "overlaps the output directory"),
        ] {
            let mut bad = args(&spec, &out);
            bad.dump_scaffold = Some(target);
            let error = run_generate(bad).unwrap_err().to_string();
            assert!(error.contains(wanted), "{error}");
        }
        assert!(!dir.0.join("nowhere").exists());
        // It describes the configured target.
        let go = dir.0.join("go");
        std::fs::create_dir_all(&go).unwrap();
        let mut dump = args(&spec, &out);
        dump.target = Some(GenerateTarget::Go);
        dump.dump_scaffold = Some(go.clone());
        run_generate(dump).unwrap();
        assert!(go.join("go/file.go.jinja").exists());
    }

    #[test]
    fn dump_scaffold_refuses_a_file_or_link_on_the_way() {
        let dir = TempDir::new("scaffold-way");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let out = dir.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        // A file where a directory goes: refused, nothing written.
        let blocked = dir.0.join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("rust"), "").unwrap();
        let mut dump = args(&spec, &out);
        dump.dump_scaffold = Some(blocked.clone());
        let error = run_generate(dump).unwrap_err().to_string();
        assert!(error.contains("is not a directory"), "{error}");
        assert!(!blocked.join("roas-templates.toml").exists());
        // A link inside the directory, here into the output: refused.
        #[cfg(unix)]
        {
            let linked = dir.0.join("linked");
            std::fs::create_dir_all(&linked).unwrap();
            std::os::unix::fs::symlink(&out, linked.join("rust")).unwrap();
            let mut dump = args(&spec, &out);
            dump.dump_scaffold = Some(linked.clone());
            let error = run_generate(dump).unwrap_err().to_string();
            assert!(error.contains("symbolic link"), "{error}");
            assert!(!linked.join("roas-templates.toml").exists());
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        }
        // A directory spelled otherwise, which a case-insensitive disk
        // resolves `rust` to: refused there.
        let cased = dir.0.join("cased");
        std::fs::create_dir_all(cased.join("RUST")).unwrap();
        // Asked before the run, which creates `rust` where it is distinct.
        let insensitive = cased.join("rust").exists();
        let mut dump = args(&spec, &out);
        dump.dump_scaffold = Some(cased.clone());
        let result = run_generate(dump);
        if insensitive {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("another spelling"), "{error}");
            assert!(!cased.join("roas-templates.toml").exists());
        } else {
            result.unwrap();
            assert!(cased.join("rust/module.rs.jinja").exists());
        }
    }

    #[test]
    fn dump_scaffold_conflicts_with_check_and_the_other_dumps() {
        #[derive(clap::Parser)]
        struct TestCli {
            #[command(flatten)]
            args: GenerateArgs,
        }
        use clap::Parser;
        assert!(
            TestCli::try_parse_from(["roas", "--out", "o", "--dump-scaffold", "d", "a.json"])
                .is_ok()
        );
        for other in [
            &["--check"][..],
            &["--print-context"],
            &["--dump-rendered", "r"],
        ] {
            let mut argv = vec!["roas", "--out", "o", "--dump-scaffold", "d", "a.json"];
            argv.extend_from_slice(other);
            assert!(TestCli::try_parse_from(argv).is_err(), "{other:?}");
        }
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
        assert_eq!(roas_codegen::go_file_name("pet-store-v2"), "pet_store_v2");
        assert_eq!(roas_codegen::go_file_name("api_test"), "api_test_types");
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
        assert_eq!(
            default_label(&InputSource::File("/dev/fd/63".into())),
            "api"
        );
        assert_eq!(
            default_label(&InputSource::File("/dev/stdin".into())),
            "api"
        );
    }

    #[test]
    fn a_bad_label_fails_before_any_work_and_names_what_was_typed() {
        let dir = TempDir::new("bad-label");
        let mut bad = args(&dir.0.join("missing.json"), &dir.0.join("out"));
        bad.label = Some("My API".into());
        let error = format!("{:#}", run_generate(bad).unwrap_err());
        assert!(error.contains("\"My API\""), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn links_after_a_dot_dot_through_a_missing_directory_are_resolved() {
        let dir = TempDir::new("dotdot-link");
        std::fs::create_dir_all(dir.0.join("real/deep")).unwrap();
        std::os::unix::fs::symlink(dir.0.join("real/deep"), dir.0.join("link")).unwrap();
        let through = resolve(&dir.0.join("missing/../link/out")).unwrap();
        let direct = resolve(&dir.0.join("real/deep/out")).unwrap();
        assert_eq!(through, direct);
    }

    #[test]
    fn a_description_named_like_its_output_directory_still_has_a_source() {
        let dir = TempDir::new("same-name");
        let spec = dir.0.join("petstore.json");
        std::fs::write(&spec, PET).unwrap();
        let out = dir.0.join("petstore");
        assert_eq!(
            source_id(&InputSource::File(spec.clone()), &out).as_deref(),
            Some(".")
        );
        run_generate(args(&spec, &out)).unwrap();
        std::fs::create_dir_all(dir.0.join("other")).unwrap();
        let other = dir.0.join("other/petstore.json");
        std::fs::write(&other, PET.replace("Pet", "Invoice")).unwrap();
        let error = format!("{:#}", run_generate(args(&other, &out)).unwrap_err());
        assert!(error.contains("--replace-source"), "{error}");
    }

    #[test]
    fn a_pipe_records_no_source() {
        assert_eq!(
            source_id(&InputSource::File("/dev/null".into()), Path::new("out")),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_description_is_named_by_its_link() {
        let dir = TempDir::new("current-link");
        std::fs::create_dir_all(dir.0.join("versions")).unwrap();
        std::fs::write(dir.0.join("versions/v1.json"), PET).unwrap();
        std::fs::write(dir.0.join("versions/v2.json"), PET).unwrap();
        let link = dir.0.join("openapi.json");
        std::os::unix::fs::symlink("versions/v1.json", &link).unwrap();
        let out = dir.0.join("gen");
        run_generate(args(&link, &out)).unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("versions/v2.json", &link).unwrap();
        run_generate(args(&link, &out)).unwrap();
        assert_eq!(
            source_id(&InputSource::File(link.clone()), &out).as_deref(),
            Some("../openapi")
        );
    }

    /// An overlay adding `age` to `Pet`, with `format` as given.
    fn age_overlay(dir: &Path, name: &str, format: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"overlay": "1.0.0", "info": {{"title": "age", "version": "1"}}, "actions": [
                    {{"target": "$.components.schemas.Pet.properties", "update": {{"age": {{"type": "integer", "format": "{format}"}}}}}}
                ]}}"#
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn overlays_apply_before_generating_checking_and_dumping() {
        if !have_rustfmt() {
            return; // the output and --check depend on it
        }
        let dir = TempDir::new("apply");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let overlay = age_overlay(&dir.0, "age.json", "int32");
        let out = dir.0.join("out");
        let dump = dir.0.join("dump");
        std::fs::create_dir_all(&dump).unwrap();
        let mut overlaid = args(&spec, &out);
        overlaid.apply = vec![overlay.clone()];
        overlaid.dump_rendered = Some(dump.clone());
        run_generate(overlaid).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(code.contains("pub age: Option<Int32>"), "{code}");
        let rendered = std::fs::read_to_string(dump.join("api.rs")).unwrap();
        assert!(rendered.contains("age"), "{rendered}");

        // `--check` compares against the overlaid description: clean with
        // the overlay, out of date without it.
        let mut check = args(&spec, &out);
        check.apply = vec![overlay.clone()];
        check.check = true;
        run_generate(check).unwrap();
        let mut plain = args(&spec, &out);
        plain.check = true;
        let error = run_generate(plain).unwrap_err().to_string();
        assert!(error.contains("out of date"), "{error}");

        // The overlay is not the source: the label still belongs to the
        // description, so dropping the overlay regenerates in place.
        run_generate(args(&spec, &out)).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(!code.contains("age"), "{code}");

        if have_gofmt() {
            let mut go = args(&spec, &out);
            go.target = Some(GenerateTarget::Go);
            go.apply = vec![overlay];
            run_generate(go).unwrap();
            let code = std::fs::read_dir(&out)
                .unwrap()
                .flatten()
                .find(|e| e.path().extension().is_some_and(|x| x == "go"))
                .map(|e| std::fs::read_to_string(e.path()).unwrap())
                .unwrap();
            assert!(code.contains("Age"), "{code}");
            assert!(code.contains("int32"), "{code}");
        } else {
            assert!(
                std::env::var_os("ROAS_CODEGEN_REQUIRE_GO").is_none(),
                "ROAS_CODEGEN_REQUIRE_GO is set but gofmt is missing"
            );
        }
    }

    #[test]
    fn several_overlays_apply_in_order() {
        if !have_rustfmt() {
            return; // the output depends on it
        }
        let dir = TempDir::new("apply-order");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let add = age_overlay(&dir.0, "add.json", "int32");
        // Matches only once `age` exists.
        let widen = dir.0.join("widen.yaml");
        std::fs::write(
            &widen,
            "overlay: 1.0.0\ninfo: {title: widen, version: '1'}\nactions:\n  - target: $.components.schemas.Pet.properties.age\n    update: {format: int64}\n",
        )
        .unwrap();
        let out = dir.0.join("out");
        let mut ordered = args(&spec, &out);
        ordered.apply = vec![add.clone(), widen.clone()];
        run_generate(ordered).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(code.contains("pub age: Option<Int64>"), "{code}");

        // The other way round, `widen` matches nothing yet.
        let mut reversed = args(&spec, &out);
        reversed.apply = vec![widen.clone(), add.clone()];
        run_generate(reversed).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(code.contains("pub age: Option<Int32>"), "{code}");

        // Which `--apply-option error-on-zero-match` refuses, naming it.
        let mut strict = args(&spec, &out);
        strict.apply = vec![widen.clone(), add];
        strict.apply_options = vec![roas_overlay::apply::ApplyOptions::ErrorOnZeroMatch];
        let error = run_generate(strict).unwrap_err().to_string();
        assert!(
            error.contains(&format!("applying overlay {}", widen.display())),
            "{error}"
        );
    }

    #[test]
    fn overlaid_yaml_descriptions_stay_yaml() {
        if !have_rustfmt() {
            return; // the output depends on it
        }
        let dir = TempDir::new("apply-yaml");
        let spec = dir.0.join("api.yaml");
        std::fs::write(
            &spec,
            "openapi: 3.2.0\ninfo: {title: t, version: '1'}\npaths: {}\ncomponents:\n  schemas:\n    Pet: {type: object, properties: {name: {type: string}}}\n",
        )
        .unwrap();
        let out = dir.0.join("out");
        let mut overlaid = args(&spec, &out);
        overlaid.apply = vec![age_overlay(&dir.0, "age.json", "int32")];
        run_generate(overlaid).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(code.contains("pub age: Option<Int32>"), "{code}");
    }

    #[test]
    fn overlay_errors_name_the_file() {
        let dir = TempDir::new("apply-bad");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let out = dir.0.join("out");

        let broken = dir.0.join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        let mut bad = args(&spec, &out);
        bad.apply = vec![broken.clone()];
        let error = run_generate(bad).unwrap_err().to_string();
        assert!(
            error.contains(&format!("reading overlay {}", broken.display())),
            "{error}"
        );

        let not_overlay = dir.0.join("not-overlay.json");
        std::fs::write(&not_overlay, r#"{"openapi": "3.2.0"}"#).unwrap();
        let mut bad = args(&spec, &out);
        bad.apply = vec![not_overlay.clone()];
        let error = run_generate(bad).unwrap_err().to_string();
        assert!(
            error.contains(&format!("parsing overlay {}", not_overlay.display())),
            "{error}"
        );

        // A description that does not parse is named before any overlay.
        std::fs::write(&spec, "{not json").unwrap();
        let mut bad = args(&spec, &out);
        bad.apply = vec![broken];
        let error = run_generate(bad).unwrap_err().to_string();
        assert!(
            error.contains(&format!("{}: cannot be read", spec.display())),
            "{error}"
        );
        assert!(!out.exists());
    }

    #[test]
    fn apply_options_need_an_overlay() {
        use clap::Parser;
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            args: GenerateArgs,
        }
        let parsed = Cli::try_parse_from([
            "roas",
            "api.json",
            "-o",
            "out",
            "--apply",
            "a.json",
            "--apply",
            "b.yaml",
            "--apply-option",
            "error-on-zero-match",
        ])
        .unwrap();
        assert_eq!(
            parsed.args.apply,
            [PathBuf::from("a.json"), PathBuf::from("b.yaml")]
        );
        assert!(
            Cli::try_parse_from([
                "roas",
                "api.json",
                "-o",
                "out",
                "--apply-option",
                "error-on-zero-match"
            ])
            .is_err()
        );
    }

    /// An overlay putting a bound on `Pet.properties.n`, as JSON or YAML.
    #[cfg(feature = "exact-numbers")]
    fn bound_overlay(dir: &Path, name: &str, bound: &str) -> PathBuf {
        let path = dir.join(name);
        let text = if name.ends_with(".json") {
            format!(
                r#"{{"overlay": "1.0.0", "info": {{"title": "b", "version": "1"}}, "actions": [
                    {{"target": "$.components.schemas.Pet.properties", "update": {{"n": {{"type": "number", "maximum": {bound}}}}}}}
                ]}}"#
            )
        } else {
            format!(
                "overlay: 1.0.0\ninfo: {{title: b, version: '1'}}\nactions:\n  - target: $.components.schemas.Pet.properties\n    update: {{n: {{type: number, maximum: {bound}}}}}\n"
            )
        };
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    #[cfg(feature = "exact-numbers")]
    fn overlays_keep_number_fidelity_honest() {
        if !have_rustfmt() {
            return; // the output depends on it
        }
        let dir = TempDir::new("apply-exact");
        let spec = dir.0.join("api.json");
        std::fs::write(&spec, PET).unwrap();
        let config = dir.0.join("roas.toml");
        std::fs::write(&config, "exact_numbers = true\n").unwrap();
        let out = dir.0.join("out");
        let bound = "0.10000000000000000001";
        // JSON throughout: exact, the bound as written.
        let mut exact = args(&spec, &out);
        exact.config = Some(config.clone());
        exact.apply = vec![bound_overlay(&dir.0, "b.json", bound)];
        run_generate(exact).unwrap();
        let code = std::fs::read_to_string(out.join("api.rs")).unwrap();
        assert!(code.contains(bound), "{code}");
        // A YAML overlay read its numbers through f64: refused, not rounded.
        let mut rounded = args(&spec, &out);
        rounded.config = Some(config);
        rounded.apply = vec![bound_overlay(&dir.0, "b.yaml", bound)];
        let error = format!("{:#}", run_generate(rounded).unwrap_err());
        assert!(error.contains("fidelity"), "{error}");
        // A YAML description with an exact JSON number overlaid is still
        // generated, without exact numbers asked for.
        let yaml = dir.0.join("api.yaml");
        std::fs::write(
            &yaml,
            "openapi: 3.2.0\ninfo: {title: t, version: '1'}\npaths: {}\ncomponents:\n  schemas:\n    Pet: {type: object, properties: {name: {type: string}}}\n",
        )
        .unwrap();
        let out = dir.0.join("out-yaml");
        let mut overlaid = args(&yaml, &out);
        overlaid.apply = vec![bound_overlay(&dir.0, "b2.json", bound)];
        run_generate(overlaid).unwrap();
    }
}
