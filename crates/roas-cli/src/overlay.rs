//! `roas overlay` subcommand group — validate, convert, and apply
//! OpenAPI Overlay documents (powered by the `roas-overlay` crate).
//!
//! Overlays transform a *target* OpenAPI document; the apply step is
//! version-agnostic on the spec side (it operates on the spec as
//! untyped JSON), so these commands never go through the
//! [`crate::versioned::DetectedSpec`] machinery — they read the spec
//! as a `serde_json::Value`, apply, and serialise back.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use enumset::EnumSet;
use roas_overlay::apply::{Apply, ApplyError, ApplyOptions, ApplyReport};
use roas_overlay::validation::{Error as OverlayError, Validate, ValidationOptions};
use roas_overlay::{v1_0, v1_1};
use serde_json::Value;
use std::path::PathBuf;

use crate::{InputFormat, InputSource, read_input, resolve_input_source, serialize_spec};

/// Overlay specification version, mirroring [`crate::versioned::SpecVersion`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum OverlayVersion {
    #[value(name = "v1_0", alias = "1.0", alias = "v1.0")]
    V1_0,
    #[value(name = "v1_1", alias = "1.1", alias = "v1.1")]
    V1_1,
}

impl OverlayVersion {
    pub(crate) fn label(self) -> &'static str {
        match self {
            OverlayVersion::V1_0 => "Overlay 1.0",
            OverlayVersion::V1_1 => "Overlay 1.1",
        }
    }
}

/// A parsed overlay tagged with its version, mirroring
/// [`crate::versioned::DetectedSpec`].
#[derive(Debug)]
pub(crate) enum DetectedOverlay {
    V1_0(v1_0::Overlay),
    V1_1(v1_1::Overlay),
}

impl DetectedOverlay {
    pub(crate) fn version(&self) -> OverlayVersion {
        match self {
            DetectedOverlay::V1_0(_) => OverlayVersion::V1_0,
            DetectedOverlay::V1_1(_) => OverlayVersion::V1_1,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.version().label()
    }

    pub(crate) fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), OverlayError> {
        match self {
            DetectedOverlay::V1_0(o) => o.validate(options),
            DetectedOverlay::V1_1(o) => o.validate(options),
        }
    }

    /// Upconvert to `target`. Same-version is the identity; v1.0 → v1.1
    /// uses the `From` impl. Downconversion is not supported.
    pub(crate) fn convert_to(self, target: OverlayVersion) -> Result<DetectedOverlay> {
        match (self, target) {
            (DetectedOverlay::V1_0(o), OverlayVersion::V1_0) => Ok(DetectedOverlay::V1_0(o)),
            (DetectedOverlay::V1_0(o), OverlayVersion::V1_1) => {
                Ok(DetectedOverlay::V1_1(v1_1::Overlay::from(o)))
            }
            (DetectedOverlay::V1_1(o), OverlayVersion::V1_1) => Ok(DetectedOverlay::V1_1(o)),
            (DetectedOverlay::V1_1(_), OverlayVersion::V1_0) => bail!(
                "downconversion is not supported: input is Overlay 1.1, target is Overlay 1.0",
            ),
        }
    }

    pub(crate) fn apply(
        &self,
        target: &mut Value,
        options: EnumSet<ApplyOptions>,
    ) -> Result<ApplyReport, ApplyError> {
        match self {
            DetectedOverlay::V1_0(o) => o.apply(target, options),
            DetectedOverlay::V1_1(o) => o.apply(target, options),
        }
    }

    pub(crate) fn into_value(self) -> Result<Value> {
        match self {
            DetectedOverlay::V1_0(o) => {
                serde_json::to_value(o).context("serialising Overlay 1.0 document")
            }
            DetectedOverlay::V1_1(o) => {
                serde_json::to_value(o).context("serialising Overlay 1.1 document")
            }
        }
    }
}

/// Detect the overlay version by reading the top-level `overlay` field
/// (`"1.0.x"` → v1.0, `"1.1.x"` → v1.1).
pub(crate) fn detect_overlay(value: &Value) -> Result<OverlayVersion> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("overlay must be an object at the top level"))?;
    let overlay = obj
        .get("overlay")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("could not detect overlay version: no `overlay` field"))?;

    if overlay.starts_with("1.0.") {
        Ok(OverlayVersion::V1_0)
    } else if overlay.starts_with("1.1.") {
        Ok(OverlayVersion::V1_1)
    } else {
        bail!("unsupported overlay version: {overlay}")
    }
}

