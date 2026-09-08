//! Strict, IO-free preparation. Runtime values never participate in compilation.

#[cfg(test)]
pub(crate) mod instrumentation {
    use std::cell::Cell;
    thread_local! { static COUNTS: Cell<[usize; 4]> = const { Cell::new([0; 4]) }; }
    pub fn compiled(parser: usize) {
        COUNTS.with(|counts| {
            let mut value = counts.get();
            value[parser] += 1;
            counts.set(value);
        });
    }
    pub fn counts() -> [usize; 4] {
        COUNTS.with(Cell::get)
    }
}

use crate::criterion::{self, Condition};
use crate::expression;
use crate::operation::{self, Endpoint};
use crate::run::{self, ParameterTemplate};
use crate::runtime_syntax::{self, Expression, Root};
use crate::select::{self, Language};
use crate::{
    AsyncHttpClient, CriterionError, ExecutionError, ExecutionFailure, ExecutionReport,
    ExpressionError, HttpClient, Options, Run, SelectError,
};
use roas_arazzo::v1_1::{
    Criterion, CriterionKind, CriterionType, Description, ExpressionKind, Parameter,
    ParameterLocation, ReusableOr, SourceType, Step, ValueOrSelector, Workflow,
};
use roas_arazzo::validation::{Validate, ValidationError};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Identity of the documented expression grammar, truthiness and recovery policy.
/// Prepared plans are in-memory only; any future persistent cache must also bind
/// the description, source documents, options and this profile identifier.
pub const CONDITION_PROFILE: &str = "roas-arazzo-conditions-1";

/// Why preparation reported a finding. Portability advice is never fatal.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreparationIssue {
    /// Document-wide structural validation.
    #[error("{0}")]
    Model(ValidationError),
    /// A static executor requirement, checked without resolving runtime data.
    #[error(transparent)]
    Execution(ExecutionError),
    /// Optional advice about this executor's implementation-defined profile.
    #[error("{0}")]
    Portability(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Fake;
    use serde_json::json;

    #[test]
    fn checked_runs_reuse_parsers_and_constant_patterns() {
        let description = serde_json::from_value(json!({
            "arazzo": "1.1.0", "info": { "title": "Compiled", "version": "1" },
            "sourceDescriptions": [{ "name": "api", "url": "https://example.com/openapi.json" }],
            "workflows": [{ "workflowId": "w", "steps": [
                { "stepId": "s", "operationId": "check", "parameters": [
                    { "name": "x", "in": "query", "value": "value={$inputs.x}" }
                ], "requestBody": { "payload": { "value": 0 }, "replacements": [
                    { "target": "$.value", "targetSelectorType": "jsonpath", "value": "$inputs.x" }
                ] }, "successCriteria": [
                    { "condition": "$statusCode == 200 && $inputs.x == 7" },
                    { "type": "regex", "context": "$statusCode", "condition": "^200$" },
                    { "type": "regex", "context": "$statusCode", "condition": "^200$" },
                    { "type": "jsonpath", "context": "$response.body", "condition": "$.value" }
                ], "outputs": { "selected": { "type": "jsonpath", "context": "$response.body", "selector": "$.value" } } }
            ], "outputs": { "selected": "$steps.s.outputs.selected" } }]
        })).unwrap();
        let options = Options::new().input("x", 7).source(
            "api",
            "https://example.com/openapi.json",
            json!({
                "openapi": "3.1.0", "servers": [{ "url": "https://example.com" }],
                "paths": { "/check": { "post": { "operationId": "check" } } }
            }),
        );
        let before = instrumentation::counts();
        let plan = prepare(&description, &options).unwrap();
        let compiled = instrumentation::counts();
        assert_eq!(compiled[0] - before[0], plan.compiled.expressions.len());
        assert_eq!(compiled[1] - before[1], plan.compiled.conditions.len());
        assert_eq!(compiled[2] - before[2], 1);
        assert_eq!(compiled[3] - before[3], 1);
        for _ in 0..3 {
            let report = plan
                .execute(&mut Fake::new().reply(200, &json!({ "value": null })))
                .unwrap();
            assert!(report.is_success());
            assert_eq!(report.outputs["selected"], Value::Null);
        }
        assert_eq!(
            instrumentation::counts(),
            compiled,
            "executing a checked plan must not recompile its static expressions"
        );
    }
}

/// A located preparation error or warning.
#[derive(Debug)]
#[non_exhaustive]
pub struct PreparationDiagnostic {
    /// Invocation context, including when `path` names a reusable component.
    pub workflow_id: Option<String>,
    /// Absent for workflow-wide actions, workflow outputs and root findings.
    pub step_id: Option<String>,
    /// Model-style human-readable field locator, not an RFC 6901 pointer.
    pub path: String,
    /// Zero-based UTF-8 byte offset in that string field, when available.
    pub offset: Option<usize>,
    pub issue: PreparationIssue,
}

