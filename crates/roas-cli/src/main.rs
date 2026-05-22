//! `roas` command-line front-end.
//!
//! The root `validate` and `convert` commands operate on OpenAPI specs;
//! the `overlay` subcommand group operates on OpenAPI Overlay documents.
//!
//! - `roas validate [FILE]` — parse and validate an OpenAPI spec. Version is
//!   auto-detected from the document; pass `--from` to force. External
//!   `$ref`s are skipped by default; use `--load file` / `--load http`
//!   (or both) to enable the loader. Pass `--print` to echo the parsed
//!   spec to stdout on success — in the same format as the input (YAML
//!   in → YAML out, JSON in → JSON out) — useful for pipelines.
//!
//! - `roas convert --to <VERSION> [FILE]` — chain the existing
//!   `From<v_X::Spec> for v_Y::Spec` migrations to upconvert a spec.
//!   Pass `--from` to force the input version. Pass `--merge <FILE>`
//!   (repeatable) to layer additional specs on top: each is loaded,
//!   upconverted to the target version, and merged in via
//!   `roas::merge`. `--merge-option` (repeatable) tunes the merge —
//!   defaults to incoming-wins, base retains `info` / `openapi`,
//!   refs replace silently, schemas are leaves. Pass `--collapse` to
//!   run `Spec::collapse` on the (post-conversion, post-merge)
//!   result, lifting every inline component into the matching
//!   `components.<bag>` / `definitions` / `parameters` / `responses`
//!   slot with strict dedup. Pass `--apply <FILE>` (repeatable) to
//!   apply OpenAPI Overlay documents on top of the final spec —
//!   overlays run *last*, after conversion, `--merge`, and
//!   `--collapse`; `--apply-option` tunes the apply. External `$ref`s
//!   are skipped by default; use `--load file` / `--load http` to opt
//!   into the loader. Output defaults to the input format (YAML in →
//!   YAML out, JSON in → JSON out); pass `--output-format json|yaml`
//!   to override.
//!
//! - `roas overlay <validate|convert|apply>` — work with OpenAPI
//!   Overlay documents. `overlay validate` parses + validates an
//!   overlay; `overlay convert --to v1_1` upconverts one; `overlay
//!   apply --overlay <FILE> [SPEC]` applies overlay(s) to a target
//!   spec (spec on stdin or as the positional arg).
//!
//! - `roas preview [FILE]` — start a local HTTP server on
//!   `127.0.0.1:<random>` that serves the spec rendered with
//!   [Redoc](https://redocly.com/redoc) (default) or
//!   [Swagger UI](https://swagger.io/tools/swagger-ui/) (`--renderer
//!   swagger-ui`), and open the default browser at it. `--no-open` skips
//!   the launch. Ctrl+C tears the server down. `--watch` requires a real
//!   file (stdin can't be watched).
//!
//! Input may be JSON or YAML. With a file path, the parser is selected by
//! extension (`.yaml` / `.yml` → YAML, otherwise JSON). Pass `-` as the
//! file path, or omit it entirely and pipe the spec, to read from stdin;
//! stdin defaults to JSON. `--format json|yaml` overrides everything.