/// Detect (or force) the overlay version and deserialise into the
/// matching typed `Overlay`.
pub(crate) fn detect_or_use_overlay(
    forced: Option<OverlayVersion>,
    value: Value,
) -> Result<DetectedOverlay> {
    let version = match forced {
        Some(v) => v,
        None => detect_overlay(&value)?,
    };
    Ok(match version {
        OverlayVersion::V1_0 => DetectedOverlay::V1_0(
            serde_json::from_value(value).context("deserialising as Overlay 1.0")?,
        ),
        OverlayVersion::V1_1 => DetectedOverlay::V1_1(
            serde_json::from_value(value).context("deserialising as Overlay 1.1")?,
        ),
    })
}

#[derive(Subcommand)]
pub(crate) enum OverlayCommand {
    /// Parse and validate an Overlay document.
    Validate(OverlayValidateArgs),
    /// Upconvert an Overlay document to a newer version.
    Convert(OverlayConvertArgs),
    /// Apply overlay(s) to a target OpenAPI spec.
    Apply(OverlayApplyArgs),
}

#[derive(clap::Args)]
pub(crate) struct OverlayValidateArgs {
    /// Path to the overlay file (JSON or YAML). Pass `-`, or omit and
    /// pipe the overlay, to read from stdin.
    file: Option<PathBuf>,

    /// Override format detection. By default, file paths use the
    /// extension (`.yaml`/`.yml` → YAML, otherwise JSON) and stdin
    /// defaults to JSON.
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Skip a specific validation check (repeatable). Maps to
    /// `roas_overlay::validation::ValidationOptions`.
    #[arg(long, value_enum)]
    ignore: Vec<ValidationOptions>,

    /// Echo the parsed overlay to stdout on success, in the input
    /// format (YAML in → YAML out, JSON in → JSON out).
    #[arg(long)]
    print: bool,
}

#[derive(clap::Args)]
pub(crate) struct OverlayConvertArgs {
    /// Path to the overlay file (JSON or YAML). Pass `-`, or omit and
    /// pipe the overlay, to read from stdin.
    file: Option<PathBuf>,

    /// Target overlay version. Only upconversion is supported.
    #[arg(long, value_enum)]
    to: OverlayVersion,

    /// Override format detection (see `overlay validate --format`).
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Output format. Defaults to the input format.
    #[arg(long, value_enum)]
    output_format: Option<InputFormat>,
}

#[derive(clap::Args)]
pub(crate) struct OverlayApplyArgs {
    /// Path to the target OpenAPI spec (JSON or YAML). Pass `-`, or
    /// omit and pipe the spec, to read from stdin.
    file: Option<PathBuf>,

    /// Path to an overlay document to apply (repeatable). Overlays are
    /// applied in the order given, each on the result of the previous.
    #[arg(long, value_name = "FILE", required = true)]
    overlay: Vec<PathBuf>,

    /// Per-call apply option (repeatable). Maps to
    /// `roas_overlay::apply::ApplyOptions`.
    #[arg(long = "apply-option", value_enum)]
    apply_options: Vec<ApplyOptions>,

    /// Override format detection for the *spec* input (see
    /// `overlay validate --format`). Each `--overlay` source uses its
    /// own extension-based detection.
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Output format. Defaults to the spec's input format.
    #[arg(long, value_enum)]
    output_format: Option<InputFormat>,
}

pub(crate) fn run_overlay(cmd: OverlayCommand) -> Result<()> {
    match cmd {
        OverlayCommand::Validate(args) => run_overlay_validate(args),
        OverlayCommand::Convert(args) => run_overlay_convert(args),
        OverlayCommand::Apply(args) => run_overlay_apply(args),
    }
}

fn run_overlay_validate(args: OverlayValidateArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_overlay(None, value)?;

    let mut options = EnumSet::<ValidationOptions>::empty();
    for ignore in &args.ignore {
        options |= *ignore;
    }

    match detected.validate(options) {
        Ok(()) => {
            eprintln!("{}: valid {}", source.display(), detected.label());
            if args.print {
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
                "{}: overlay validation failed ({} error{})",
                source.display(),
                err.errors.len(),
                if err.errors.len() == 1 { "" } else { "s" }
            ))
        }
    }
}

