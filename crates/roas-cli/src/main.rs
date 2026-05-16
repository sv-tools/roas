//! `roas` command-line front-end.
//!
//! Two subcommands today:
//!
//! - `roas validate <FILE>` — parse and validate an OpenAPI spec.
//!   Version is auto-detected from the document; pass `--from` to force.
//!   External `$ref`s are skipped by default; use `--load file` / `--load http`
//!   (or both) to enable the loader.
//!
//! - `roas convert --to <VERSION> <FILE>` — chain the existing
//!   `From<v_X::Spec> for v_Y::Spec` migrations to upconvert a spec.
//!   Pass `--from` to force the input version.
//!
//! Input may be JSON or YAML; the parser is selected by file extension
//! (`.yaml` / `.yml` → YAML, otherwise JSON).

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use roas::loader::Loader;
use roas::validation::Options;
use roas_file_fetcher::FileFetcher;
use roas_http_fetcher::HttpFetcher;
use std::fs;
use std::path::PathBuf;

// `roas::validation::Options` implements `clap::ValueEnum` under the `clap`
// feature (enabled on the `roas` dep in this crate's Cargo.toml), so we can
// hand it straight to `#[arg(value_enum)]` without a CLI-local mirror enum.
// Variants render as kebab-case with the `Ignore` prefix dropped: e.g.
// `Options::IgnoreMissingTags` ↔ `--ignore missing-tags`.

mod versioned;

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
}

#[derive(clap::Args)]
struct ValidateArgs {
    /// Path to the spec file (JSON or YAML).
    file: PathBuf,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,

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

    /// Shorthand for `--ignore missing-tags --ignore unused-tags`. Matches
    /// the in-repo fixture-suite default.
    #[arg(long)]
    lenient_tags: bool,
}

#[derive(clap::Args)]
struct ConvertArgs {
    /// Path to the spec file (JSON or YAML).
    file: PathBuf,

    /// Target spec version.
    #[arg(long, value_enum)]
    to: SpecVersion,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LoaderKind {
    File,
    Http,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate(args) => run_validate(args),
        Command::Convert(args) => run_convert(args),
    }
}

fn read_and_parse(path: &std::path::Path) -> Result<serde_json::Value> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_value(&raw, path_looks_like_yaml(path))
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

fn run_validate(args: ValidateArgs) -> Result<()> {
    let value = read_and_parse(&args.file)?;
    let detected = versioned::detect_or_use(args.from, value)?;

    let mut loader = build_loader(&args.load);

    let mut options = enumset::EnumSet::<Options>::new();
    for ignore in &args.ignore {
        options |= *ignore;
    }
    if args.lenient_tags {
        options |= Options::IgnoreMissingTags;
        options |= Options::IgnoreUnusedTags;
    }
    match detected.validate(options, loader.as_mut()) {
        Ok(()) => {
            // Diagnostics go to stderr so stdout stays clean for shell pipelines.
            eprintln!("{}: valid {}", args.file.display(), detected.label());
            Ok(())
        }
        Err(err) => {
            for e in &err.errors {
                eprintln!("- {e}");
            }
            Err(anyhow!(
                "{}: validation failed ({} error{})",
                args.file.display(),
                err.errors.len(),
                if err.errors.len() == 1 { "" } else { "s" }
            ))
        }
    }
}

fn run_convert(args: ConvertArgs) -> Result<()> {
    let value = read_and_parse(&args.file)?;
    let detected = versioned::detect_or_use(args.from, value)?;

    let target = args.to;
    if (detected.version() as u8) > (target as u8) {
        bail!(
            "downconversion is not supported: input is {}, target is {}",
            detected.label(),
            target.label(),
        );
    }

    let converted = detected.convert_to(target)?;
    let json = serde_json::to_string_pretty(&converted).context("serializing converted spec")?;
    println!("{json}");
    Ok(())
}