impl PreparationDiagnostic {
    #[must_use]
    pub fn is_error(&self) -> bool {
        !matches!(self.issue, PreparationIssue::Portability(_))
    }
}

impl fmt::Display for PreparationDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path)?;
        if let Some(offset) = self.offset {
            write!(f, " at byte {offset}")?;
        }
        match &self.issue {
            PreparationIssue::Model(error) => write!(f, ": {}", error.message),
            issue => write!(f, ": {issue}"),
        }
    }
}

/// All findings from a failed preparation, sorted deterministically by location.
#[derive(Debug)]
#[non_exhaustive]
pub struct PreparationError {
    pub diagnostics: Vec<PreparationDiagnostic>,
}

impl fmt::Display for PreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "workflow preparation failed; nothing was run")?;
        for diagnostic in &self.diagnostics {
            writeln!(f, "- {diagnostic}")?;
        }
        Ok(())
    }
}
impl std::error::Error for PreparationError {}

#[derive(Debug, Default)]
pub(crate) struct Compiled<'d> {
    pub templates: BTreeMap<&'d str, Vec<expression::TemplatePart<'d>>>,
    pub expressions: BTreeMap<&'d str, Expression<'d>>,
    pub conditions: BTreeMap<&'d str, Condition<'d>>,
    pub regexes: BTreeMap<&'d str, regex::Regex>,
    pub paths: BTreeMap<&'d str, serde_json_path::JsonPath>,
}

/// An immutable checked plan borrowing its description and configuration.
///
/// Preparation validates all document structure and the selected workflow's
/// potential calls, actions and dependencies. It performs no network IO and does
/// not evaluate input schemas or response-dependent expressions. Every run owns
/// its mutable inputs, history, outputs, retry budgets and timers.
///
/// ```no_run
/// # use roas_arazzo_executor::{prepare, Options, testing::Fake};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let description = serde_json::from_str("{}")?;
/// let options = Options::new().workflow("buyPet"); // also supply source documents
/// let plan = prepare(&description, &options)?;
/// let report = plan.execute(&mut Fake::default())?;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct PreparedWorkflow<'d> {
    pub(crate) description: &'d Description,
    pub(crate) options: &'d Options,
    pub(crate) compiled: Compiled<'d>,
    pub(crate) queue: Vec<&'d Workflow>,
    pub(crate) orders: BTreeMap<&'d str, Vec<usize>>,
    pub(crate) endpoints: BTreeMap<(&'d str, &'d str), Endpoint>,
    selected: &'d Workflow,
    diagnostics: Vec<PreparationDiagnostic>,
}

impl PreparedWorkflow<'_> {
    /// The selected workflow, including its opaque `inputs` schema for caller
    /// validation. This API does not claim JSON Schema validation support.
    #[must_use]
    pub fn workflow(&self) -> &Workflow {
        self.selected
    }

    #[must_use]
    pub fn condition_profile(&self) -> &'static str {
        CONDITION_PROFILE
    }

    /// Non-fatal preparation findings. A plan never contains fatal diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> &[PreparationDiagnostic] {
        &self.diagnostics
    }

    /// Start an independent manually driven run with the configured inputs.
    /// # Errors
    /// Runtime configuration errors, such as a zero workflow-depth limit.
    pub fn start(&self) -> Result<Run<'_>, ExecutionError> {
        Run::start_prepared(self, None)
    }

    /// Replace the selected workflow's and root dependencies' initial inputs.
    /// Compiled syntax and resolved operations remain unchanged.
    /// # Errors
    /// As [`Self::start`].
    pub fn start_with_inputs(&self, inputs: Map<String, Value>) -> Result<Run<'_>, ExecutionError> {
        Run::start_prepared(self, Some(inputs))
    }

    /// Execute the checked plan with a blocking client, preserving partial history.
    /// # Errors
    /// Runtime errors follow [`crate::execute_with_report`], not preparation rules.
    pub fn execute<C: HttpClient + ?Sized>(
        &self,
        client: &mut C,
    ) -> Result<ExecutionReport, ExecutionFailure> {
        let mut run = self.start().map_err(|error| ExecutionFailure {
            error,
            report: None,
        })?;
        crate::drive(&mut run, client)
    }

    /// Async execution of the same plan; retry waits use the client's sleep.
    /// # Errors
    /// As [`Self::execute`].
    pub async fn execute_async<C: AsyncHttpClient + ?Sized>(
        &self,
        client: &mut C,
    ) -> Result<ExecutionReport, ExecutionFailure> {
        let mut run = self.start().map_err(|error| ExecutionFailure {
            error,
            report: None,
        })?;
        crate::drive_async(&mut run, client).await
    }
}

