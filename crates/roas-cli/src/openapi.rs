//! `roas openapi` subcommand group — validate, convert and preview
//! OpenAPI descriptions (powered by the `roas` crate).
//!
//! One group per specification: this is OpenAPI's, as [`crate::overlay`]
//! is Overlay's and [`crate::arazzo`] is Arazzo's. `preview` belongs
//! here too — it renders an OpenAPI description, and renders nothing
//! else.
//!
//! - `openapi validate [FILE]` — parse and validate an OpenAPI spec.
//!   Version is auto-detected from the document; pass `--from` to
//!   force. External `$ref`s are skipped by default; use `--load file`
//!   / `--load http` (or both) to enable the loader. Pass `--print` to
//!   echo the parsed spec to stdout on success — in the same format as
//!   the input (YAML in → YAML out, JSON in → JSON out) — useful for
//!   pipelines.
//!
//! - `openapi convert --to <VERSION> [FILE]` — chain the existing
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
//!   apply OpenAPI Overlay documents to the spec; `--apply-option`
//!   tunes the apply. The pipeline runs convert → `--merge` →
//!   `--apply` → `--collapse`, so overlay edits are visible to
//!   collapse. External `$ref`s are skipped by default; use `--load
//!   file` / `--load http` to opt into the loader. Output defaults to
//!   the input format (YAML in → YAML out, JSON in → JSON out); pass
//!   `--output-format json|yaml` to override.
//!
//! - `openapi preview [FILE]` — start a local HTTP server on
//!   `127.0.0.1:<random>` that serves the spec rendered with
//!   [Redoc](https://redocly.com/redoc) (default) or
//!   [Swagger UI](https://swagger.io/tools/swagger-ui/) (`--renderer
//!   swagger-ui`), and open the default browser at it. `--no-open`
//!   skips the launch. Ctrl+C tears the server down. `--watch`
//!   requires a real file (stdin can't be watched).

