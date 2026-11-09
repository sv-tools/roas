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
use roas::LoadedDocument;
use roas::loader::{Loader, LoaderError, ResourceFetcher};
use roas_arazzo::source_map::SourceMap;
use roas_arazzo::validation::{Error as ArazzoError, Validate, ValidationError, ValidationOptions};
use roas_arazzo::{v1_0, v1_1};
use roas_arazzo_executor::{
    Client, Options, SourceLoadOptions, SourceRegistry, prepare, required_sources,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use url::Url;

use crate::{
    Fetcher, InputFormat, InputSource, LoaderKind, build_loader_with, read_input, read_input_text,
    resolve_input_source, serialize_spec,
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
/// matching typed `Description`. `text` is what `value` was read from: an
/// error names the field that did not fit, and where it is.
pub(crate) fn detect_or_use_arazzo(
    forced: Option<ArazzoVersion>,
    value: Value,
    text: &str,
) -> Result<DetectedArazzo> {
    let version = match forced {
        Some(v) => v,
        None => detect_arazzo(&value)?,
    };
    Ok(match version {
        ArazzoVersion::V1_0 => DetectedArazzo::V1_0(typed(value, text, version)?),
        ArazzoVersion::V1_1 => DetectedArazzo::V1_1(typed(value, text, version)?),
    })
}

/// An Arazzo description read from the command line, with the text it
/// was read from.
pub(crate) struct ReadArazzo {
    pub(crate) detected: DetectedArazzo,
    /// The parsed document, kept only when asked for (`run` loads its
    /// sources from it).
    pub(crate) value: Option<Value>,
    pub(crate) format: InputFormat,
    pub(crate) text: String,
}

impl ReadArazzo {
    /// Where the description's fields are; `None` when the text cannot be
    /// mapped, and findings then name paths only.
    pub(crate) fn source_map(&self) -> Option<SourceMap> {
        SourceMap::parse(self.text.as_str()).ok()
    }
}

/// Read, detect and deserialize an Arazzo description. A description that
/// does not fit the model names the field that did not, and where it is.
pub(crate) fn read_arazzo(source: &InputSource, format: Option<InputFormat>) -> Result<ReadArazzo> {
    read_arazzo_as(source, format, false)
}

/// [`read_arazzo`], keeping the parsed document when `keep` is set.
fn read_arazzo_as(
    source: &InputSource,
    format: Option<InputFormat>,
    keep: bool,
) -> Result<ReadArazzo> {
    let (text, value, format) = read_input_text(source, format)?;
    let kept = keep.then(|| value.clone());
    let detected = detect_or_use_arazzo(None, value, &text)?;
    Ok(ReadArazzo {
        detected,
        value: kept,
        format,
        text,
    })
}

/// Deserialize `value`, tracking the path: an error names the deepest
/// field serde reached, located in `text` when the map has it.
fn typed<T: DeserializeOwned>(value: Value, text: &str, version: ArazzoVersion) -> Result<T> {
    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = model_path(error.path());
        let at = SourceMap::parse(text)
            .ok()
            .and_then(|map| map.locate(&path))
            .map(|range| format!(" at {}", range.start))
            .unwrap_or_default();
        anyhow!(
            "deserializing as {}: {path}{at}: {}",
            version.label(),
            error.inner()
        )
    })
}

/// A serde path in the model's notation: `#.workflows[0].workflowId`. It
/// stops at a segment serde could not name, keeping the part it knows.
fn model_path(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut model = String::from("#");
    for segment in path.iter() {
        match segment {
            Segment::Seq { index } => model.push_str(&format!("[{index}]")),
            // An externally tagged variant is a member named after it.
            Segment::Map { key } | Segment::Enum { variant: key } => {
                model.push('.');
                model.push_str(key);
            }
            Segment::Unknown => break,
        }
    }
    model
}

/// A validation finding, with its line and column when the map has them:
/// those of the member name for a finding about the name, else the value.
fn located(map: Option<&SourceMap>, error: &ValidationError) -> String {
    match map.and_then(|map| {
        if error.is_about_key() {
            map.locate_key(&error.path)
        } else {
            map.locate(&error.path)
        }
    }) {
        Some(range) => format!("{} at {}: {}", error.path, range.start, error.message),
        None => error.to_string(),
    }
}

#[derive(Subcommand)]
pub(crate) enum ArazzoCommand {
    /// Parse and validate an Arazzo description.
    Validate(ArazzoValidateArgs),
    /// Upconvert an Arazzo description to a newer version.
    Convert(ArazzoConvertArgs),
    /// Run a workflow: perform every step's request and report what
    /// happened.
    Run(Box<ArazzoRunArgs>),
    /// List the workflows a description offers, and what each one takes.
    List(ArazzoListArgs),
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
pub(crate) struct ArazzoListArgs {
    /// Path to the Arazzo file (JSON or YAML). Pass `-`, or omit and
    /// pipe the description, to read from stdin.
    file: Option<PathBuf>,

    /// Override format detection (see `arazzo validate --format`).
    #[arg(long, value_enum)]
    format: Option<InputFormat>,
}

#[derive(clap::Args)]
pub(crate) struct ArazzoRunArgs {
    /// Path to the Arazzo file (JSON or YAML). Pass `-`, or omit and
    /// pipe the description, to read from stdin.
    file: Option<PathBuf>,

    /// The workflow to run. Required where the description offers more
    /// than one — `arazzo list` says what it offers.
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

    /// Disable JSON Schema validation of workflow inputs (no coercion is done either way).
    #[arg(long)]
    skip_input_validation: bool,

    /// Supply an offline input-schema resource, e.g. `--schema https://example.com/input.json=./input.json`.
    /// Repeat for every external resource; --load never fetches input schemas implicitly.
    #[arg(long, value_name = "URI=FILE")]
    schema: Vec<String>,

    /// A source description document, e.g.
    /// `--source petStore=./openapi.yaml` (repeatable). Without this,
    /// `--load` fetches what the description points at.
    #[arg(long, value_name = "NAME=PATH")]
    source: Vec<String>,

    /// Preload a source, schema or complete Path Item reference document without a root alias.
    #[arg(long, value_name = "FILE")]
    source_document: Vec<PathBuf>,

    /// Traverse every declared source, including linked Arazzo documents.
    /// Fetching still requires --load; unrelated failures are reported as diagnostics.
    #[arg(long)]
    load_all_sources: bool,

    /// Source-graph document/fetch-attempt budget, separate from workflow limits.
    #[arg(long, default_value_t = 256, value_name = "N")]
    source_max_documents: usize,

    /// Source-graph depth budget (entry document is depth zero).
    #[arg(long, default_value_t = 32, value_name = "N")]
    source_max_depth: usize,

    /// Allow Arazzo retrieval aliases instead of canonical $self identities.
    #[arg(long)]
    allow_source_retrieval_aliases: bool,

    /// Send a source description's requests somewhere else, e.g.
    /// `--base-url petStore=http://127.0.0.1:8080` (repeatable) —
    /// whatever its document says. A dotted name reaches a source of an
    /// external Arazzo document by its alias path, e.g.
    /// `--base-url shared.petStore=…` for the `petStore` source of the
    /// document the entry calls `shared`; it applies to that document only.
    #[arg(long, value_name = "NAME=URL")]
    base_url: Vec<String>,

    /// A header for every request a step does not set itself, e.g.
    /// `--header 'Authorization: Bearer …'` (repeatable).
    #[arg(long, value_name = "NAME: VALUE")]
    header: Vec<String>,

    /// Fetch the source descriptions the run needs, by the URLs the
    /// description gives them. Same shape as `roas openapi validate --load`:
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

    /// Skip a validation check before running (repeatable), as
    /// `arazzo validate --ignore` does. The description is validated
    /// first either way: a run makes real requests, and a description
    /// that does not hold together should not make them.
    #[arg(long, value_enum)]
    ignore: Vec<ValidationOptions>,

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
        ArazzoCommand::Run(args) => run_arazzo_run(*args),
        ArazzoCommand::List(args) => run_arazzo_list(args),
    }
}