/// Prepare a selected workflow without sending requests or fetching sources.
///
/// Unlike the legacy `execute` and `Run::start` entry points, this strict path
/// checks potentially reachable branches even when runtime dispatch might skip
/// them. Missing runtime data remains a runtime concern. Diagnostics distinguish
/// model errors, static executor errors and optional portability advice.
///
/// # Errors
/// All independent findings available from structural validation and compilation,
/// before any execution. An invalid description can never yield a checked plan.
pub fn prepare<'d>(
    description: &'d Description,
    options: &'d Options,
) -> Result<PreparedWorkflow<'d>, PreparationError> {
    let Compilation {
        compiler,
        selected,
        queue,
    } = compile(description, options, true)?;
    Ok(PreparedWorkflow {
        description,
        options,
        selected,
        compiled: compiler.compiled,
        queue,
        orders: compiler.orders,
        endpoints: compiler.endpoints,
        diagnostics: compiler.diagnostics,
    })
}

/// Source names potentially needed by the selected workflow's execution closure.
/// A bare operation ID requires every non-Arazzo source to prove uniqueness;
/// qualified IDs and operation paths require only their named sources.
/// This performs structural/static checks but no IO or operation lookup. After
/// supplying these sources, call [`prepare`] to resolve and check their contents.
/// # Errors
/// Located document, expression, reference and capability errors independent of
/// source contents. Unrelated workflows' operation requirements are not visited.
pub fn required_sources(
    description: &Description,
    options: &Options,
) -> Result<BTreeSet<String>, PreparationError> {
    Ok(compile(description, options, false)?
        .compiler
        .required_sources)
}

struct Compilation<'d> {
    compiler: Compiler<'d>,
    selected: &'d Workflow,
    queue: Vec<&'d Workflow>,
}

fn compile<'d>(
    description: &'d Description,
    options: &'d Options,
    resolve_operations: bool,
) -> Result<Compilation<'d>, PreparationError> {
    let mut compiler = Compiler {
        description,
        options,
        compiled: Compiled::default(),
        diagnostics: Vec::new(),
        pending: BTreeSet::new(),
        visited: BTreeSet::new(),
        orders: BTreeMap::new(),
        endpoints: BTreeMap::new(),
        reads: BTreeMap::new(),
        required_sources: BTreeSet::new(),
        resolve_operations,
    };
    if let Err(error) = description.validate(options.validation) {
        for error in error.errors {
            let site = compiler.model_site(&error.path);
            compiler.finding(&site, None, PreparationIssue::Model(error));
        }
    }
    let selected = match &options.workflow {
        Some(id) => description
            .workflows
            .iter()
            .position(|workflow| &workflow.workflow_id == id),
        None => (!description.workflows.is_empty()).then_some(0),
    };
    let mut queue = Vec::new();
    if let Some(index) = selected {
        compiler.pending.insert(index);
        match run::ordered_workflows(description, &description.workflows[index]) {
            Ok(ordered) => queue = ordered,
            Err(error) => compiler.error(
                &Site::workflow(index, &description.workflows[index]).field("dependsOn"),
                error,
            ),
        }
    } else {
        compiler.error(
            &Site::root("#.workflows"),
            ExecutionError::UnknownWorkflow(options.workflow.clone().unwrap_or_default()),
        );
    }
    while let Some(index) = compiler.pending.pop_first() {
        if compiler.visited.insert(index) {
            compiler.workflow(index);
        }
    }
    compiler.diagnostics.sort_by_cached_key(|diagnostic| {
        (
            diagnostic.path.clone(),
            diagnostic.workflow_id.clone(),
            diagnostic.step_id.clone(),
            diagnostic.offset,
            diagnostic.to_string(),
        )
    });
    if compiler
        .diagnostics
        .iter()
        .any(PreparationDiagnostic::is_error)
    {
        return Err(PreparationError {
            diagnostics: compiler.diagnostics,
        });
    }
    Ok(Compilation {
        selected: &description.workflows
            [selected.expect("a selected workflow without diagnostics")],
        queue,
        compiler,
    })
}

#[derive(Clone)]
struct Site<'d> {
    workflow: Option<&'d Workflow>,
    step: Option<&'d Step>,
    path: String,
}
impl<'d> Site<'d> {
    fn root(path: &str) -> Self {
        Self {
            workflow: None,
            step: None,
            path: path.into(),
        }
    }
    fn workflow(index: usize, workflow: &'d Workflow) -> Self {
        Self {
            workflow: Some(workflow),
            step: None,
            path: format!("#.workflows[{index}]"),
        }
    }
    fn field(&self, field: &str) -> Self {
        Self {
            path: format!("{}.{field}", self.path),
            ..self.clone()
        }
    }
    fn item(&self, field: &str, index: usize) -> Self {
        self.field(&format!("{field}[{index}]"))
    }
    fn component<T>(&self, entry: &ReusableOr<T>) -> Self {
        match entry {
            ReusableOr::Reusable(reference) => Self {
                path: reference.reference.replacen('$', "#.", 1),
                ..self.clone()
            },
            ReusableOr::Item(_) => self.clone(),
        }
    }
}