fn run_overlay_convert(args: OverlayConvertArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_overlay(None, value)?;

    let converted = detected.convert_to(args.to)?;
    let value = converted.into_value()?;
    let out_format = args.output_format.unwrap_or(input_format);
    print!("{}", serialize_spec(&value, out_format, true)?);
    Ok(())
}

fn run_overlay_apply(args: OverlayApplyArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (mut spec, input_format) = read_input(&source, args.format)?;

    let mut options = EnumSet::<ApplyOptions>::empty();
    for opt in &args.apply_options {
        options |= *opt;
    }

    apply_overlays(&mut spec, &args.overlay, options)?;

    let out_format = args.output_format.unwrap_or(input_format);
    print!("{}", serialize_spec(&spec, out_format, true)?);
    Ok(())
}

/// Apply each overlay at `paths` to `spec` in order. Shared by
/// `overlay apply` and `convert --apply`. Each overlay is loaded with
/// extension-based format detection and its version detected from the
/// `overlay` field.
pub(crate) fn apply_overlays(
    spec: &mut Value,
    paths: &[PathBuf],
    options: EnumSet<ApplyOptions>,
) -> Result<()> {
    for path in paths {
        let (value, _format) = read_input(&InputSource::File(path.clone()), None)
            .with_context(|| format!("reading overlay {}", path.display()))?;
        let overlay = detect_or_use_overlay(None, value)
            .with_context(|| format!("parsing overlay {}", path.display()))?;
        overlay
            .apply(spec, options)
            .map_err(|e| anyhow!("applying overlay {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    // A minimal `Cli` mirror so we can exercise clap parsing of the
    // overlay subcommand tree in isolation.
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: OverlayCommand,
    }

    fn v1_0_overlay() -> Value {
        json!({
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1.0.0" },
            "actions": [ { "target": "$.info", "update": { "description": "d" } } ]
        })
    }

    fn v1_1_overlay() -> Value {
        json!({
            "overlay": "1.1.0",
            "info": { "title": "T", "version": "1.0.0", "description": "via copy" },
            "actions": [ { "target": "$.paths['/b']", "copy": "$.paths['/a']" } ]
        })
    }

    /// A temp file that cleans itself up on drop (mirrors the helper in
    /// `main.rs`'s test module).
    struct TempFile(PathBuf);

    impl TempFile {
        fn write(name: &str, value: &Value) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "roas-cli-overlay-{}-{n}-{name}",
                std::process::id(),
            ));
            std::fs::write(&path, serde_json::to_string(value).unwrap()).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn detect_overlay_distinguishes_versions() {
        assert_eq!(
            detect_overlay(&v1_0_overlay()).unwrap(),
            OverlayVersion::V1_0
        );
        let v11 = json!({
            "overlay": "1.1.0",
            "info": { "title": "T", "version": "1.0.0" },
            "actions": [ { "target": "$", "remove": true } ]
        });
        assert_eq!(detect_overlay(&v11).unwrap(), OverlayVersion::V1_1);
    }

    #[test]
    fn detect_overlay_rejects_missing_or_unknown_version() {
        let err = detect_overlay(&json!({ "info": {} })).unwrap_err();
        assert!(err.to_string().contains("no `overlay` field"));
        let err = detect_overlay(&json!({ "overlay": "2.0.0" })).unwrap_err();
        assert!(err.to_string().contains("unsupported overlay version"));
        let err = detect_overlay(&json!("not an object")).unwrap_err();
        assert!(err.to_string().contains("object at the top level"));
    }

    #[test]
    fn convert_upconverts_v1_0_to_v1_1_and_rejects_downconvert() {
        let d = detect_or_use_overlay(None, v1_0_overlay()).unwrap();
        let up = d.convert_to(OverlayVersion::V1_1).unwrap();
        assert_eq!(up.version(), OverlayVersion::V1_1);
        let value = up.into_value().unwrap();
        assert_eq!(value["overlay"], "1.1.0");

        // identity (v1.0 → v1.0)
        let d = detect_or_use_overlay(None, v1_0_overlay()).unwrap();
        assert_eq!(
            d.convert_to(OverlayVersion::V1_0).unwrap().version(),
            OverlayVersion::V1_0
        );

        // identity (v1.1 → v1.1)
        let d = detect_or_use_overlay(None, v1_1_overlay()).unwrap();
        assert_eq!(
            d.convert_to(OverlayVersion::V1_1).unwrap().version(),
            OverlayVersion::V1_1
        );

        // downconvert errors
        let d = detect_or_use_overlay(None, v1_1_overlay()).unwrap();
        let err = d.convert_to(OverlayVersion::V1_0).unwrap_err();
        assert!(err.to_string().contains("downconversion is not supported"));
    }

    #[test]
    fn detect_or_use_overlay_honors_forced_version() {
        // Force v1.1 even though a bare doc could be ambiguous; the
        // forced branch skips `detect_overlay`.
        let doc = json!({
            "overlay": "1.1.0",
            "info": { "title": "T", "version": "1.0.0" },
            "actions": [ { "target": "$", "remove": true } ]
        });
        let d = detect_or_use_overlay(Some(OverlayVersion::V1_1), doc).unwrap();
        assert_eq!(d.version(), OverlayVersion::V1_1);
    }

    #[test]
    fn apply_overlays_transforms_spec_in_order() {
        let mut spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "API", "version": "1.0.0" }
        });
        // Write the v1.0 overlay to a temp file and apply it.
        let f = TempFile::write("apply.json", &v1_0_overlay());
        apply_overlays(&mut spec, std::slice::from_ref(&f.0), EnumSet::empty()).unwrap();
        assert_eq!(spec["info"]["description"], "d");
    }

    #[test]
    fn apply_overlays_surfaces_apply_errors_with_path() {
        let mut spec = json!({ "info": { "title": "x" } });
        let bad = json!({
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1.0.0" },
            // target a primitive → PrimitiveActionTarget
            "actions": [ { "target": "$.info.title", "update": { "x": 1 } } ]
        });
        let f = TempFile::write("bad.json", &bad);
        let err =
            apply_overlays(&mut spec, std::slice::from_ref(&f.0), EnumSet::empty()).unwrap_err();
        assert!(err.to_string().contains("applying overlay"));
    }

    #[test]
    fn cli_parses_overlay_validate() {
        let cli = TestCli::try_parse_from(["roas", "validate", "overlay.yaml"]).unwrap();
        assert!(matches!(cli.command, OverlayCommand::Validate(_)));
    }

    #[test]
    fn cli_parses_overlay_convert_with_to() {
        let cli =
            TestCli::try_parse_from(["roas", "convert", "--to", "v1_1", "overlay.json"]).unwrap();
        match cli.command {
            OverlayCommand::Convert(a) => assert_eq!(a.to, OverlayVersion::V1_1),
            _ => panic!("expected convert"),
        }
    }

    #[test]
    fn cli_parses_overlay_apply_with_overlay_flag() {
        let res = TestCli::try_parse_from([
            "roas",
            "apply",
            "--overlay",
            "o.yaml",
            "--overlay",
            "p.yaml",
            "spec.json",
        ]);
        match res {
            Ok(cli) => match cli.command {
                OverlayCommand::Apply(a) => {
                    assert_eq!(a.overlay.len(), 2);
                    assert_eq!(a.file.as_deref(), Some(std::path::Path::new("spec.json")));
                }
                _ => panic!("expected apply"),
            },
            Err(e) => panic!("parse failed: {e}"),
        }
    }

    #[test]
    fn cli_rejects_overlay_apply_without_overlay_flag() {
        match TestCli::try_parse_from(["roas", "apply", "spec.json"]) {
            Err(e) => assert_eq!(e.kind(), clap::error::ErrorKind::MissingRequiredArgument),
            Ok(_) => panic!("expected a missing-`--overlay` error"),
        }
    }

    #[test]
    fn overlay_version_value_enum_aliases_parse() {
        assert_eq!(
            OverlayVersion::from_str("1.0", true).unwrap(),
            OverlayVersion::V1_0
        );
        assert_eq!(
            OverlayVersion::from_str("v1.1", true).unwrap(),
            OverlayVersion::V1_1
        );
    }

    // --- end-to-end run-function coverage (build args directly, like
    // the `run_convert` tests in main.rs; assert Ok/Err since stdout
    // isn't captured here). ---

    #[test]
    fn run_overlay_validate_ok_with_print_covers_v1_0() {
        let f = TempFile::write("ok.json", &v1_0_overlay());
        let args = OverlayValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: true, // exercises into_value + serialize_spec
        };
        run_overlay(OverlayCommand::Validate(args)).expect("valid overlay must pass");
    }

    #[test]
    fn run_overlay_validate_ok_covers_v1_1() {
        let f = TempFile::write("ok11.json", &v1_1_overlay());
        let args = OverlayValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: false,
        };
        run_overlay(OverlayCommand::Validate(args)).expect("valid v1.1 overlay must pass");
    }

    #[test]
    fn run_overlay_validate_reports_single_error() {
        // Valid info, empty actions → exactly one error (singular "").
        let doc = json!({
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1.0.0" },
            "actions": []
        });
        let f = TempFile::write("empty-actions.json", &doc);
        let args = OverlayValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: false,
        };
        let err = run_overlay(OverlayCommand::Validate(args)).unwrap_err();
        assert!(
            err.to_string().contains("1 error)"),
            "expected singular error count, got: {err}",
        );
    }

    #[test]
    fn run_overlay_validate_reports_multiple_errors() {
        // Empty info title+version AND empty actions → 3 errors (plural).
        let doc = json!({
            "overlay": "1.0.0",
            "info": { "title": "", "version": "" },
            "actions": []
        });
        let f = TempFile::write("many-errors.json", &doc);
        let args = OverlayValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: false,
        };
        let err = run_overlay(OverlayCommand::Validate(args)).unwrap_err();
        assert!(
            err.to_string().contains("errors)"),
            "expected plural error count, got: {err}",
        );
    }

    #[test]
    fn run_overlay_validate_ignore_suppresses_check() {
        // Empty title but `--ignore empty-info-title` → still other
        // diagnostics, but proves the ignore set is threaded through.
        let doc = json!({
            "overlay": "1.0.0",
            "info": { "title": "", "version": "1.0.0" },
            "actions": [ { "target": "$.info", "update": {} } ]
        });
        let f = TempFile::write("ignore.json", &doc);
        let args = OverlayValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![ValidationOptions::IgnoreEmptyInfoTitle],
            print: false,
        };
        run_overlay(OverlayCommand::Validate(args)).expect("empty title is ignored");
    }

    #[test]
    fn run_overlay_convert_upconverts_v1_0_to_v1_1() {
        let f = TempFile::write("conv.json", &v1_0_overlay());
        let args = OverlayConvertArgs {
            file: Some(f.0.clone()),
            to: OverlayVersion::V1_1,
            format: None,
            output_format: None,
        };
        run_overlay(OverlayCommand::Convert(args)).expect("upconvert must succeed");
    }

    #[test]
    fn run_overlay_apply_applies_v1_1_overlay_to_spec() {
        let spec = json!({
            "openapi": "3.1.0",
            "info": { "title": "API", "version": "1.0.0" },
            "paths": { "/a": { "get": { "summary": "s" } }, "/b": {} }
        });
        let spec_file = TempFile::write("spec.json", &spec);
        let overlay_file = TempFile::write("ov.json", &v1_1_overlay());
        let args = OverlayApplyArgs {
            file: Some(spec_file.0.clone()),
            overlay: vec![overlay_file.0.clone()],
            apply_options: vec![],
            format: None,
            output_format: Some(InputFormat::Yaml), // exercise output-format override
        };
        run_overlay(OverlayCommand::Apply(args)).expect("apply must succeed");
    }

    #[test]
    fn run_overlay_apply_surfaces_apply_error() {
        let spec = json!({ "info": { "title": "x" } });
        let spec_file = TempFile::write("spec.json", &spec);
        let bad = json!({
            "overlay": "1.0.0",
            "info": { "title": "T", "version": "1.0.0" },
            "actions": [ { "target": "$.nope", "update": {} } ]
        });
        let overlay_file = TempFile::write("bad.json", &bad);
        let args = OverlayApplyArgs {
            file: Some(spec_file.0.clone()),
            overlay: vec![overlay_file.0.clone()],
            apply_options: vec![ApplyOptions::ErrorOnZeroMatch],
            format: None,
            output_format: None,
        };
        let err = run_overlay(OverlayCommand::Apply(args)).unwrap_err();
        assert!(err.to_string().contains("applying overlay"));
    }
}
