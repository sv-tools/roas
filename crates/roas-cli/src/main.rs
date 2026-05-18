//! `roas` command-line front-end.
//!
//! Three subcommands today:
//!
//! - `roas validate <FILE>` — parse and validate an OpenAPI spec. Version is
//!   auto-detected from the document; pass `--from` to force. External
//!   `$ref`s are skipped by default; use `--load file` / `--load http`
//!   (or both) to enable the loader.
//!
//! - `roas convert --to <VERSION> <FILE>` — chain the existing
//!   `From<v_X::Spec> for v_Y::Spec` migrations to upconvert a spec.
//!   Pass `--from` to force the input version. Pass `--collapse` to
//!   run `Spec::collapse` on the (post-conversion) result, lifting
//!   every inline component into the matching `components.<bag>` /
//!   `definitions` / `parameters` / `responses` slot with strict
//!   dedup. External `$ref`s are skipped by default; use
//!   `--load file` / `--load http` to opt into the loader.
//!
//! - `roas preview <FILE>` — start a local HTTP server on
//!   `127.0.0.1:<random>` that serves the spec rendered with
//!   [Redoc](https://redocly.com/redoc) (default) or
//!   [Swagger UI](https://swagger.io/tools/swagger-ui/) (`--renderer
//!   swagger-ui`), and open the default browser at it. `--no-open` skips
//!   the launch. Ctrl+C tears the server down.
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

mod preview;
mod versioned;

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
    /// Preview the spec in a browser, rendered with Redoc or Swagger UI.
    Preview(PreviewArgs),
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

    /// Lift every inline component into the matching root bag
    /// (`components.<bag>` for v3.x, `definitions` / `parameters` /
    /// `responses` for v2) and replace its call sites with a `$ref`.
    /// Structurally identical components collapse to a single entry.
    /// Runs after the version conversion.
    #[arg(long)]
    collapse: bool,

    /// Enable external-reference loading during `--collapse`. Same
    /// semantics as `roas validate --load`: pass `--load file` to
    /// allow `file://` refs, `--load http` for `http(s)://`; repeat
    /// to combine. Without it, external `$ref`s in the input are
    /// left untouched. Requires `--collapse` (clap rejects the flag
    /// on its own — collapse is the only consumer).
    #[arg(long, value_enum, requires = "collapse")]
    load: Vec<LoaderKind>,
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
        Command::Preview(args) => preview::run_preview(args),
    }
}

pub(crate) fn read_and_parse(path: &std::path::Path) -> Result<serde_json::Value> {
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

    let mut converted = detected.convert_to_detected(target)?;
    if args.collapse {
        let mut loader = build_loader(&args.load);
        converted.collapse(loader.as_mut())?;
    }
    let value = converted.into_value()?;
    let json = serde_json::to_string_pretty(&value).context("serializing converted spec")?;
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
            file: f.0.clone(),
            from: None,
            load: Vec::new(),
            ignore: Vec::new(),
        };
        run_validate(args).expect("clean spec must validate");
    }

    #[test]
    fn run_validate_returns_err_for_spec_with_unused_tag() {
        // Default ignore set fires on unused tags.
        let body = br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"unused"}]}"#;
        let f = TempFile::write("unused-tag.json", body);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: Vec::new(),
            ignore: Vec::new(),
        };
        let err = run_validate(args).expect_err("unused tag must fail");
        assert!(err.to_string().contains("validation failed"), "got: {err}",);
    }

    #[test]
    fn run_validate_with_ignore_suppresses_validation_failure() {
        let body = br#"{"openapi":"3.2.0","info":{"title":"x","version":"1"},"paths":{},"tags":[{"name":"unused"}]}"#;
        let f = TempFile::write("ignored.json", body);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: Vec::new(),
            ignore: vec![Options::IgnoreUnusedTags],
        };
        run_validate(args).expect("--ignore unused-tags must suppress");
    }

    #[test]
    fn run_validate_with_load_file_builds_loader() {
        let f = TempFile::write("with-load.json", MINIMAL_V3_2);
        let args = ValidateArgs {
            file: f.0.clone(),
            from: None,
            load: vec![LoaderKind::File],
            ignore: Vec::new(),
        };
        run_validate(args).expect("clean spec with file loader must validate");
    }

    #[test]
    fn run_validate_missing_file_errors_with_reading_context() {
        let args = ValidateArgs {
            file: temp_path("missing.json"),
            from: None,
            load: Vec::new(),
            ignore: Vec::new(),
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
            file: f.0.clone(),
            to: SpecVersion::V3_2,
            from: None,
            collapse: false,
            load: vec![],
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
            file: f.0.clone(),
            to: SpecVersion::V3_2,
            from: None,
            collapse: true,
            load: vec![LoaderKind::File],
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
            file: f.0.clone(),
            to: SpecVersion::V3_2,
            from: None,
            collapse: true,
            load: vec![],
        };
        run_convert(args).expect("convert + collapse must succeed");
    }

    #[test]
    fn run_convert_rejects_downconversion() {
        let f = TempFile::write("v3.json", MINIMAL_V3_2);
        let args = ConvertArgs {
            file: f.0.clone(),
            to: SpecVersion::V2,
            from: None,
            collapse: false,
            load: vec![],
        };
        let err = run_convert(args).expect_err("downconversion must error");
        assert!(
            err.to_string().contains("downconversion is not supported"),
            "got: {err}",
        );
    }

    #[test]
    fn run_convert_missing_file_errors_with_reading_context() {
        let args = ConvertArgs {
            file: temp_path("missing.json"),
            to: SpecVersion::V3_2,
            from: None,
            collapse: false,
            load: vec![],
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
                assert_eq!(args.file.to_string_lossy(), "spec.json");
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
}
