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
use roas_arazzo_executor::{Client, Options, execute};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{
    InputFormat, InputSource, LoaderKind, build_loader, read_input, resolve_input_source,
    serialize_spec,
};

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
    /// Run a workflow: perform every step's request and report what
    /// happened.
    Run(ArazzoRunArgs),
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

#[derive(clap::Args)]
pub(crate) struct ArazzoRunArgs {
    /// Path to the Arazzo file (JSON or YAML). Pass `-`, or omit and
    /// pipe the description, to read from stdin.
    file: Option<PathBuf>,

    /// The workflow to run. Defaults to the description's first.
    #[arg(long, value_name = "ID")]
    workflow: Option<String>,

    /// A workflow input, e.g. `--input petId=7` (repeatable). The value
    /// is read as JSON where it is JSON, and as a string otherwise, so
    /// `--input n=7` gives a number and `--input n=seven` a string.
    #[arg(long, value_name = "NAME=VALUE")]
    input: Vec<String>,

    /// A file of inputs — a JSON or YAML object. Anything `--input`
    /// names as well wins over it.
    #[arg(long, value_name = "FILE")]
    inputs: Option<PathBuf>,

    /// A source description document, e.g.
    /// `--source petStore=./openapi.yaml` (repeatable). Without this,
    /// `--load` fetches what the description points at.
    #[arg(long, value_name = "NAME=PATH")]
    source: Vec<String>,

    /// Send a source description's requests somewhere else, e.g.
    /// `--base-url petStore=http://127.0.0.1:8080` (repeatable) —
    /// whatever its document says.
    #[arg(long, value_name = "NAME=URL")]
    base_url: Vec<String>,

    /// A header for every request a step does not set itself, e.g.
    /// `--header 'Authorization: Bearer …'` (repeatable).
    #[arg(long, value_name = "NAME: VALUE")]
    header: Vec<String>,

    /// Fetch the source descriptions the run needs, by the URLs the
    /// description gives them. Same shape as `roas validate --load`:
    /// `--load file` for `file://` (and paths beside the description),
    /// `--load http` for `http(s)://`; repeat to combine.
    #[arg(long, value_enum)]
    load: Vec<LoaderKind>,

    /// Stop the run after this many steps, in case a `goto` loops.
    #[arg(long, value_name = "N")]
    max_steps: Option<usize>,

    /// Do not print the report to stderr.
    #[arg(long)]
    quiet: bool,

    /// Override format detection (see `arazzo validate --format`).
    #[arg(long, value_enum)]
    format: Option<InputFormat>,

    /// Format for the outputs printed to stdout. Defaults to the input
    /// format.
    #[arg(long, value_enum)]
    output_format: Option<InputFormat>,
}