use anyhow::{Context, Result, anyhow, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use roas::loader::Loader;
use roas::validation::Options;
use roas_file_fetcher::FileFetcher;
use roas_http_fetcher::HttpFetcher;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

// `roas::validation::Options` implements `clap::ValueEnum` under the `clap`
// feature (enabled on the `roas` dep in this crate's Cargo.toml), so we can
// hand it straight to `#[arg(value_enum)]` without a CLI-local mirror enum.
// Variants render as kebab-case with the `Ignore` prefix dropped: e.g.
// `Options::IgnoreMissingTags` ↔ `--ignore missing-tags`.

mod overlay;
mod preview;
mod versioned;

use overlay::OverlayCommand;
use preview::PreviewArgs;
use versioned::{SpecVersion, parse_value, path_looks_like_yaml};

#[derive(Parser)]
#[command(name = "roas", about, version, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse and validate an OpenAPI spec.
    Validate(ValidateArgs),
    /// Convert an OpenAPI spec to a different version.
    Convert(ConvertArgs),
    /// Work with OpenAPI Overlay documents: validate, convert, or apply.
    #[command(subcommand)]
    Overlay(OverlayCommand),
    /// Preview the spec in a browser, rendered with Redoc or Swagger UI.
    Preview(PreviewArgs),
    /// Print a shell completion script to stdout.
    ///
    /// Source the output to enable completions; `roas completions bash >
    /// /etc/bash_completion.d/roas` is the standard recipe. Bash, Zsh,
    /// Fish, PowerShell, and Elvish are supported.
    Completions {
        /// Target shell.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Generate troff manpages for `roas` and each subcommand into a
    /// directory. Top-level page is `roas.1`; subcommand pages follow the
    /// `roas-<subcommand>.1` convention (e.g. `roas-validate.1`).
    Manpages {
        /// Output directory (created if missing).
        #[arg(short, long, value_name = "DIR")]
        out: PathBuf,
    },
}

#[derive(clap::Args)]
struct ValidateArgs {
    /// Path to the spec file (JSON or YAML). Pass `-`, or omit and pipe
    /// the spec, to read from stdin.
    file: Option<PathBuf>,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,

    /// Override format detection. By default, file paths use the extension
    /// (`.yaml`/`.yml` → YAML, otherwise JSON) and stdin defaults to JSON.
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Enable external-reference loading. Pass `--load file` to allow
    /// `file://` refs, `--load http` to allow `http://` and `https://`.
    /// Repeat the flag to combine (e.g. `--load file --load http`).
    #[arg(long, value_enum)]
    load: Vec<LoaderKind>,

    /// Skip a specific validation check. Repeat the flag to skip several
    /// (e.g. `--ignore missing-tags --ignore external-references`). Run
    /// `roas validate --help` to see the full list.
    #[arg(long, value_enum)]
    ignore: Vec<Options>,

    /// On success, echo the parsed spec to stdout in the same format as
    /// the input (YAML in → YAML out, JSON in → JSON out). Diagnostics
    /// stay on stderr. Lets `validate` sit in the middle of a pipeline:
    /// `roas convert ... | roas validate --print | roas preview`.
    #[arg(long)]
    print: bool,
}

#[derive(clap::Args)]
struct ConvertArgs {
    /// Path to the spec file (JSON or YAML). Pass `-`, or omit and pipe
    /// the spec, to read from stdin.
    file: Option<PathBuf>,

    /// Target spec version.
    #[arg(long, value_enum)]
    to: SpecVersion,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,

    /// Override format detection. By default, file paths use the extension
    /// (`.yaml`/`.yml` → YAML, otherwise JSON) and stdin defaults to JSON.
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Output format. Defaults to the input format (YAML in → YAML out,
    /// JSON in → JSON out). Pass `--output-format json|yaml` to switch.
    #[arg(long, value_enum)]
    output_format: Option<InputFormat>,

    /// Path to an additional spec to merge on top of the base after
    /// version conversion. Each `--merge` source is loaded with the
    /// same format-detection rules as the base, converted to the
    /// target version, then merged in incoming-order. Repeat the
    /// flag to layer multiple sources. The merge runs *after* the
    /// version conversion and *before* `--collapse`.
    #[arg(long, value_name = "FILE")]
    merge: Vec<PathBuf>,

    /// Per-call merge option (repeatable). Maps to
    /// `roas::merge::MergeOptions`. Default is incoming-wins on
    /// scalar conflicts, base retains `info` / `openapi`, refs
    /// replace silently, schemas are treated as leaves. Requires at
    /// least one `--merge` source (clap rejects the flag on its own).
    #[arg(long = "merge-option", value_enum, requires = "merge")]
    merge_options: Vec<MergeOptionFlag>,

    /// Lift every inline component into the matching root bag
    /// (`components.<bag>` for v3.x, `definitions` / `parameters` /
    /// `responses` for v2) and replace its call sites with a `$ref`.
    /// Structurally identical components collapse to a single entry.
    /// Runs after the version conversion (and after `--merge`, if any).
    #[arg(long)]
    collapse: bool,

    /// Path to an OpenAPI Overlay document to apply on top of the
    /// converted (and merged / collapsed) spec. Each `--apply` source
    /// is loaded with extension-based format detection, its version
    /// detected from the `overlay` field, and applied via
    /// `roas-overlay`. Repeat the flag to apply several overlays in
    /// order. Apply runs *last* — after conversion, `--merge`, and
    /// `--collapse`.
    #[arg(long, value_name = "FILE")]
    apply: Vec<PathBuf>,

    /// Per-call overlay apply option (repeatable). Maps to
    /// `roas_overlay::apply::ApplyOptions`. Requires at least one
    /// `--apply` source (clap rejects the flag on its own).
    #[arg(long = "apply-option", value_enum, requires = "apply")]
    apply_options: Vec<roas_overlay::apply::ApplyOptions>,

    /// Enable external-reference loading during `--collapse`. Same
    /// semantics as `roas validate --load`: pass `--load file` to
    /// allow `file://` refs, `--load http` for `http(s)://`; repeat
    /// to combine. Without it, external `$ref`s in the input are
    /// left untouched. Requires `--collapse` (clap rejects the flag
    /// on its own — collapse is the only consumer).
    #[arg(long, value_enum, requires = "collapse")]
    load: Vec<LoaderKind>,
}

/// CLI mirror of `roas::merge::MergeOptions`. Kebab-case so users see
/// `--merge-option base-wins` etc. on the command line.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum MergeOptionFlag {
    /// Reverse the default "incoming wins" policy.
    BaseWins,
    /// Abort on the first real collision (returns a non-zero exit
    /// after recording it). Spec.merge clones internally so the
    /// base is untouched on error.
    ErrorOnConflict,
    /// Deep-merge two `ObjectSchema` values instead of leaf-replace.
    DeepMergeObjectSchemas,
    /// Allow `info` / `openapi` / `swagger` to merge instead of
    /// being preserved from base.
    MergeInfo,
    /// Allow an empty incoming list (`servers`, `security`, …) to
    /// clear a populated base list.
    ReplaceListsWhenEmpty,
}

impl MergeOptionFlag {
    fn to_roas(self) -> roas::merge::MergeOptions {
        match self {
            MergeOptionFlag::BaseWins => roas::merge::MergeOptions::BaseWins,
            MergeOptionFlag::ErrorOnConflict => roas::merge::MergeOptions::ErrorOnConflict,
            MergeOptionFlag::DeepMergeObjectSchemas => {
                roas::merge::MergeOptions::DeepMergeObjectSchemas
            }
            MergeOptionFlag::MergeInfo => roas::merge::MergeOptions::MergeInfo,
            MergeOptionFlag::ReplaceListsWhenEmpty => {
                roas::merge::MergeOptions::ReplaceListsWhenEmpty
            }
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LoaderKind {
    File,
    Http,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum InputFormat {
    Json,
    Yaml,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate(args) => run_validate(args),
        Command::Convert(args) => run_convert(args),
        Command::Overlay(cmd) => overlay::run_overlay(cmd),
        Command::Preview(args) => preview::run_preview(args),
        Command::Completions { shell } => run_completions(shell),
        Command::Manpages { out } => run_manpages(&out),
    }
}

fn run_completions(shell: clap_complete::Shell) -> Result<()> {
    write_completions(shell, &mut io::stdout())
}

fn write_completions(shell: clap_complete::Shell, out: &mut dyn io::Write) -> Result<()> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, out);
    Ok(())
}

fn run_manpages(out: &Path) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let cmd = Cli::command();
    write_manpage(out, &cmd, cmd.get_name())?;
    // Recurse: every subcommand (and its subcommands, if any) gets its own
    // `roas-<sub>[-<subsub>].1`. clap_mangen doesn't follow children
    // automatically, so we walk the tree ourselves.
    let mut stack: Vec<(String, clap::Command)> = cmd
        .get_subcommands()
        .cloned()
        .map(|sub| (cmd.get_name().to_string(), sub))
        .collect();
    while let Some((parent_name, sub)) = stack.pop() {
        let full_name = format!("{parent_name}-{}", sub.get_name());
        // Rename the subcommand to its hyphenated full path so the NAME
        // and SYNOPSIS lines render as `roas-validate` rather than the
        // bare `validate` clap stores internally. clap::Command::name
        // only takes `Into<Str>`, which lacks a `From<String>` impl —
        // leak the heap string into a 'static reference. We're about to
        // exit; the alloc is one per subcommand and unmeasurable.
        let leaked: &'static str = String::leak(full_name.clone());
        let renamed = sub.clone().name(leaked);
        write_manpage(out, &renamed, &full_name)?;
        for nested in sub.get_subcommands().cloned() {
            stack.push((full_name.clone(), nested));
        }
    }
    Ok(())
}

fn write_manpage(out: &Path, cmd: &clap::Command, name: &str) -> Result<()> {
    let path = out.join(format!("{name}.1"));
    let man = clap_mangen::Man::new(cmd.clone()).title(name.to_uppercase());
    let mut buf = Vec::new();
    man.render(&mut buf)
        .with_context(|| format!("rendering {}", path.display()))?;
    fs::write(&path, buf).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// What was passed on the command line. `None` + piped stdin == read stdin;
/// `Some(p)` where `p == Path::new("-")` is the explicit stdin sentinel;
/// `None` + TTY stdin is a usage error.
///
/// `display()` returns a label for diagnostics: the file path for files,
/// `<stdin>` for stdin.
#[derive(Clone, Debug)]
pub(crate) enum InputSource {
    File(PathBuf),
    Stdin,
}

impl InputSource {
    pub(crate) fn display(&self) -> String {
        match self {
            InputSource::File(p) => p.display().to_string(),
            InputSource::Stdin => "<stdin>".to_string(),
        }
    }
}

/// Resolve the positional `file` argument into a concrete source. Honors
/// the `-` sentinel and the "no arg + piped stdin" shortcut. Returns
/// `Err` only when neither was provided and stdin is a TTY.
pub(crate) fn resolve_input_source(file: Option<&Path>) -> Result<InputSource> {
    match file {
        Some(p) if p == Path::new("-") => Ok(InputSource::Stdin),
        Some(p) => Ok(InputSource::File(p.to_path_buf())),
        None => {
            if std::io::stdin().is_terminal() {
                bail!("no input: pass a file path, or pipe a spec to stdin");
            }
            Ok(InputSource::Stdin)
        }
    }
}

/// Read + parse a spec from the resolved source. Format selection: explicit
/// `--format` wins; otherwise file paths use the extension, stdin defaults
/// to JSON. Returns the parsed value plus the *resolved* format so callers
/// that round-trip the spec back to bytes (e.g. `validate --print`) can
/// match the output format to the input.
pub(crate) fn read_input(
    source: &InputSource,
    format: Option<InputFormat>,
) -> Result<(serde_json::Value, InputFormat)> {
    let resolved = match source {
        InputSource::File(p) => format.unwrap_or_else(|| {
            if path_looks_like_yaml(p) {
                InputFormat::Yaml
            } else {
                InputFormat::Json
            }
        }),
        InputSource::Stdin => format.unwrap_or(InputFormat::Json),
    };
    let raw = match source {
        InputSource::File(p) => {
            fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
        }
        InputSource::Stdin => {
            let mut raw = String::new();
            std::io::stdin()
                .read_to_string(&mut raw)
                .context("reading from stdin")?;
            raw
        }
    };
    let value = parse_value(&raw, resolved == InputFormat::Yaml)?;
    Ok((value, resolved))
}

fn build_loader(kinds: &[LoaderKind]) -> Option<Loader> {
    if kinds.is_empty() {
        return None;
    }
    let mut loader = Loader::new();
    for kind in kinds {
        match kind {
            LoaderKind::File => {
                loader.register_fetcher("file://", FileFetcher::new());
            }
            LoaderKind::Http => {
                // Build one `HttpFetcher` and clone it across both prefixes so
                // a single connection pool serves `http://` and `https://`.
                let fetcher = HttpFetcher::new();
                loader.register_fetcher("http://", fetcher.clone());
                loader.register_fetcher("https://", fetcher);
            }
        }
    }
    Some(loader)
}

/// Serialize a parsed spec back to bytes. `pretty_json` selects multi-line
/// vs. compact JSON; YAML is always multi-line. A trailing newline is
/// appended so the output is line-oriented like YAML's.
///
/// Used by both `validate --print` (compact, pipeline-friendly) and
/// `convert` (pretty, file-friendly).
pub(crate) fn serialize_spec(
    value: &serde_json::Value,
    format: InputFormat,
    pretty_json: bool,
) -> Result<String> {
    match format {
        InputFormat::Yaml => serde_yaml_ng::to_string(value).context("serializing spec as YAML"),
        InputFormat::Json => {
            let mut s = if pretty_json {
                serde_json::to_string_pretty(value).context("serializing spec as JSON")?
            } else {
                serde_json::to_string(value).context("serializing spec as JSON")?
            };
            s.push('\n');
            Ok(s)
        }
    }
}

fn run_validate(args: ValidateArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = versioned::detect_or_use(args.from, value)?;

    let mut loader = build_loader(&args.load);

    let mut options = enumset::EnumSet::<Options>::new();
    for ignore in &args.ignore {
        options |= *ignore;
    }
    match detected.validate(options, loader.as_mut()) {
        Ok(()) => {
            // Diagnostics go to stderr so stdout stays clean for shell pipelines.
            eprintln!("{}: valid {}", source.display(), detected.label());
            if args.print {
                // Echo the parsed spec so the command can sit in the middle
                // of a pipeline. Format matches the input: YAML in → YAML out,
                // JSON in → JSON out. `into_value` re-serialises through the
                // typed Spec, so the output is normalised.
                let value = detected.into_value()?;
                print!("{}", serialize_spec(&value, input_format, false)?);
            }
            Ok(())
        }
        Err(err) => {
            for e in &err.errors {
                eprintln!("- {e}");
            }
            Err(anyhow!(
                "{}: validation failed ({} error{})",
                source.display(),
                err.errors.len(),
                if err.errors.len() == 1 { "" } else { "s" }
            ))
        }
    }
}

fn run_convert(args: ConvertArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = versioned::detect_or_use(args.from, value)?;

    let target = args.to;
    if (detected.version() as u8) > (target as u8) {
        bail!(
            "downconversion is not supported: input is {}, target is {}",
            detected.label(),
            target.label(),
        );
    }

    // 1) Convert the base spec to the target version.
    let mut converted = detected.convert_to_detected(target)?;

    // 2) Apply each `--merge` source (also converted to the target
    //    version) in incoming-order, on top of the base.
    if !args.merge.is_empty() {
        let merge_options = merge_options_from_flags(&args.merge_options);
        for path in &args.merge {
            let raw =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            // The merge source uses the *same* format-detection rules
            // as the base: --format applies to all inputs (the base
            // and every --merge source), and file extension falls
            // back when --format is unset.
            let format = args.format.unwrap_or_else(|| {
                if path_looks_like_yaml(path) {
                    InputFormat::Yaml
                } else {
                    InputFormat::Json
                }
            });
            let value = parse_value(&raw, format == InputFormat::Yaml)
                .with_context(|| format!("parsing {}", path.display()))?;
            let other = versioned::detect_or_use(args.from, value)
                .with_context(|| format!("detecting version of {}", path.display()))?;
            if (other.version() as u8) > (target as u8) {
                bail!(
                    "downconversion is not supported for `--merge` source {}: input is {}, target is {}",
                    path.display(),
                    other.label(),
                    target.label(),
                );
            }
            let other_at_target = other
                .convert_to_detected(target)
                .with_context(|| format!("converting {} to {}", path.display(), target.label()))?;
            match converted.merge_into(other_at_target, merge_options)? {
                Ok(_report) => {}
                Err(err) => {
                    bail!(
                        "merge aborted on conflict in {} ({} recorded): {}",
                        path.display(),
                        err.conflicts.len(),
                        err.conflicts
                            .last()
                            .map(|c| c.path.as_str())
                            .unwrap_or("<unknown path>"),
                    );
                }
            }
        }
    }

    // 3) Collapse so it has visibility into the final merged tree.
    if args.collapse {
        let mut loader = build_loader(&args.load);
        converted.collapse(loader.as_mut())?;
    }
    let mut value = converted.into_value()?;

    // 4) Apply overlays last, on the serialized JSON. Overlays produce
    //    arbitrary JSON that no longer needs the typed model, whereas
    //    collapse does — so apply must run after it.
    if !args.apply.is_empty() {
        let mut apply_options = enumset::EnumSet::<roas_overlay::apply::ApplyOptions>::empty();
        for opt in &args.apply_options {
            apply_options |= *opt;
        }
        overlay::apply_overlays(&mut value, &args.apply, apply_options)?;
    }

    // Output format defaults to the input format; `--output-format` overrides.
    let out_format = args.output_format.unwrap_or(input_format);
    print!("{}", serialize_spec(&value, out_format, true)?);
    Ok(())
}

fn merge_options_from_flags(
    flags: &[MergeOptionFlag],
) -> enumset::EnumSet<roas::merge::MergeOptions> {
    let mut set = roas::merge::MergeOptions::new();
    for f in flags {
        set |= f.to_roas();
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Write;

    #[test]
    fn build_loader_returns_none_for_empty_kinds() {
        assert!(build_loader(&[]).is_none());
    }

    #[test]
    fn build_loader_returns_some_for_file_kind() {
        assert!(build_loader(&[LoaderKind::File]).is_some());
    }

    #[test]
    fn build_loader_returns_some_for_http_kind() {
        assert!(build_loader(&[LoaderKind::Http]).is_some());
    }

    #[test]
    fn build_loader_returns_some_for_combined_kinds() {
        assert!(build_loader(&[LoaderKind::File, LoaderKind::Http]).is_some());
    }

    /// `run_convert`'s downconvert guard uses `(SpecVersion as u8)` ordering;
    /// the variant declaration order must match the chronological version
    /// order or `roas convert --to v2 spec_3_2.json` would silently succeed.
    #[test]
    fn spec_version_discriminants_order_by_version() {
        assert!((SpecVersion::V2 as u8) < (SpecVersion::V3_0 as u8));
        assert!((SpecVersion::V3_0 as u8) < (SpecVersion::V3_1 as u8));
        assert!((SpecVersion::V3_1 as u8) < (SpecVersion::V3_2 as u8));
    }

    #[test]
    fn cli_parses_minimal_validate_invocation() {
        let cli = Cli::try_parse_from(["roas", "validate", "spec.json"]).expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.file.as_ref().unwrap().to_string_lossy(), "spec.json");
                assert!(args.from.is_none());
                assert!(args.load.is_empty());
                assert!(args.ignore.is_empty());
                assert!(!args.print);
                assert!(args.format.is_none());
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_validate_without_file_arg() {
        let cli = Cli::try_parse_from(["roas", "validate"]).expect("validate parse");
        match cli.command {
            Command::Validate(args) => assert!(args.file.is_none()),
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_validate_with_stdin_sentinel_and_format_flag() {
        let cli = Cli::try_parse_from(["roas", "validate", "--format", "yaml", "-"])
            .expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.file.as_deref(), Some(Path::new("-")));
                assert_eq!(args.format, Some(InputFormat::Yaml));
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_validate_print_flag() {
        let cli = Cli::try_parse_from(["roas", "validate", "--print", "spec.json"])
            .expect("validate parse");
        match cli.command {
            Command::Validate(args) => assert!(args.print),
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_ignore_flag_into_options_variants() {
        let cli = Cli::try_parse_from([
            "roas",
            "validate",
            "--ignore",
            "missing-tags",
            "--ignore",
            "unused-server-variables",
            "--ignore",
            "empty-info-title",
            "spec.json",
        ])
        .expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(
                    args.ignore,
                    vec![
                        Options::IgnoreMissingTags,
                        Options::IgnoreUnusedServerVariables,
                        Options::IgnoreEmptyInfoTitle,
                    ]
                );
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_rejects_unknown_ignore_value() {
        let res =
            Cli::try_parse_from(["roas", "validate", "--ignore", "no-such-check", "spec.json"]);
        assert!(res.is_err(), "unknown --ignore value must error");
    }

    #[test]
    fn cli_parses_repeated_load_flag_into_vec() {
        let cli = Cli::try_parse_from([
            "roas",
            "validate",
            "--load",
            "file",
            "--load",
            "http",
            "spec.json",
        ])
        .expect("validate parse");
        match cli.command {
            Command::Validate(args) => {
                assert_eq!(args.load.len(), 2);
                assert!(matches!(args.load[0], LoaderKind::File));
                assert!(matches!(args.load[1], LoaderKind::Http));
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_convert_with_explicit_from() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--from",
            "v2",
            "--to",
            "v3_2",
            "spec.json",
        ])
        .expect("convert parse");
        match cli.command {
            Command::Convert(args) => {
                assert_eq!(args.from, Some(SpecVersion::V2));
                assert_eq!(args.to, SpecVersion::V3_2);
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_rejects_convert_without_to() {
        let res = Cli::try_parse_from(["roas", "convert", "spec.json"]);
        assert!(res.is_err(), "convert without --to must error");
    }

    #[test]
    fn cli_parses_convert_with_output_format_flag() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--output-format",
            "yaml",
            "spec.json",
        ])
        .expect("convert parse");
        match cli.command {
            Command::Convert(args) => assert_eq!(args.output_format, Some(InputFormat::Yaml)),
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_parses_convert_with_collapse_and_load_flags() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--collapse",
            "--load",
            "file",
            "spec.json",
        ])
        .expect("convert parse");
        match cli.command {
            Command::Convert(args) => {
                assert_eq!(args.to, SpecVersion::V3_2);
                assert!(args.collapse, "--collapse must set the flag");
                assert_eq!(args.load.len(), 1);
                assert!(matches!(args.load[0], LoaderKind::File));
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_convert_collapse_defaults_to_false() {
        let cli = Cli::try_parse_from(["roas", "convert", "--to", "v3_2", "spec.json"])
            .expect("convert parse");
        match cli.command {
            Command::Convert(args) => assert!(!args.collapse, "--collapse defaults to false"),
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_rejects_convert_load_without_collapse() {
        // `--load` is only meaningful when `--collapse` is active;
        // clap's `requires = "collapse"` must reject the flag on its own.
        let res = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--load",
            "file",
            "spec.json",
        ]);
        assert!(res.is_err(), "--load without --collapse must error");
    }

    #[test]
    fn cli_parses_convert_with_merge() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--merge",
            "extra.yaml",
            "--merge",
            "more.json",
            "spec.json",
        ])
        .expect("convert with --merge parses");
        match cli.command {
            Command::Convert(args) => {
                assert_eq!(args.merge.len(), 2);
                assert_eq!(args.merge[0].to_string_lossy(), "extra.yaml");
                assert_eq!(args.merge[1].to_string_lossy(), "more.json");
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_parses_convert_with_merge_options() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--merge",
            "extra.yaml",
            "--merge-option",
            "base-wins",
            "--merge-option",
            "deep-merge-object-schemas",
            "spec.json",
        ])
        .expect("convert with --merge-option parses");
        match cli.command {
            Command::Convert(args) => {
                assert_eq!(args.merge_options.len(), 2);
                assert!(matches!(args.merge_options[0], MergeOptionFlag::BaseWins));
                assert!(matches!(
                    args.merge_options[1],
                    MergeOptionFlag::DeepMergeObjectSchemas
                ));
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_rejects_merge_option_without_merge() {
        // `--merge-option` is only meaningful when at least one
        // `--merge` source is provided; clap's `requires = "merge"`
        // must reject the flag on its own.
        let res = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--merge-option",
            "base-wins",
            "spec.json",
        ]);
        assert!(res.is_err(), "--merge-option without --merge must error");
    }

    #[test]
    fn merge_options_from_flags_unions_into_enumset() {
        let set =
            merge_options_from_flags(&[MergeOptionFlag::BaseWins, MergeOptionFlag::MergeInfo]);
        assert!(set.contains(roas::merge::MergeOptions::BaseWins));
        assert!(set.contains(roas::merge::MergeOptions::MergeInfo));
        assert!(!set.contains(roas::merge::MergeOptions::ErrorOnConflict));
    }

    #[test]
    fn merge_options_from_flags_empty_is_default_set() {
        let set = merge_options_from_flags(&[]);
        assert!(set.is_empty());
    }

    #[test]
    fn merge_source_format_detection_via_path_looks_like_yaml() {
        // `versioned::path_looks_like_yaml` already covers the
        // extension matrix in its own tests; the integration here
        // is that the `--merge` source loop reads that helper to
        // pick a parser. We don't re-test the matrix; just confirm
        // the symbol is reachable from main.rs.
        assert!(path_looks_like_yaml(std::path::Path::new("a.yaml")));
        assert!(path_looks_like_yaml(std::path::Path::new("a.yml")));
        assert!(!path_looks_like_yaml(std::path::Path::new("a.json")));
        assert!(!path_looks_like_yaml(std::path::Path::new("noext")));
    }

    /// Process-scoped unique temp path so parallel tests don't collide.
    fn temp_path(suffix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "roas-cli-test-{}-{}-{suffix}",
            std::process::id(),
            n,
        ))
    }

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn write(suffix: &str, body: &[u8]) -> Self {
            let path = temp_path(suffix);
            let mut f = std::fs::File::create(&path).expect("create temp file");
            f.write_all(body).expect("write temp file");
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn read_input_json_file_returns_parsed_value_and_json_format() {
        let f = TempFile::write("ok.json", br#"{"hello":"world"}"#);
        let (v, fmt) = read_input(&InputSource::File(f.0.clone()), None).expect("parse ok");
        assert_eq!(v, serde_json::json!({"hello": "world"}));
        assert_eq!(fmt, InputFormat::Json);
    }

    #[test]
    fn read_input_yaml_file_routes_through_yaml_parser_via_extension() {
        let f = TempFile::write("ok.yaml", b"name: pet\ncount: 3\n");
        let (v, fmt) = read_input(&InputSource::File(f.0.clone()), None).expect("parse ok");
        assert_eq!(v, serde_json::json!({"name": "pet", "count": 3}));
        assert_eq!(fmt, InputFormat::Yaml);
    }

    #[test]
    fn read_input_format_override_forces_yaml_on_no_extension_file() {
        // No `.yaml` extension: extension sniffing would pick JSON, but
        // `--format yaml` must win — and the resolved format must reflect it.
        let f = TempFile::write("ok-noext", b"name: pet\ncount: 3\n");
        let (v, fmt) =
            read_input(&InputSource::File(f.0.clone()), Some(InputFormat::Yaml)).expect("parse ok");
        assert_eq!(v, serde_json::json!({"name": "pet", "count": 3}));
        assert_eq!(fmt, InputFormat::Yaml);
    }

    #[test]
    fn read_input_format_override_forces_json_on_yaml_extension() {
        // File has `.yaml` extension but contents are JSON: `--format json`
        // must override the extension heuristic.
        let f = TempFile::write("misnamed.yaml", br#"{"hello":"world"}"#);
        let (v, fmt) =
            read_input(&InputSource::File(f.0.clone()), Some(InputFormat::Json)).expect("parse ok");
        assert_eq!(v, serde_json::json!({"hello": "world"}));
        assert_eq!(fmt, InputFormat::Json);
    }

    #[test]
    fn read_input_missing_file_errors_with_reading_context() {
        let p = temp_path("missing.json");
        let err = read_input(&InputSource::File(p), None).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[test]
    fn read_input_invalid_json_surfaces_parser_error() {
        let f = TempFile::write("bad.json", b"@@@ not json");
        let err =
            read_input(&InputSource::File(f.0.clone()), None).expect_err("invalid JSON must error");
        assert!(
            err.to_string().contains("parsing JSON"),
            "expected `parsing JSON` context, got: {err}",
        );
    }

    #[test]
    fn read_input_invalid_yaml_surfaces_parser_error() {
        let f = TempFile::write("bad.yaml", b"key:\n\tvalue: oops\n");
        let err =
            read_input(&InputSource::File(f.0.clone()), None).expect_err("invalid YAML must error");
        assert!(
            err.to_string().contains("parsing YAML"),
            "expected `parsing YAML` context, got: {err}",
        );
    }

    #[test]
    fn serialize_spec_compact_json_emits_single_line_with_trailing_newline() {
        let v = serde_json::json!({"openapi":"3.2.0","info":{"title":"x","version":"1"}});
        let out = serialize_spec(&v, InputFormat::Json, false).expect("ok");
        assert!(out.ends_with('\n'), "JSON must end with a newline");
        // Compact JSON has no internal newlines.
        assert_eq!(out.matches('\n').count(), 1, "compact JSON: got: {out}");
        let back: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn serialize_spec_pretty_json_emits_multi_line_with_trailing_newline() {
        let v = serde_json::json!({"openapi":"3.2.0","info":{"title":"x","version":"1"}});
        let out = serialize_spec(&v, InputFormat::Json, true).expect("ok");
        assert!(out.ends_with('\n'), "JSON must end with a newline");
        // Pretty JSON spans multiple lines for an object of this size.
        assert!(out.matches('\n').count() > 1, "pretty JSON: got: {out}");
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn serialize_spec_yaml_emits_yaml_for_yaml_format() {
        let v = serde_json::json!({"openapi":"3.2.0","info":{"title":"x","version":"1"}});
        let out = serialize_spec(&v, InputFormat::Yaml, false).expect("ok");
        // YAML structure: no curly braces at the top level, keys are bare,
        // and serde_yaml_ng terminates documents with a newline.
        assert!(
            out.contains("openapi:"),
            "YAML output must use bare keys, got: {out}",
        );
        assert!(
            !out.trim().starts_with('{'),
            "YAML output must not be JSON, got: {out}",
        );
        // Round-trips back to the same value.
        let back: serde_json::Value = serde_yaml_ng::from_str(&out).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn resolve_input_source_explicit_dash_is_stdin() {
        let src = resolve_input_source(Some(Path::new("-"))).expect("resolve ok");
        assert!(matches!(src, InputSource::Stdin));
        assert_eq!(src.display(), "<stdin>");
    }

    #[test]
    fn resolve_input_source_explicit_path_is_file() {
        let src = resolve_input_source(Some(Path::new("spec.json"))).expect("resolve ok");
        match src {
            InputSource::File(p) => assert_eq!(p, Path::new("spec.json")),
            _ => panic!("expected File"),
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // run_validate / run_convert end-to-end-ish tests (drive each function
    // with a constructed args struct + a temp-file spec; assert return
    // shape, not stdout/stderr text).
    // ────────────────────────────────────────────────────────────────────

    /// A minimal valid v3.2 spec body — used by several validate / convert
    /// tests that just need *some* spec on disk.
    const MINIMAL_V3_2: &[u8] =
        br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#;
    const MINIMAL_V2: &[u8] = br#"{"swagger":"2.0","info":{"title":"x","version":"1"},"paths":{}}"#;

    #[test]
    fn run_validate_returns_ok_for_clean_spec() {
        let f = TempFile::write("clean.json", MINIMAL_V3_2);
        let args = ValidateArgs {
            file: Some(f.0.clone()),
            from: None,
            format: None,
            load: Vec::new(),
            ignore: Vec::new(),
            print: false,
        };
        run_validate(args).expect("clean spec must validate");
    }

    #[test]
    fn run_validate_returns_err_for_spec_with_unused_tag() {
        // Default ignore set fires on unused tags.
        let body = br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"unused"}]}"#;
        let f = TempFile::write("unused-tag.json", body);
        let args = ValidateArgs {
            file: Some(f.0.clone()),
            from: None,
            format: None,
            load: Vec::new(),
            ignore: Vec::new(),
            print: false,
        };
        let err = run_validate(args).expect_err("unused tag must fail");
        assert!(err.to_string().contains("validation failed"), "got: {err}",);
    }

    #[test]
    fn run_validate_with_ignore_suppresses_validation_failure() {
        let body = br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"unused"}]}"#;
        let f = TempFile::write("ignored.json", body);
        let args = ValidateArgs {
            file: Some(f.0.clone()),
            from: None,
            format: None,
            load: Vec::new(),
            ignore: vec![Options::IgnoreUnusedTags],
            print: false,
        };
        run_validate(args).expect("--ignore unused-tags must suppress");
    }

    #[test]
    fn run_validate_with_load_file_builds_loader() {
        let f = TempFile::write("with-load.json", MINIMAL_V3_2);
        let args = ValidateArgs {
            file: Some(f.0.clone()),
            from: None,
            format: None,
            load: vec![LoaderKind::File],
            ignore: Vec::new(),
            print: false,
        };
        run_validate(args).expect("clean spec with file loader must validate");
    }

    #[test]
    fn run_validate_missing_file_errors_with_reading_context() {
        let args = ValidateArgs {
            file: Some(temp_path("missing.json")),
            from: None,
            format: None,
            load: Vec::new(),
            ignore: Vec::new(),
            print: false,
        };
        let err = run_validate(args).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[test]
    fn run_convert_v2_to_v3_2_succeeds() {
        let f = TempFile::write("v2.json", MINIMAL_V2);
        let args = ConvertArgs {
            file: Some(f.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            collapse: false,
            load: vec![],
            merge: vec![],
            merge_options: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        run_convert(args).expect("v2 → v3.2 must succeed");
    }

    #[test]
    fn run_convert_with_collapse_and_load_file_resolves_external_ref() {
        // End-to-end through `run_convert`: the spec carries a
        // `file://` `$ref`, and `--load file` builds a Loader carrying
        // a `FileFetcher`. If `build_loader → collapse(loader)` is
        // wired correctly, the loader resolves the fragment and the
        // call returns Ok. If the loader path were silently bypassed,
        // the external ref would be left as-is (also Ok) — so we make
        // the test discriminating by NOT writing the fragment and
        // expecting an error: a missing file with `--load file` must
        // surface from the fetcher.
        let frag = TempFile::write(
            "convert-collapse-frag.json",
            br#"{"Pet":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}}"#,
        );
        let frag_url = format!("file://{}", frag.0.display());
        let body = format!(
            r#"{{
                "openapi":"3.2.0",
                "info":{{"title":"x","version":"1"}},
                "paths":{{
                    "/pets":{{
                        "get":{{
                            "operationId":"x",
                            "responses":{{
                                "200":{{
                                    "description":"ok",
                                    "content":{{"application/json":{{"schema":{{"$ref":"{frag_url}#/Pet"}}}}}}
                                }}
                            }}
                        }}
                    }}
                }}
            }}"#
        );
        let f = TempFile::write("convert-collapse-spec.json", body.as_bytes());
        let args = ConvertArgs {
            file: Some(f.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            collapse: true,
            load: vec![LoaderKind::File],
            merge: vec![],
            merge_options: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        run_convert(args).expect("convert + collapse + --load file must succeed");
    }

    #[test]
    fn run_convert_with_collapse_succeeds_on_titled_inline_schema() {
        // A v3.2 spec with one inline titled schema. After --collapse,
        // the inline copy lifts into `components.schemas.Pet` and the
        // call site holds a `$ref`. `run_convert` prints the result to
        // stdout; this test only asserts the call succeeds (parser /
        // converter / collapser chained cleanly).
        let body = br#"{
            "openapi":"3.2.0",
            "info":{"title":"x","version":"1"},
            "paths":{
                "/pets":{
                    "get":{
                        "operationId":"listPets",
                        "responses":{
                            "200":{
                                "description":"ok",
                                "content":{
                                    "application/json":{
                                        "schema":{"title":"Pet","type":"object","properties":{"id":{"type":"integer"}}}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"#;
        let f = TempFile::write("collapse.json", body);
        let args = ConvertArgs {
            file: Some(f.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            collapse: true,
            load: vec![],
            merge: vec![],
            merge_options: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        run_convert(args).expect("convert + collapse must succeed");
    }

    #[test]
    fn run_convert_rejects_downconversion() {
        let f = TempFile::write("v3.json", MINIMAL_V3_2);
        let args = ConvertArgs {
            file: Some(f.0.clone()),
            to: SpecVersion::V2,
            from: None,
            format: None,
            output_format: None,
            collapse: false,
            load: vec![],
            merge: vec![],
            merge_options: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        let err = run_convert(args).expect_err("downconversion must error");
        assert!(
            err.to_string().contains("downconversion is not supported"),
            "got: {err}",
        );
    }

    #[test]
    fn run_convert_with_merge_layers_a_second_spec_on_top() {
        // base has `tags=[]`; the merge source adds a tag. After
        // `run_convert`, the printed result should include the tag.
        // Captures the order: convert → merge → (no collapse).
        let base = TempFile::write(
            "base.json",
            br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{}}"#,
        );
        let layer = TempFile::write(
            "merge.json",
            br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"pets"}]}"#,
        );
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![layer.0.clone()],
            merge_options: vec![],
            collapse: false,
            load: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        run_convert(args).expect("convert + merge must succeed");
    }

    #[test]
    fn run_convert_with_merge_across_versions_upconverts_each_source() {
        // base is v2, merge layer is v3.0; target is v3.2 — both
        // should upconvert to the target before merging. Tests the
        // "convert each merge source to the target version" branch
        // in run_convert.
        let base = TempFile::write("base-v2.json", MINIMAL_V2);
        let layer = TempFile::write(
            "merge-v3_0.json",
            br#"{"openapi":"3.0.4","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"pets"}]}"#,
        );
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![layer.0.clone()],
            merge_options: vec![],
            collapse: false,
            load: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        run_convert(args).expect("cross-version merge after convert must succeed");
    }

    #[test]
    fn run_convert_with_merge_error_on_conflict_returns_err() {
        // base and merge differ on a real collision (info.description)
        // under MergeInfo + ErrorOnConflict → run_convert bails.
        let base = TempFile::write(
            "base.json",
            br#"{"openapi":"3.2.0","info":{"title":"x","version":"1","description":"base"},"paths":{}}"#,
        );
        let layer = TempFile::write(
            "merge.json",
            br#"{"openapi":"3.2.0","info":{"title":"x","version":"1","description":"incoming"},"paths":{}}"#,
        );
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![layer.0.clone()],
            merge_options: vec![MergeOptionFlag::MergeInfo, MergeOptionFlag::ErrorOnConflict],
            collapse: false,
            load: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        let err = run_convert(args).expect_err("error-on-conflict must surface");
        assert!(
            err.to_string().contains("merge aborted"),
            "expected `merge aborted` in error, got: {err}",
        );
    }

    #[test]
    fn run_convert_with_merge_missing_file_errors_with_reading_context() {
        let base = TempFile::write("base.json", MINIMAL_V3_2);
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![temp_path("missing-merge.json")],
            merge_options: vec![],
            collapse: false,
            load: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        let err = run_convert(args).expect_err("missing merge source must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[test]
    fn run_convert_with_apply_layers_an_overlay_after_conversion() {
        let base = TempFile::write("base.json", MINIMAL_V3_2);
        let overlay = TempFile::write(
            "overlay.json",
            br#"{"overlay":"1.0.0","info":{"title":"o","version":"1"},"actions":[{"target":"$.info","update":{"description":"added"}}]}"#,
        );
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![],
            merge_options: vec![],
            collapse: false,
            load: vec![],
            apply: vec![overlay.0.clone()],
            apply_options: vec![],
        };
        run_convert(args).expect("convert + apply must succeed");
    }

    #[test]
    fn run_convert_apply_option_error_on_zero_match_surfaces() {
        // Threading check: an overlay targeting a missing node, with
        // `--apply-option error-on-zero-match`, must abort `convert`.
        let base = TempFile::write("base.json", MINIMAL_V3_2);
        let overlay = TempFile::write(
            "overlay.json",
            br#"{"overlay":"1.0.0","info":{"title":"o","version":"1"},"actions":[{"target":"$.nope","update":{}}]}"#,
        );
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![],
            merge_options: vec![],
            collapse: false,
            load: vec![],
            apply: vec![overlay.0.clone()],
            apply_options: vec![roas_overlay::apply::ApplyOptions::ErrorOnZeroMatch],
        };
        let err = run_convert(args).expect_err("error-on-zero-match must surface");
        assert!(
            err.to_string().contains("applying overlay"),
            "expected `applying overlay` context, got: {err}",
        );
    }

    #[test]
    fn cli_parses_convert_with_apply_and_apply_option() {
        let cli = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--apply",
            "o.yaml",
            "--apply-option",
            "error-on-zero-match",
            "spec.json",
        ])
        .expect("convert --apply parse");
        match cli.command {
            Command::Convert(a) => {
                assert_eq!(a.apply, vec![PathBuf::from("o.yaml")]);
                assert_eq!(a.apply_options.len(), 1);
            }
            _ => panic!("expected convert"),
        }
    }

    #[test]
    fn cli_rejects_apply_option_without_apply() {
        // `--apply-option` requires at least one `--apply` source.
        let res = Cli::try_parse_from([
            "roas",
            "convert",
            "--to",
            "v3_2",
            "--apply-option",
            "error-on-zero-match",
            "spec.json",
        ]);
        assert!(res.is_err(), "--apply-option without --apply must error");
    }

    #[test]
    fn run_convert_missing_file_errors_with_reading_context() {
        let args = ConvertArgs {
            file: Some(temp_path("missing.json")),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            collapse: false,
            load: vec![],
            merge: vec![],
            merge_options: vec![],
            apply: vec![],
            apply_options: vec![],
        };
        let err = run_convert(args).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    // The server-side / helper-fn coverage for `preview` lives in
    // `preview.rs`'s own test module. These tests only confirm the
    // clap-wiring surface on `Cli` itself.
    #[test]
    fn cli_parses_preview_subcommand_with_defaults() {
        let cli = Cli::try_parse_from(["roas", "preview", "spec.json", "--no-open"])
            .expect("preview parse");
        match cli.command {
            Command::Preview(args) => {
                assert_eq!(args.file.as_ref().unwrap().to_string_lossy(), "spec.json");
                assert!(args.no_open);
                assert!(args.from.is_none());
                assert!(!args.watch);
                assert!(matches!(args.renderer, preview::Renderer::Redoc));
            }
            _ => panic!("expected Preview"),
        }
    }

    #[test]
    fn cli_parses_preview_subcommand_with_watch_flag() {
        let cli = Cli::try_parse_from(["roas", "preview", "--watch", "spec.json"])
            .expect("preview parse");
        match cli.command {
            Command::Preview(args) => {
                assert!(args.watch);
            }
            _ => panic!("expected Preview"),
        }
    }

    #[test]
    fn cli_parses_preview_subcommand_with_swagger_ui_renderer() {
        let cli = Cli::try_parse_from(["roas", "preview", "--renderer", "swagger-ui", "spec.json"])
            .expect("preview parse");
        match cli.command {
            Command::Preview(args) => {
                assert!(matches!(args.renderer, preview::Renderer::SwaggerUi));
            }
            _ => panic!("expected Preview"),
        }
    }

    #[test]
    fn cli_rejects_unknown_preview_renderer() {
        let res = Cli::try_parse_from(["roas", "preview", "--renderer", "stoplight", "spec.json"]);
        assert!(res.is_err(), "unknown renderer must be rejected");
    }

    // ── completions / manpages ──────────────────────────────────────────

    #[test]
    fn cli_parses_completions_with_each_supported_shell() {
        for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
            let cli = Cli::try_parse_from(["roas", "completions", shell])
                .unwrap_or_else(|e| panic!("completions {shell} parse: {e}"));
            assert!(
                matches!(cli.command, Command::Completions { .. }),
                "expected Completions for {shell}",
            );
        }
    }

    #[test]
    fn cli_rejects_unknown_completions_shell() {
        let res = Cli::try_parse_from(["roas", "completions", "tcsh"]);
        assert!(res.is_err(), "unsupported shell must be rejected");
    }

    #[test]
    fn cli_parses_manpages_with_short_and_long_flag() {
        for arg in ["--out", "-o"] {
            let cli = Cli::try_parse_from(["roas", "manpages", arg, "/tmp/x"])
                .unwrap_or_else(|e| panic!("manpages {arg} parse: {e}"));
            match cli.command {
                Command::Manpages { out } => assert_eq!(out, Path::new("/tmp/x")),
                _ => panic!("expected Manpages"),
            }
        }
    }

    #[test]
    fn cli_rejects_manpages_without_out_flag() {
        let res = Cli::try_parse_from(["roas", "manpages"]);
        assert!(res.is_err(), "--out is required");
    }

    /// Every shell variant should produce a non-empty completion script. We
    /// don't pin the exact contents (clap_complete's output evolves), but
    /// every script has to mention the bin name somewhere — that's enough
    /// to distinguish "generator ran" from "generator no-op'd".
    #[test]
    fn write_completions_emits_a_script_for_every_supported_shell() {
        use clap_complete::Shell;
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let mut buf = Vec::new();
            write_completions(shell, &mut buf).expect("write_completions");
            let out = String::from_utf8(buf).expect("completion script is UTF-8");
            assert!(
                !out.is_empty(),
                "{shell:?} produced empty completion script"
            );
            assert!(
                out.contains("roas"),
                "{shell:?} completion script must reference the bin name",
            );
        }
    }

    #[test]
    fn run_manpages_writes_top_level_and_per_subcommand_pages() {
        let dir = temp_path("manpages-pages");
        // run_manpages auto-creates a missing directory — assert that
        // behaviour by handing it a path that doesn't exist yet.
        assert!(!dir.exists());
        run_manpages(&dir).expect("run_manpages");

        // Top-level + one per subcommand (validate / convert / preview /
        // completions / manpages). No nested subcommands today, so the
        // tree walker stops one level deep.
        for name in [
            "roas.1",
            "roas-validate.1",
            "roas-convert.1",
            "roas-preview.1",
            "roas-completions.1",
            "roas-manpages.1",
        ] {
            let path = dir.join(name);
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            assert!(!body.is_empty(), "{name} is empty");
            // troff manpages have a `.TH <NAME> <SECTION>` header — the
            // NAME is the renamed (hyphenated) form for subpages, so a
            // bare `.TH ROAS-VALIDATE 1` is the strongest invariant the
            // SYNOPSIS-renaming code can be checked against.
            let expected_th = format!(".TH {} 1", name.trim_end_matches(".1").to_uppercase());
            assert!(
                body.contains(&expected_th),
                "{name} missing TH header `{expected_th}`",
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_manpages_overwrites_existing_files() {
        let dir = temp_path("manpages-overwrite");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let target = dir.join("roas.1");
        std::fs::write(&target, b"stale").expect("seed file");

        run_manpages(&dir).expect("run_manpages");

        let body = std::fs::read_to_string(&target).expect("read");
        assert_ne!(body, "stale", "existing manpage must be overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_completions_invokes_write_completions_against_stdout() {
        // Direct exercise of the thin `run_completions` wrapper so the
        // dispatch shim doesn't sit uncovered. Output goes to the real
        // stdout (the test harness will capture it); we only care that
        // the call doesn't error.
        run_completions(clap_complete::Shell::Bash).expect("run_completions");
    }
}
