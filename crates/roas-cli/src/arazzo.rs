//! `roas arazzo` subcommand group — validate and convert OpenAPI
//! Arazzo workflow descriptions (powered by the `roas-arazzo` crate).
//!
//! Arazzo *describes* sequences of API calls; unlike Overlay there is no
//! transform/apply step, so this group is just `validate` and `convert`
//! (upconvert v1.0 → v1.1). The version is detected from the top-level
//! `arazzo` field, mirroring [`crate::overlay`]'s `DetectedOverlay`.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use enumset::EnumSet;
use roas_arazzo::validation::{Error as ArazzoError, Validate, ValidationOptions};
use roas_arazzo::{v1_0, v1_1};
use serde_json::Value;
use std::path::PathBuf;

use crate::{InputFormat, read_input, resolve_input_source, serialize_spec};

/// Arazzo specification version, mirroring [`crate::overlay::OverlayVersion`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ArazzoVersion {
    #[value(name = "v1_0", alias = "1.0", alias = "v1.0")]
    V1_0,
    #[value(name = "v1_1", alias = "1.1", alias = "v1.1")]
    V1_1,
}

impl ArazzoVersion {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ArazzoVersion::V1_0 => "Arazzo 1.0",
            ArazzoVersion::V1_1 => "Arazzo 1.1",
        }
    }
}

/// A parsed Arazzo description tagged with its version.
#[derive(Debug)]
pub(crate) enum DetectedArazzo {
    V1_0(v1_0::Description),
    V1_1(v1_1::Description),
}

impl DetectedArazzo {
    pub(crate) fn version(&self) -> ArazzoVersion {
        match self {
            DetectedArazzo::V1_0(_) => ArazzoVersion::V1_0,
            DetectedArazzo::V1_1(_) => ArazzoVersion::V1_1,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.version().label()
    }

    pub(crate) fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), ArazzoError> {
        match self {
            DetectedArazzo::V1_0(d) => d.validate(options),
            DetectedArazzo::V1_1(d) => d.validate(options),
        }
    }

    /// Upconvert to `target`. Same-version is the identity; v1.0 → v1.1
    /// uses the `From` impl. Downconversion is not supported.
    pub(crate) fn convert_to(self, target: ArazzoVersion) -> Result<DetectedArazzo> {
        match (self, target) {
            (DetectedArazzo::V1_0(d), ArazzoVersion::V1_0) => Ok(DetectedArazzo::V1_0(d)),
            (DetectedArazzo::V1_0(d), ArazzoVersion::V1_1) => {
                Ok(DetectedArazzo::V1_1(v1_1::Description::from(d)))
            }
            (DetectedArazzo::V1_1(d), ArazzoVersion::V1_1) => Ok(DetectedArazzo::V1_1(d)),
            (DetectedArazzo::V1_1(_), ArazzoVersion::V1_0) => {
                bail!("downconversion is not supported: input is Arazzo 1.1, target is Arazzo 1.0",)
            }
        }
    }

    pub(crate) fn into_value(self) -> Result<Value> {
        match self {
            DetectedArazzo::V1_0(d) => {
                serde_json::to_value(d).context("serializing Arazzo 1.0 description")
            }
            DetectedArazzo::V1_1(d) => {
                serde_json::to_value(d).context("serializing Arazzo 1.1 description")
            }
        }
    }
}

/// Detect the Arazzo version by reading the top-level `arazzo` field
/// (`"1.0.x"` → v1.0, `"1.1.x"` → v1.1).
pub(crate) fn detect_arazzo(value: &Value) -> Result<ArazzoVersion> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("Arazzo description must be an object at the top level"))?;
    let arazzo = obj
        .get("arazzo")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("could not detect Arazzo version: no `arazzo` field"))?;

    if arazzo.starts_with("1.0.") {
        Ok(ArazzoVersion::V1_0)
    } else if arazzo.starts_with("1.1.") {
        Ok(ArazzoVersion::V1_1)
    } else {
        bail!("unsupported Arazzo version: {arazzo}")
    }
}

/// Detect (or force) the Arazzo version and deserialize into the
/// matching typed `Description`.
pub(crate) fn detect_or_use_arazzo(
    forced: Option<ArazzoVersion>,
    value: Value,
) -> Result<DetectedArazzo> {
    let version = match forced {
        Some(v) => v,
        None => detect_arazzo(&value)?,
    };
    Ok(match version {
        ArazzoVersion::V1_0 => DetectedArazzo::V1_0(
            serde_json::from_value(value).context("deserializing as Arazzo 1.0")?,
        ),
        ArazzoVersion::V1_1 => DetectedArazzo::V1_1(
            serde_json::from_value(value).context("deserializing as Arazzo 1.1")?,
        ),
    })
}

#[derive(Subcommand)]
pub(crate) enum ArazzoCommand {
    /// Parse and validate an Arazzo description.
    Validate(ArazzoValidateArgs),
    /// Upconvert an Arazzo description to a newer version.
    Convert(ArazzoConvertArgs),
}