pub(crate) fn run_arazzo(cmd: ArazzoCommand) -> Result<()> {
    match cmd {
        ArazzoCommand::Validate(args) => run_arazzo_validate(args),
        ArazzoCommand::Convert(args) => run_arazzo_convert(args),
        ArazzoCommand::Run(args) => run_arazzo_run(args),
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

fn run_arazzo_run(args: ArazzoRunArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let (value, input_format) = read_input(&source, args.format)?;
    let detected = detect_or_use_arazzo(None, value)?;
    // One interpreter: a v1.0 description is upconverted first.
    let description = match detected {
        DetectedArazzo::V1_1(description) => description,
        DetectedArazzo::V1_0(description) => v1_1::Description::from(description),
    };

    let mut options = Options::new();
    if let Some(workflow) = &args.workflow {
        options = options.workflow(workflow);
    }
    if let Some(path) = &args.inputs {
        let (inputs, _) = read_input(&InputSource::File(path.clone()), None)
            .with_context(|| format!("reading inputs {}", path.display()))?;
        if !inputs.is_object() {
            bail!("{}: inputs must be an object", path.display());
        }
        options = options.inputs(inputs);
    }
    for input in &args.input {
        let (name, value) = split_pair(input, "--input")?;
        // JSON where it is JSON, so `n=7` is a number and `n=seven` is
        // the word.
        let value: Value =
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
        options = options.input(name, value);
    }
    for base_url in &args.base_url {
        let (name, url) = split_pair(base_url, "--base-url")?;
        options = options.base_url(name, url);
    }
    for header in &args.header {
        let (name, value) = header
            .split_once(':')
            .ok_or_else(|| anyhow!("`--header` wants `Name: value`, got `{header}`"))?;
        options = options.header(name.trim(), value.trim());
    }
    if let Some(max_steps) = args.max_steps {
        options = options.max_steps(max_steps);
    }
    let (options, any) = sources(options, &description, &source, &args)?;

    let report = execute(&description, &options, &mut Client::blocking()).map_err(|error| {
        if any || description.source_descriptions.is_empty() {
            anyhow!(error)
        } else {
            // The executor names what it could not find; the reason it
            // has nothing to find it in belongs here.
            anyhow!(
                "{error}\nno source description was supplied — pass `--source <name>=<path>`, \
                 or `--load file` / `--load http` to fetch what this description points at"
            )
        }
    })?;

    if !args.quiet {
        eprint!("{report}");
    }
    let outputs = serde_json::to_value(&report.outputs).context("serializing the outputs")?;
    let out_format = args.output_format.unwrap_or(input_format);
    print!("{}", serialize_spec(&outputs, out_format, true)?);

    if report.is_success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{}: workflow `{}` failed",
            source.display(),
            report.workflow_id
        ))
    }
}

/// The source descriptions the run is given: those named on the command
/// line, then — where `--load` allows it — those the description points
/// at itself.
fn sources(
    mut options: Options,
    description: &v1_1::Description,
    from: &InputSource,
    args: &ArazzoRunArgs,
) -> Result<(Options, bool)> {
    let mut supplied = BTreeMap::new();
    for source in &args.source {
        let (name, path) = split_pair(source, "--source")?;
        let (document, _) = read_input(&InputSource::File(PathBuf::from(path)), None)
            .with_context(|| format!("reading source description {path}"))?;
        supplied.insert(name.to_owned(), document);
    }

    let mut loader = build_loader(&args.load);
    let mut any = false;
    for declared in &description.source_descriptions {
        let url = declared.url.clone();
        let document = match supplied.remove(&declared.name) {
            Some(document) => document,
            None => {
                let Some(loader) = loader.as_mut() else {
                    // Nothing to load it with. The executor says so if a
                    // step turns out to need it, naming the source.
                    continue;
                };
                let uri = beside(&url, from);
                loader
                    .load_resource(&uri)
                    .with_context(|| {
                        format!("loading source description `{}` from {uri}", declared.name)
                    })?
                    .clone()
            }
        };
        options = options.source(declared.name.clone(), url, document);
        any = true;
    }
    // A `--source` for something the description does not declare is
    // more likely a typo than a spare.
    if let Some((name, _)) = supplied.into_iter().next() {
        bail!("`--source {name}=…` names no source description of this document");
    }
    Ok((options, any))
}

/// A source description's URL as something the loader can fetch: an
/// absolute URL as it stands, a relative one from beside the document
/// that named it.
fn beside(url: &str, from: &InputSource) -> String {
    if url.contains("://") {
        return url.to_owned();
    }
    let path = match from {
        InputSource::File(path) => path.parent().unwrap_or(Path::new(".")).join(url),
        // Nothing to be beside: read it from the working directory.
        InputSource::Stdin => PathBuf::from(url),
    };
    let absolute = path
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&path));
    format!("file://{}", absolute.display())
}

