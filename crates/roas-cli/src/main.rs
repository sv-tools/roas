//! `roas` command-line front-end.
//!
//! Two subcommands today:
//!
//! - `roas validate <FILE>` — parse and validate an OpenAPI spec.
//!   Version is auto-detected from the document; pass `--from` to
//!   force. External `$ref`s are skipped by default; use
//!   `--load file` / `--load http` (or both) to enable the loader.
//!
//! - `roas convert --to <VERSION> <FILE>` — chain the existing
//!   `From<v_X::Spec> for v_Y::Spec` migrations to upconvert a spec.
//!   Pass `--from` to force the input version.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use roas::loader::{JsonFileFetcher, Loader};
use roas::validation::Options;
use roas_http_fetcher::HttpFetcher;
use std::fs;
use std::path::PathBuf;

mod versioned;

use versioned::SpecVersion;

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
    /// Path to the JSON spec file.
    file: PathBuf,

    /// Force the input version (auto-detected by default).
    #[arg(long, value_enum)]
    from: Option<SpecVersion>,

    /// Enable external-reference loading. Pass `--load file` to
    /// allow `file://` refs, `--load http` to allow `http://` and
    /// `https://`. Repeat the flag to combine
    /// (e.g. `--load file --load http`).
    #[arg(long, value_enum)]
    load: Vec<LoaderKind>,

    /// Treat unused / missing tags as warnings rather than errors.
    /// Matches the in-repo fixture-suite default.
    #[arg(long)]
    lenient_tags: bool,
}

#[derive(clap::Args)]
struct ConvertArgs {
    /// Path to the JSON spec file.
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

fn run_validate(args: ValidateArgs) -> Result<()> {
    let raw = fs::read_to_string(&args.file)
        .with_context(|| format!("reading {}", args.file.display()))?;
    let detected = versioned::detect_or_use(args.from, &raw)?;

    let mut loader = if args.load.is_empty() {
        None
    } else {
        let mut l = Loader::new();
        for kind in &args.load {
            match kind {
                LoaderKind::File => {
                    l.register_fetcher("file://", JsonFileFetcher);
                }
                LoaderKind::Http => {
                    l.register_fetcher("http://", HttpFetcher::new());
                    l.register_fetcher("https://", HttpFetcher::new());
                }
            }
        }
        Some(l)
    };

    let mut options = Options::new();
    if args.lenient_tags {
        options |= Options::IgnoreMissingTags;
        options |= Options::IgnoreUnusedTags;
    }
    let result = detected.validate(options, loader.as_mut());
    match result {
        Ok(()) => {
            // Success line goes to stderr too: stdout stays empty on
            // `validate`, so it can be safely composed in shell
            // pipelines that only want the validation status.
            eprintln!("{}: valid {}", args.file.display(), detected.label());
            Ok(())
        }
        Err(err) => {
            // All diagnostics go to stderr so stdout stays clean —
            // especially for `convert`, which emits machine-readable
            // JSON on stdout, and for any future caller that pipes the
            // CLI's output.
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
    let raw = fs::read_to_string(&args.file)
        .with_context(|| format!("reading {}", args.file.display()))?;
    let detected = versioned::detect_or_use(args.from, &raw)?;

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