#[derive(clap::Args)]
pub(crate) struct ArazzoValidateArgs {
    /// Path to the Arazzo file (JSON or YAML). Pass `-`, or omit and
    /// pipe the description, to read from stdin.
    file: Option<PathBuf>,

    /// Override format detection. By default, file paths use the
    /// extension (`.yaml`/`.yml` → YAML, otherwise JSON) and stdin
    /// defaults to JSON.
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Skip a specific validation check (repeatable). Maps to
    /// `roas_arazzo::validation::ValidationOptions`.
    #[arg(long, value_enum)]
    ignore: Vec<ValidationOptions>,

    /// Echo the parsed description to stdout on success, in the input
    /// format (YAML in → YAML out, JSON in → JSON out).
    #[arg(long)]
    print: bool,
}

#[derive(clap::Args)]
pub(crate) struct ArazzoConvertArgs {
    /// Path to the Arazzo file (JSON or YAML). Pass `-`, or omit and
    /// pipe the description, to read from stdin.
    file: Option<PathBuf>,

    /// Target Arazzo version. Only upconversion is supported.
    #[arg(long, value_enum)]
    to: ArazzoVersion,

    /// Override format detection (see `arazzo validate --format`).
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Output format. Defaults to the input format.
    #[arg(long, value_enum)]
    output_format: Option<InputFormat>,
}

pub(crate) fn run_arazzo(cmd: ArazzoCommand) -> Result<()> {
    match cmd {
        ArazzoCommand::Validate(args) => run_arazzo_validate(args),
        ArazzoCommand::Convert(args) => run_arazzo_convert(args),
    }
}

fn run_arazzo_validate(args: ArazzoValidateArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_arazzo(None, value)?;

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
                "{}: Arazzo validation failed ({} error{})",
                source.display(),
                err.errors.len(),
                if err.errors.len() == 1 { "" } else { "s" }
            ))
        }
    }
}

