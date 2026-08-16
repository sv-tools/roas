//! `roas asyncapi` subcommand group — validate and convert AsyncAPI
//! documents (powered by the `roas-asyncapi` crate).
//!
//! AsyncAPI describes event-driven APIs rather than transforming a
//! spec, so, like [`crate::arazzo`], this group is `validate` and
//! `convert` with no `apply`. The version is detected from the
//! top-level `asyncapi` field.
//!
//! Conversion differs from the other groups in one way that shows in
//! the command: 2.6 → 3.0 is *lossy*. v3 reshaped the document — a
//! channel carries an address instead of being keyed by one, operations
//! moved to a map of their own and changed point of view — so names
//! have to be invented and some things cannot be carried across. The
//! conversion says what it did in a [`ConversionReport`], which this
//! prints to stderr, leaving stdout the converted document alone.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use enumset::EnumSet;
use roas_asyncapi::v3_0::from_v2_6::ConversionReport;
use roas_asyncapi::validation::{Error as AsyncApiError, Validate, ValidationOptions};
use roas_asyncapi::{v2_6, v3_0, v3_1};
use serde_json::Value;
use std::path::PathBuf;

use crate::{InputFormat, read_input, resolve_input_source, serialize_spec};

/// AsyncAPI specification version, mirroring [`crate::arazzo::ArazzoVersion`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum AsyncApiVersion {
    #[value(name = "v2_6", alias = "2.6", alias = "v2.6")]
    V2_6,
    #[value(name = "v3_0", alias = "3.0", alias = "v3.0")]
    V3_0,
    #[value(name = "v3_1", alias = "3.1", alias = "v3.1")]
    V3_1,
}

impl AsyncApiVersion {
    pub(crate) fn label(self) -> &'static str {
        match self {
            AsyncApiVersion::V2_6 => "AsyncAPI 2.6",
            AsyncApiVersion::V3_0 => "AsyncAPI 3.0",
            AsyncApiVersion::V3_1 => "AsyncAPI 3.1",
        }
    }
}

/// A parsed AsyncAPI document tagged with its version.
#[derive(Debug)]
pub(crate) enum DetectedAsyncApi {
    V2_6(Box<v2_6::Document>),
    V3_0(Box<v3_0::Document>),
    V3_1(Box<v3_1::Document>),
}

impl DetectedAsyncApi {
    pub(crate) fn version(&self) -> AsyncApiVersion {
        match self {
            DetectedAsyncApi::V2_6(_) => AsyncApiVersion::V2_6,
            DetectedAsyncApi::V3_0(_) => AsyncApiVersion::V3_0,
            DetectedAsyncApi::V3_1(_) => AsyncApiVersion::V3_1,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        self.version().label()
    }

    pub(crate) fn validate(
        &self,
        options: EnumSet<ValidationOptions>,
    ) -> Result<(), AsyncApiError> {
        match self {
            DetectedAsyncApi::V2_6(d) => d.validate(options),
            DetectedAsyncApi::V3_0(d) => d.validate(options),
            DetectedAsyncApi::V3_1(d) => d.validate(options),
        }
    }

    /// Upconvert to `target`, with what the conversion had to decide or
    /// leave behind. Same-version is the identity and 3.0 → 3.1 is the
    /// `From` impl, so both report nothing; 2.6 → 3.x is lossy and has
    /// something to say. Downconversion is not supported.
    pub(crate) fn convert_to(
        self,
        target: AsyncApiVersion,
    ) -> Result<(DetectedAsyncApi, ConversionReport)> {
        use AsyncApiVersion as To;
        use DetectedAsyncApi as Document;
        Ok(match (self, target) {
            (Document::V2_6(d), To::V2_6) => (Document::V2_6(d), ConversionReport::default()),
            (Document::V2_6(d), To::V3_0) => {
                let (converted, report) = v3_0::from_v2_6::convert(*d);
                (Document::V3_0(Box::new(converted)), report)
            }
            // 3.1 is 3.0's object model, so the 2.6 conversion is the
            // whole of the distance; the last step costs nothing.
            (Document::V2_6(d), To::V3_1) => {
                let (converted, report) = v3_0::from_v2_6::convert(*d);
                (
                    Document::V3_1(Box::new(v3_1::Document::from(converted))),
                    report,
                )
            }
            (Document::V3_0(d), To::V3_0) => (Document::V3_0(d), ConversionReport::default()),
            (Document::V3_0(d), To::V3_1) => (
                Document::V3_1(Box::new(v3_1::Document::from(*d))),
                ConversionReport::default(),
            ),
            (Document::V3_1(d), To::V3_1) => (Document::V3_1(d), ConversionReport::default()),
            (document, target) => bail!(
                "downconversion is not supported: input is {}, target is {}",
                document.label(),
                target.label(),
            ),
        })
    }

    pub(crate) fn into_value(self) -> Result<Value> {
        match self {
            DetectedAsyncApi::V2_6(d) => {
                serde_json::to_value(d).context("serializing AsyncAPI 2.6 document")
            }
            DetectedAsyncApi::V3_0(d) => {
                serde_json::to_value(d).context("serializing AsyncAPI 3.0 document")
            }
            DetectedAsyncApi::V3_1(d) => {
                serde_json::to_value(d).context("serializing AsyncAPI 3.1 document")
            }
        }
    }
}

/// Detect the AsyncAPI version by reading the top-level `asyncapi`
/// field (`"2.6.x"` → v2.6, `"3.0.x"` → v3.0, `"3.1.x"` → v3.1).
pub(crate) fn detect_asyncapi(value: &Value) -> Result<AsyncApiVersion> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("AsyncAPI document must be an object at the top level"))?;
    let asyncapi = obj
        .get("asyncapi")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("could not detect AsyncAPI version: no `asyncapi` field"))?;

    if asyncapi.starts_with("2.6.") {
        Ok(AsyncApiVersion::V2_6)
    } else if asyncapi.starts_with("3.0.") {
        Ok(AsyncApiVersion::V3_0)
    } else if asyncapi.starts_with("3.1.") {
        Ok(AsyncApiVersion::V3_1)
    } else {
        bail!("unsupported AsyncAPI version: {asyncapi} (this tool reads 2.6, 3.0 and 3.1)")
    }
}