fn run_arazzo_validate(args: ArazzoValidateArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let ReadArazzo {
        detected,
        format: input_format,
        text,
        ..
    } = read_arazzo(&source, args.format)?;

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
            // Built only when there is something to locate.
            let map = SourceMap::parse(text).ok();
            for e in &err.errors {
                eprintln!("- {}", located(map.as_ref(), e));
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
    let ReadArazzo {
        detected,
        format: input_format,
        ..
    } = read_arazzo(&source, args.format)?;

    let converted = detected.convert_to(args.to)?;
    let value = converted.into_value()?;
    let out_format = args.output_format.unwrap_or(input_format);
    print!("{}", serialize_spec(&value, out_format, true)?);
    Ok(())
}

fn run_arazzo_list(args: ArazzoListArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let detected = read_arazzo(&source, args.format)?.detected;
    let description = match detected {
        DetectedArazzo::V1_1(description) => description,
        DetectedArazzo::V1_0(description) => v1_1::Description::from(description),
    };

    for workflow in &description.workflows {
        println!("{}", workflow.workflow_id);
        if let Some(summary) = workflow
            .summary
            .as_deref()
            .or(workflow.description.as_deref())
        {
            println!("  summary: {}", summary.lines().next().unwrap_or_default());
        }
        if !workflow.depends_on.is_empty() {
            println!("  depends on: {}", workflow.depends_on.join(", "));
        }
        let inputs = inputs_of(workflow);
        if !inputs.is_empty() {
            println!("  inputs: {}", inputs.join(", "));
        }
        println!(
            "  steps: {} ({})",
            workflow.steps.len(),
            workflow
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// The inputs a workflow takes, read off the JSON Schema it describes
/// them with. Required ones say so; a schema this cannot read at all
/// simply contributes nothing.
fn inputs_of(workflow: &v1_1::Workflow) -> Vec<String> {
    let Some(schema) = &workflow.inputs else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, property)| {
                    let type_ = property.get("type").and_then(Value::as_str);
                    match (type_, required.contains(&name.as_str())) {
                        (Some(type_), true) => format!("{name} ({type_}, required)"),
                        (Some(type_), false) => format!("{name} ({type_})"),
                        (None, true) => format!("{name} (required)"),
                        (None, false) => name.clone(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn run_arazzo_run(args: ArazzoRunArgs) -> Result<()> {
    let source = resolve_input_source(args.file.as_deref())?;
    let read = read_arazzo_as(&source, args.format, true)?;
    let map = read.source_map();
    let ReadArazzo {
        detected,
        value,
        format: input_format,
        ..
    } = read;
    let value = value.expect("kept for `run`");
    // One interpreter: a v1.0 description is upconverted first.
    let description = match detected {
        DetectedArazzo::V1_1(description) => description,
        DetectedArazzo::V1_0(description) => v1_1::Description::from(description),
    };

    // Before a single request: a description with two workflows of the
    // same id, or a step that names nothing, has no business reaching
    // an API.
    let mut checks = EnumSet::<ValidationOptions>::empty();
    for ignore in &args.ignore {
        checks |= *ignore;
    }
    if let Err(errors) = description.validate(checks) {
        for error in &errors.errors {
            eprintln!("- {}", located(map.as_ref(), error));
        }
        return Err(anyhow!(
            "{}: the description does not validate ({} error{}), so nothing was run",
            source.display(),
            errors.errors.len(),
            if errors.errors.len() == 1 { "" } else { "s" }
        ));
    }

    let mut options =
        Options::new()
            .validation_options(checks)
            .input_validation(if args.skip_input_validation {
                roas_arazzo_executor::InputValidation::Disabled
            } else {
                roas_arazzo_executor::InputValidation::Draft202012
            });
    if let Some(map) = map {
        options = options.source_map(map);
    }
    for schema in &args.schema {
        let (uri, path) = split_pair(schema, "--schema")?;
        let (document, _) = read_input(&InputSource::File(PathBuf::from(path)), None)
            .with_context(|| format!("reading input schema {path}"))?;
        options = options.schema_document(uri, document)?;
    }
    match &args.workflow {
        Some(workflow) => options = options.workflow(workflow),
        // Running a workflow means real requests against a real API, so
        // which one is not a choice to make on someone's behalf. One
        // workflow is no choice at all.
        None if description.workflows.len() > 1 => {
            let ids: Vec<&str> = description
                .workflows
                .iter()
                .map(|workflow| workflow.workflow_id.as_str())
                .collect();
            bail!(
                "this description has {} workflows — name one with `--workflow <ID>`: {}",
                ids.len(),
                ids.join(", ")
            );
        }
        None => {}
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
        // Source names cannot contain dots: a dotted name is an alias path
        // into an external Arazzo document, applied once it is loaded.
        if name.contains('.') {
            continue;
        }
        // Silently ignoring a typo would send the request to whatever
        // the document says — production, most likely.
        if !description
            .source_descriptions
            .iter()
            .any(|source| source.name == name)
        {
            bail!("`--base-url {name}=…` names no source description of this document");
        }
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
    let (options, any, source_diagnostics) = sources(options, &description, value, &source, &args)?;

    let explain = |error: anyhow::Error| {
        let error = if source_diagnostics.is_empty() {
            error
        } else {
            anyhow!("{error}\n{}", source_diagnostics.join("\n"))
        };
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
    };
    let plan = prepare(&description, &options).map_err(|error| explain(anyhow!(error)))?;
    // Preparation failures carry these diagnostics in the returned error.
    // Once preparation succeeds, emit optional warnings here, exactly once.
    if !args.quiet {
        if args.skip_input_validation {
            eprintln!("- workflow input-schema validation is disabled (--skip-input-validation)");
        }
        for diagnostic in &source_diagnostics {
            eprintln!("- {diagnostic}");
        }
    }
    let report = plan.execute(&mut Client::blocking()).map_err(|failure| {
        if !args.quiet
            && let Some(report) = &failure.report
        {
            eprint!("{report}");
        }
        anyhow!(failure.error)
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
    options: Options,
    description: &v1_1::Description,
    value: Value,
    from: &InputSource,
    args: &ArazzoRunArgs,
) -> Result<(Options, bool, Vec<String>)> {
    let explicit: BTreeSet<String> = args
        .source
        .iter()
        .filter_map(|source| source.split_once('=').map(|(name, _)| name.to_owned()))
        .collect();
    let mut loader = None;
    let mut load_options = SourceLoadOptions::default();
    load_options.max_documents = args.source_max_documents;
    load_options.max_depth = args.source_max_depth;
    load_options.retrieval_aliases = args.allow_source_retrieval_aliases;
    // Loading an external Arazzo document can reveal calls back into this
    // one, whose workflows may need entry sources nobody asked for yet. Each
    // pass is one complete load of the whole selection into a fresh registry,
    // so links, budgets and diagnostics are those of a single load; the
    // loader keeps what was fetched, and remembers what failed, across passes.
    let mut selection: Option<BTreeSet<String>> = None;
    // The text of every document read or fetched, by where it came from.
    let mut texts = BTreeMap::new();
    let mut fetched = None;
    // Each text parsed once, however many passes the loading takes.
    let mut maps = BTreeMap::new();
    let (mut registry, root, report) = loop {
        let (mut registry, root) = seeded_registry(&value, from, args, &mut texts)?;
        // The budget covers supplied documents plus distinct fetches of the
        // whole loading operation, not of each pass.
        let loader = loader.get_or_insert_with(|| {
            let (loader, fetches) = remembering_loader(
                &args.load,
                args.source_max_documents.saturating_sub(registry.len()),
            );
            fetched = Some(fetches);
            loader
        });
        let selected = match selection.take() {
            Some(selected) => selected,
            None => {
                let mut selected = explicit.clone();
                selected.extend(required_sources(
                    description,
                    &scoped(
                        &options,
                        &registry,
                        root,
                        &texts,
                        fetched.as_ref(),
                        &mut maps,
                    )?,
                )?);
                selected
            }
        };
        load_options.root_sources = (!args.load_all_sources).then(|| selected.clone());
        let report = registry.load_sources(root, loader, &load_options)?;
        if args.load_all_sources {
            break (registry, root, report);
        }
        // A static error in a newly linked document is preparation's to
        // report, with the loading diagnostics beside it.
        let Ok(grown) = required_sources(
            description,
            &scoped(
                &options,
                &registry,
                root,
                &texts,
                fetched.as_ref(),
                &mut maps,
            )?,
        ) else {
            break (registry, root, report);
        };
        // Each further pass strictly grows the selection of declared aliases,
        // so this terminates.
        if grown.is_subset(&selected) {
            break (registry, root, report);
        }
        selection = Some(selected.union(&grown).cloned().collect());
    };
    let diagnostics = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{}: {} (`{}`): {}",
                registry
                    .document(diagnostic.owner)
                    .expect("diagnostic owner")
                    .identity(),
                diagnostic.path,
                diagnostic.source_name,
                with_causes(&diagnostic.error),
            )
        })
        .chain(report.reference_diagnostics.iter().map(ToString::to_string))
        .collect::<Vec<_>>();
    // A missing external document is usually why an alias path names nothing.
    scoped_base_urls(
        &mut registry,
        root,
        &args.base_url,
        args.allow_source_retrieval_aliases,
    )
    .map_err(|error| {
        if diagnostics.is_empty() {
            error
        } else {
            anyhow!("{error}\n{}", diagnostics.join("\n"))
        }
    })?;
    let any = registry
        .sources(root)?
        .iter()
        .any(|source| source.target.is_some());
    Ok((
        scoped(
            &options,
            &registry,
            root,
            &texts,
            fetched.as_ref(),
            &mut maps,
        )?,
        any,
        diagnostics,
    ))
}

/// `options` adapted to the registry, with a source map for every external
/// Arazzo document the entry reaches whose text was read (`texts`) or
/// fetched, so preparation can say where in it a finding is. `maps` keeps
/// what was already parsed, by retrieval URI, with the value it is of.
fn scoped(
    options: &Options,
    registry: &SourceRegistry,
    root: roas_arazzo_executor::DocumentId,
    texts: &Texts,
    fetched: Option<&SharedFetches>,
    maps: &mut BTreeMap<Url, (Value, Option<Arc<SourceMap>>)>,
) -> Result<Options> {
    let mut options = options.clone();
    let fetched = fetched.map(|fetched| fetched.borrow());
    // The documents the executor's run table holds: those linked from the
    // root, transitively (`source_registry`'s walk; the CLI sets no
    // `Options::source` that could redirect an alias of the root).
    let mut seen = BTreeSet::from([root]);
    let mut next = vec![root];
    while let Some(id) = next.pop() {
        for link in registry.sources(id)? {
            let Some(target) = link.target else {
                continue;
            };
            let document = registry.document(target)?;
            if document.arazzo().is_none() || !seen.insert(target) {
                continue;
            }
            next.push(target);
            let uri = document.retrieval_uri();
            // Only a text that parsed to the very value registered: one URL
            // can answer two fetches differently, and only one is kept.
            let value = document.value();
            if maps.get(uri).is_none_or(|(parsed, _)| parsed != value) {
                let mut matching = texts
                    .get(uri)
                    .into_iter()
                    .flatten()
                    .chain(
                        fetched
                            .as_ref()
                            .and_then(|fetched| fetched.texts.get(uri))
                            .into_iter()
                            .flatten(),
                    )
                    .filter(|(parsed, _)| parsed == value)
                    .map(|(_, text)| text.as_str())
                    .collect::<Vec<_>>();
                matching.sort_unstable();
                matching.dedup();
                // Two texts of the same value: which one it was parsed from
                // is not known, so neither is used.
                let map = match matching[..] {
                    [text] => SourceMap::parse(text).ok().map(Arc::new),
                    _ => None,
                };
                maps.insert(uri.clone(), (value.clone(), map));
            }
            if let Some((_, Some(map))) = maps.get(uri) {
                options =
                    options.document_source_map(document.identity().as_str(), Arc::clone(map));
            }
        }
    }
    Ok(options.source_registry(registry, root)?)
}

/// An error and what caused it, as one line: a loading diagnostic's own
/// message often says only which URL failed, and its source says why.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut line = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        // Some messages already end with their source's.
        let text = error.to_string();
        if !line.ends_with(&text) {
            line.push_str(": ");
            line.push_str(&text);
        }
        cause = error.source();
    }
    line
}

/// A registry holding the entry and every document named on the command
/// line, with `--source` overrides applied and nothing linked yet.
fn seeded_registry(
    value: &Value,
    from: &InputSource,
    args: &ArazzoRunArgs,
    texts: &mut Texts,
) -> Result<(SourceRegistry, roas_arazzo_executor::DocumentId)> {
    let mut registry = SourceRegistry::new();
    let root = registry.insert(base_uri(from, None)?.as_str(), value.clone())?;
    // Index every explicitly supplied document before linking any source.
    let mut supplied = Vec::new();
    for source in &args.source {
        let (name, path) = split_pair(source, "--source")?;
        if registry.source(root, name).is_none() {
            bail!("`--source {name}=…` names no source description of this document");
        }
        let from = InputSource::File(PathBuf::from(path));
        let (text, document, _) = read_input_text(&from, None)
            .with_context(|| format!("reading source description {path}"))?;
        let retrieval = base_uri(&from, None)?;
        keep_text(texts, &retrieval, &document, text);
        let id = registry.insert(retrieval.as_str(), document)?;
        supplied.push((name.to_owned(), id));
    }
    for path in &args.source_document {
        let from = InputSource::File(path.clone());
        let (text, document, _) = read_input_text(&from, None)
            .with_context(|| format!("reading source document {}", path.display()))?;
        let retrieval = base_uri(&from, None)?;
        keep_text(texts, &retrieval, &document, text);
        registry.insert_reference_document(retrieval.as_str(), document)?;
    }
    for (name, id) in supplied {
        registry.override_source(root, &name, id)?;
    }
    Ok((registry, root))
}

/// The `--load` loader, with each fetcher wrapped to remember its failures
/// and the text it fetched, and to stop after `budget` distinct URLs.
fn remembering_loader(kinds: &[LoaderKind], budget: usize) -> (Loader, SharedFetches) {
    let fetches = Rc::new(RefCell::new(Fetches {
        budget,
        ..Fetches::default()
    }));
    let loader =
        build_loader_with(kinds, |fetcher| Remembering::new(fetcher, &fetches)).unwrap_or_default();
    (loader, fetches)
}

/// What the fetchers of one loading operation have asked for, across its
/// passes. The loader caches only successes, so a fetcher sees each URL
/// once unless it failed.
#[derive(Default)]
struct Fetches {
    /// Distinct URLs the fetchers may still be asked for.
    budget: usize,
    asked: BTreeSet<Url>,
    failed: BTreeMap<Url, LoaderError>,
    /// The text of each fetched Arazzo document, by its final retrieval URI.
    texts: Texts,
}

/// A fetcher that can also say what text it parsed a document from.
trait TextFetcher: ResourceFetcher {
    fn fetch_document_text(
        &mut self,
        uri: &Url,
    ) -> Result<(LoadedDocument, Option<String>), LoaderError> {
        self.fetch_document(uri).map(|document| (document, None))
    }
}

impl TextFetcher for Fetcher {
    fn fetch_document_text(
        &mut self,
        uri: &Url,
    ) -> Result<(LoadedDocument, Option<String>), LoaderError> {
        Fetcher::fetch_document_text(self, uri).map(|(document, text)| (document, Some(text)))
    }
}

type SharedFetches = Rc<RefCell<Fetches>>;

/// Every Arazzo text read at a URI, with the value it parsed to.
type Texts = BTreeMap<Url, Vec<(Value, String)>>;

/// Keep `text`, which parsed to `value`, if it is an Arazzo document's:
/// only those have findings placed in their text.
fn keep_text(texts: &mut Texts, uri: &Url, value: &Value, text: String) {
    if value.get("arazzo").is_some() {
        texts
            .entry(uri.clone())
            .or_default()
            .push((value.clone(), text));
    }
}

/// A fetcher that answers a URL it already failed on with the same error,
/// without asking again, and refuses new URLs past the operation's budget.
/// Without it a later loading pass would repeat every failed request and
/// start its document budget afresh.
struct Remembering<F> {
    inner: F,
    fetches: SharedFetches,
}

impl<F> Remembering<F> {
    fn new(inner: F, fetches: &SharedFetches) -> Self {
        Self {
            inner,
            fetches: Rc::clone(fetches),
        }
    }

    fn remember<T>(
        &mut self,
        uri: &Url,
        fetch: impl FnOnce(&mut F) -> Result<T, LoaderError>,
    ) -> Result<T, LoaderError> {
        {
            let mut fetches = self.fetches.borrow_mut();
            if let Some(error) = fetches.failed.get(uri) {
                return Err(replay(error));
            }
            if !fetches.asked.contains(uri) {
                if fetches.asked.len() >= fetches.budget {
                    return Err(LoaderError::Fetch {
                        uri: uri.to_string(),
                        source: "the --source-max-documents budget is spent".into(),
                    });
                }
                fetches.asked.insert(uri.clone());
            }
        }
        fetch(&mut self.inner).inspect_err(|error| {
            self.fetches
                .borrow_mut()
                .failed
                .insert(uri.clone(), replay(error));
        })
    }
}

impl<F: TextFetcher> ResourceFetcher for Remembering<F> {
    fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
        self.fetch_document(uri).map(|document| document.document)
    }

    fn fetch_document(&mut self, uri: &Url) -> Result<LoadedDocument, LoaderError> {
        let (document, text) = self.remember(uri, |inner| inner.fetch_document_text(uri))?;
        if let Some(text) = text {
            let retrieval = document.retrieval_uri.clone();
            keep_text(
                &mut self.fetches.borrow_mut().texts,
                &retrieval,
                &document.document,
                text,
            );
        }
        Ok(document)
    }
}

/// A copy of a loading error that reads the same: its message is the
/// original's, and its source the original's whole cause chain as text.
fn replay(error: &LoaderError) -> LoaderError {
    match error {
        LoaderError::NoFetcherRegistered { uri } => {
            LoaderError::NoFetcherRegistered { uri: uri.clone() }
        }
        LoaderError::UnsupportedFetcherUri(uri) => LoaderError::UnsupportedFetcherUri(uri.clone()),
        LoaderError::InvalidFileUri(uri) => LoaderError::InvalidFileUri(uri.clone()),
        LoaderError::MissingBaseUri(uri) => LoaderError::MissingBaseUri(uri.clone()),
        LoaderError::InvalidFragment(uri) => LoaderError::InvalidFragment(uri.clone()),
        LoaderError::InvalidUri { uri, source } => LoaderError::InvalidUri {
            uri: uri.clone(),
            source: *source,
        },
        LoaderError::ReadFile { path, source } => LoaderError::ReadFile {
            path: path.clone(),
            source: std::io::Error::new(source.kind(), with_causes(source)),
        },
        LoaderError::Parse { uri, source } => LoaderError::Parse {
            uri: uri.clone(),
            source: serde::de::Error::custom(source),
        },
        LoaderError::PointerNotFound { uri, reference } => LoaderError::PointerNotFound {
            uri: uri.clone(),
            reference: reference.clone(),
        },
        LoaderError::Fetch { uri, source } => LoaderError::Fetch {
            uri: uri.clone(),
            source: with_causes(source.as_ref()).into(),
        },
        // A variant added later: its message is kept, if not its shape.
        other => LoaderError::Fetch {
            uri: String::new(),
            source: other.to_string().into(),
        },
    }
}

/// `--base-url a.b.api=URL`: follow loaded Arazzo aliases `a`, then `b`, and
/// override `api` for that document only. Plain names are options of the entry.
fn scoped_base_urls(
    registry: &mut SourceRegistry,
    root: roas_arazzo_executor::DocumentId,
    base_urls: &[String],
    retrieval_aliases: bool,
) -> Result<()> {
    // Two flags reaching one document's source by different paths would
    // otherwise race, the later one silently winning.
    let mut claimed: BTreeMap<(roas_arazzo_executor::DocumentId, String), (&str, &str)> =
        BTreeMap::new();
    for base_url in base_urls {
        let (path, url) = split_pair(base_url, "--base-url")?;
        if !path.contains('.') {
            claimed.insert((root, path.to_owned()), (path, url));
        }
    }
    'flags: for base_url in base_urls {
        let (path, url) = split_pair(base_url, "--base-url")?;
        let Some((documents, name)) = path.rsplit_once('.') else {
            continue;
        };
        let mut owner = root;
        for alias in documents.split('.') {
            let document = registry.document(owner)?;
            let declared = document.arazzo().and_then(|description| {
                description
                    .source_descriptions
                    .iter()
                    .find(|source| source.name == alias)
            });
            let Some(declared) = declared else {
                bail!(
                    "`--base-url {path}=…`: `{alias}` names no source description of {}",
                    document.identity()
                );
            };
            // Selective loading links only the aliases it follows; another
            // alias may still have loaded the same document. A reference that
            // cannot be resolved stops the run: dropping the override would
            // send its requests to the original server.
            let target = match registry.source(owner, alias) {
                Some(link) => match link.target {
                    Some(target) => Some(target),
                    None => registry
                        .resolve(owner, &link.declared_uri, retrieval_aliases)
                        .with_context(|| format!("`--base-url {path}=…`: `{alias}`"))?,
                },
                None => None,
            };
            let arazzo = match target {
                Some(target) => registry.document(target)?.arazzo().is_some(),
                None => declared
                    .type_
                    .is_none_or(|type_| type_ == roas_arazzo::v1_1::SourceType::Arazzo),
            };
            if !arazzo {
                bail!("`--base-url {path}=…`: `{alias}` is not an Arazzo source description");
            }
            // A document the selected workflow never reaches is not loaded,
            // and nothing runs there for the override to change.
            let Some(target) = target else {
                continue 'flags;
            };
            owner = target;
        }
        if registry.source(owner, name).is_none() {
            bail!(
                "`--base-url {path}=…`: `{name}` names no source description of {}",
                registry.document(owner)?.identity()
            );
        }
        if let Some((other, earlier)) = claimed.insert((owner, name.to_owned()), (path, url))
            && earlier != url
        {
            bail!(
                "`--base-url {path}=…` and `--base-url {other}=…` name the same source of {} with different URLs",
                registry.document(owner)?.identity()
            );
        }
        registry.override_base_url(owner, name, url)?;
    }
    Ok(())
}

/// What a relative source description URL is resolved against.
///
/// `$self`, where the description sets one: it "provides the
/// self-assigned URI of this Arazzo Description, which also serves as
/// its base URI in accordance with RFC 3986 Section 5.1.1". A relative
/// `$self` is itself resolved against where the document was read from,
/// and where there is no `$self` that retrieval URI is the base.
///
/// This matters for more than tidiness: a description read from one
/// directory but self-identifying as another names its sources relative
/// to the second, and resolving against the first can load a different
/// OpenAPI document — and run the workflow against a different API.
fn base_uri(from: &InputSource, self_: Option<&str>) -> Result<Url> {
    let retrieval = match from {
        InputSource::File(path) => {
            let absolute = path
                .canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(path));
            Url::from_file_path(&absolute)
                .map_err(|()| anyhow!("`{}` is not a path a URL can name", absolute.display()))?
        }
        // Nothing to have been retrieved from: resolve from the working
        // directory.
        InputSource::Stdin => {
            let directory = std::env::current_dir().context("reading the working directory")?;
            Url::from_directory_path(&directory)
                .map_err(|()| anyhow!("`{}` is not a path a URL can name", directory.display()))?
        }
    };
    let Some(self_) = self_ else {
        return Ok(retrieval);
    };
    // An absolute `$self` is the base, whatever shape it takes; a
    // relative one is resolved against the retrieval URI first. Falling
    // back to the retrieval URI is only for a description that sets no
    // `$self` at all — doing it for a `$self` that cannot be a base
    // would quietly resolve against the wrong document, which is the
    // very thing `$self` is there to prevent.
    retrieval
        .join(self_)
        .with_context(|| format!("resolving `$self` `{self_}`"))
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

        /// `text` as written, for tests about where in it something is.
        fn text(name: &str, text: &str) -> Self {
            let file = Self::write(name, &Value::Null);
            std::fs::write(&file.0, text).unwrap();
            file
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
        let d = detect_or_use_arazzo(None, v1_0_doc(), "").unwrap();
        let up = d.convert_to(ArazzoVersion::V1_1).unwrap();
        assert_eq!(up.version(), ArazzoVersion::V1_1);
        assert_eq!(up.into_value().unwrap()["arazzo"], "1.1.0");

        // identity (v1.0 → v1.0)
        let d = detect_or_use_arazzo(None, v1_0_doc(), "").unwrap();
        assert_eq!(
            d.convert_to(ArazzoVersion::V1_0).unwrap().version(),
            ArazzoVersion::V1_0
        );

        // identity (v1.1 → v1.1)
        let d = detect_or_use_arazzo(None, v1_1_doc(), "").unwrap();
        assert_eq!(
            d.convert_to(ArazzoVersion::V1_1).unwrap().version(),
            ArazzoVersion::V1_1
        );

        // downconvert errors
        let d = detect_or_use_arazzo(None, v1_1_doc(), "").unwrap();
        let err = d.convert_to(ArazzoVersion::V1_0).unwrap_err();
        assert!(err.to_string().contains("downconversion is not supported"));
    }

    #[test]
    fn detect_or_use_arazzo_honors_forced_version() {
        let d = detect_or_use_arazzo(Some(ArazzoVersion::V1_1), v1_1_doc(), "").unwrap();
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

    const LOCATED: &str = "arazzo: 1.1.0
info:
  title: \"\"
  version: \"1.0.0\"
sourceDescriptions:
  - name: petStore
    url: https://api.example.com/openapi.json
    type: openapi
workflows:
  - workflowId: buyPet
    steps:
      - stepId: findPet
        operationId: getPetById
        parameters:
          - name: petId
            in: path
            value: $inputs.petId
        successCriteria:
          - condition: \"$statusCode == 'caf\\u00e9' && (\"
";

    #[test]
    fn validation_findings_name_their_line_and_column() {
        let file = TempFile::text("located.yaml", LOCATED);
        let read = read_arazzo(&InputSource::File(file.0.clone()), None).unwrap();
        let errors = read.detected.validate(EnumSet::empty()).unwrap_err();
        let title = errors
            .errors
            .iter()
            .find(|error| error.path == "#.info.title")
            .unwrap();
        assert!(
            located(read.source_map().as_ref(), title)
                .starts_with("#.info.title at line 3, column 10: "),
            "{}",
            located(read.source_map().as_ref(), title)
        );
        // Without a map, or for a path the text does not have: as before.
        assert_eq!(located(None, title), title.to_string());
        let key = TempFile::text(
            "key.yaml",
            &format!("{LOCATED}components:\n  inputs:\n    bad key: {{}}\n"),
        );
        let read = read_arazzo(&InputSource::File(key.0.clone()), None).unwrap();
        let errors = read.detected.validate(EnumSet::empty()).unwrap_err();
        let key = errors
            .errors
            .iter()
            .find(|error| error.path.ends_with("bad key"))
            .unwrap();
        // A finding about a member name points at the name.
        assert!(
            located(read.source_map().as_ref(), key).contains(" at line 22, column 5: "),
            "{}",
            located(read.source_map().as_ref(), key)
        );
        let unmapped = TempFile::text(
            "unmapped.yaml",
            &LOCATED.replace("title: \"\"", "title: [\"\"]"),
        );
        let error = read_arazzo(&InputSource::File(unmapped.0.clone()), None)
            .err()
            .expect("a sequence is not a title")
            .to_string();
        assert!(
            error.contains("#.info.title at line 3, column 10: invalid type: sequence"),
            "{error}"
        );
    }

    #[test]
    fn a_description_that_does_not_fit_the_model_names_where() {
        // An extension the typed text readers would refuse (an integer
        // key) must not take the blame for the field that is wrong.
        let text = LOCATED
            .replace("workflowId: buyPet", "workflowId: [1]")
            .replace(
                "  version: \"1.0.0\"\n",
                "  version: \"1.0.0\"\n  x-ext: {1: a}\n",
            );
        let yaml = TempFile::text("typed.yaml", &text);
        let error = read_arazzo(&InputSource::File(yaml.0.clone()), None)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "deserializing as Arazzo 1.1: #.workflows[0].workflowId at line 11, column 17: \
             invalid type: sequence, expected a string"
        );
        let json = TempFile::text(
            "typed.json",
            "{\"arazzo\": \"1.0.1\",\n \"info\": {\"title\": 7, \"version\": \"1\"},\n \"sourceDescriptions\": [], \"workflows\": []}",
        );
        let error = read_arazzo(&InputSource::File(json.0.clone()), None)
            .err()
            .unwrap();
        // serde_json words the cause by feature (`arbitrary_precision`
        // says "number"); the location is what this is about.
        let shown = error.to_string();
        assert!(
            shown.starts_with(
                "deserializing as Arazzo 1.0: #.info.title at line 2, column 20: invalid type: "
            ),
            "{shown}"
        );
        assert!(shown.ends_with(", expected a string"), "{shown}");
        // A missing member is reported at the object that lacks it.
        let error = detect_or_use_arazzo(
            Some(ArazzoVersion::V1_1),
            json!({ "arazzo": "1.1.0" }),
            "arazzo: 1.1.0\n",
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "deserializing as Arazzo 1.1: # at line 1, column 1: missing field `info`"
        );
    }

    #[test]
    fn serde_paths_read_as_model_paths() {
        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        enum Shape {
            Ring { size: Vec<u8> },
        }
        let error = serde_path_to_error::deserialize::<_, Vec<Shape>>(json!([
            { "Ring": { "size": [1, "x"] } }
        ]))
        .unwrap_err();
        assert_eq!(model_path(error.path()), "#[0].Ring.size[1]");
    }

    #[test]
    fn run_reports_where_a_malformed_condition_is() {
        let (_, openapi) = runnable();
        let text = LOCATED.replace("title: \"\"", "title: T");
        let description = TempFile::text("located-run.yaml", &text);
        let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
        args.quiet = true;
        let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.contains(
                "#.workflows[0].steps[0].successCriteria[0].condition at line 19, column 56: "
            ),
            "{shown}"
        );
        assert!(!shown.contains(" at byte "), "{shown}");
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
            skip_input_validation: false,
            schema: Vec::new(),
            source: vec![format!("petStore={}", openapi.0.display())],
            source_document: Vec::new(),
            load_all_sources: false,
            source_max_documents: 256,
            source_max_depth: 32,
            allow_source_retrieval_aliases: false,
            base_url: vec![format!("petStore={base}")],
            header: vec!["Authorization: Bearer abc".to_owned()],
            load: Vec::new(),
            max_steps: Some(20),
            quiet: false,
            ignore: Vec::new(),
            format: None,
            output_format: Some(InputFormat::Json),
        }
    }

    fn input_schema_workflow(schema: Value) -> TempFile {
        TempFile::write(
            "input-schema.json",
            &json!({
                "arazzo":"1.1.0", "info":{"title":"Input validation","version":"1"},
                "sourceDescriptions":[{"name":"petStore","url":"https://example.com/api.json","type":"openapi"}],
                "workflows":[{"workflowId":"buyPet","inputs":schema,"steps":[{
                    "stepId":"request","operationId":"getPetById",
                    "parameters":[{"name":"petId","in":"path","value":"$inputs.petId"}]
                }]}]
            }),
        )
    }

    #[test]
    fn run_validates_declared_inputs_even_when_no_expression_reads_them() {
        let (_, openapi) = runnable();
        for schema in [
            json!({"required":["unusedButRequired"]}),
            json!({"properties":{"petId":{"type":"string"}}}),
        ] {
            let description = input_schema_workflow(schema);
            let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
            args.quiet = true;
            let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();
            assert!(error.to_string().contains("invalid inputs"), "{error:#}");
        }
    }

    #[test]
    fn run_rejects_malformed_or_unavailable_input_schemas_before_execution() {
        let (_, openapi) = runnable();
        for schema in [
            json!({"required":17}),
            json!({"$ref":"https://127.0.0.1:1/not-preloaded"}),
        ] {
            let description = input_schema_workflow(schema);
            let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
            args.quiet = true;
            let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();
            assert!(error.to_string().contains("nothing was run"), "{error:#}");
        }
    }

    #[test]
    fn run_can_explicitly_skip_schema_validation() {
        let (_, openapi) = runnable();
        let description = input_schema_workflow(json!({"required":["missing"]}));
        let (base, join) = server(1, 200, "{}");
        let mut args = run_args(&description, &openapi, &base);
        args.skip_input_validation = true;
        run_arazzo(ArazzoCommand::Run(args.into())).unwrap();
        assert_eq!(join.join().unwrap().len(), 1);
    }

    #[test]
    fn default_input_validation_ignores_unreferenced_api_component_schemas() {
        for unused in [
            json!({"$schema":"http://json-schema.org/draft-04/schema#","type":"object"}),
            json!({"$anchor":"unused","$ref":"https://127.0.0.1:1/not-preloaded"}),
        ] {
            let openapi = TempFile::write(
                "unused-input-schema-api.json",
                &json!({
                    "openapi":"3.1.0","servers":[{"url":"https://example.com"}],
                    "paths":{"/pets/{petId}":{"get":{"operationId":"getPetById"}}},
                    "components":{"schemas":{"Unused":unused}}
                }),
            );
            let description =
                input_schema_workflow(json!({"properties":{"petId":{"type":"integer"}}}));
            let (base, join) = server(1, 200, "{}");
            let args = run_args(&description, &openapi, &base);
            run_arazzo(ArazzoCommand::Run(args.into())).unwrap();
            assert_eq!(join.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn run_accepts_explicit_offline_schema_resources_and_flag_overrides() {
        let (_, openapi) = runnable();
        let description =
            input_schema_workflow(json!({"$ref":"https://example.com/input.json#params"}));
        let schema = TempFile::write(
            "schema.json",
            &json!({"$defs":{"Params":{"$anchor":"params","required":["petId"],"properties":{"petId":{"type":"integer"}}}}}),
        );
        let inputs = TempFile::write("inputs.json", &json!({"petId":"overridden"}));
        let (base, join) = server(1, 200, "{}");
        let mut args = run_args(&description, &openapi, &base);
        args.inputs = Some(inputs.0.clone());
        args.schema = vec![format!(
            "https://example.com/input.json={}",
            schema.0.display()
        )];
        run_arazzo(ArazzoCommand::Run(args.into())).unwrap();
        assert_eq!(join.join().unwrap().len(), 1);
    }

    /// An entry whose only step calls a workflow of another Arazzo document,
    /// which calls the API through its own `api` source.
    fn external_workflow() -> (TempFile, TempFile, TempFile) {
        let api = TempFile::write(
            "external-api.json",
            &json!({
                "openapi": "3.1.0", "info": { "title": "A", "version": "1" },
                "servers": [{ "url": "https://production.example.invalid" }],
                "paths": { "/things": { "get": { "operationId": "getThing" } } }
            }),
        );
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        let child = TempFile::write(
            "external-child.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [{ "name": "api", "url": api_name, "type": "openapi" }],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                    "parameters": [{ "name": "id", "in": "query", "value": "$inputs.id" }],
                    "outputs": { "name": "$response.body#/name" } }],
                    "outputs": { "name": "$steps.get.outputs.name" } }]
            }),
        );
        let entry = TempFile::write(
            "external-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "shared", "url": "https://example.invalid/shared.arazzo.json", "type": "arazzo" }
                ],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.shared.child",
                    "parameters": [{ "name": "id", "value": "$inputs.petId" }] }],
                    "outputs": { "name": "$steps.call.outputs.name" } }]
            }),
        );
        (entry, child, api)
    }

    #[test]
    fn run_executes_a_workflow_of_a_supplied_external_document() {
        let (base, join) = server(1, 200, r#"{"name":"Rex"}"#);
        let (entry, child, api) = external_workflow();
        let mut args = run_args(&entry, &api, &base);
        args.source = vec![format!("shared={}", child.0.display())];
        // The child's own source is loaded beside it, and only its
        // requests are redirected.
        args.load = vec![LoaderKind::File];
        args.base_url = vec![format!("shared.api={base}")];
        args.quiet = true;
        run_arazzo_run(args).unwrap();
        let asked = join.join().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("GET /things?id=7 "), "{asked:?}");
    }

    /// An entry calling `child` of a YAML document whose condition is
    /// malformed behind escapes and a multi-byte character.
    fn malformed_external_workflow() -> (TempFile, TempFile, TempFile) {
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        let child = TempFile::text(
            "malformed-child.yaml",
            &format!(
                r#"arazzo: 1.1.0
info: {{ title: C, version: "1" }}
sourceDescriptions:
  - {{ name: api, url: {api_name}, type: openapi }}
workflows:
  - workflowId: child
    steps:
      - stepId: get
        operationId: $sourceDescriptions.api.getThing
        parameters:
          - {{ name: id, in: query, value: $inputs.id }}
        successCriteria:
          - condition: "$response.body#/n\u00e9 == \"café\" && ("
"#
            ),
        );
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        let entry = TempFile::write(
            "malformed-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [{ "name": "shared", "url": child_name, "type": "arazzo" }],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.shared.child",
                    "parameters": [{ "name": "id", "value": "$inputs.petId" }] }] }]
            }),
        );
        (entry, child, api)
    }

    #[test]
    fn run_reports_where_a_malformed_condition_is_in_a_linked_document() {
        let (entry, child, api) = malformed_external_workflow();
        let identity = Url::from_file_path(child.0.canonicalize().unwrap()).unwrap();
        // Supplied for its alias, preloaded, and fetched by the loader.
        for supplied in ["source", "document", "fetched"] {
            let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
            args.source = Vec::new();
            match supplied {
                "source" => args.source = vec![format!("shared={}", child.0.display())],
                "document" => args.source_document = vec![child.0.clone()],
                _ => {}
            }
            args.base_url = Vec::new();
            args.load = vec![LoaderKind::File];
            args.quiet = true;
            let error = run_arazzo_run(args).unwrap_err();
            let shown = format!("{error:#}");
            assert!(
                shown.contains(&format!(
                    "{identity} #.workflows[0].steps[0].successCriteria[0].condition \
                     at line 13, column 65: "
                )),
                "supplied: {supplied}: {shown}"
            );
        }
    }

    #[test]
    fn a_reformatted_copy_of_a_linked_document_does_not_move_its_findings() {
        let (entry, child, api) = malformed_external_workflow();
        let identity = "https://example.test/shared.arazzo.yaml";
        let text = std::fs::read_to_string(&child.0).unwrap().replacen(
            "arazzo: 1.1.0\n",
            &format!("arazzo: 1.1.0\n$self: {identity}\n"),
            1,
        );
        std::fs::write(&child.0, &text).unwrap();
        let value: Value = serde_saphyr::from_str(&text).unwrap();
        // The same document on one line, for a second alias: the registry
        // keeps the first.
        let copy = TempFile::write("malformed-copy.json", &value);
        let mut description: Value =
            serde_json::from_str(&std::fs::read_to_string(&entry.0).unwrap()).unwrap();
        let mut again = description["sourceDescriptions"][0].clone();
        again["name"] = json!("again");
        description["sourceDescriptions"]
            .as_array_mut()
            .unwrap()
            .push(again);
        std::fs::write(&entry.0, description.to_string()).unwrap();
        let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
        args.source = vec![
            format!("shared={}", child.0.display()),
            format!("again={}", copy.0.display()),
        ];
        args.base_url = Vec::new();
        args.load = vec![LoaderKind::File];
        args.quiet = true;
        let shown = format!("{:#}", run_arazzo_run(args).unwrap_err());
        assert!(
            shown.contains(&format!(
                "{identity} #.workflows[0].steps[0].successCriteria[0].condition \
                 at line 14, column 65: "
            )),
            "{shown}"
        );
    }

    #[test]
    fn a_document_reached_through_another_is_located_by_its_self() {
        let (_, child, api) = malformed_external_workflow();
        let identity = "https://example.test/leaf.arazzo.yaml";
        let text = std::fs::read_to_string(&child.0).unwrap().replacen(
            "arazzo: 1.1.0\n",
            &format!("arazzo: 1.1.0\n$self: {identity}\n"),
            1,
        );
        std::fs::write(&child.0, text).unwrap();
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        let middle = TempFile::write(
            "middle.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "M", "version": "1" },
                "sourceDescriptions": [{ "name": "leaf", "url": child_name, "type": "arazzo" }],
                "workflows": [{ "workflowId": "middle", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.leaf.child",
                    "parameters": [{ "name": "id", "value": "$inputs.id" }] }] }]
            }),
        );
        let middle_name = middle.0.file_name().unwrap().to_str().unwrap().to_owned();
        let entry = TempFile::write(
            "middle-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [{ "name": "shared", "url": middle_name, "type": "arazzo" }],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.shared.middle",
                    "parameters": [{ "name": "id", "value": "$inputs.petId" }] }] }]
            }),
        );
        let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
        args.source = Vec::new();
        args.base_url = Vec::new();
        args.load = vec![LoaderKind::File];
        // The middle names the leaf by where it is, not by its `$self`.
        args.allow_source_retrieval_aliases = true;
        args.quiet = true;
        let shown = format!("{:#}", run_arazzo_run(args).unwrap_err());
        assert!(
            shown.contains(&format!(
                "{identity} #.workflows[0].steps[0].successCriteria[0].condition \
                 at line 14, column 65: "
            )),
            "{shown}"
        );
    }

    #[test]
    fn a_document_is_located_in_the_text_it_was_parsed_from() {
        // The same final URL answers the second fetch with another
        // document: the registered one is placed in its own text.
        let shown = refetched(true);
        assert!(
            shown.contains("condition at line 13, column 65: "),
            "{shown}"
        );
        // With the same document in other text, which text it was parsed
        // from is not known: nothing is guessed.
        let shown = refetched(false);
        assert!(shown.contains("condition at byte 35: "), "{shown}");
    }

    /// `run` of an entry whose two sources redirect to one URL, which
    /// answers its second fetch with its first text five lines down and,
    /// if `retitled`, a different title.
    fn refetched(retitled: bool) -> String {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let (_, child, api) = malformed_external_workflow();
        let first = std::fs::read_to_string(&child.0).unwrap();
        let mut second = format!("#\n#\n#\n#\n#\n{first}");
        if retitled {
            second = second.replace("title: C", "title: D");
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        std::thread::spawn(move || {
            let mut served = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let response = if line.starts_with("GET /b ") {
                    served += 1;
                    let body = if served == 1 { &first } else { &second };
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/yaml\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else if line.starts_with("GET /a ") || line.starts_with("GET /c ") {
                    "HTTP/1.1 302 Found\r\nLocation: /b\r\nContent-Length: 0\r\n\
                     Connection: close\r\n\r\n"
                        .to_owned()
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let entry = TempFile::write(
            "refetched-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "shared", "url": format!("{base}/a"), "type": "arazzo" },
                    { "name": "again", "url": format!("{base}/c"), "type": "arazzo" }
                ],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.shared.child",
                    "parameters": [{ "name": "id", "value": "$inputs.petId" }] }] }]
            }),
        );
        let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
        args.source = Vec::new();
        args.base_url = Vec::new();
        args.load = vec![LoaderKind::Http];
        args.load_all_sources = true;
        args.quiet = true;
        let shown = format!("{:#}", run_arazzo_run(args).unwrap_err());
        assert!(
            shown.contains(&format!(
                "{base}/b #.workflows[0].steps[0].successCriteria[0].condition at "
            )),
            "{shown}"
        );
        shown
    }

    #[test]
    fn selective_loading_follows_calls_back_into_the_entry_document() {
        let (base, join) = server(1, 200, r#"{"name":"Rex"}"#);
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        // `helper` is reached only through the external document, and is the
        // only user of the entry's `extra` source.
        let entry = TempFile::write("back-entry.json", &json!({}));
        let entry_name = entry.0.file_name().unwrap().to_str().unwrap().to_owned();
        let child = TempFile::write(
            "back-child.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [{ "name": "root", "url": entry_name, "type": "arazzo" }],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "back", "workflowId": "$sourceDescriptions.root.helper" }] }]
            }),
        );
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        std::fs::write(
            &entry.0,
            serde_json::to_string(&json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "shared", "url": child_name, "type": "arazzo" },
                    { "name": "extra", "url": api_name, "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "buyPet", "steps": [{
                        "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] },
                    { "workflowId": "helper", "steps": [{
                        "stepId": "get", "operationId": "$sourceDescriptions.extra.getThing",
                        "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let mut args = run_args(&entry, &api, &base);
        args.source = Vec::new();
        args.load = vec![LoaderKind::File];
        args.base_url = vec![format!("extra={base}")];
        args.quiet = true;
        run_arazzo_run(args).unwrap();
        let asked = join.join().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("GET /things?id=1 "), "{asked:?}");
    }

    #[test]
    fn a_later_loading_pass_does_not_fetch_a_failed_source_again() {
        let (base, join) = server(1, 200, r#"{"name":"Rex"}"#);
        let (gone, gone_join) = server(2, 404, "{}");
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        let entry = TempFile::write("refetch-entry.json", &json!({}));
        let entry_name = entry.0.file_name().unwrap().to_str().unwrap().to_owned();
        // `gone` fails in the first pass; the call back into the entry then
        // asks for `extra`, which needs a second pass.
        let child = TempFile::write(
            "refetch-child.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [
                    { "name": "root", "url": entry_name, "type": "arazzo" },
                    { "name": "gone", "url": format!("{gone}/gone.json"), "type": "openapi" }
                ],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "back", "workflowId": "$sourceDescriptions.root.helper" }] }]
            }),
        );
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        std::fs::write(
            &entry.0,
            serde_json::to_string(&json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "shared", "url": child_name, "type": "arazzo" },
                    { "name": "extra", "url": api_name, "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "buyPet", "steps": [{
                        "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] },
                    { "workflowId": "helper", "steps": [{
                        "stepId": "get", "operationId": "$sourceDescriptions.extra.getThing",
                        "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let mut args = run_args(&entry, &api, &base);
        args.source = Vec::new();
        args.load = vec![LoaderKind::File, LoaderKind::Http];
        args.base_url = vec![format!("extra={base}")];
        args.quiet = true;
        run_arazzo_run(args).unwrap();
        assert_eq!(join.join().unwrap().len(), 1);
        // Release the second accept if the failed source was not asked again.
        let _ = std::net::TcpStream::connect(gone.trim_start_matches("http://"));
        let asked = gone_join.join().unwrap();
        assert_eq!(
            asked.iter().filter(|line| line.starts_with("GET ")).count(),
            1,
            "{asked:?}"
        );
    }

    #[test]
    fn remembered_fetches_replay_failures_and_spend_one_budget() {
        struct Counting(Rc<RefCell<Vec<String>>>);
        impl ResourceFetcher for Counting {
            fn fetch(&mut self, uri: &Url) -> Result<Value, LoaderError> {
                self.0.borrow_mut().push(uri.to_string());
                if uri.path().ends_with("bad") {
                    Err(LoaderError::Fetch {
                        uri: uri.to_string(),
                        source: "refused".into(),
                    })
                } else {
                    Ok(json!({}))
                }
            }
        }
        impl TextFetcher for Counting {}
        let calls = Rc::new(RefCell::new(Vec::new()));
        let fetches = Rc::new(RefCell::new(Fetches {
            budget: 2,
            ..Fetches::default()
        }));
        let mut fetcher = Remembering::new(Counting(Rc::clone(&calls)), &fetches);
        let good = Url::parse("http://h/good").unwrap();
        let bad = Url::parse("http://h/bad").unwrap();
        let late = Url::parse("http://h/late").unwrap();
        fetcher.fetch(&good).unwrap();
        let first = fetcher.fetch(&bad).unwrap_err().to_string();
        // A failure is answered again without asking; the URL already counted.
        assert_eq!(fetcher.fetch_document(&bad).unwrap_err().to_string(), first);
        // A URL already asked for stays within the budget.
        fetcher.fetch(&good).unwrap();
        // A third distinct URL is past it, and is not asked for.
        let refused = fetcher.fetch(&late).unwrap_err();
        assert!(
            std::error::Error::source(&refused)
                .is_some_and(|source| source.to_string().contains("--source-max-documents")),
            "{refused}"
        );
        assert_eq!(
            *calls.borrow(),
            ["http://h/good", "http://h/bad", "http://h/good"]
        );
    }

    /// An error with a fixed message and an `io::Error` source.
    #[derive(Debug)]
    struct Wrapping {
        message: &'static str,
        source: std::io::Error,
    }

    impl std::fmt::Display for Wrapping {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.message)
        }
    }

    impl std::error::Error for Wrapping {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    #[test]
    fn a_cause_already_in_the_message_is_not_repeated() {
        let error = Wrapping {
            message: "cannot parse `x`: bad type",
            source: std::io::Error::other("bad type"),
        };
        assert_eq!(with_causes(&error), "cannot parse `x`: bad type");
    }

    #[test]
    fn a_loading_diagnostic_says_why_as_well_as_what() {
        let fetches = Rc::new(RefCell::new(Fetches::default()));
        let mut fetcher = Remembering::new(
            Fetcher::File(roas_file_fetcher::FileFetcher::new()),
            &fetches,
        );
        let spent = fetcher
            .fetch(&Url::parse("file:///budget/spent.json").unwrap())
            .unwrap_err();
        assert_eq!(
            with_causes(&spent),
            "failed to fetch `file:///budget/spent.json`: the --source-max-documents budget is spent"
        );
    }

    #[test]
    fn a_replayed_loading_error_reads_like_the_original() {
        let errors = [
            LoaderError::NoFetcherRegistered { uri: "x:a".into() },
            LoaderError::UnsupportedFetcherUri("x:b".into()),
            LoaderError::InvalidFileUri("x:c".into()),
            LoaderError::MissingBaseUri("#/d".into()),
            LoaderError::InvalidFragment("x:e#%".into()),
            LoaderError::InvalidUri {
                uri: "::".into(),
                source: url::ParseError::RelativeUrlWithoutBase,
            },
            LoaderError::ReadFile {
                path: PathBuf::from("/missing"),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
            },
            LoaderError::Parse {
                uri: "file:///p".into(),
                source: serde_json::from_str::<Value>("{").unwrap_err(),
            },
            LoaderError::PointerNotFound {
                uri: "file:///q".into(),
                reference: "#/r".into(),
            },
            LoaderError::Fetch {
                uri: "http://h/s".into(),
                source: "refused".into(),
            },
        ];
        let nested = LoaderError::Fetch {
            uri: "http://h/t".into(),
            source: Box::new(Wrapping {
                message: "HTTP request failed",
                source: std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    "connection refused",
                ),
            }),
        };
        for error in errors.into_iter().chain([nested]) {
            let replayed = replay(&error);
            assert_eq!(replayed.to_string(), error.to_string());
            assert_eq!(with_causes(&replayed), with_causes(&error));
        }
    }

    /// Run `sources` for an entry `entry(child_name)` whose linked document is
    /// `child(entry_name)`, both loaded with `--load file,http`.
    fn load_back_call(
        name: &str,
        child: impl FnOnce(&str) -> Value,
        entry: impl FnOnce(&str) -> Value,
        api: &TempFile,
        tweak: impl FnOnce(&mut ArazzoRunArgs),
    ) -> (Options, v1_1::Description, Vec<String>) {
        let entry_file = TempFile::write(&format!("{name}-entry.json"), &json!({}));
        let entry_name = entry_file
            .0
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let child = TempFile::write(&format!("{name}-child.json"), &child(&entry_name));
        let value = entry(child.0.file_name().unwrap().to_str().unwrap());
        std::fs::write(&entry_file.0, serde_json::to_string(&value).unwrap()).unwrap();
        let parsed: v1_1::Description = serde_json::from_value(value.clone()).unwrap();
        let mut args = run_args(&entry_file, api, "http://127.0.0.1:1");
        args.source = Vec::new();
        args.load = vec![LoaderKind::File, LoaderKind::Http];
        tweak(&mut args);
        let (options, _, diagnostics) = sources(
            Options::new(),
            &parsed,
            value,
            &InputSource::File(entry_file.0.clone()),
            &args,
        )
        .unwrap();
        (options, parsed, diagnostics)
    }

    #[test]
    fn a_failed_source_of_a_diamond_document_is_fetched_and_reported_once() {
        let (gone, gone_join) = server(2, 404, "{}");
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        // `other` names the same document as `shared`, and is needed only once
        // the call back into the entry is seen: the second pass revisits it.
        let child = |entry: &str| {
            json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [
                    { "name": "root", "url": entry, "type": "arazzo" },
                    { "name": "gone", "url": format!("{gone}/gone.json"), "type": "openapi" },
                    { "name": "api", "url": api_name, "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "child", "steps": [{
                        "stepId": "back", "workflowId": "$sourceDescriptions.root.helper" }] },
                    { "workflowId": "leaf", "steps": [{
                        "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                        "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }
                ]
            })
        };
        let (options, parsed, diagnostics) = load_back_call(
            "diamond",
            child,
            |child| {
                json!({
                    "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                    "sourceDescriptions": [
                        { "name": "shared", "url": child, "type": "arazzo" },
                        { "name": "other", "url": child, "type": "arazzo" }
                    ],
                    "workflows": [
                        { "workflowId": "buyPet", "steps": [{
                            "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] },
                        { "workflowId": "helper", "steps": [{
                            "stepId": "leaf", "workflowId": "$sourceDescriptions.other.leaf" }] }
                    ]
                })
            },
            &api,
            |_| {},
        );
        let _ = std::net::TcpStream::connect(gone.trim_start_matches("http://"));
        let asked = gone_join.join().unwrap();
        assert_eq!(
            asked.iter().filter(|line| line.starts_with("GET ")).count(),
            1,
            "{asked:?}"
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|line| line.contains("`gone`"))
                .count(),
            1,
            "{diagnostics:?}"
        );
        prepare(&parsed, &options).unwrap();
    }

    #[test]
    fn a_later_pass_links_a_source_whose_identity_it_supplies() {
        let (_, _, api) = external_workflow();
        // The linked document's `api` URL cannot be fetched, but the entry's
        // `extra` source — selected only once the call back is seen — is that
        // document, by its `$self`.
        let unreachable = "http://127.0.0.1:1/reconcile-api.json";
        let mut identified: Value =
            serde_json::from_str(&std::fs::read_to_string(&api.0).unwrap()).unwrap();
        identified["openapi"] = json!("3.2.0");
        identified["$self"] = json!(unreachable);
        let extra = TempFile::write("reconcile-api.json", &identified);
        let extra_name = extra.0.file_name().unwrap().to_str().unwrap().to_owned();
        let child = |entry: &str| {
            json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [
                    { "name": "root", "url": entry, "type": "arazzo" },
                    { "name": "api", "url": unreachable, "type": "openapi" }
                ],
                "workflows": [{ "workflowId": "child", "steps": [
                    { "stepId": "back", "workflowId": "$sourceDescriptions.root.helper" },
                    { "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                      "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }]
            })
        };
        let (options, parsed, diagnostics) = load_back_call(
            "reconcile",
            child,
            |child| {
                json!({
                    "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                    "sourceDescriptions": [
                        { "name": "shared", "url": child, "type": "arazzo" },
                        { "name": "extra", "url": extra_name, "type": "openapi" }
                    ],
                    "workflows": [
                        { "workflowId": "buyPet", "steps": [{
                            "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] },
                        { "workflowId": "helper", "steps": [{
                            "stepId": "get", "operationId": "$sourceDescriptions.extra.getThing",
                            "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }
                    ]
                })
            },
            &api,
            |_| {},
        );
        prepare(&parsed, &options).unwrap_or_else(|error| panic!("{error}\n{diagnostics:?}"));
    }

    #[test]
    fn the_document_budget_bounds_every_loading_pass() {
        let (_, _, api) = external_workflow();
        let copy = |name: &str| {
            let file = TempFile::write(
                name,
                &serde_json::from_str::<Value>(&std::fs::read_to_string(&api.0).unwrap()).unwrap(),
            );
            let name = file.0.file_name().unwrap().to_str().unwrap().to_owned();
            (file, name)
        };
        let (_a1, a1) = copy("budget-a1.json");
        let (_a2, a2) = copy("budget-a2.json");
        let (_a3, a3) = copy("budget-a3.json");
        let get = |source: &str| {
            json!({ "stepId": format!("get-{source}"),
            "operationId": format!("$sourceDescriptions.{source}.getThing"),
            "parameters": [{ "name": "id", "in": "query", "value": 1 }] })
        };
        let child = |entry: &str| {
            json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [
                    { "name": "root", "url": entry, "type": "arazzo" },
                    { "name": "a1", "url": a1, "type": "openapi" },
                    { "name": "a2", "url": a2, "type": "openapi" }
                ],
                "workflows": [{ "workflowId": "child", "steps": [
                    { "stepId": "back", "workflowId": "$sourceDescriptions.root.helper" },
                    get("a1"), get("a2")] }]
            })
        };
        let entry = |child: &str| {
            json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "shared", "url": child, "type": "arazzo" },
                    { "name": "extra", "url": a3, "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "buyPet", "steps": [{
                        "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] },
                    { "workflowId": "helper", "steps": [get("extra")] }
                ]
            })
        };
        let (options, parsed, diagnostics) = load_back_call("budget", child, entry, &api, |args| {
            args.source_max_documents = 4
        });
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains("(`a2`): source graph document limit (4) exceeded")),
            "{diagnostics:?}"
        );
        assert!(
            prepare(&parsed, &options).is_err(),
            "five documents loaded under a budget of four"
        );
    }

    /// An entry linking `shared` to `child_url`, loaded with `--load file`.
    fn unloadable(child_url: &str, base_url: Option<&str>) -> String {
        let (_, _, api) = external_workflow();
        let entry = TempFile::write(
            "unloadable-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [{ "name": "shared", "url": child_url, "type": "arazzo" }],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] }]
            }),
        );
        let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
        args.source = Vec::new();
        args.load = vec![LoaderKind::File];
        args.base_url = base_url.map(str::to_owned).into_iter().collect();
        args.quiet = true;
        run_arazzo_run(args).unwrap_err().to_string()
    }

    #[test]
    fn a_scoped_base_url_into_a_document_that_failed_to_load_leaves_the_error_to_preparation() {
        let error = unloadable(
            "missing-external-document.json",
            Some("shared.api=http://127.0.0.1:9"),
        );
        assert!(!error.contains("--base-url"), "{error}");
        assert!(error.contains("missing-external-document.json"), "{error}");
    }

    #[test]
    fn a_static_error_in_a_linked_document_is_reported_with_loading_diagnostics() {
        let child = TempFile::write(
            "broken-child.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [
                    { "name": "gone", "url": "gone-external-api.json", "type": "openapi" }
                ],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "get", "operationId": "$sourceDescriptions.gone.getThing" }],
                    "outputs": { "x": "$steps.nope.outputs.x" } }]
            }),
        );
        let error = unloadable(child.0.file_name().unwrap().to_str().unwrap(), None);
        assert!(error.contains("$steps.nope.outputs.x"), "{error}");
        assert!(error.contains("gone-external-api.json"), "{error}");
    }

    #[test]
    fn a_scoped_base_url_for_a_document_the_workflow_never_reaches_is_harmless() {
        let (base, join) = server(1, 200, r#"{"name":"Rex"}"#);
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        let entry = TempFile::write(
            "unreached-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "shared", "url": "https://example.invalid/shared.arazzo.json", "type": "arazzo" },
                    { "name": "api", "url": api_name, "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "viaShared", "steps": [{
                        "stepId": "call", "workflowId": "$sourceDescriptions.shared.child" }] },
                    { "workflowId": "local", "steps": [{
                        "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                        "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }
                ]
            }),
        );
        let mut args = run_args(&entry, &api, &base);
        args.source = Vec::new();
        args.load = vec![LoaderKind::File];
        args.workflow = Some("local".to_owned());
        args.base_url = vec![
            format!("api={base}"),
            "shared.api=http://127.0.0.1:9".to_owned(),
        ];
        args.quiet = true;
        run_arazzo_run(args).unwrap();
        assert_eq!(join.join().unwrap().len(), 1);
    }

    #[test]
    fn a_scoped_base_url_through_an_unresolvable_alias_stops_the_run() {
        let (_, _, api) = external_workflow();
        let api_url = Url::from_file_path(&api.0).unwrap();
        // `a` names the child by its `$self`; `b` by where it was read from,
        // which identity-based referencing does not accept.
        let child = TempFile::write(
            "identity-child.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "$self": "https://canonical.example.test/child.arazzo.json",
                "sourceDescriptions": [{ "name": "api", "url": api_url.as_str(), "type": "openapi" }],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                    "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }]
            }),
        );
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        let entry = TempFile::write(
            "identity-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "a", "url": "https://canonical.example.test/child.arazzo.json", "type": "arazzo" },
                    { "name": "b", "url": child_name, "type": "arazzo" }
                ],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.a.child" }] }]
            }),
        );
        let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
        args.source = Vec::new();
        args.source_document = vec![child.0.clone()];
        args.load = vec![LoaderKind::File];
        args.base_url = vec!["b.api=http://127.0.0.1:9".to_owned()];
        args.quiet = true;
        let error = format!("{:#}", run_arazzo_run(args).unwrap_err());
        assert!(error.starts_with("`--base-url b.api=…`: `b`"), "{error}");
    }

    #[test]
    fn a_scoped_base_url_reaches_a_document_loaded_through_another_alias() {
        let (base, join) = server(1, 200, r#"{"name":"Rex"}"#);
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        let child = TempFile::write(
            "aliased-child.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [{ "name": "api", "url": api_name, "type": "openapi" }],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                    "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }]
            }),
        );
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        // Only `a` is followed; `b` names the same document and stays unlinked.
        let entry = TempFile::write(
            "aliased-entry.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "a", "url": child_name, "type": "arazzo" },
                    { "name": "b", "url": child_name, "type": "arazzo" }
                ],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.a.child" }] }]
            }),
        );
        let mut args = run_args(&entry, &api, &base);
        args.source = Vec::new();
        args.load = vec![LoaderKind::File];
        args.base_url = vec![format!("b.api={base}")];
        args.quiet = true;
        run_arazzo_run(args).unwrap();
        let asked = join.join().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("GET /things?id=1 "), "{asked:?}");
    }

    /// An entry whose aliases `a` and `b` both name a child linking `root`
    /// back to the entry, run with `base_urls`; the error, if any.
    fn conflicting(name: &str, base_urls: &[&str]) -> Option<String> {
        let (_, _, api) = external_workflow();
        let api_name = api.0.file_name().unwrap().to_str().unwrap().to_owned();
        let entry = TempFile::write(&format!("{name}-entry.json"), &json!({}));
        let entry_name = entry.0.file_name().unwrap().to_str().unwrap().to_owned();
        let child = TempFile::write(
            &format!("{name}-child.json"),
            &json!({
                "arazzo": "1.1.0", "info": { "title": "C", "version": "1" },
                "sourceDescriptions": [
                    { "name": "root", "url": entry_name, "type": "arazzo" },
                    { "name": "api", "url": api_name, "type": "openapi" }
                ],
                "workflows": [{ "workflowId": "child", "steps": [{
                    "stepId": "get", "operationId": "$sourceDescriptions.api.getThing",
                    "parameters": [{ "name": "id", "in": "query", "value": 1 }] }] }]
            }),
        );
        let child_name = child.0.file_name().unwrap().to_str().unwrap().to_owned();
        std::fs::write(
            &entry.0,
            serde_json::to_string(&json!({
                "arazzo": "1.1.0", "info": { "title": "E", "version": "1" },
                "sourceDescriptions": [
                    { "name": "a", "url": child_name, "type": "arazzo" },
                    { "name": "b", "url": child_name, "type": "arazzo" },
                    { "name": "api", "url": api_name, "type": "openapi" }
                ],
                "workflows": [{ "workflowId": "buyPet", "steps": [{
                    "stepId": "call", "workflowId": "$sourceDescriptions.a.child" }] }]
            }))
            .unwrap(),
        )
        .unwrap();
        let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
        args.source = Vec::new();
        args.load = vec![LoaderKind::File];
        args.base_url = base_urls.iter().map(|flag| (*flag).to_owned()).collect();
        args.quiet = true;
        run_arazzo_run(args).err().map(|error| error.to_string())
    }

    #[test]
    fn scoped_base_urls_that_reach_one_source_must_agree() {
        let error = conflicting(
            "conflict",
            &["a.api=http://127.0.0.1:8", "b.api=http://127.0.0.1:9"],
        )
        .expect("a conflict");
        assert!(
            error.starts_with(
                "`--base-url b.api=…` and `--base-url a.api=…` name the same source of "
            ),
            "{error}"
        );
        let error = conflicting(
            "loop",
            &["api=http://127.0.0.1:8", "a.root.api=http://127.0.0.1:9"],
        )
        .expect("a conflict through the loop back to the entry");
        assert!(
            error.starts_with("`--base-url a.root.api=…` and `--base-url api=…`"),
            "{error}"
        );
        // Agreeing flags are not a conflict.
        let agreed = conflicting(
            "agreed",
            &["a.api=http://127.0.0.1:8", "b.api=http://127.0.0.1:8"],
        );
        assert!(
            agreed
                .as_deref()
                .is_none_or(|error| !error.contains("name the same source")),
            "{agreed:?}"
        );
    }

    #[test]
    fn run_refuses_a_scoped_base_url_that_names_nothing() {
        let (entry, child, api) = external_workflow();
        for (base_url, expected) in [
            (
                "ghost.api=http://127.0.0.1:9",
                "`--base-url ghost.api=…`: `ghost` names no source description of file://",
            ),
            (
                "shared.api.x=http://127.0.0.1:9",
                "`--base-url shared.api.x=…`: `api` is not an Arazzo source description",
            ),
            (
                "shared.nope=http://127.0.0.1:9",
                "`--base-url shared.nope=…`: `nope` names no source description of file://",
            ),
        ] {
            let mut args = run_args(&entry, &api, "http://127.0.0.1:1");
            args.source = vec![format!("shared={}", child.0.display())];
            args.load = vec![LoaderKind::File];
            args.base_url = vec![base_url.to_owned()];
            args.quiet = true;
            let error = run_arazzo_run(args).unwrap_err().to_string();
            assert!(error.starts_with(expected), "{error}");
        }
    }

    #[test]
    fn run_refuses_a_base_url_for_a_source_that_is_not_there() {
        let (description, openapi) = runnable();
        let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
        // A typo would otherwise be ignored, and the request would go
        // wherever the document says — production, most likely.
        args.base_url = vec!["petStroe=http://127.0.0.1:9".to_owned()];
        args.quiet = true;

        let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();

        assert_eq!(
            error.to_string(),
            "`--base-url petStroe=…` names no source description of this document"
        );
    }

    #[test]
    fn run_validates_before_it_sends_anything() {
        // Two workflows of the same id: `arazzo validate` rejects this,
        // and a run has no business reaching an API with it.
        let invalid = TempFile::write(
            "invalid.json",
            &json!({
                "arazzo": "1.1.0",
                "info": { "title": "T", "version": "1.0.0" },
                "sourceDescriptions": [
                    { "name": "petStore", "url": "u", "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "w", "steps": [{ "stepId": "a", "operationId": "getPetById" }] },
                    { "workflowId": "w", "steps": [{ "stepId": "b", "operationId": "getPetById" }] }
                ]
            }),
        );
        let (_, openapi) = runnable();
        let mut args = run_args(&invalid, &openapi, "http://127.0.0.1:1");
        args.workflow = Some("w".to_owned());
        args.quiet = true;

        let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();

        assert!(
            error.to_string().contains("does not validate"),
            "got: {error}"
        );
        assert!(
            error.to_string().contains("nothing was run"),
            "and it says so plainly: {error}"
        );
    }

    #[test]
    fn run_prepares_all_steps_before_attempting_a_request() {
        let (_, openapi) = runnable();
        for criterion in [
            json!({ "condition": "true || $steps.typo.outputs.value" }),
            json!({ "type": "regex", "context": "$response.body", "condition": "[" }),
        ] {
            let description = TempFile::write(
                "unprepared.json",
                &json!({
                    "arazzo": "1.1.0", "info": { "title": "T", "version": "1" },
                    "sourceDescriptions": [{ "name": "petStore", "url": "https://example.com/openapi.json", "type": "openapi" }],
                    "workflows": [{ "workflowId": "buyPet", "steps": [
                        { "stepId": "first", "operationId": "getPetById", "parameters": [{ "name": "petId", "in": "path", "value": 7 }] },
                        { "stepId": "second", "operationId": "getPetById", "parameters": [{ "name": "petId", "in": "path", "value": 7 }],
                          "successCriteria": [criterion], "onFailure": [{ "name": "recover", "type": "goto", "stepId": "first" }] }
                    ] }]
                }),
            );
            let args = run_args(&description, &openapi, "http://127.0.0.1:1");
            let error = run_arazzo_run(args).unwrap_err().to_string();
            assert!(error.contains("preparation failed"), "{error}");
            assert!(error.contains("steps[1].successCriteria[0]"), "{error}");
        }
    }

    #[test]
    fn run_loads_only_sources_required_by_the_selected_workflow() {
        let (base, join) = server(1, 200, "{}");
        let (_, openapi) = runnable();
        let description = TempFile::write(
            "selected.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "T", "version": "1" },
                "sourceDescriptions": [
                    { "name": "petStore", "url": "https://example.com/openapi.json", "type": "openapi" },
                    { "name": "unused", "url": "unprovided-source-that-does-not-exist.json", "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "buyPet", "steps": [{ "stepId": "first", "operationId": "$sourceDescriptions.petStore.getPetById",
                        "parameters": [{ "name": "petId", "in": "path", "value": 7 }] }] },
                    { "workflowId": "unrelated", "steps": [{ "stepId": "unused", "operationId": "$sourceDescriptions.unused.absent" }] }
                ]
            }),
        );
        let mut args = run_args(&description, &openapi, &base);
        args.load.push(LoaderKind::File);
        run_arazzo_run(args).unwrap();
        assert_eq!(join.join().unwrap().len(), 1);
    }

    #[test]
    fn full_graph_failures_are_located_but_only_required_sources_block_preparation() {
        let (description, openapi) = runnable();
        let (mut value, _) = read_input(&InputSource::File(description.0.clone()), None).unwrap();
        value["sourceDescriptions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "name": "unavailable", "url": "missing-for-source-graph.json", "type": "openapi"
            }));
        value["workflows"][0]["steps"][0]["operationId"] =
            json!("$sourceDescriptions.petStore.getPetById");
        let parsed = serde_json::from_value(value.clone()).unwrap();
        let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
        args.load_all_sources = true;
        args.load.push(LoaderKind::File);
        let (options, any, diagnostics) = sources(
            Options::new(),
            &parsed,
            value.clone(),
            &InputSource::File(description.0.clone()),
            &args,
        )
        .unwrap();
        assert!(any);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("sourceDescriptions[1].url"));
        assert!(diagnostics[0].contains("unavailable"));
        prepare(&parsed, &options).unwrap();
        value["workflows"][0]["steps"][0]["operationId"] = json!("getPetById");
        let bare = serde_json::from_value(value).unwrap();
        assert!(
            prepare(&bare, &options)
                .unwrap_err()
                .to_string()
                .contains("was not supplied")
        );

        args.source_max_documents = 1; // root + explicitly supplied API exceed the initial budget
        let error = run_arazzo_run(args).unwrap_err().to_string();
        assert!(error.contains("source graph document limit (1)"), "{error}");
    }

    #[test]
    fn operation_reference_files_use_load_policy_or_explicit_preloads() {
        let parts = TempFile::write(
            "parts.json",
            &json!({"item":{"get":{"operationId":"getPetById"}}}),
        );
        let reference = format!("{}#/item", parts.0.file_name().unwrap().to_string_lossy());
        let (description, _) = runnable();
        let openapi = TempFile::write(
            "referencing.json",
            &json!({"openapi":"3.1.0", "paths":{"/pets/{petId}":{"$ref":reference}}}),
        );
        let (value, _) = read_input(&InputSource::File(description.0.clone()), None).unwrap();
        let parsed = serde_json::from_value(value.clone()).unwrap();
        for mode in ["blocked", "file", "preload"] {
            let mut args = run_args(&description, &openapi, "https://override.test");
            if mode == "file" {
                args.load.push(LoaderKind::File);
            }
            if mode == "preload" {
                args.source_document.push(parts.0.clone());
            }
            let (options, _, diagnostics) = sources(
                Options::new()
                    .input("petId", 7)
                    .base_url("petStore", "https://override.test"),
                &parsed,
                value.clone(),
                &InputSource::File(description.0.clone()),
                &args,
            )
            .unwrap();
            if mode == "blocked" {
                assert_eq!(diagnostics.len(), 1);
                assert!(diagnostics[0].contains("/paths/~1pets~1{petId}/$ref"));
                assert!(diagnostics[0].contains("no fetcher"));
                assert!(prepare(&parsed, &options).is_err());
            } else {
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
                let mut fake =
                    roas_arazzo_executor::testing::Fake::new().reply(200, &json!({"name":"pet"}));
                prepare(&parsed, &options)
                    .unwrap()
                    .execute(&mut fake)
                    .unwrap();
                assert_eq!(fake.sent()[0].url, "https://override.test/pets/7");
            }
        }
    }

    #[test]
    fn bare_ids_ignore_linked_non_openapi_sources_in_cli_options() {
        let (description, openapi) = runnable();
        let from = InputSource::File(description.0.clone());
        let (mut value, _) = read_input(&from, None).unwrap();
        let mut child = value.clone();
        child["sourceDescriptions"] = json!([]);
        child["$self"] = json!("https://identity.test/child.json");
        let child_file = TempFile::write("mixed-child.json", &child);
        let events_file = TempFile::write("mixed-events.json", &json!({"asyncapi":"3.0.0"}));
        value["workflows"][0]["steps"][0]["operationId"] = json!("getPetById");
        value["sourceDescriptions"].as_array_mut().unwrap().extend([
            json!({"name":"child", "url":"https://identity.test/child.json", "type":"arazzo"}),
            json!({"name":"events", "url":base_uri(&InputSource::File(events_file.0.clone()), None).unwrap().as_str(), "type":"asyncapi"}),
        ]);
        let parsed = serde_json::from_value(value.clone()).unwrap();
        let mut args = run_args(&description, &openapi, "https://override.test");
        args.load_all_sources = true;
        args.source_document = vec![child_file.0.clone(), events_file.0.clone()];
        let (options, _, diagnostics) = sources(
            Options::new()
                .input("petId", 7)
                .base_url("petStore", "https://override.test"),
            &parsed,
            value,
            &from,
            &args,
        )
        .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(options.source_document("child").is_some());
        assert!(options.source_document("events").is_some());
        let mut fake =
            roas_arazzo_executor::testing::Fake::new().reply(200, &json!({"name":"pet"}));
        assert!(
            prepare(&parsed, &options)
                .unwrap()
                .execute(&mut fake)
                .unwrap()
                .is_success()
        );
        assert_eq!(fake.sent()[0].url, "https://override.test/pets/7");
    }

    #[test]
    fn supplied_documents_are_indexed_offline_before_linking_and_follow_identity_policy() {
        let (description, openapi) = runnable();
        let (mut value, _) = read_input(&InputSource::File(description.0.clone()), None).unwrap();
        value["workflows"][0]["steps"][0]["operationId"] =
            json!("$sourceDescriptions.petStore.getPetById");
        let mut child = value.clone();
        child["$self"] = json!("https://identity.test/child.json");
        child["sourceDescriptions"] = json!([]);
        let supplied = TempFile::write("child.json", &child);
        value["sourceDescriptions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "name": "child", "url": "https://identity.test/child.json", "type": "arazzo"
            }));
        let mut args = run_args(&description, &openapi, "http://127.0.0.1:1");
        args.load_all_sources = true;
        args.source_document.push(supplied.0.clone());
        let from = InputSource::File(description.0.clone());
        let parsed = serde_json::from_value(value.clone()).unwrap();
        let (options, _, diagnostics) =
            sources(Options::new(), &parsed, value.clone(), &from, &args).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            options
                .source_document("child")
                .unwrap()
                .identity()
                .as_str(),
            "https://identity.test/child.json"
        );

        value["sourceDescriptions"][1]["url"] = json!(
            base_uri(&InputSource::File(supplied.0.clone()), None)
                .unwrap()
                .as_str()
        );
        let parsed = serde_json::from_value(value.clone()).unwrap();
        let (_, _, diagnostics) =
            sources(Options::new(), &parsed, value.clone(), &from, &args).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("not the Arazzo identity"));
        args.allow_source_retrieval_aliases = true;
        let (_, _, diagnostics) = sources(Options::new(), &parsed, value, &from, &args).unwrap();
        assert!(diagnostics.is_empty());

        args.source_document = vec![description.0.with_extension("not-found")];
        let error = run_arazzo_run(args).unwrap_err();
        assert!(format!("{error:#}").contains("reading source document"));
    }

    #[test]
    fn run_retains_a_runtime_output_failure_with_or_without_quiet() {
        let (_, openapi) = runnable();
        let description = TempFile::write(
            "output-error.json",
            &json!({
                "arazzo": "1.1.0", "info": { "title": "T", "version": "1" },
                "sourceDescriptions": [{ "name": "petStore", "url": "https://example.com/openapi.json", "type": "openapi" }],
                "workflows": [{ "workflowId": "buyPet", "steps": [{ "stepId": "first", "operationId": "getPetById",
                    "parameters": [{ "name": "petId", "in": "path", "value": 7 }], "outputs": { "missing": "$response.body#/missing" } }] }]
            }),
        );
        for quiet in [false, true] {
            let (base, join) = server(1, 200, "{}");
            let mut args = run_args(&description, &openapi, &base);
            args.quiet = quiet;
            let error = run_arazzo_run(args).unwrap_err().to_string();
            assert!(error.contains("missing"), "{error}");
            assert_eq!(join.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn run_can_be_told_to_let_a_check_pass() {
        // An empty `info.title` fails validation but has nothing to do
        // with whether the workflow runs.
        let (base, join) = server(1, 200, r#"{"id":7,"name":"fluffy"}"#);
        let (_, openapi) = runnable();
        let lenient = TempFile::write(
            "lenient.json",
            &json!({
                "arazzo": "1.1.0",
                "info": { "title": "", "version": "1.0.0" },
                "sourceDescriptions": [
                    { "name": "petStore", "url": "https://api.example.com/openapi.json",
                      "type": "openapi" }
                ],
                "workflows": [{
                    "workflowId": "buyPet",
                    "steps": [{
                        "stepId": "findPet",
                        "operationId": "getPetById",
                        "parameters": [
                            { "name": "petId", "in": "path", "value": "$inputs.petId" }
                        ]
                    }]
                }]
            }),
        );
        let mut args = run_args(&lenient, &openapi, &base);
        args.quiet = true;

        let strict = run_arazzo(ArazzoCommand::Run(
            ArazzoRunArgs {
                ignore: Vec::new(),
                ..clone_args(&args)
            }
            .into(),
        ))
        .unwrap_err();
        assert!(strict.to_string().contains("does not validate"), "{strict}");

        args.ignore = vec![ValidationOptions::IgnoreEmptyInfoTitle];
        run_arazzo(ArazzoCommand::Run(args.into())).expect("the check was let pass");
        assert_eq!(join.join().expect("the server thread").len(), 1);
    }

    /// `ArazzoRunArgs` is not `Clone` — clap args rarely are — so the
    /// test that needs two of them says how.
    fn clone_args(args: &ArazzoRunArgs) -> ArazzoRunArgs {
        ArazzoRunArgs {
            file: args.file.clone(),
            workflow: args.workflow.clone(),
            input: args.input.clone(),
            inputs: args.inputs.clone(),
            skip_input_validation: args.skip_input_validation,
            schema: args.schema.clone(),
            source: args.source.clone(),
            source_document: args.source_document.clone(),
            load_all_sources: args.load_all_sources,
            source_max_documents: args.source_max_documents,
            source_max_depth: args.source_max_depth,
            allow_source_retrieval_aliases: args.allow_source_retrieval_aliases,
            base_url: args.base_url.clone(),
            header: args.header.clone(),
            load: args.load.clone(),
            max_steps: args.max_steps,
            quiet: args.quiet,
            ignore: args.ignore.clone(),
            format: args.format,
            output_format: args.output_format,
        }
    }

    /// Where a relative source URL of `document` would be loaded from.
    fn resolved(document: &str, self_: Option<&str>, url: &str) -> String {
        let from = InputSource::File(PathBuf::from(document));
        base_uri(&from, self_)
            .expect("a base")
            .join(url)
            .expect("a URL")
            .to_string()
    }

    #[test]
    fn a_relative_url_keeps_its_uri_escapes() {
        // `api%20file.json` names `api file.json`. Read as path text it
        // would be encoded again, and name `api%20file.json` itself.
        let url = resolved("/tmp/flows/buy.arazzo.yaml", None, "api%20file.json");
        assert!(url.ends_with("/api%20file.json"), "{url}");
        assert!(!url.contains("%2520"), "escaped a second time: {url}");

        // A query and a fragment are the URL's, not the path's.
        let with_query = resolved("/tmp/flows/buy.arazzo.yaml", None, "api.json?v=2");
        assert!(with_query.ends_with("api.json?v=2"), "{with_query}");
    }

    #[test]
    fn a_relative_url_survives_a_directory_with_a_hash_in_its_name() {
        // Written as `file://{path}`, this would end at the `#` and the
        // loader would read something else entirely.
        let url = resolved("/tmp/hash#dir/buy.arazzo.yaml", None, "./api.json");
        assert!(url.starts_with("file://"), "{url}");
        assert!(url.contains("hash%23dir"), "the `#` is escaped: {url}");
        assert!(url.ends_with("api.json"), "{url}");
    }

    #[test]
    fn an_absolute_url_is_left_alone() {
        assert_eq!(
            resolved(
                "/tmp/flows/buy.arazzo.yaml",
                None,
                "https://api.example.com/openapi.json"
            ),
            "https://api.example.com/openapi.json"
        );
    }

    #[test]
    fn self_is_the_base_a_relative_url_is_resolved_against() {
        // A description read from one place and self-identifying as
        // another names its sources relative to the second. Resolving
        // against the first can load a different document, and run the
        // workflow against a different API.
        assert_eq!(
            resolved(
                "/tmp/retrieved/flow.json",
                Some("file:///tmp/canonical/flow.json"),
                "api.json"
            ),
            "file:///tmp/canonical/api.json"
        );

        // A relative `$self` is itself resolved against where the
        // document was read from.
        assert_eq!(
            resolved(
                "/tmp/retrieved/flow.json",
                Some("../canonical/flow.json"),
                "api.json"
            ),
            "file:///tmp/canonical/api.json"
        );
    }

    #[test]
    fn a_self_that_is_not_hierarchical_is_still_the_base() {
        // An absolute `$self` is the base whatever shape it takes. A
        // relative source cannot be resolved against a URN — but the
        // answer to that is to say so, not to quietly resolve against
        // the directory the document happened to be read from, where a
        // different `api.json` may well be sitting.
        let from = InputSource::File(PathBuf::from("/tmp/retrieved/flow.json"));
        let base = base_uri(&from, Some("urn:example:arazzo:flow")).expect("a base");
        assert_eq!(base.as_str(), "urn:example:arazzo:flow");
        assert!(base.join("api.json").is_err(), "nothing to resolve against");

        // An absolute source URL needs no base, so a URN `$self` costs
        // such a description nothing.
        assert_eq!(
            base.join("https://api.example.com/openapi.json")
                .expect("an absolute URL stands on its own")
                .as_str(),
            "https://api.example.com/openapi.json"
        );
    }

    #[test]
    fn run_asks_which_workflow_when_the_description_offers_several() {
        let several = TempFile::write(
            "several.json",
            &json!({
                "arazzo": "1.1.0",
                "info": { "title": "T", "version": "1.0.0" },
                "sourceDescriptions": [
                    { "name": "petStore", "url": "u", "type": "openapi" }
                ],
                "workflows": [
                    { "workflowId": "first", "steps": [{ "stepId": "s", "workflowId": "second" }] },
                    { "workflowId": "second", "steps": [{ "stepId": "s", "workflowId": "first" }] }
                ]
            }),
        );
        let (_, openapi) = runnable();
        let mut args = run_args(&several, &openapi, "http://127.0.0.1:1");
        args.workflow = None;
        args.quiet = true;

        let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();

        assert_eq!(
            error.to_string(),
            "this description has 2 workflows — name one with `--workflow <ID>`: first, second",
            "which one to run is not a choice to make on someone's behalf"
        );
    }

    #[test]
    fn run_needs_no_workflow_when_there_is_only_one() {
        let (description, openapi) = runnable();
        let (base, join) = server(1, 200, r#"{"id":7,"name":"fluffy"}"#);
        let mut args = run_args(&description, &openapi, &base);
        args.workflow = None;
        args.quiet = true;

        run_arazzo(ArazzoCommand::Run(args.into())).expect("one workflow is no choice at all");

        assert_eq!(join.join().expect("the server thread").len(), 1);
    }

    #[test]
    fn list_says_what_each_workflow_takes() {
        // Reading it is enough to prove the shape; what it prints goes
        // to stdout, which this harness does not capture.
        let described = TempFile::write(
            "described.json",
            &json!({
                "arazzo": "1.1.0",
                "info": { "title": "T", "version": "1.0.0" },
                "sourceDescriptions": [{ "name": "petStore", "url": "u", "type": "openapi" }],
                "workflows": [{
                    "workflowId": "buyPet",
                    "summary": "Find then order a pet",
                    "dependsOn": ["authenticate"],
                    "inputs": {
                        "type": "object",
                        "required": ["petId"],
                        "properties": {
                            "petId": { "type": "string" },
                            "note": {}
                        }
                    },
                    "steps": [{ "stepId": "findPet", "operationId": "getPetById" }]
                }, {
                    "workflowId": "authenticate",
                    "steps": [{ "stepId": "login", "operationId": "login" }]
                }]
            }),
        );
        run_arazzo(ArazzoCommand::List(ArazzoListArgs {
            file: Some(described.0.clone()),
            format: None,
        }))
        .expect("listing reads the document");

        let description: v1_1::Description =
            serde_json::from_str(&std::fs::read_to_string(&described.0).expect("the file"))
                .expect("a description");
        assert_eq!(
            inputs_of(&description.workflows[0]),
            ["note", "petId (string, required)"],
            "a type and a requirement are said where the schema says them"
        );
        assert!(
            inputs_of(&description.workflows[1]).is_empty(),
            "a workflow that takes nothing lists nothing"
        );
    }

    #[test]
    fn cli_parses_arazzo_list() {
        let cli = TestCli::try_parse_from(["roas", "list", "wf.yaml"]).unwrap();
        assert!(matches!(cli.command, ArazzoCommand::List(_)));
    }

    #[test]
    fn run_performs_the_workflows_requests() {
        let (description, openapi) = runnable();
        let (base, join) = server(1, 200, r#"{"id":7,"name":"fluffy"}"#);

        run_arazzo(ArazzoCommand::Run(
            run_args(&description, &openapi, &base).into(),
        ))
        .expect("the workflow runs");

        let asked = join.join().expect("the server thread");
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("GET /pets/7 "), "{}", asked[0]);
    }

    #[test]
    fn run_fails_when_the_workflow_does() {
        let (description, openapi) = runnable();
        let (base, join) = server(1, 500, r#"{"error":"gone"}"#);

        let error = run_arazzo(ArazzoCommand::Run(
            run_args(&description, &openapi, &base).into(),
        ))
        .unwrap_err();

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

        run_arazzo(ArazzoCommand::Run(args.into())).expect("the workflow runs");

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
            let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();
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
            "--source-document",
            "./child.yaml",
            "--load-all-sources",
            "--source-max-documents",
            "10",
            "--source-max-depth",
            "3",
            "--allow-source-retrieval-aliases",
            "--base-url",
            "petStore=http://127.0.0.1:8080",
            "--header",
            "Authorization: Bearer abc",
            "--load",
            "file",
            "--max-steps",
            "50",
            "--quiet",
            "--skip-input-validation",
            "--schema",
            "https://example.com/schema.json=./schema.json",
            "wf.yaml",
        ])
        .unwrap();
        match cli.command {
            ArazzoCommand::Run(a) => {
                assert_eq!(a.workflow.as_deref(), Some("buyPet"));
                assert_eq!(a.input, ["petId=7"]);
                assert_eq!(a.source, ["petStore=./openapi.yaml"]);
                assert_eq!(a.source_document, [PathBuf::from("./child.yaml")]);
                assert!(a.load_all_sources);
                assert_eq!(a.source_max_documents, 10);
                assert_eq!(a.source_max_depth, 3);
                assert!(a.allow_source_retrieval_aliases);
                assert_eq!(a.base_url, ["petStore=http://127.0.0.1:8080"]);
                assert_eq!(a.header, ["Authorization: Bearer abc"]);
                assert_eq!(a.max_steps, Some(50));
                assert!(a.quiet);
                assert!(a.skip_input_validation);
                assert_eq!(a.schema, ["https://example.com/schema.json=./schema.json"]);
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
        let url = resolved("/tmp/flows/buy.arazzo.yaml", None, "./petstore.yaml");
        assert!(url.starts_with("file://"), "{url}");
        assert!(url.ends_with("petstore.yaml"), "{url}");
        assert!(url.contains("flows"), "{url}");
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
            skip_input_validation: false,
            schema: Vec::new(),
            source: Vec::new(),
            source_document: Vec::new(),
            load_all_sources: false,
            source_max_documents: 256,
            source_max_depth: 32,
            allow_source_retrieval_aliases: false,
            base_url: Vec::new(),
            header: Vec::new(),
            load: Vec::new(),
            max_steps: None,
            quiet: true,
            ignore: Vec::new(),
            format: None,
            output_format: None,
        };
        let error = run_arazzo(ArazzoCommand::Run(args.into())).unwrap_err();
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