/// `name=value`, which is how the repeatable flags are written.
fn split_pair<'a>(pair: &'a str, flag: &str) -> Result<(&'a str, &'a str)> {
    pair.split_once('=')
        .ok_or_else(|| anyhow!("`{flag}` wants `name=value`, got `{pair}`"))
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

    /// A server that answers a fixed number of requests, then stops.
    /// The `run` command really sends requests, so something has to
    /// really answer them.
    fn server(
        count: usize,
        status: u16,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let join = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                asked.push(line);
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
            asked
        });
        (base, join)
    }

    fn runnable() -> (TempFile, TempFile) {
        let openapi = TempFile::write(
            "openapi.json",
            &json!({
                "openapi": "3.0.3",
                "servers": [{ "url": "https://api.example.com/v1" }],
                "paths": { "/pets/{petId}": { "get": { "operationId": "getPetById" } } }
            }),
        );
        let description = TempFile::write(
            "run.json",
            &json!({
                "arazzo": "1.1.0",
                "info": { "title": "T", "version": "1.0.0" },
                "sourceDescriptions": [
                    { "name": "petStore", "url": "https://api.example.com/openapi.json",
                      "type": "openapi" }
                ],
                "workflows": [{
                    "workflowId": "buyPet",
                    "steps": [{
                        "stepId": "findPet",
                        "operationId": "getPetById",
                        "parameters": [{ "name": "petId", "in": "path", "value": "$inputs.petId" }],
                        "successCriteria": [{ "condition": "$statusCode == 200" }],
                        "outputs": { "name": "$response.body#/name" }
                    }],
                    "outputs": { "petName": "$steps.findPet.outputs.name" }
                }]
            }),
        );
        (description, openapi)
    }

    fn run_args(description: &TempFile, openapi: &TempFile, base: &str) -> ArazzoRunArgs {
        ArazzoRunArgs {
            file: Some(description.0.clone()),
            workflow: Some("buyPet".to_owned()),
            input: vec!["petId=7".to_owned()],
            inputs: None,
            source: vec![format!("petStore={}", openapi.0.display())],
            base_url: vec![format!("petStore={base}")],
            header: vec!["Authorization: Bearer abc".to_owned()],
            load: Vec::new(),
            max_steps: Some(20),
            quiet: false,
            format: None,
            output_format: Some(InputFormat::Json),
        }
    }

    #[test]
    fn run_performs_the_workflows_requests() {
        let (description, openapi) = runnable();
        let (base, join) = server(1, 200, r#"{"id":7,"name":"fluffy"}"#);

        run_arazzo(ArazzoCommand::Run(run_args(&description, &openapi, &base)))
            .expect("the workflow runs");

        let asked = join.join().expect("the server thread");
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("GET /pets/7 "), "{}", asked[0]);
    }

    #[test]
    fn run_fails_when_the_workflow_does() {
        let (description, openapi) = runnable();
        let (base, join) = server(1, 500, r#"{"error":"gone"}"#);

        let error =
            run_arazzo(ArazzoCommand::Run(run_args(&description, &openapi, &base))).unwrap_err();

        assert!(
            error.to_string().contains("workflow `buyPet` failed"),
            "the exit status follows the workflow: {error}"
        );
        let _ = join.join();
    }

    #[test]
    fn run_reads_inputs_from_a_file_and_the_command_line() {
        let (description, openapi) = runnable();
        let (base, join) = server(1, 200, r#"{"id":9,"name":"rex"}"#);
        let inputs = TempFile::write("inputs.json", &json!({ "petId": "1" }));
        let mut args = run_args(&description, &openapi, &base);
        args.inputs = Some(inputs.0.clone());
        // `--input` is named as well, so it wins over the file.
        args.input = vec!["petId=9".to_owned()];
        args.quiet = true;

        run_arazzo(ArazzoCommand::Run(args)).expect("the workflow runs");

        let asked = join.join().expect("the server thread");
        assert!(asked[0].starts_with("GET /pets/9 "), "{}", asked[0]);
    }

    #[test]
    fn run_refuses_what_it_cannot_make_sense_of() {
        let (description, openapi) = runnable();
        for (change, expected) in [
            (
                Box::new(|args: &mut ArazzoRunArgs| args.input = vec!["petId".to_owned()])
                    as Box<dyn Fn(&mut ArazzoRunArgs)>,
                "`--input` wants `name=value`",
            ),
            (
                Box::new(|args: &mut ArazzoRunArgs| args.header = vec!["nocolon".to_owned()]),
                "`--header` wants `Name: value`",
            ),
            (
                // A real document, under a name this description does
                // not declare — more likely a typo than a spare.
                Box::new(|args: &mut ArazzoRunArgs| {
                    args.source = args
                        .source
                        .iter()
                        .map(|source| source.replace("petStore=", "nope="))
                        .collect();
                }),
                "names no source description",
            ),
        ] {
            let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
            args.quiet = true;
            change(&mut args);
            let error = run_arazzo(ArazzoCommand::Run(args)).unwrap_err();
            assert!(
                format!("{error:#}").contains(expected),
                "expected {expected:?}, got: {error:#}"
            );
        }
    }

    #[test]
    fn cli_parses_arazzo_run_with_everything_it_takes() {
        let cli = TestCli::try_parse_from([
            "roas",
            "run",
            "--workflow",
            "buyPet",
            "--input",
            "petId=7",
            "--source",
            "petStore=./openapi.yaml",
            "--base-url",
            "petStore=http://127.0.0.1:8080",
            "--header",
            "Authorization: Bearer abc",
            "--load",
            "file",
            "--max-steps",
            "50",
            "--quiet",
            "wf.yaml",
        ])
        .unwrap();
        match cli.command {
            ArazzoCommand::Run(a) => {
                assert_eq!(a.workflow.as_deref(), Some("buyPet"));
                assert_eq!(a.input, ["petId=7"]);
                assert_eq!(a.source, ["petStore=./openapi.yaml"]);
                assert_eq!(a.base_url, ["petStore=http://127.0.0.1:8080"]);
                assert_eq!(a.header, ["Authorization: Bearer abc"]);
                assert_eq!(a.max_steps, Some(50));
                assert!(a.quiet);
            }
            _ => panic!("expected run"),
        }
    }

    #[test]
    fn a_pair_is_split_where_the_flag_says() {
        assert_eq!(split_pair("petId=7", "--input").unwrap(), ("petId", "7"));
        // A value may hold `=` of its own; only the first one splits.
        assert_eq!(split_pair("q=a=b", "--input").unwrap(), ("q", "a=b"));
        let error = split_pair("petId", "--input").unwrap_err();
        assert_eq!(
            error.to_string(),
            "`--input` wants `name=value`, got `petId`"
        );
    }

    #[test]
    fn a_relative_source_url_is_read_from_beside_the_document() {
        let from = InputSource::File(PathBuf::from("/tmp/flows/buy.arazzo.yaml"));
        assert_eq!(
            beside("https://api.example.com/openapi.json", &from),
            "https://api.example.com/openapi.json",
            "an absolute URL is left alone"
        );
        let beside_it = beside("./petstore.yaml", &from);
        assert!(beside_it.starts_with("file://"), "{beside_it}");
        assert!(beside_it.ends_with("petstore.yaml"), "{beside_it}");
        assert!(beside_it.contains("flows"), "{beside_it}");
    }

    #[test]
    fn run_reports_a_workflow_that_could_not_be_run() {
        // No source documents and no `--load`, so the step's operation
        // is nowhere to be found — and the reason is said as well.
        let f = TempFile::write("run.json", &v1_0_doc());
        let args = ArazzoRunArgs {
            file: Some(f.0.clone()),
            workflow: None,
            input: vec!["petId=7".to_owned()],
            inputs: None,
            source: Vec::new(),
            base_url: Vec::new(),
            header: Vec::new(),
            load: Vec::new(),
            max_steps: None,
            quiet: true,
            format: None,
            output_format: None,
        };
        let error = run_arazzo(ArazzoCommand::Run(args)).unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("no source description was supplied"),
            "got: {error}"
        );
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