/// Detect (or force) the AsyncAPI version and deserialize into the
/// matching typed `Document`.
pub(crate) fn detect_or_use_asyncapi(
    forced: Option<AsyncApiVersion>,
    value: Value,
) -> Result<DetectedAsyncApi> {
    let version = match forced {
        Some(v) => v,
        None => detect_asyncapi(&value)?,
    };
    Ok(match version {
        AsyncApiVersion::V2_6 => DetectedAsyncApi::V2_6(
            serde_json::from_value(value).context("deserializing as AsyncAPI 2.6")?,
        ),
        AsyncApiVersion::V3_0 => DetectedAsyncApi::V3_0(
            serde_json::from_value(value).context("deserializing as AsyncAPI 3.0")?,
        ),
        AsyncApiVersion::V3_1 => DetectedAsyncApi::V3_1(
            serde_json::from_value(value).context("deserializing as AsyncAPI 3.1")?,
        ),
    })
}

#[derive(Subcommand)]
pub(crate) enum AsyncApiCommand {
    /// Parse and validate an AsyncAPI document.
    Validate(AsyncApiValidateArgs),
    /// Upconvert an AsyncAPI document to a newer version.
    Convert(AsyncApiConvertArgs),
}

#[derive(clap::Args)]
pub(crate) struct AsyncApiValidateArgs {
    /// Path to the AsyncAPI file (JSON or YAML). Pass `-`, or omit and
    /// pipe the document, to read from stdin.
    file: Option<PathBuf>,

    /// Override format detection. By default, file paths use the
    /// extension (`.yaml`/`.yml` → YAML, otherwise JSON) and stdin
    /// defaults to JSON.
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Adjust a validation check (repeatable). `empty-info-title`,
    /// `empty-info-version` and `unused-channel-parameter` relax a
    /// check; `external-reference` adds one, requiring the document to
    /// be self-contained. Maps to
    /// `roas_asyncapi::validation::ValidationOptions`.
    #[arg(long, value_enum)]
    check: Vec<ValidationOptions>,

    /// Echo the parsed document to stdout on success, in the input
    /// format (YAML in → YAML out, JSON in → JSON out).
    #[arg(long)]
    print: bool,
}

#[derive(clap::Args)]
pub(crate) struct AsyncApiConvertArgs {
    /// Path to the AsyncAPI file (JSON or YAML). Pass `-`, or omit and
    /// pipe the document, to read from stdin.
    file: Option<PathBuf>,

    /// Target AsyncAPI version. Only upconversion is supported.
    #[arg(long, value_enum)]
    to: AsyncApiVersion,

