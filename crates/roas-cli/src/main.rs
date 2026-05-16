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
                assert_eq!(args.file.to_string_lossy(), "spec.json");
                assert!(args.from.is_none());
                assert!(args.load.is_empty());
                assert!(args.ignore.is_empty());
            }
            Command::Convert(_) => panic!("expected Validate"),
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
            Command::Convert(_) => panic!("expected Validate"),
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
            Command::Convert(_) => panic!("expected Validate"),
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
            Command::Validate(_) => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_rejects_convert_without_to() {
        let res = Cli::try_parse_from(["roas", "convert", "spec.json"]);
        assert!(res.is_err(), "convert without --to must error");
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
    fn read_and_parse_json_file_returns_parsed_value() {
        let f = TempFile::write("ok.json", br#"{"hello":"world"}"#);
        let v = read_and_parse(&f.0).expect("parse ok");
        assert_eq!(v, serde_json::json!({"hello": "world"}));
    }

    #[test]
    fn read_and_parse_yaml_file_routes_through_yaml_parser() {
        let f = TempFile::write("ok.yaml", b"name: pet\ncount: 3\n");
        let v = read_and_parse(&f.0).expect("parse ok");
        assert_eq!(v, serde_json::json!({"name": "pet", "count": 3}));
    }

    #[test]
    fn read_and_parse_missing_file_errors_with_reading_context() {
        let p = temp_path("missing.json");
        let err = read_and_parse(&p).expect_err("missing file must error");
        assert!(
            err.to_string().contains("reading"),
            "expected `reading` context, got: {err}",
        );
    }

    #[test]
    fn read_and_parse_invalid_json_surfaces_parser_error() {
        let f = TempFile::write("bad.json", b"@@@ not json");
        let err = read_and_parse(&f.0).expect_err("invalid JSON must error");
        assert!(
            err.to_string().contains("parsing JSON"),
            "expected `parsing JSON` context, got: {err}",
        );
    }

    #[test]
    fn read_and_parse_invalid_yaml_surfaces_parser_error() {
        let f = TempFile::write("bad.yaml", b"key:\n\tvalue: oops\n");
        let err = read_and_parse(&f.0).expect_err("invalid YAML must error");
        assert!(
            err.to_string().contains("parsing YAML"),
            "expected `parsing YAML` context, got: {err}",
        );
    }
}