use anyhow::{Context, Result, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use roas::validation::Options;
use std::path::PathBuf;

use std::fs;

use crate::preview::{self, PreviewArgs};
use crate::versioned::{self, SpecVersion, parse_value, path_looks_like_yaml};
use crate::{
    InputFormat, LoaderKind, build_loader, overlay, read_input, resolve_input_source,
    serialize_spec,
};

#[derive(Subcommand)]
pub(crate) enum OpenApiCommand {
    /// Parse and validate an OpenAPI description.
    Validate(ValidateArgs),
    /// Convert an OpenAPI description to a different version.
    Convert(ConvertArgs),
    /// Preview the description in a browser, rendered with Redoc or
    /// Swagger UI.
    Preview(PreviewArgs),
}

pub(crate) fn run_openapi(cmd: OpenApiCommand) -> Result<()> {
    match cmd {
        OpenApiCommand::Validate(args) => run_validate(args),
        OpenApiCommand::Convert(args) => run_convert(args),
        OpenApiCommand::Preview(args) => preview::run_preview(args),
    }
}

#[derive(clap::Args)]
pub(crate) struct ValidateArgs {
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
    /// `roas openapi validate --help` to see the full list.
    #[arg(long, value_enum)]
    ignore: Vec<Options>,

    /// On success, echo the parsed spec to stdout in the same format as
    /// the input (YAML in → YAML out, JSON in → JSON out). Diagnostics
    /// stay on stderr. Lets `validate` sit in the middle of a pipeline:
    /// `roas openapi convert ... | roas openapi validate --print |
    /// roas openapi preview`.
    #[arg(long)]
    print: bool,
}

#[derive(clap::Args)]
pub(crate) struct ConvertArgs {
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
    /// semantics as `roas openapi validate --load`: pass `--load file` to
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
                // JSON in → JSON out. `into_value` re-serializes through the
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

    // Pipeline order is convert → merge → apply → collapse, so overlay
    // edits are visible to collapse (overlay-introduced inline
    // components get lifted into `$ref`s too). `collapse` works on the
    // typed spec while `apply` works on `serde_json::Value`, so:
    //
    //   - no `--apply`: collapse on the typed spec directly (no
    //     serialize/re-parse round-trip; identical to plain convert).
    //   - with `--apply`: serialize, apply the overlays, then — if
    //     `--collapse` — re-parse the result at the target version and
    //     collapse the typed spec.
    let value = if args.apply.is_empty() {
        // 3) Collapse on the typed (post-merge) spec.
        if args.collapse {
            let mut loader = build_loader(&args.load);
            converted.collapse(loader.as_mut())?;
        }
        converted.into_value()?
    } else {
        // 3) Apply overlays on the serialized (post-merge) spec.
        let mut apply_options = enumset::EnumSet::<roas_overlay::apply::ApplyOptions>::empty();
        for opt in &args.apply_options {
            apply_options |= *opt;
        }
        let mut value = converted.into_value()?;
        overlay::apply_overlays(&mut value, &args.apply, apply_options)?;

        // 4) Collapse the overlaid spec. Re-parse at the target
        //    version first, since collapse needs the typed model.
        if args.collapse {
            let mut respec = versioned::detect_or_use(Some(target), value)
                .context("re-parsing the overlaid spec for --collapse")?;
            let mut loader = build_loader(&args.load);
            respec.collapse(loader.as_mut())?;
            respec.into_value()?
        } else {
            value
        }
    };

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
    use crate::tests::{TempFile, temp_path};

    /// The `openapi` group's own command, out of a whole command line.
    fn group(cli: Cli) -> OpenApiCommand {
        match cli.command {
            crate::Command::Openapi(cmd) => cmd,
            _ => panic!("expected the `openapi` group"),
        }
    }
    use crate::{Cli, InputSource};
    use clap::Parser;
    use clap::error::ErrorKind;
    use std::path::Path;

    #[test]
    fn build_loader_returns_some_for_combined_kinds() {
        assert!(build_loader(&[LoaderKind::File, LoaderKind::Http]).is_some());
    }

    /// `run_convert`'s downconvert guard uses `(SpecVersion as u8)` ordering;
    /// the variant declaration order must match the chronological version
    /// order or `roas openapi convert --to v2 spec_3_2.json` would silently
    /// succeed.
    #[test]
    fn spec_version_discriminants_order_by_version() {
        assert!((SpecVersion::V2 as u8) < (SpecVersion::V3_0 as u8));
        assert!((SpecVersion::V3_0 as u8) < (SpecVersion::V3_1 as u8));
        assert!((SpecVersion::V3_1 as u8) < (SpecVersion::V3_2 as u8));
    }

    #[test]
    fn cli_parses_minimal_validate_invocation() {
        let cli = Cli::try_parse_from(["roas", "openapi", "validate", "spec.json"])
            .expect("validate parse");
        match group(cli) {
            OpenApiCommand::Validate(args) => {
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
        let cli = Cli::try_parse_from(["roas", "openapi", "validate"]).expect("validate parse");
        match group(cli) {
            OpenApiCommand::Validate(args) => assert!(args.file.is_none()),
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_validate_with_stdin_sentinel_and_format_flag() {
        let cli = Cli::try_parse_from(["roas", "openapi", "validate", "--format", "yaml", "-"])
            .expect("validate parse");
        match group(cli) {
            OpenApiCommand::Validate(args) => {
                assert_eq!(args.file.as_deref(), Some(Path::new("-")));
                assert_eq!(args.format, Some(InputFormat::Yaml));
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_validate_print_flag() {
        let cli = Cli::try_parse_from(["roas", "openapi", "validate", "--print", "spec.json"])
            .expect("validate parse");
        match group(cli) {
            OpenApiCommand::Validate(args) => assert!(args.print),
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn cli_parses_ignore_flag_into_options_variants() {
        let cli = Cli::try_parse_from([
            "roas",
            "openapi",
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
        match group(cli) {
            OpenApiCommand::Validate(args) => {
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
    fn cli_parses_repeated_load_flag_into_vec() {
        let cli = Cli::try_parse_from([
            "roas",
            "openapi",
            "validate",
            "--load",
            "file",
            "--load",
            "http",
            "spec.json",
        ])
        .expect("validate parse");
        match group(cli) {
            OpenApiCommand::Validate(args) => {
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
            "openapi",
            "convert",
            "--from",
            "v2",
            "--to",
            "v3_2",
            "spec.json",
        ])
        .expect("convert parse");
        match group(cli) {
            OpenApiCommand::Convert(args) => {
                assert_eq!(args.from, Some(SpecVersion::V2));
                assert_eq!(args.to, SpecVersion::V3_2);
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_parses_convert_with_output_format_flag() {
        let cli = Cli::try_parse_from([
            "roas",
            "openapi",
            "convert",
            "--to",
            "v3_2",
            "--output-format",
            "yaml",
            "spec.json",
        ])
        .expect("convert parse");
        match group(cli) {
            OpenApiCommand::Convert(args) => {
                assert_eq!(args.output_format, Some(InputFormat::Yaml))
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_parses_convert_with_collapse_and_load_flags() {
        let cli = Cli::try_parse_from([
            "roas",
            "openapi",
            "convert",
            "--to",
            "v3_2",
            "--collapse",
            "--load",
            "file",
            "spec.json",
        ])
        .expect("convert parse");
        match group(cli) {
            OpenApiCommand::Convert(args) => {
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
        let cli = Cli::try_parse_from(["roas", "openapi", "convert", "--to", "v3_2", "spec.json"])
            .expect("convert parse");
        match group(cli) {
            OpenApiCommand::Convert(args) => {
                assert!(!args.collapse, "--collapse defaults to false")
            }
            _ => panic!("expected Convert"),
        }
    }

    #[test]
    fn cli_parses_convert_with_merge() {
        let cli = Cli::try_parse_from([
            "roas",
            "openapi",
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
        match group(cli) {
            OpenApiCommand::Convert(args) => {
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
            "openapi",
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
        match group(cli) {
            OpenApiCommand::Convert(args) => {
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
    fn run_convert_with_apply_then_collapse_reparses_and_collapses() {
        // Exercises the apply → collapse path: the overlay edits the
        // spec (Value), then it's re-parsed at the target version and
        // collapsed. The overlay adds an inline schema that collapse
        // can lift into components.
        let base = TempFile::write("base.json", MINIMAL_V3_2);
        let overlay = TempFile::write(
            "overlay.json",
            br#"{"overlay":"1.0.0","info":{"title":"o","version":"1"},"actions":[{"target":"$","update":{"components":{"schemas":{"Pet":{"type":"object"}}}}}]}"#,
        );
        let args = ConvertArgs {
            file: Some(base.0.clone()),
            to: SpecVersion::V3_2,
            from: None,
            format: None,
            output_format: None,
            merge: vec![],
            merge_options: vec![],
            collapse: true,
            load: vec![],
            apply: vec![overlay.0.clone()],
            apply_options: vec![],
        };
        run_convert(args).expect("convert + apply + collapse must succeed");
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
            "openapi",
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
        match group(cli) {
            OpenApiCommand::Convert(a) => {
                assert_eq!(a.apply, vec![PathBuf::from("o.yaml")]);
                assert_eq!(a.apply_options.len(), 1);
            }
            _ => panic!("expected convert"),
        }
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
        let cli = Cli::try_parse_from(["roas", "openapi", "preview", "spec.json", "--no-open"])
            .expect("preview parse");
        match group(cli) {
            OpenApiCommand::Preview(args) => {
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
        let cli = Cli::try_parse_from(["roas", "openapi", "preview", "--watch", "spec.json"])
            .expect("preview parse");
        match group(cli) {
            OpenApiCommand::Preview(args) => {
                assert!(args.watch);
            }
            _ => panic!("expected Preview"),
        }
    }

    #[test]
    fn cli_parses_preview_subcommand_with_swagger_ui_renderer() {
        let cli = Cli::try_parse_from([
            "roas",
            "openapi",
            "preview",
            "--renderer",
            "swagger-ui",
            "spec.json",
        ])
        .expect("preview parse");
        match group(cli) {
            OpenApiCommand::Preview(args) => {
                assert!(matches!(args.renderer, preview::Renderer::SwaggerUi));
            }
            _ => panic!("expected Preview"),
        }
    }
    // ── argument constraints ────────────────────────────────────────────

    /// The error a bad command line produces — guarded against passing
    /// for the wrong reason. `InvalidSubcommand` would mean the test
    /// lost track of where the command lives (as these did when the
    /// OpenAPI commands moved under `openapi`), not that the constraint
    /// under test held.
    fn parse_error(args: &[&str]) -> ErrorKind {
        let kind = Cli::try_parse_from(args)
            .err()
            .unwrap_or_else(|| panic!("{args:?} must not parse"))
            .kind();
        assert_ne!(
            kind,
            ErrorKind::InvalidSubcommand,
            "{args:?} names a command that does not exist",
        );
        kind
    }

    #[test]
    fn cli_rejects_unknown_ignore_value() {
        let args = [
            "roas",
            "openapi",
            "validate",
            "--ignore",
            "no-such-check",
            "spec.json",
        ];
        assert_eq!(parse_error(&args), ErrorKind::InvalidValue);
    }

    #[test]
    fn cli_rejects_convert_without_to() {
        let args = ["roas", "openapi", "convert", "spec.json"];
        assert_eq!(parse_error(&args), ErrorKind::MissingRequiredArgument);
    }

    /// `--load` is only meaningful when `--collapse` is active; clap's
    /// `requires = "collapse"` must reject the flag on its own.
    #[test]
    fn cli_rejects_convert_load_without_collapse() {
        let args = [
            "roas",
            "openapi",
            "convert",
            "--to",
            "v3_2",
            "--load",
            "file",
            "spec.json",
        ];
        assert_eq!(parse_error(&args), ErrorKind::MissingRequiredArgument);
    }

    /// `--merge-option` is only meaningful when at least one `--merge`
    /// source is provided; clap's `requires = "merge"` must reject the
    /// flag on its own.
    #[test]
    fn cli_rejects_merge_option_without_merge() {
        let args = [
            "roas",
            "openapi",
            "convert",
            "--to",
            "v3_2",
            "--merge-option",
            "base-wins",
            "spec.json",
        ];
        assert_eq!(parse_error(&args), ErrorKind::MissingRequiredArgument);
    }

    /// `--apply-option` requires at least one `--apply` source.
    #[test]
    fn cli_rejects_apply_option_without_apply() {
        let args = [
            "roas",
            "openapi",
            "convert",
            "--to",
            "v3_2",
            "--apply-option",
            "error-on-zero-match",
            "spec.json",
        ];
        assert_eq!(parse_error(&args), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn cli_rejects_unknown_preview_renderer() {
        let args = [
            "roas",
            "openapi",
            "preview",
            "--renderer",
            "stoplight",
            "spec.json",
        ];
        assert_eq!(parse_error(&args), ErrorKind::InvalidValue);
    }
}