fn run_arazzo_convert(args: ArazzoConvertArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_arazzo(None, value)?;

    let converted = detected.convert_to(args.to)?;
    let value = converted.into_value()?;
    let out_format = args.output_format.unwrap_or(input_format);
    print!("{}", serialize_spec(&value, out_format, true)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    /// A minimal `Cli` mirror exercising clap parsing of the arazzo
    /// subcommand tree in isolation.
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: ArazzoCommand,
    }

    fn v1_0_doc() -> Value {
        json!({
            "arazzo": "1.0.1",
            "info": { "title": "T", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "src", "url": "openapi.yaml", "type": "openapi" } ],
            "workflows": [
                { "workflowId": "wf", "steps": [ { "stepId": "s", "operationId": "op",
                    "parameters": [ { "name": "p", "in": "query", "value": 1 } ] } ] }
            ]
        })
    }

    fn v1_1_doc() -> Value {
        json!({
            "arazzo": "1.1.0",
            "info": { "title": "T", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "events", "url": "asyncapi.yaml", "type": "asyncapi" } ],
            "workflows": [
                { "workflowId": "wf", "steps": [
                    { "stepId": "s", "channelPath": "$sourceDescriptions.events#/c", "action": "send" }
                ] }
            ]
        })
    }

    /// A temp file that cleans itself up on drop (mirrors `overlay`'s helper).
    struct TempFile(PathBuf);

    impl TempFile {
        fn write(name: &str, value: &Value) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("roas-cli-arazzo-{}-{n}-{name}", std::process::id(),));
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
    fn detect_arazzo_distinguishes_versions() {
        assert_eq!(detect_arazzo(&v1_0_doc()).unwrap(), ArazzoVersion::V1_0);
        assert_eq!(detect_arazzo(&v1_1_doc()).unwrap(), ArazzoVersion::V1_1);
    }

    #[test]
    fn detect_arazzo_rejects_missing_or_unknown_version() {
        let err = detect_arazzo(&json!({ "info": {} })).unwrap_err();
        assert!(err.to_string().contains("no `arazzo` field"));
        let err = detect_arazzo(&json!({ "arazzo": "2.0.0" })).unwrap_err();
        assert!(err.to_string().contains("unsupported Arazzo version"));
        let err = detect_arazzo(&json!("not an object")).unwrap_err();
        assert!(err.to_string().contains("object at the top level"));
    }

    #[test]
    fn convert_upconverts_v1_0_to_v1_1_and_rejects_downconvert() {
        let d = detect_or_use_arazzo(None, v1_0_doc()).unwrap();
        let up = d.convert_to(ArazzoVersion::V1_1).unwrap();
        assert_eq!(up.version(), ArazzoVersion::V1_1);
        assert_eq!(up.into_value().unwrap()["arazzo"], "1.1.0");

        // identity (v1.0 → v1.0)
        let d = detect_or_use_arazzo(None, v1_0_doc()).unwrap();
        assert_eq!(
            d.convert_to(ArazzoVersion::V1_0).unwrap().version(),
            ArazzoVersion::V1_0
        );

        // identity (v1.1 → v1.1)
        let d = detect_or_use_arazzo(None, v1_1_doc()).unwrap();
        assert_eq!(
            d.convert_to(ArazzoVersion::V1_1).unwrap().version(),
            ArazzoVersion::V1_1
        );

        // downconvert errors
        let d = detect_or_use_arazzo(None, v1_1_doc()).unwrap();
        let err = d.convert_to(ArazzoVersion::V1_0).unwrap_err();
        assert!(err.to_string().contains("downconversion is not supported"));
    }

    #[test]
    fn detect_or_use_arazzo_honors_forced_version() {
        let d = detect_or_use_arazzo(Some(ArazzoVersion::V1_1), v1_1_doc()).unwrap();
        assert_eq!(d.version(), ArazzoVersion::V1_1);
    }

    #[test]
    fn cli_parses_arazzo_validate() {
        let cli = TestCli::try_parse_from(["roas", "validate", "wf.yaml"]).unwrap();
        assert!(matches!(cli.command, ArazzoCommand::Validate(_)));
    }

    #[test]
    fn cli_parses_arazzo_convert_with_to() {
        let cli = TestCli::try_parse_from(["roas", "convert", "--to", "v1_1", "wf.json"]).unwrap();
        match cli.command {
            ArazzoCommand::Convert(a) => assert_eq!(a.to, ArazzoVersion::V1_1),
            _ => panic!("expected convert"),
        }
    }

    #[test]
    fn cli_rejects_arazzo_convert_without_to() {
        match TestCli::try_parse_from(["roas", "convert", "wf.json"]) {
            Err(e) => assert_eq!(e.kind(), clap::error::ErrorKind::MissingRequiredArgument),
            Ok(_) => panic!("expected a missing-`--to` error"),
        }
    }

    #[test]
    fn arazzo_version_value_enum_aliases_parse() {
        assert_eq!(
            ArazzoVersion::from_str("1.0", true).unwrap(),
            ArazzoVersion::V1_0
        );
        assert_eq!(
            ArazzoVersion::from_str("v1.1", true).unwrap(),
            ArazzoVersion::V1_1
        );
    }

    // --- end-to-end run-function coverage (build args directly; assert
    // Ok/Err since stdout isn't captured here). ---

    #[test]
    fn run_arazzo_validate_ok_with_print_covers_v1_0() {
        let f = TempFile::write("ok.json", &v1_0_doc());
        let args = ArazzoValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: true, // exercises into_value + serialize_spec
        };
        run_arazzo(ArazzoCommand::Validate(args)).expect("valid description must pass");
    }

    #[test]
    fn run_arazzo_validate_ok_covers_v1_1() {
        let f = TempFile::write("ok11.json", &v1_1_doc());
        let args = ArazzoValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: false,
        };
        run_arazzo(ArazzoCommand::Validate(args)).expect("valid v1.1 description must pass");
    }

    #[test]
    fn run_arazzo_validate_reports_single_error() {
        // Valid info + source, but an empty workflows array → one error.
        let doc = json!({
            "arazzo": "1.0.1",
            "info": { "title": "T", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "src", "url": "o.yaml" } ],
            "workflows": []
        });
        let f = TempFile::write("empty-workflows.json", &doc);
        let args = ArazzoValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![],
            print: false,
        };
        let err = run_arazzo(ArazzoCommand::Validate(args)).unwrap_err();
        assert!(
            err.to_string().contains("1 error)"),
            "expected singular error count, got: {err}",
        );
    }

    #[test]
    fn run_arazzo_validate_ignore_suppresses_check() {
        // Empty info title, but `--ignore empty-info-title` clears it; the
        // rest of the doc is valid, so validation passes.
        let doc = json!({
            "arazzo": "1.0.1",
            "info": { "title": "", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "src", "url": "o.yaml" } ],
            "workflows": [
                { "workflowId": "wf", "steps": [ { "stepId": "s", "workflowId": "x" } ] }
            ]
        });
        let f = TempFile::write("ignore.json", &doc);
        let args = ArazzoValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            ignore: vec![ValidationOptions::IgnoreEmptyInfoTitle],
            print: false,
        };
        run_arazzo(ArazzoCommand::Validate(args)).expect("empty title is ignored");
    }

    #[test]
    fn run_arazzo_convert_upconverts_v1_0_to_v1_1() {
        let f = TempFile::write("conv.json", &v1_0_doc());
        let args = ArazzoConvertArgs {
            file: Some(f.0.clone()),
            to: ArazzoVersion::V1_1,
            format: None,
            output_format: Some(InputFormat::Yaml), // exercise output-format override
        };
        run_arazzo(ArazzoCommand::Convert(args)).expect("upconvert must succeed");
    }
}