    /// Override format detection (see `asyncapi validate --format`).
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Output format. Defaults to the input format.
    #[arg(long, value_enum)]
    output_format: Option<InputFormat>,

    /// Fail if converting had to invent a name or leave something
    /// behind, rather than reporting it and exiting 0. Nothing is
    /// written to stdout when it fails.
    #[arg(long)]
    strict: bool,

    /// Do not print the conversion report to stderr.
    #[arg(long)]
    quiet: bool,
}

pub(crate) fn run_asyncapi(cmd: AsyncApiCommand) -> Result<()> {
    match cmd {
        AsyncApiCommand::Validate(args) => run_asyncapi_validate(args),
        AsyncApiCommand::Convert(args) => run_asyncapi_convert(args),
    }
}

fn run_asyncapi_validate(args: AsyncApiValidateArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_asyncapi(None, value)?;

    let mut options = EnumSet::<ValidationOptions>::empty();
    for check in &args.check {
        options |= *check;
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
                "{}: AsyncAPI validation failed ({} error{})",
                source.display(),
                err.errors.len(),
                if err.errors.len() == 1 { "" } else { "s" }
            ))
        }
    }
}

fn run_asyncapi_convert(args: AsyncApiConvertArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_asyncapi(None, value)?;

    let (converted, report) = detected.convert_to(args.to)?;

    // The document goes to stdout, so what the conversion had to say
    // about it goes to stderr — a pipeline keeps working either way.
    if !report.is_clean() && !args.quiet {
        eprintln!(
            "{}: converted with {} note{}",
            source.display(),
            report.notes.len(),
            if report.notes.len() == 1 { "" } else { "s" }
        );
        for note in &report.notes {
            eprintln!("- {note}");
        }
    }
    if args.strict && !report.is_clean() {
        bail!(
            "{}: conversion is not lossless ({} note{}); drop --strict to accept it",
            source.display(),
            report.notes.len(),
            if report.notes.len() == 1 { "" } else { "s" }
        );
    }

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

    /// A minimal `Cli` mirror exercising clap parsing of the asyncapi
    /// subcommand tree in isolation.
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: AsyncApiCommand,
    }

    fn v2_6_doc() -> Value {
        json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1.0.0" },
            "channels": {
                "light/measured": {
                    "publish": {
                        "operationId": "measure",
                        "message": { "name": "Measured", "payload": { "type": "object" } }
                    }
                }
            }
        })
    }

    fn v3_0_doc() -> Value {
        json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1.0.0" },
            "channels": { "measured": { "address": "light/measured" } }
        })
    }

    fn v3_1_doc() -> Value {
        json!({
            "asyncapi": "3.1.0",
            "info": { "title": "T", "version": "1.0.0" },
            "channels": { "measured": { "address": "light/measured" } }
        })
    }

    /// A temp file that cleans itself up on drop (mirrors `arazzo`'s helper).
    struct TempFile(PathBuf);

    impl TempFile {
        fn write(name: &str, value: &Value) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "roas-cli-asyncapi-{}-{n}-{name}",
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
    fn detect_asyncapi_distinguishes_versions() {
        assert_eq!(detect_asyncapi(&v2_6_doc()).unwrap(), AsyncApiVersion::V2_6);
        assert_eq!(detect_asyncapi(&v3_0_doc()).unwrap(), AsyncApiVersion::V3_0);
        assert_eq!(detect_asyncapi(&v3_1_doc()).unwrap(), AsyncApiVersion::V3_1);
    }

    #[test]
    fn detect_asyncapi_rejects_missing_or_unknown_version() {
        let err = detect_asyncapi(&json!({ "info": {} })).unwrap_err();
        assert!(err.to_string().contains("no `asyncapi` field"));
        // 2.0 through 2.5 are AsyncAPI, but not versions this reads.
        let err = detect_asyncapi(&json!({ "asyncapi": "2.0.0" })).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported AsyncAPI version: 2.0.0"),
            "got: {err}"
        );
        let err = detect_asyncapi(&json!("not an object")).unwrap_err();
        assert!(err.to_string().contains("object at the top level"));
    }

    #[test]
    fn convert_upconverts_and_rejects_downconvert() {
        // 2.6 → 3.0, which is the lossy one and says so.
        let d = detect_or_use_asyncapi(None, v2_6_doc()).unwrap();
        let (up, report) = d.convert_to(AsyncApiVersion::V3_0).unwrap();
        assert_eq!(up.version(), AsyncApiVersion::V3_0);
        assert_eq!(up.into_value().unwrap()["asyncapi"], "3.0.0");
        assert!(!report.is_clean(), "a 2.6 channel key has to be invented");

        // 2.6 → 3.1 goes the same way and then one step further.
        let d = detect_or_use_asyncapi(None, v2_6_doc()).unwrap();
        let (up, report) = d.convert_to(AsyncApiVersion::V3_1).unwrap();
        assert_eq!(up.into_value().unwrap()["asyncapi"], "3.1.0");
        assert!(!report.is_clean());

        // 3.0 → 3.1 is the object model unchanged, so nothing to report.
        let d = detect_or_use_asyncapi(None, v3_0_doc()).unwrap();
        let (up, report) = d.convert_to(AsyncApiVersion::V3_1).unwrap();
        assert_eq!(up.into_value().unwrap()["asyncapi"], "3.1.0");
        assert!(report.is_clean());

        // identities
        for (document, version) in [
            (v2_6_doc(), AsyncApiVersion::V2_6),
            (v3_0_doc(), AsyncApiVersion::V3_0),
            (v3_1_doc(), AsyncApiVersion::V3_1),
        ] {
            let d = detect_or_use_asyncapi(None, document).unwrap();
            let (same, report) = d.convert_to(version).unwrap();
            assert_eq!(same.version(), version);
            assert!(report.is_clean());
        }

        // downconversion errors, whichever way round
        for (document, target, expected) in [
            (v3_1_doc(), AsyncApiVersion::V3_0, "input is AsyncAPI 3.1"),
            (v3_0_doc(), AsyncApiVersion::V2_6, "target is AsyncAPI 2.6"),
            (v3_1_doc(), AsyncApiVersion::V2_6, "input is AsyncAPI 3.1"),
        ] {
            let d = detect_or_use_asyncapi(None, document).unwrap();
            let err = d.convert_to(target).unwrap_err();
            assert!(
                err.to_string().contains("downconversion is not supported")
                    && err.to_string().contains(expected),
                "got: {err}"
            );
        }
    }

    #[test]
    fn detect_or_use_asyncapi_honors_forced_version() {
        let d = detect_or_use_asyncapi(Some(AsyncApiVersion::V3_1), v3_1_doc()).unwrap();
        assert_eq!(d.version(), AsyncApiVersion::V3_1);
        // Forcing a version the document is not still has to parse as
        // that version, and 2.6 pins its own `asyncapi` string.
        let err = detect_or_use_asyncapi(Some(AsyncApiVersion::V2_6), v3_0_doc()).unwrap_err();
        assert!(
            err.to_string().contains("deserializing as AsyncAPI 2.6"),
            "got: {err}"
        );
    }

    #[test]
    fn cli_parses_asyncapi_validate() {
        let cli = TestCli::try_parse_from(["roas", "validate", "events.yaml"]).unwrap();
        assert!(matches!(cli.command, AsyncApiCommand::Validate(_)));
    }

    #[test]
    fn cli_parses_asyncapi_validate_checks() {
        let cli = TestCli::try_parse_from([
            "roas",
            "validate",
            "--check",
            "external-reference",
            "--check",
            "empty-info-title",
            "events.yaml",
        ])
        .unwrap();
        match cli.command {
            AsyncApiCommand::Validate(a) => assert_eq!(
                a.check,
                vec![
                    ValidationOptions::ErrorOnExternalReference,
                    ValidationOptions::IgnoreEmptyInfoTitle,
                ]
            ),
            AsyncApiCommand::Convert(_) => panic!("expected validate"),
        }
    }

    #[test]
    fn cli_parses_asyncapi_convert_with_to() {
        let cli =
            TestCli::try_parse_from(["roas", "convert", "--to", "v3_1", "events.json"]).unwrap();
        match cli.command {
            AsyncApiCommand::Convert(a) => {
                assert_eq!(a.to, AsyncApiVersion::V3_1);
                assert!(!a.strict && !a.quiet, "both default to off");
            }
            AsyncApiCommand::Validate(_) => panic!("expected convert"),
        }
    }

    #[test]
    fn cli_rejects_asyncapi_convert_without_to() {
        match TestCli::try_parse_from(["roas", "convert", "events.json"]) {
            Err(e) => assert_eq!(e.kind(), clap::error::ErrorKind::MissingRequiredArgument),
            Ok(_) => panic!("expected a missing-`--to` error"),
        }
    }

    #[test]
    fn asyncapi_version_value_enum_aliases_parse() {
        assert_eq!(
            AsyncApiVersion::from_str("2.6", true).unwrap(),
            AsyncApiVersion::V2_6
        );
        assert_eq!(
            AsyncApiVersion::from_str("v3.0", true).unwrap(),
            AsyncApiVersion::V3_0
        );
        assert_eq!(
            AsyncApiVersion::from_str("v3_1", true).unwrap(),
            AsyncApiVersion::V3_1
        );
    }

    // --- end-to-end run-function coverage (build args directly; assert
    // Ok/Err since stdout isn't captured here). ---

    #[test]
    fn run_asyncapi_validate_ok_with_print_covers_v2_6() {
        let f = TempFile::write("ok26.json", &v2_6_doc());
        let args = AsyncApiValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            check: vec![],
            print: true, // exercises into_value + serialize_spec
        };
        run_asyncapi(AsyncApiCommand::Validate(args)).expect("valid document must pass");
    }

    #[test]
    fn run_asyncapi_validate_ok_covers_v3_0_and_v3_1() {
        for (name, document) in [("ok30.json", v3_0_doc()), ("ok31.json", v3_1_doc())] {
            let f = TempFile::write(name, &document);
            let args = AsyncApiValidateArgs {
                file: Some(f.0.clone()),
                format: None,
                check: vec![],
                print: false,
            };
            run_asyncapi(AsyncApiCommand::Validate(args)).expect("valid document must pass");
        }
    }

    #[test]
    fn run_asyncapi_validate_reports_single_error() {
        // Everything else is valid, so the missing channel address is
        // the only complaint.
        let document = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "T", "version": "1.0.0" },
            "channels": { "c": { "address": "a/{p}" } }
        });
        let f = TempFile::write("unused-parameter.json", &document);
        let args = AsyncApiValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            check: vec![],
            print: false,
        };
        let err = run_asyncapi(AsyncApiCommand::Validate(args)).unwrap_err();
        assert!(
            err.to_string().contains("1 error)"),
            "expected singular error count, got: {err}",
        );
    }

    #[test]
    fn run_asyncapi_validate_check_adjusts_the_check() {
        // An empty title is an error until the check is relaxed.
        let document = json!({
            "asyncapi": "3.0.0",
            "info": { "title": "", "version": "1.0.0" },
            "channels": { "c": { "address": "a" } }
        });
        let f = TempFile::write("empty-title.json", &document);
        let strict = AsyncApiValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            check: vec![],
            print: false,
        };
        run_asyncapi(AsyncApiCommand::Validate(strict)).unwrap_err();
        let relaxed = AsyncApiValidateArgs {
            file: Some(f.0.clone()),
            format: None,
            check: vec![ValidationOptions::IgnoreEmptyInfoTitle],
            print: false,
        };
        run_asyncapi(AsyncApiCommand::Validate(relaxed)).expect("empty title is allowed");
    }

    #[test]
    fn run_asyncapi_convert_upconverts_v2_6_to_v3_0() {
        let f = TempFile::write("conv.json", &v2_6_doc());
        let args = AsyncApiConvertArgs {
            file: Some(f.0.clone()),
            to: AsyncApiVersion::V3_0,
            format: None,
            output_format: Some(InputFormat::Yaml), // exercise output-format override
            strict: false,
            quiet: false, // exercise the report going to stderr
        };
        run_asyncapi(AsyncApiCommand::Convert(args)).expect("upconvert must succeed");
    }

    #[test]
    fn run_asyncapi_convert_strict_refuses_a_lossy_conversion() {
        let f = TempFile::write("strict.json", &v2_6_doc());
        let args = AsyncApiConvertArgs {
            file: Some(f.0.clone()),
            to: AsyncApiVersion::V3_0,
            format: None,
            output_format: None,
            strict: true,
            quiet: true, // the refusal says it; the notes need not
        };
        let err = run_asyncapi(AsyncApiCommand::Convert(args)).unwrap_err();
        assert!(
            err.to_string().contains("conversion is not lossless"),
            "got: {err}"
        );

        // A conversion with nothing to report passes --strict.
        let f = TempFile::write("strict-ok.json", &v3_0_doc());
        let args = AsyncApiConvertArgs {
            file: Some(f.0.clone()),
            to: AsyncApiVersion::V3_1,
            format: None,
            output_format: None,
            strict: true,
            quiet: false,
        };
        run_asyncapi(AsyncApiCommand::Convert(args)).expect("nothing was lost");
    }
}
