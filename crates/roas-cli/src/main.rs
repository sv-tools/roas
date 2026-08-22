//! `roas` command-line front-end.
//!
//! One subcommand group per specification: `openapi` for OpenAPI specs,
//! `overlay` for OpenAPI Overlay documents, `arazzo` for OpenAPI Arazzo
//! workflow descriptions, and `asyncapi` for AsyncAPI documents.
//! `arazzo run` is the one command that talks to an API rather than
//! reading a document.
//!
//! - `roas openapi <validate|convert|preview>` — work with OpenAPI
//!   specs: `openapi validate` parses + validates one, `openapi
//!   convert --to <VERSION>` upconverts one (merging, applying
//!   overlays and collapsing on the way if asked), and `openapi
//!   preview` renders one in a browser. The `openapi` module documents
//!   each in full.
//!
//! - `roas overlay <validate|convert|apply>` — work with OpenAPI
//!   Overlay documents. `overlay validate` parses + validates an
//!   overlay; `overlay convert --to v1_1` upconverts one; `overlay
//!   apply --overlay <FILE> [SPEC]` applies overlay(s) to a target
//!   spec (spec on stdin or as the positional arg).
//!
//! - `roas arazzo <validate|convert|run|list>` — work with OpenAPI
//!   Arazzo workflow descriptions. `arazzo validate` parses + validates
//!   a description; `arazzo convert --to v1_1` upconverts one (v1.0 →
//!   v1.1). Arazzo describes workflows rather than transforming a spec,
//!   so there is no `apply` — but there is `arazzo run`, which performs
//!   every step's request against a real API and reports what happened.
//!   It needs the source descriptions the workflow points at: pass them
//!   with `--source <name>=<path>`, or `--load file` / `--load http` to
//!   fetch them. The report goes to stderr and the workflow's outputs to
//!   stdout, and the exit status follows the workflow. `--workflow <ID>`
//!   says which one to run — required where a description offers more
//!   than one, since the choice means real requests — and `arazzo list`
//!   says what a description offers and what each workflow takes.
//!
//! - `roas asyncapi <validate|convert>` — work with AsyncAPI
//!   documents. `asyncapi validate` parses + validates one (2.6, 3.0 or
//!   3.1, detected from the `asyncapi` field); `asyncapi convert --to
//!   <v3_0|v3_1>` upconverts one. 2.6 → 3.x is lossy — v3 reshaped the
//!   document — so the conversion reports what it invented or left
//!   behind on stderr, keeping stdout the document; `--strict` turns
//!   that report into a failure and `--quiet` silences it.
//!
//! Input may be JSON or YAML. With a file path, the parser is selected by
//! extension (`.yaml` / `.yml` → YAML, otherwise JSON). Pass `-` as the
//! file path, or omit it entirely and pipe the spec, to read from stdin;
//! stdin defaults to JSON. `--format json|yaml` overrides everything.

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use roas::loader::Loader;
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

mod arazzo;
mod asyncapi;
mod openapi;
mod overlay;
mod preview;
mod versioned;

use arazzo::ArazzoCommand;
use asyncapi::AsyncApiCommand;
use openapi::OpenApiCommand;
use overlay::OverlayCommand;
use versioned::{parse_value, path_looks_like_yaml};

#[derive(Parser)]
#[command(name = "roas", about, version, propagate_version = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work with OpenAPI descriptions: validate, convert, or preview.
    #[command(subcommand)]
    Openapi(OpenApiCommand),
    /// Work with OpenAPI Overlay documents: validate, convert, or apply.
    #[command(subcommand)]
    Overlay(OverlayCommand),
    /// Work with OpenAPI Arazzo descriptions: validate, convert, run, or
    /// list the workflows one offers.
    #[command(subcommand)]
    Arazzo(ArazzoCommand),
    /// Work with AsyncAPI documents: validate or convert.
    #[command(subcommand)]
    Asyncapi(AsyncApiCommand),
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
    /// `roas-<subcommand>.1` convention (e.g. `roas-openapi.1`,
    /// `roas-openapi-validate.1`).
    Manpages {
        /// Output directory (created if missing).
        #[arg(short, long, value_name = "DIR")]
        out: PathBuf,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum LoaderKind {
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
        Command::Openapi(cmd) => openapi::run_openapi(cmd),
        Command::Overlay(cmd) => overlay::run_overlay(cmd),
        Command::Arazzo(cmd) => arazzo::run_arazzo(cmd),
        Command::Asyncapi(cmd) => asyncapi::run_asyncapi(cmd),
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
        // and SYNOPSIS lines render as `roas-openapi` rather than the
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

pub(crate) fn build_loader(kinds: &[LoaderKind]) -> Option<Loader> {
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Write;

    pub(crate) fn temp_path(suffix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "roas-cli-test-{}-{}-{suffix}",
            std::process::id(),
            n,
        ))
    }

    pub(crate) struct TempFile(pub(crate) std::path::PathBuf);

    impl TempFile {
        pub(crate) fn write(suffix: &str, body: &[u8]) -> Self {
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
    fn cli_rejects_unknown_ignore_value() {
        let res =
            Cli::try_parse_from(["roas", "validate", "--ignore", "no-such-check", "spec.json"]);
        assert!(res.is_err(), "unknown --ignore value must error");
    }

    #[test]
    fn cli_rejects_convert_without_to() {
        let res = Cli::try_parse_from(["roas", "convert", "spec.json"]);
        assert!(res.is_err(), "convert without --to must error");
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

        // Top-level, one per group, and one per command inside a group —
        // the tree walker follows children, so `openapi validate` gets
        // its own `roas-openapi-validate.1`.
        for name in [
            "roas.1",
            "roas-openapi.1",
            "roas-openapi-validate.1",
            "roas-openapi-convert.1",
            "roas-openapi-preview.1",
            "roas-overlay.1",
            "roas-overlay-apply.1",
            "roas-arazzo.1",
            "roas-arazzo-run.1",
            "roas-asyncapi.1",
            "roas-asyncapi-convert.1",
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