struct Compiler<'d> {
    description: &'d Description,
    options: &'d Options,
    compiled: Compiled<'d>,
    diagnostics: Vec<PreparationDiagnostic>,
    pending: BTreeSet<usize>,
    visited: BTreeSet<usize>,
    orders: BTreeMap<&'d str, Vec<usize>>,
    endpoints: BTreeMap<(&'d str, &'d str), Endpoint>,
    reads: BTreeMap<(&'d str, &'d str), BTreeSet<String>>,
    required_sources: BTreeSet<String>,
    resolve_operations: bool,
}

fn checked_selector_kind(kind: &roas_arazzo::v1_1::SelectorType) -> Result<Language, SelectError> {
    if let roas_arazzo::v1_1::SelectorType::Expression(expression) = kind
        && expression.type_ == ExpressionKind::Jsonpath
        && expression.version != "rfc9535"
    {
        return Err(SelectError::Unsupported("non-RFC9535 JSONPath"));
    }
    select::kind_of(kind)
}

impl<'d> Compiler<'d> {
    fn model_site(&self, path: &str) -> Site<'d> {
        let mut site = Site::root(path);
        if let Some(rest) = path.strip_prefix("#.workflows[")
            && let Some((index, rest)) = rest.split_once(']')
            && let Some(workflow) = index
                .parse::<usize>()
                .ok()
                .and_then(|index| self.description.workflows.get(index))
        {
            site.workflow = Some(workflow);
            if let Some(rest) = rest.strip_prefix(".steps[")
                && let Some((index, _)) = rest.split_once(']')
            {
                site.step = index
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| workflow.steps.get(index));
            }
        }
        site
    }

    fn finding(&mut self, site: &Site<'d>, offset: Option<usize>, issue: PreparationIssue) {
        self.diagnostics.push(PreparationDiagnostic {
            workflow_id: site.workflow.map(|workflow| workflow.workflow_id.clone()),
            step_id: site.step.map(|step| step.step_id.clone()),
            path: site.path.clone(),
            offset,
            issue,
        });
    }

    fn error(&mut self, site: &Site<'d>, error: impl Into<ExecutionError>) {
        let error = error.into();
        let offset = match &error {
            ExecutionError::Expression(ExpressionError::Syntax { offset, .. }) => Some(*offset),
            // The legacy public Syntax variant encodes its parser offset in the
            // message; preserve that variant's field shape for downstream users.
            ExecutionError::Criterion(CriterionError::Syntax { message, .. }) => message
                .strip_prefix("at byte ")
                .and_then(|message| message.split_once(':'))
                .and_then(|(offset, _)| offset.parse().ok()),
            _ => None,
        };
        self.finding(site, offset, PreparationIssue::Execution(error));
    }

    fn target(&mut self, id: &str, site: &Site<'d>, invocation: bool) {
        if id.starts_with('$') {
            self.error(
                site,
                ExecutionError::Unsupported(format!(
                    "external workflow `{id}` is not supported by this executor"
                )),
            );
        } else if let Some(index) = self
            .description
            .workflows
            .iter()
            .position(|workflow| workflow.workflow_id == id)
        {
            if invocation && !self.description.workflows[index].depends_on.is_empty() {
                self.error(site, ExecutionError::Unsupported(format!("called workflow `{id}` declares dependsOn; dependency scheduling for nested calls is not supported")));
            }
            self.pending.insert(index);
        } else {
            self.error(site, ExecutionError::UnknownWorkflow(id.into()));
        }
    }

    fn workflow(&mut self, index: usize) {
        let workflow = &self.description.workflows[index];
        let site = Site::workflow(index, workflow);
        for (index, id) in workflow.depends_on.iter().enumerate() {
            self.target(id, &site.item("dependsOn", index), false);
        }
        // Dependency cycles in a called/recovery workflow are invalid too.
        if let Err(error) = run::ordered_workflows(self.description, workflow) {
            self.error(&site.field("dependsOn"), error);
        }
        self.success_actions(&workflow.success_actions, &site, "successActions");
        self.failure_actions(&workflow.failure_actions, &site, "failureActions");
        for (name, value) in &workflow.outputs {
            self.value(value, &site.field(&format!("outputs.{name}")));
        }
        for (index, step) in workflow.steps.iter().enumerate() {
            let step_site = Site {
                step: Some(step),
                ..site.item("steps", index)
            };
            if let Some(id) = &step.workflow_id {
                self.target(id, &step_site.field("workflowId"), true);
            } else {
                let missing = self
                    .description
                    .source_descriptions
                    .iter()
                    .filter(|source| {
                        source.type_ != Some(SourceType::Arazzo)
                            && !self.options.sources.contains_key(&source.name)
                    })
                    .map(|source| source.name.clone())
                    .collect::<Vec<_>>();
                self.operation_sources(step, &step_site);
                if self.resolve_operations {
                    match operation::resolve(
                        step,
                        &self.options.sources,
                        &self.options.base_urls,
                        &missing,
                    ) {
                        Ok(endpoint) => {
                            self.endpoints
                                .insert((&workflow.workflow_id, &step.step_id), endpoint);
                        }
                        Err(error) => self.error(
                            &step_site.field(if step.operation_path.is_some() {
                                "operationPath"
                            } else {
                                "operationId"
                            }),
                            error,
                        ),
                    }
                }
            }
            let path = self.parameters(&site, &step_site);
            if let Some(endpoint) = self
                .endpoints
                .get(&(workflow.workflow_id.as_str(), step.step_id.as_str()))
                && let Err(reason) = run::url(endpoint, &path, &BTreeMap::new(), None)
            {
                self.error(
                    &step_site,
                    ExecutionError::BadRequest {
                        step: step.step_id.clone(),
                        reason,
                    },
                );
            }
            self.criteria(&step.success_criteria, &step_site, "successCriteria");
            self.success_actions(&step.on_success, &step_site, "onSuccess");
            self.failure_actions(&step.on_failure, &step_site, "onFailure");
            for (name, value) in &step.outputs {
                self.value(value, &step_site.field(&format!("outputs.{name}")));
            }
            if let Some(body) = &step.request_body {
                let body_site = step_site.field("requestBody");
                if let Some(payload) = &body.payload {
                    self.literal(payload, &body_site.field("payload"));
                }
                for (index, replacement) in body.replacements.iter().enumerate() {
                    let site = body_site.item("replacements", index);
                    self.value(&replacement.value, &site.field("value"));
                    let language = replacement
                        .target_selector_type
                        .as_ref()
                        .map(checked_selector_kind)
                        .unwrap_or(Ok(Language::Pointer));
                    match language {
                        Ok(language) => {
                            self.selector(language, &replacement.target, &site.field("target"))
                        }
                        Err(error) => self.error(&site.field("targetSelectorType"), error),
                    }
                }
            }
        }
        match run::order_steps(workflow, |step| {
            Ok(self
                .reads
                .get(&(workflow.workflow_id.as_str(), step.step_id.as_str()))
                .cloned()
                .unwrap_or_default())
        }) {
            Ok(order) => {
                self.orders.insert(&workflow.workflow_id, order);
            }
            Err(error) => self.error(&site.field("steps"), error),
        }
    }

    fn parameters(
        &mut self,
        workflow_site: &Site<'d>,
        step_site: &Site<'d>,
    ) -> BTreeMap<String, Value> {
        let step = step_site.step.expect("a step");
        let mut effective: Vec<(ParameterTemplate<'d>, Site<'d>)> = Vec::new();
        for (list, base, overrides) in [
            (
                &workflow_site.workflow.expect("a workflow").parameters,
                workflow_site,
                false,
            ),
            (&step.parameters, step_site, true),
        ] {
            for (index, entry) in list.iter().enumerate() {
                let site = Site {
                    step: Some(step),
                    ..base.item("parameters", index)
                };
                match run::parameter_templates(std::slice::from_ref(entry), self.description) {
                    Ok(templates) => {
                        for template in templates {
                            if overrides {
                                effective.retain(|(existing, _)| {
                                    !template.overrides(existing, step.workflow_id.is_some())
                                });
                            }
                            let site = if template.overridden.is_some() {
                                site.clone()
                            } else {
                                site.component(entry)
                            };
                            effective.push((template, site));
                        }
                    }
                    Err(error) => self.error(&site, error),
                }
            }
        }
        let mut path = BTreeMap::new();
        for (template, site) in effective {
            if template.location() == ParameterLocation::Path {
                path.insert(
                    template.parameter.name.clone(),
                    Value::String("prepared".into()),
                );
            }
            self.parameter(&template, &site, step.workflow_id.is_none());
        }
        path
    }

    fn need_source(&mut self, name: &str, site: &Site<'d>) {
        match self
            .description
            .source_descriptions
            .iter()
            .find(|source| source.name == name)
        {
            Some(source)
                if source.type_ == Some(SourceType::Arazzo)
                    || source.type_ == Some(SourceType::Asyncapi) =>
            {
                self.error(
                    site,
                    ExecutionError::Unsupported(format!(
                        "source `{name}` does not describe supported HTTP operations"
                    )),
                )
            }
            Some(_) => {
                self.required_sources.insert(name.into());
            }
            None => self.error(
                site,
                ExpressionError::Missing {
                    expression: name.into(),
                    what: "a source description not declared by this document".into(),
                },
            ),
        }
    }

    fn operation_sources(&mut self, step: &Step, site: &Site<'d>) {
        if step.channel_path.is_some() || step.action.is_some() {
            self.error(site, operation::OperationError::Async(step.step_id.clone()));
            return;
        }
        if let Some(operation) = &step.operation_id {
            let site = site.field("operationId");
            if let Some(rest) = operation.strip_prefix("$sourceDescriptions.") {
                match rest.split_once('.') {
                    Some((name, id)) if !id.is_empty() => self.need_source(name, &site),
                    _ => self.error(
                        &site,
                        operation::OperationError::Unknown {
                            operation: operation.clone(),
                        },
                    ),
                }
            } else if operation.starts_with('$') {
                self.error(
                    &site,
                    ExecutionError::Unsupported("dynamic operation IDs are not supported".into()),
                );
            } else {
                self.required_sources.extend(
                    self.description
                        .source_descriptions
                        .iter()
                        .filter(|source| source.type_ != Some(SourceType::Arazzo))
                        .map(|source| source.name.clone()),
                );
            }
        } else if let Some(path) = &step.operation_path {
            let site = site.field("operationPath");
            let Some((document, _)) = path.split_once('#') else {
                self.error(
                    &site,
                    operation::OperationError::BadPath {
                        path: path.clone(),
                        reason: "no operation pointer fragment".into(),
                    },
                );
                return;
            };
            if let Some(name) = document
                .trim()
                .strip_prefix("{$sourceDescriptions.")
                .and_then(|rest| rest.strip_suffix(".url}"))
            {
                self.need_source(name, &site);
            } else {
                let matching = self
                    .description
                    .source_descriptions
                    .iter()
                    .filter(|source| source.url == document)
                    .collect::<Vec<_>>();
                if matching.is_empty() {
                    self.error(
                        &site,
                        operation::OperationError::BadPath {
                            path: path.clone(),
                            reason: "no source description has that URL".into(),
                        },
                    );
                }
                for source in matching {
                    self.need_source(&source.name, &site);
                }
            }
        }
    }

    fn parameter(&mut self, template: &ParameterTemplate<'d>, site: &Site<'d>, operation: bool) {
        if operation
            && (template.parameter.in_.is_none()
                || template.location() == ParameterLocation::Channel)
        {
            self.error(
                &site.field("in"),
                ExecutionError::Unsupported(
                    "operation parameters require a supported HTTP location".into(),
                ),
            );
        }
        match template.overridden {
            Some(value) => self.literal(value, &site.field("value")),
            None => self.value(&template.parameter.value, &site.field("value")),
        }
    }

    fn arguments(&mut self, parameters: &'d [ReusableOr<Parameter>], site: &Site<'d>) {
        for (index, entry) in parameters.iter().enumerate() {
            let site = site.item("parameters", index);
            match run::parameter_templates(std::slice::from_ref(entry), self.description) {
                Ok(parameters) => {
                    for parameter in parameters {
                        let site = if parameter.overridden.is_some() {
                            site.clone()
                        } else {
                            site.component(entry)
                        };
                        self.parameter(&parameter, &site, false);
                    }
                }
                Err(error) => self.error(&site, error),
            }
        }
    }

    fn action_target(&mut self, step: Option<&str>, workflow: Option<&str>, site: &Site<'d>) {
        if let Some(id) = step
            && let Some(workflow) = site.workflow
            && !workflow.steps.iter().any(|step| step.step_id == id)
        {
            self.error(
                &site.field("stepId"),
                ExecutionError::UnknownStep {
                    workflow: workflow.workflow_id.clone(),
                    step: id.into(),
                },
            );
        }
        if let Some(id) = workflow {
            self.target(id, &site.field("workflowId"), true);
        }
    }

    fn success_actions(
        &mut self,
        actions: &'d [ReusableOr<roas_arazzo::v1_1::SuccessAction>],
        site: &Site<'d>,
        field: &str,
    ) {
        for (index, entry) in actions.iter().enumerate() {
            let site = site.item(field, index);
            match run::success_action(entry, self.description) {
                Ok(action) => {
                    let site = site.component(entry);
                    self.action_target(
                        action.step_id.as_deref(),
                        action.workflow_id.as_deref(),
                        &site,
                    );
                    self.criteria(&action.criteria, &site, "criteria");
                    self.arguments(&action.parameters, &site);
                }
                Err(error) => self.error(&site, error),
            }
        }
    }

    fn failure_actions(
        &mut self,
        actions: &'d [ReusableOr<roas_arazzo::v1_1::FailureAction>],
        site: &Site<'d>,
        field: &str,
    ) {
        for (index, entry) in actions.iter().enumerate() {
            let site = site.item(field, index);
            match run::failure_action(entry, self.description) {
                Ok(action) => {
                    let site = site.component(entry);
                    self.action_target(
                        action.step_id.as_deref(),
                        action.workflow_id.as_deref(),
                        &site,
                    );
                    self.criteria(&action.criteria, &site, "criteria");
                    self.arguments(&action.parameters, &site);
                    if action.retry_after.is_some_and(|seconds| {
                        std::time::Duration::try_from_secs_f64(seconds).is_err()
                    }) {
                        self.error(
                            &site.field("retryAfter"),
                            ExecutionError::Unsupported(
                                "retry delay cannot be represented as a duration".into(),
                            ),
                        );
                    }
                }
                Err(error) => self.error(&site, error),
            }
        }
    }

    fn criteria(&mut self, criteria: &'d [Criterion], site: &Site<'d>, field: &str) {
        for (index, criterion) in criteria.iter().enumerate() {
            let site = site.item(field, index);
            if let Some(context) = &criterion.context {
                self.expression(context, &site.field("context"), 0);
            }
            if let Some(CriterionType::Expression(expression)) = &criterion.type_
                && expression.type_ == ExpressionKind::Jsonpath
                && expression.version != "rfc9535"
            {
                self.error(
                    &site.field("type.version"),
                    ExecutionError::Unsupported(format!(
                        "JSONPath version `{}` is not supported; the executor implements rfc9535",
                        expression.version
                    )),
                );
            }
            let condition_site = site.field("condition");
            match criterion.type_.as_ref() {
                None | Some(CriterionType::Simple(CriterionKind::Simple)) => {
                    let tree = match self.compiled.conditions.get(criterion.condition.as_str()) {
                        Some(tree) => tree.clone(),
                        None => match criterion::parse(&criterion.condition) {
                            Ok(tree) => tree,
                            Err(error) => {
                                self.error(&condition_site, error);
                                continue;
                            }
                        },
                    };
                    tree.visit_expressions(&mut |parsed, offset| {
                        self.reference(parsed, &condition_site, offset);
                    });
                    if self.options.portability_lints && tree.uses_bare_values() {
                        self.finding(&condition_site, None, PreparationIssue::Portability("bare-value truthiness uses the roas profile; explicit comparisons or typed criteria can improve portability (advice, not a specification error)".into()));
                    }
                    self.compiled.conditions.insert(&criterion.condition, tree);
                }
                Some(kind) => {
                    let references = expression::interpolations(&criterion.condition);
                    let dynamic = !references.is_empty();
                    self.interpolations(&criterion.condition, &condition_site);
                    match kind {
                        CriterionType::Simple(CriterionKind::Regex)
                            if !dynamic
                                && !self
                                    .compiled
                                    .regexes
                                    .contains_key(criterion.condition.as_str()) =>
                        {
                            match criterion::compile_regex(&criterion.condition) {
                                Ok(regex) => {
                                    self.compiled.regexes.insert(&criterion.condition, regex);
                                }
                                Err(error) => self.error(&condition_site, error),
                            }
                        }
                        CriterionType::Simple(CriterionKind::Jsonpath)
                        | CriterionType::Expression(roas_arazzo::v1_1::ExpressionType {
                            type_: ExpressionKind::Jsonpath,
                            ..
                        }) if !dynamic => {
                            self.selector(Language::Path, &criterion.condition, &condition_site)
                        }
                        CriterionType::Expression(roas_arazzo::v1_1::ExpressionType {
                            type_: ExpressionKind::Jsonpointer,
                            ..
                        }) if !dynamic => {
                            self.selector(Language::Pointer, &criterion.condition, &condition_site)
                        }
                        CriterionType::Simple(CriterionKind::Xpath)
                        | CriterionType::Expression(roas_arazzo::v1_1::ExpressionType {
                            type_: ExpressionKind::Xpath,
                            ..
                        }) => self.error(&site.field("type"), CriterionError::Unsupported("XPath")),
                        _ => {}
                    }
                }
            }
        }
    }

    fn value(&mut self, value: &'d ValueOrSelector, site: &Site<'d>) {
        match value {
            ValueOrSelector::Literal(value) => self.literal(value, site),
            ValueOrSelector::Selector(selector) => {
                self.expression(&selector.context, &site.field("context"), 0);
                match checked_selector_kind(&selector.type_) {
                    Ok(language) => {
                        self.selector(language, &selector.selector, &site.field("selector"))
                    }
                    Err(error) => self.error(&site.field("type"), error),
                }
            }
        }
    }

    fn literal(&mut self, value: &'d Value, site: &Site<'d>) {
        match value {
            Value::String(text) if expression::is_expression(text) => {
                self.expression(text, site, 0)
            }
            Value::String(text) => self.interpolations(text, site),
            Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    self.literal(
                        value,
                        &Site {
                            path: format!("{}[{index}]", site.path),
                            ..site.clone()
                        },
                    );
                }
            }
            Value::Object(members) => {
                for (name, value) in members {
                    self.literal(value, &site.field(name));
                }
            }
            _ => {}
        }
    }

    fn interpolations(&mut self, text: &'d str, site: &Site<'d>) {
        let template = expression::template(text);
        for part in &template {
            if let expression::TemplatePart::Expression { text, offset } = part {
                self.expression(text, site, *offset);
            }
        }
        self.compiled.templates.insert(text, template);
    }

    fn expression(&mut self, text: &'d str, site: &Site<'d>, offset: usize) {
        let parsed = match self.compiled.expressions.get(text) {
            Some(parsed) => parsed.clone(),
            None => match runtime_syntax::parse(text) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let relative = match error {
                        ExpressionError::Syntax { offset, .. } => offset,
                        _ => 0,
                    };
                    self.finding(
                        site,
                        Some(offset + relative),
                        PreparationIssue::Execution(error.into()),
                    );
                    return;
                }
            },
        };
        self.reference(&parsed, site, offset);
        self.compiled.expressions.insert(text, parsed);
    }

    fn reference(&mut self, parsed: &Expression<'_>, site: &Site<'d>, offset: usize) {
        let error = match parsed.root {
            Root::Steps => {
                let id = parsed.parts[0];
                if !site
                    .workflow
                    .is_some_and(|workflow| workflow.steps.iter().any(|step| step.step_id == id))
                {
                    Some(ExecutionError::UnknownStep {
                        workflow: site
                            .workflow
                            .map(|workflow| workflow.workflow_id.clone())
                            .unwrap_or_default(),
                        step: id.into(),
                    })
                } else {
                    if let (Some(workflow), Some(step)) = (site.workflow, site.step)
                        && step.step_id != id
                    {
                        self.reads
                            .entry((&workflow.workflow_id, &step.step_id))
                            .or_default()
                            .insert(id.into());
                    }
                    None
                }
            }
            Root::Workflows
                if !self
                    .description
                    .workflows
                    .iter()
                    .any(|workflow| workflow.workflow_id == parsed.parts[0]) =>
            {
                Some(ExecutionError::UnknownWorkflow(parsed.parts[0].into()))
            }
            Root::Message => Some(ExpressionError::Unsupported(parsed.text.into()).into()),
            Root::Sources
                if !self
                    .description
                    .source_descriptions
                    .iter()
                    .any(|source| source.name == parsed.parts[0]) =>
            {
                Some(
                    ExpressionError::Missing {
                        expression: parsed.text.into(),
                        what: "a source description not declared by this document".into(),
                    }
                    .into(),
                )
            }
            Root::Self_ if self.description.self_.is_none() => Some(
                ExpressionError::Missing {
                    expression: parsed.text.into(),
                    what: "`$self`, which this description does not set".into(),
                }
                .into(),
            ),
            Root::Components if parsed.parts.len() >= 2 => {
                let exists = self
                    .description
                    .components
                    .as_ref()
                    .is_some_and(|components| match parsed.parts[0] {
                        "parameters" => components.parameters.contains_key(parsed.parts[1]),
                        "successActions" => {
                            components.success_actions.contains_key(parsed.parts[1])
                        }
                        "failureActions" => {
                            components.failure_actions.contains_key(parsed.parts[1])
                        }
                        "inputs" => components.inputs.contains_key(parsed.parts[1]),
                        _ => false,
                    });
                (!exists).then(|| {
                    ExpressionError::Missing {
                        expression: parsed.text.into(),
                        what: "a component not declared by this document".into(),
                    }
                    .into()
                })
            }
            _ => None,
        };
        if let Some(error) = error {
            self.finding(site, Some(offset), PreparationIssue::Execution(error));
        }
    }

    fn selector(&mut self, language: Language, text: &'d str, site: &Site<'d>) {
        match language {
            Language::Path => {
                if !self.compiled.paths.contains_key(text) {
                    match select::compile_path(text) {
                        Ok(path) => {
                            self.compiled.paths.insert(text, path);
                        }
                        Err(error) => self.error(site, error),
                    }
                }
            }
            Language::Pointer => {
                let pointer = text.strip_prefix('#').unwrap_or(text);
                let bad_escape = pointer
                    .as_bytes()
                    .windows(2)
                    .any(|pair| pair[0] == b'~' && !matches!(pair[1], b'0' | b'1'))
                    || pointer.ends_with('~');
                if (!pointer.is_empty() && !pointer.starts_with('/')) || bad_escape {
                    self.error(site, SelectError::Malformed { selector: text.into(), kind: "JSON Pointer", message: "expected an empty pointer or slash-delimited tokens with only ~0/~1 escapes".into() });
                }
            }
        }
    }
}
