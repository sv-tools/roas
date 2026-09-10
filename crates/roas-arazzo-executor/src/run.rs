//! The engine: what to send, what it meant, and what to do next.
//!
//! [`Run`] performs no IO. It hands out a request, is handed a response,
//! and decides where that leaves the workflow — which is what lets one
//! engine serve a blocking caller, an async one, and a test with no
//! network at all.

use crate::criterion::{self, CriterionError};
use crate::expression::{self, Exchange, ExpressionError, Scope, StepState, WorkflowState};
use crate::http::{HttpRequest, HttpResponse};
use crate::operation::{self, Source};
use crate::report::{
    ActionCriteriaOutcome, CriterionOutcome, ExecutionError, ExecutionFailure, ExecutionReport,
    Outcome, Performed, StepRecord,
};
use crate::runtime_syntax::{self, Expression};
use crate::select;
use crate::select::SelectError;
use roas_arazzo::v1_1::{
    Criterion, Description, FailureActionType, Parameter, ParameterLocation, ReusableOr,
    SourceType, Step, SuccessActionType, ValueOrSelector, Workflow,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How far a run can go before it is treated as looping.
#[derive(Clone, Copy, Debug)]
struct Limits {
    steps: usize,
    depth: usize,
    retries: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: 1_000,
            depth: 8,
            retries: 10,
        }
    }
}

/// Everything a run needs besides the description itself.
///
/// Built by chaining: `Options::new().workflow("buyPet").input("petId", 7)`.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub(crate) workflow: Option<String>,
    inputs: Map<String, Value>,
    pub(crate) sources: BTreeMap<String, Source>,
    pub(crate) base_urls: BTreeMap<String, String>,
    pub(crate) validation: enumset::EnumSet<roas_arazzo::validation::ValidationOptions>,
    pub(crate) portability_lints: bool,
    headers: Vec<(String, String)>,
    limits: Limits,
}

impl Options {
    /// Structural validation exceptions used by [`crate::prepare`]. Legacy
    /// execution does not perform this validation pass.
    #[must_use]
    pub fn validation_options(
        mut self,
        options: enumset::EnumSet<roas_arazzo::validation::ValidationOptions>,
    ) -> Self {
        self.validation = options;
        self
    }

    /// Report bare-value truthiness as an advisory portability warning during
    /// preparation. This does not reject conditions or change their semantics.
    #[must_use]
    pub fn portability_lints(mut self, enabled: bool) -> Self {
        self.portability_lints = enabled;
        self
    }
    /// Options with nothing set: the first workflow, no inputs, no
    /// source documents.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run this workflow rather than the description's first.
    #[must_use]
    pub fn workflow(mut self, workflow_id: impl Into<String>) -> Self {
        self.workflow = Some(workflow_id.into());
        self
    }

    /// Set one workflow input.
    #[must_use]
    pub fn input(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.inputs.insert(name.into(), value.into());
        self
    }

    /// Set every input at once, from a JSON object.
    #[must_use]
    pub fn inputs(mut self, inputs: Value) -> Self {
        if let Value::Object(inputs) = inputs {
            self.inputs = inputs;
        }
        self
    }

    /// Supply a source description: the `name` it was declared with, the
    /// `url` it was declared with, and the parsed document.
    ///
    /// Fetching documents is IO, which this crate leaves to its caller —
    /// `roas-file-fetcher` and `roas-http-fetcher` do it for the loader
    /// and do it here just as well.
    #[must_use]
    pub fn source(
        mut self,
        name: impl Into<String>,
        url: impl Into<String>,
        document: Value,
    ) -> Self {
        self.sources.insert(
            name.into(),
            Source {
                url: url.into(),
                document,
                #[cfg(feature = "source-graph")]
                origin: None,
            },
        );
        self
    }

    /// Send this source description's requests somewhere else — a test
    /// server, a staging host — whatever its document says.
    #[must_use]
    pub fn base_url(mut self, source_name: impl Into<String>, url: impl Into<String>) -> Self {
        self.base_urls.insert(source_name.into(), url.into());
        self
    }

    /// Add a header to every request a step does not set itself.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The most steps a run may take before it is called a loop.
    #[must_use]
    pub fn max_steps(mut self, steps: usize) -> Self {
        self.limits.steps = steps;
        self
    }

    /// The deepest a workflow may call another.
    #[must_use]
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.limits.depth = depth;
        self
    }

    /// The most times one step may be retried.
    #[must_use]
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.limits.retries = retries;
        self
    }
}

/// What the engine wants next.
///
/// Deliberately not `#[non_exhaustive]`: a driving loop must handle
/// every variant, and a new one would have to break that loop to mean
/// anything — so a catch-all arm would hide the very change it was
/// there to absorb.
#[derive(Debug)]
pub enum Progress {
    /// Perform this request, then hand the response to
    /// [`Run::supply`].
    Send(HttpRequest),
    /// Wait this long — a retry asked for it — then carry on.
    Wait(Duration),
    /// The run is over.
    Done(Box<ExecutionReport>),
}

/// Everything a runtime expression can name at this point in the run.
fn scope<'s>(
    frame: &'s Frame<'_>,
    steps: &'s BTreeMap<String, StepState>,
    here: Option<&'s Exchange>,
    finished: &'s BTreeMap<String, WorkflowState>,
    ambient: &'s Ambient<'_>,
) -> Scope<'s> {
    Scope {
        compiled: ambient.compiled,
        inputs: &frame.inputs,
        outputs: &frame.outputs,
        steps,
        workflows: finished,
        here,
        sources: &ambient.sources,
        components: &ambient.components,
        self_: ambient.self_.as_deref(),
        declared_steps: &frame.declared,
        declared_workflows: &ambient.workflows,
    }
}

/// What the calling step does when the workflow it started finishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    /// Finish the calling step, as any other step finishes.
    Advance,
    /// Try the calling step again — a `retry` that named a workflow.
    Retry,
    /// End the workflow that left: a `goto` does not come back.
    EndCaller,
}

/// One workflow in progress.
struct Frame<'d> {
    workflow: &'d Workflow,
    inputs: Value,
    /// Step indices in the order `dependsOn` puts them.
    order: Vec<usize>,
    at: usize,
    steps: BTreeMap<String, StepState>,
    outputs: BTreeMap<String, Value>,
    /// How many times each step has been attempted, for the report and
    /// for the caller's safety limit.
    attempts: BTreeMap<String, u32>,
    /// Every step id this workflow declares.
    declared: BTreeSet<String>,
    /// How many retries each *failure action* has spent on a step.
    /// `retryLimit` belongs to the action that states it, so a step
    /// that fails two different ways gets both actions' budgets.
    retries: BTreeMap<(String, usize), u32>,
    /// The step of the calling frame that is waiting for this one, and
    /// what it does when this one is done.
    caller: Option<(String, Then)>,
    /// When this frame started, for a calling step's `timeout`.
    started: Instant,
    /// Where to come back to when a retry sent the run to another step
    /// first: "context transfers back upon completion".
    detour: Option<usize>,
    outcome: Outcome,
}

/// What a step's completion needs beyond what the step itself says.
struct Completion {
    /// The exchange, for a step that sent a request.
    exchange: Option<Exchange>,
    /// Outputs the step already has — a called workflow's, which its
    /// own `outputs` may then add to or override.
    given: BTreeMap<String, Value>,
    /// Whether it counts as passed when the step states no criteria.
    default_pass: bool,
    attempt: u32,
    performed: Performed,
    elapsed: Duration,
}

/// The step waiting for a response.
struct Pending {
    step: usize,
    attempt: u32,
    exchange: Exchange,
    started: Instant,
}

/// The parts of the description every expression can see, whichever
/// workflow is running.
struct Ambient<'d> {
    compiled: Option<&'d crate::prepare::Compiled<'d>>,
    /// `sourceDescriptions` as JSON, for `$sourceDescriptions.…`.
    sources: Value,
    /// `components` as JSON, for `$components.…`.
    components: Value,
    /// The description's `$self`, for `$self`.
    self_: Option<String>,
    /// Every workflow id the description declares.
    workflows: BTreeSet<String>,
}

/// A workflow run, one request at a time.
pub struct Run<'d> {
    description: &'d Description,
    options: &'d Options,
    ambient: Ambient<'d>,
    prepared: Option<&'d crate::PreparedWorkflow<'d>>,
    root_inputs: Map<String, Value>,
    frames: Vec<Frame<'d>>,
    /// Workflows still to run — dependencies first, then the one asked
    /// for.
    queue: Vec<&'d Workflow>,
    finished: BTreeMap<String, WorkflowState>,
    pending: Option<Pending>,
    wait: Option<Duration>,
    records: Vec<StepRecord>,
    /// Source descriptions the run was not given, and which could hold
    /// an operation — a bare `operationId` cannot be shown to be unique
    /// while one of these is missing.
    unsupplied: Vec<String>,
    taken: usize,
    report: Option<Box<ExecutionReport>>,
    halted: bool,
}

impl<'d> Run<'d> {
    /// Prepare a run: pick the workflow, order what it depends on, and
    /// stop before anything is sent.
    ///
    /// # Errors
    ///
    /// An unknown workflow, a dependency cycle, or an error encountered while
    /// ordering and syntax-checking the initial workflow. No requests are sent.
    pub fn start(
        description: &'d Description,
        options: &'d Options,
    ) -> Result<Self, ExecutionError> {
        Self::start_inner(description, options, None, options.inputs.clone())
    }

    pub(crate) fn start_prepared(
        prepared: &'d crate::PreparedWorkflow<'d>,
        inputs: Option<Map<String, Value>>,
    ) -> Result<Self, ExecutionError> {
        Self::start_inner(
            prepared.description,
            prepared.options,
            Some(prepared),
            inputs.unwrap_or_else(|| prepared.options.inputs.clone()),
        )
    }

    fn start_inner(
        description: &'d Description,
        options: &'d Options,
        prepared: Option<&'d crate::PreparedWorkflow<'d>>,
        root_inputs: Map<String, Value>,
    ) -> Result<Self, ExecutionError> {
        let wanted = match &options.workflow {
            Some(id) => description
                .workflows
                .iter()
                .find(|workflow| &workflow.workflow_id == id)
                .ok_or_else(|| ExecutionError::UnknownWorkflow(id.clone()))?,
            None => description
                .workflows
                .first()
                .ok_or_else(|| ExecutionError::UnknownWorkflow(String::new()))?,
        };

        let sources = serde_json::to_value(
            description
                .source_descriptions
                .iter()
                .map(|source| (source.name.clone(), source))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap_or(Value::Null);
        let components = description
            .components
            .as_ref()
            .and_then(|components| serde_json::to_value(components).ok())
            .unwrap_or(Value::Null);

        let mut queue = match prepared {
            Some(prepared) => prepared.queue.clone(),
            None => ordered_workflows(description, wanted)?,
        };
        let first = queue.remove(0);
        let mut run = Self {
            description,
            options,
            prepared,
            root_inputs: root_inputs.clone(),
            ambient: Ambient {
                compiled: prepared.map(|prepared| &prepared.compiled),
                sources,
                components,
                self_: description.self_.clone(),
                workflows: description
                    .workflows
                    .iter()
                    .map(|workflow| workflow.workflow_id.clone())
                    .collect(),
            },
            frames: Vec::new(),
            queue,
            finished: BTreeMap::new(),
            pending: None,
            wait: None,
            records: Vec::new(),
            unsupplied: description
                .source_descriptions
                .iter()
                // An Arazzo document holds workflows, not operations, so
                // its absence cannot make an operation ambiguous.
                .filter(|source| source.type_ != Some(SourceType::Arazzo))
                .filter(|source| !options.sources.contains_key(&source.name))
                .map(|source| source.name.clone())
                .collect(),
            taken: 0,
            report: None,
            halted: false,
        };
        run.enter(first, Value::Object(root_inputs), None)?;
        Ok(run)
    }

    /// Clone the history available so far without evaluating more expressions.
    ///
    /// Until completion the outcome is [`Outcome::Incomplete`], and workflow
    /// outputs are not available. Attempts with actual responses or completed
    /// workflow calls are retained even when output evaluation or dispatch fails.
    /// Requests that never received a response have no fabricated step record.
    /// After completion this returns the completed report, including its outputs.
    #[must_use]
    pub fn partial_report(&self) -> ExecutionReport {
        if let Some(report) = &self.report {
            return report.as_ref().clone();
        }
        ExecutionReport {
            workflow_id: self
                .options
                .workflow
                .clone()
                .or_else(|| {
                    self.description
                        .workflows
                        .first()
                        .map(|w| w.workflow_id.clone())
                })
                .unwrap_or_default(),
            outcome: Outcome::Incomplete,
            outputs: BTreeMap::new(),
            steps: self.records.clone(),
        }
    }

    pub(crate) fn failure(&self, error: ExecutionError) -> ExecutionFailure {
        ExecutionFailure {
            error,
            report: Some(Box::new(self.partial_report())),
        }
    }

    /// Advance until something is needed from the caller.
    ///
    /// Not called `next`: a run is not an iterator, because what comes
    /// out of it has to be answered with [`Run::supply`] before there is
    /// anything more to come.
    ///
    /// # Errors
    ///
    /// Whatever stopped the run — see [`ExecutionError`].
    /// After a terminal error only the partial report can be inspected; later
    /// calls return [`ExecutionError::Stopped`]. An outstanding-response
    /// [`ExecutionError::Awaiting`] is a correctable driving error, not terminal.
    pub fn advance(&mut self) -> Result<Progress, ExecutionError> {
        if self.halted {
            return Err(ExecutionError::Stopped);
        }
        let result = self.advance_inner();
        if let Err(error) = &result
            && !matches!(error, ExecutionError::Awaiting { .. })
        {
            self.halted = true;
        }
        result
    }

    fn advance_inner(&mut self) -> Result<Progress, ExecutionError> {
        if let Some(wait) = self.wait.take() {
            return Ok(Progress::Wait(wait));
        }
        // One request is outstanding at a time. Handing out another
        // would send it twice and lose the exchange the first one is
        // waiting to be judged by.
        if let Some(pending) = &self.pending {
            return Err(ExecutionError::Awaiting {
                method: pending.exchange.request.method.clone(),
                url: pending.exchange.request.url.clone(),
            });
        }
        loop {
            if let Some(report) = &self.report {
                return Ok(Progress::Done(report.clone()));
            }
            let Some(frame) = self.frames.last() else {
                return Err(ExecutionError::Stopped);
            };
            // The frame is spent: name its outputs and hand them back.
            if frame.at >= frame.order.len() {
                self.leave()?;
                continue;
            }

            self.taken += 1;
            if self.taken > self.options.limits.steps {
                return Err(ExecutionError::Limit {
                    limit: "step",
                    at: self.options.limits.steps,
                });
            }

            let index = frame.order[frame.at];
            let step = &frame.workflow.steps[index];
            if step.workflow_id.is_some() {
                self.call(index)?;
                continue;
            }

            let (request, exchange) = self.build(index)?;
            let attempt = self
                .frames
                .last()
                .and_then(|frame| frame.attempts.get(&step.step_id).copied())
                .unwrap_or(0)
                + 1;
            self.pending = Some(Pending {
                step: index,
                attempt,
                exchange,
                started: Instant::now(),
            });
            return Ok(Progress::Send(request));
        }
    }

    /// Hand back the response to the request [`Run::advance`] asked for.
    ///
    /// # Errors
    ///
    /// Unsupported capabilities, output evaluation failures, or action dispatch
    /// errors. Ordinary runtime criterion errors are recorded as failed criteria
    /// and follow failure actions. Terminal errors leave a partial report and
    /// stop later progress; [`ExecutionError::NotWaiting`] remains correctable.
    pub fn supply(&mut self, response: HttpResponse) -> Result<(), ExecutionError> {
        if self.halted {
            return Err(ExecutionError::Stopped);
        }
        let result = self.supply_inner(response);
        if let Err(error) = &result
            && !matches!(error, ExecutionError::NotWaiting)
        {
            self.halted = true;
        }
        result
    }

    fn supply_inner(&mut self, response: HttpResponse) -> Result<(), ExecutionError> {
        let Some(mut pending) = self.pending.take() else {
            return Err(ExecutionError::NotWaiting);
        };
        let elapsed = pending.started.elapsed();
        let status = response.status;
        pending.exchange.response_body = response.body_as_json();
        pending.exchange.response = Some(response);

        if self.frames.last().is_none() {
            return Err(ExecutionError::NotWaiting);
        }
        let performed = Performed::Request {
            method: pending.exchange.request.method.clone(),
            url: pending.exchange.request.url.clone(),
            status,
        };
        // No criteria means the status is the whole judgement.
        self.complete(
            pending.step,
            Completion {
                exchange: Some(pending.exchange),
                given: BTreeMap::new(),
                default_pass: (200..400).contains(&status),
                attempt: pending.attempt,
                performed,
                elapsed,
            },
        )
    }

    // ---- the steps of a run -----------------------------------------

    /// Everything a step's completion needs that the step itself does
    /// not say.
    ///
    /// A step ends the same way whether it sent a request or called a
    /// workflow: its criteria are judged, its outputs are named, and
    /// its actions decide where the workflow goes next.
    fn complete(&mut self, index: usize, done: Completion) -> Result<(), ExecutionError> {
        let frame = self.frames.last().expect("a frame to complete in");
        let step = &frame.workflow.steps[index];
        let step_id = step.step_id.clone();
        let workflow_id = frame.workflow.workflow_id.clone();

        // A real response/completed call already exists. Retain this attempt
        // before output evaluation or action selection can stop the engine.
        let record = self.records.len();
        self.records.push(StepRecord {
            workflow_id,
            step_id: step_id.clone(),
            attempt: done.attempt,
            performed: done.performed,
            criteria: Vec::with_capacity(step.success_criteria.len()),
            action_criteria: Vec::new(),
            passed: false,
            outputs: BTreeMap::new(),
            action: None,
            elapsed: done.elapsed,
        });

        // What the step produced is in scope while its own outputs are
        // named — that is how a workflow step reads what it called.
        let mut state = StepState {
            exchange: done.exchange.clone(),
            outputs: done.given.clone(),
            passed: true,
        };
        let (passed, outputs) = {
            let mut ahead = frame.steps.clone();
            ahead.insert(step_id.clone(), state.clone());
            let scope = scope(
                frame,
                &ahead,
                done.exchange.as_ref(),
                &self.finished,
                &self.ambient,
            );

            // Criteria, where a step states them, are the whole
            // judgement: a step that says `$statusCode == 404` means it.
            let mut passed = if step.success_criteria.is_empty() {
                done.default_pass
            } else {
                true
            };
            for criterion in &step.success_criteria {
                let holds =
                    evaluate_criterion(criterion, &scope, &mut self.records[record].criteria)?;
                passed = passed && holds;
            }
            // Only a step that did what it said can name what it
            // produced: a failed one is about to be retried or given up
            // on, and its outputs would name what is not there.
            let outputs = if passed {
                let mut outputs = done.given.clone();
                outputs.extend(evaluate_outputs(&step.outputs, &scope)?);
                outputs
            } else {
                // What it was handed only seeded the scope above: a
                // step that failed names nothing, a workflow step
                // included, so no recovery step reads a token from a
                // call that went wrong.
                BTreeMap::new()
            };
            (passed, outputs)
        };
        state.outputs = outputs.clone();
        state.passed = passed;

        // The step is in scope before its actions are chosen: an
        // `onSuccess` criterion reading `$steps.<this step>.outputs` is
        // asking about the step that just finished.
        let frame = self.frames.last_mut().expect("the frame is still there");
        frame.steps.insert(step_id.clone(), state);

        self.records[record].passed = passed;
        self.records[record].outputs = outputs;
        let action = self.decide(index, passed, done.exchange.as_ref(), record)?;
        self.records[record].action = describe(&action);
        self.apply(action)
    }

    /// Push a frame for `workflow`.
    fn enter(
        &mut self,
        workflow: &'d Workflow,
        inputs: Value,
        caller: Option<(String, Then)>,
    ) -> Result<(), ExecutionError> {
        if self.frames.len() >= self.options.limits.depth {
            return Err(ExecutionError::Limit {
                limit: "workflow depth",
                at: self.options.limits.depth,
            });
        }
        self.frames.push(Frame {
            workflow,
            inputs,
            order: match self.prepared {
                Some(prepared) => prepared
                    .orders
                    .get(workflow.workflow_id.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        ExecutionError::Unsupported("workflow is outside the prepared plan".into())
                    })?,
                None => ordered_steps(workflow, self.description)?,
            },
            at: 0,
            steps: BTreeMap::new(),
            outputs: BTreeMap::new(),
            declared: workflow
                .steps
                .iter()
                .map(|step| step.step_id.clone())
                .collect(),
            attempts: BTreeMap::new(),
            retries: BTreeMap::new(),
            caller,
            started: Instant::now(),
            detour: None,
            outcome: Outcome::Succeeded,
        });
        Ok(())
    }

    /// Finish the top frame: name its outputs and give them to whoever
    /// is waiting.
    fn leave(&mut self) -> Result<(), ExecutionError> {
        let frame = self.frames.pop().expect("a frame to leave");
        let outputs = {
            let scope = scope(&frame, &frame.steps, None, &self.finished, &self.ambient);
            if frame.outcome == Outcome::Succeeded {
                evaluate_outputs(&frame.workflow.outputs, &scope)?
            } else {
                // A workflow that stopped early names outputs from steps
                // that never ran. Those go with the steps; anything else
                // wrong with an output is still worth saying.
                evaluate_what_ran(&frame.workflow.outputs, &scope)?
            }
        };
        self.finished.insert(
            frame.workflow.workflow_id.clone(),
            WorkflowState {
                inputs: frame.inputs.clone(),
                outputs: outputs.clone(),
            },
        );

        let Some((step_id, then)) = frame.caller else {
            // A root workflow: its outputs are the run's, unless it was
            // only a dependency of the one that was asked for.
            if self.queue.is_empty() {
                self.report = Some(Box::new(ExecutionReport {
                    workflow_id: frame.workflow.workflow_id.clone(),
                    outcome: frame.outcome,
                    outputs,
                    steps: std::mem::take(&mut self.records),
                }));
            } else {
                let next = self.queue.remove(0);
                let inputs = Value::Object(self.root_inputs.clone());
                self.enter(next, inputs, None)?;
            }
            return Ok(());
        };
        let Some(parent) = self.frames.last() else {
            return Ok(());
        };
        let index = parent.order[parent.at];
        debug_assert_eq!(parent.workflow.steps[index].step_id, step_id);

        match then {
            // A `goto` handed the workflow over: what it did is what the
            // workflow that left it did, and there is nothing to come
            // back to.
            Then::EndCaller => {
                let parent = self.frames.last_mut().expect("the parent is still there");
                parent.steps.insert(
                    step_id,
                    StepState {
                        exchange: None,
                        outputs,
                        passed: frame.outcome != Outcome::Failed,
                    },
                );
                parent.at = parent.order.len();
                parent.outcome = frame.outcome;
                Ok(())
            }
            // A `retry` sent the run through another workflow first;
            // now the step that failed is tried again.
            Then::Retry => Ok(()),
            // An ordinary workflow step: it ends like any other step,
            // with its own criteria, outputs and actions.
            Then::Advance => {
                let elapsed = frame.started.elapsed();
                let timed_out = parent.workflow.steps[index]
                    .timeout
                    .and_then(|timeout| u64::try_from(timeout).ok())
                    .is_some_and(|timeout| elapsed > Duration::from_millis(timeout));
                let attempt = self
                    .frames
                    .last()
                    .and_then(|parent| parent.attempts.get(&step_id).copied())
                    .unwrap_or(0)
                    + 1;
                self.complete(
                    index,
                    Completion {
                        exchange: None,
                        given: outputs,
                        default_pass: frame.outcome != Outcome::Failed && !timed_out,
                        attempt,
                        performed: Performed::Workflow {
                            workflow_id: frame.workflow.workflow_id.clone(),
                            outcome: frame.outcome,
                        },
                        elapsed,
                    },
                )
            }
        }
    }

    /// A step that calls a workflow.
    fn call(&mut self, index: usize) -> Result<(), ExecutionError> {
        let frame = self.frames.last().expect("a frame to call from");
        let step = &frame.workflow.steps[index];
        let step_id = step.step_id.clone();
        let wanted = step.workflow_id.clone().unwrap_or_default();
        if wanted.starts_with('$') {
            return Err(ExecutionError::Unsupported(format!(
                "step `{step_id}` calls `{wanted}`, and this executor runs only workflows of the description it was given"
            )));
        }
        let workflow = self
            .description
            .workflows
            .iter()
            .find(|workflow| workflow.workflow_id == wanted)
            .ok_or_else(|| ExecutionError::UnknownWorkflow(wanted.clone()))?;

        // A workflow step's parameters are the workflow's inputs.
        // "When the step... specifies a `workflowId`, then all
        // parameters map to workflow inputs", and a workflow's own
        // parameters are "applicable for all steps described under this
        // workflow... can be overridden at the step level but cannot be
        // removed there" — so both lists go, the step's last.
        let scope = scope(frame, &frame.steps, None, &self.finished, &self.ambient);
        let mut inputs = Map::new();
        for parameter in effective_parameters(frame.workflow, step, self.description)? {
            let parameter = parameter.resolve(&scope)?;
            inputs.insert(parameter.name, parameter.value);
        }
        let inputs = Value::Object(inputs);
        self.enter(workflow, inputs, Some((step_id, Then::Advance)))
    }

    /// The inputs a called workflow starts with: the caller's own,
    /// with whatever parameters were passed to it written over them.
    fn arguments(&self, arguments: &[ReusableOr<Parameter>]) -> Result<Value, ExecutionError> {
        let frame = self.frames.last().expect("a frame to pass arguments from");
        let scope = scope(frame, &frame.steps, None, &self.finished, &self.ambient);
        // A called workflow starts with what it was passed and nothing
        // else: only the parameters are forwarded, not the caller's
        // whole input context, so a child reading `$inputs.x` is asking
        // for something the calling step gave it.
        let mut inputs = Map::new();
        for parameter in parameters(arguments, self.description, &scope)? {
            inputs.insert(parameter.name, parameter.value);
        }
        Ok(Value::Object(inputs))
    }

    /// Assemble the request a step wants sent.
    fn build(&self, index: usize) -> Result<(HttpRequest, Exchange), ExecutionError> {
        let frame = self.frames.last().expect("a frame to build in");
        let step = &frame.workflow.steps[index];
        let endpoint = match self.prepared {
            Some(prepared) => prepared
                .endpoints
                .get(&(frame.workflow.workflow_id.as_str(), step.step_id.as_str()))
                .cloned()
                .ok_or_else(|| {
                    ExecutionError::Unsupported("operation is outside the prepared plan".into())
                })?,
            None => operation::resolve(
                step,
                &self.options.sources,
                &self.options.base_urls,
                &self.unsupplied,
            )?,
        };
        let scope = scope(frame, &frame.steps, None, &self.finished, &self.ambient);

        // The workflow's parameters first, so a step's own override them.
        let resolved = effective_parameters(frame.workflow, step, self.description)?
            .into_iter()
            .map(|parameter| parameter.resolve(&scope))
            .collect::<Result<Vec<_>, _>>()?;

        let mut path = BTreeMap::new();
        let mut query = BTreeMap::new();
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut cookies = Vec::new();
        let mut querystring = None;
        for parameter in resolved {
            match parameter.location {
                ParameterLocation::Path => {
                    path.insert(parameter.name, parameter.value);
                }
                ParameterLocation::Query => {
                    query.insert(parameter.name, parameter.value);
                }
                ParameterLocation::Querystring => {
                    querystring = Some(text(&parameter.value));
                }
                ParameterLocation::Header => {
                    headers.push((parameter.name, text(&parameter.value)));
                }
                ParameterLocation::Cookie => {
                    cookies.push(format!("{}={}", parameter.name, text(&parameter.value)));
                }
                ParameterLocation::Channel => {
                    return Err(ExecutionError::Unsupported(format!(
                        "step `{}` has a `channel` parameter, which belongs to an AsyncAPI step",
                        step.step_id
                    )));
                }
            }
        }
        if !cookies.is_empty() {
            headers.push(("Cookie".to_owned(), cookies.join("; ")));
        }
        for (name, value) in &self.options.headers {
            if !headers
                .iter()
                .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
            {
                headers.push((name.clone(), value.clone()));
            }
        }

        let body = body(step, &scope)?;
        if let Some(body) = &body
            && !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".to_owned(), body.content_type.clone()));
        }

        let url = url(&endpoint, &path, &query, querystring.as_deref()).map_err(|reason| {
            ExecutionError::BadRequest {
                step: step.step_id.clone(),
                reason,
            }
        })?;
        let request = HttpRequest {
            method: endpoint.method,
            url,
            headers,
            body: body.as_ref().map(|body| body.bytes.clone()),
            timeout: step
                .timeout
                .and_then(|timeout| u64::try_from(timeout).ok())
                .map(Duration::from_millis),
        };
        Ok((
            request.clone(),
            Exchange {
                request,
                path,
                query,
                body: body.map(|body| body.value),
                response: None,
                response_body: None,
            },
        ))
    }

    /// Which action a step's outcome calls for.
    ///
    /// The exchange is in scope: `$statusCode == 503` is how a failure
    /// action says which failure it is about.
    fn decide(
        &mut self,
        index: usize,
        passed: bool,
        exchange: Option<&Exchange>,
        record: usize,
    ) -> Result<Action, ExecutionError> {
        let frame = self.frames.last().expect("a frame to decide in");
        let step = &frame.workflow.steps[index];
        let scope = scope(frame, &frame.steps, exchange, &self.finished, &self.ambient);
        let outcomes = &mut self.records[record].action_criteria;

        if passed {
            // A step's own actions first, then the workflow's.
            let actions = step
                .on_success
                .iter()
                .chain(frame.workflow.success_actions.iter());
            for action in actions {
                let action = success_action(action, self.description)?;
                if !holds(&action.name, &action.criteria, &scope, outcomes)? {
                    continue;
                }
                return Ok(match action.type_ {
                    SuccessActionType::End => Action::End(Outcome::Ended),
                    SuccessActionType::Goto => Action::Goto {
                        step: action.step_id.clone(),
                        workflow: action.workflow_id.clone(),
                        parameters: action.parameters.clone(),
                    },
                });
            }
            return Ok(Action::Advance);
        }

        let actions = step
            .on_failure
            .iter()
            .chain(frame.workflow.failure_actions.iter());
        for (at, action) in actions.enumerate() {
            let action = failure_action(action, self.description)?;
            if !holds(&action.name, &action.criteria, &scope, outcomes)? {
                continue;
            }
            return Ok(match action.type_ {
                FailureActionType::End => Action::End(Outcome::Failed),
                FailureActionType::Goto => Action::Goto {
                    step: action.step_id.clone(),
                    workflow: action.workflow_id.clone(),
                    parameters: action.parameters.clone(),
                },
                FailureActionType::Retry => {
                    // "A non-negative integer indicating how many
                    // attempts to retry the step MAY be attempted... If
                    // not specified then a single retry SHALL be
                    // attempted", and "The retryLimit MUST be exhausted
                    // prior to executing subsequent failure actions" —
                    // so an exhausted retry gives way to whatever the
                    // description says next rather than ending here.
                    let allowed = action
                        .retry_limit
                        .map_or(1, |limit| u32::try_from(limit).unwrap_or(u32::MAX));
                    let spent = frame
                        .retries
                        .get(&(step.step_id.clone(), at))
                        .copied()
                        .unwrap_or(0);
                    // The caller's cap is a rail against a description
                    // that would retry forever, counted over the step;
                    // the action's own limit is what the description
                    // asked for.
                    let taken = frame.attempts.get(&step.step_id).copied().unwrap_or(0);
                    if spent >= allowed || taken >= self.options.limits.retries {
                        continue;
                    }
                    Action::Retry {
                        at,
                        after: action.retry_after,
                        step: action.step_id.clone(),
                        workflow: action.workflow_id.clone(),
                        parameters: action.parameters.clone(),
                    }
                }
            });
        }
        // Nothing said what to do about a failure, so the workflow stops
        // where it is.
        Ok(Action::End(Outcome::Failed))
    }

    /// Carry an action out.
    fn apply(&mut self, action: Action) -> Result<(), ExecutionError> {
        let frame = self.frames.last_mut().expect("a frame to act in");
        match action {
            Action::Advance => {
                // A step run as a retry's detour hands control back to
                // the step that asked for it, which is tried again.
                match frame.detour.take() {
                    Some(back) => frame.at = back,
                    None => frame.at += 1,
                }
                Ok(())
            }
            Action::End(outcome) => {
                frame.outcome = outcome;
                frame.at = frame.order.len();
                // Nothing is forced on the caller here: a workflow that
                // ends failed comes back through the step that called
                // it, whose own `onFailure` may yet have something to
                // say about it.
                Ok(())
            }
            Action::Retry {
                at,
                after,
                step: target,
                workflow,
                parameters: arguments,
            } => {
                // `decide` has already refused a retry whose limit is
                // used up, so reaching here means another attempt is
                // owed. Count it before anything else.
                let index = frame.order[frame.at];
                let step_id = frame.workflow.steps[index].step_id.clone();
                *frame.attempts.entry(step_id.clone()).or_insert(0) += 1;
                *frame.retries.entry((step_id.clone(), at)).or_insert(0) += 1;
                if let Some(after) = after.filter(|after| *after > 0.0) {
                    self.wait = Some(Duration::from_secs_f64(after));
                }
                match (target, workflow) {
                    // "When used with `retry`, context transfers back
                    // upon completion of the specified step" — so the
                    // named step runs, then this one is tried again.
                    (Some(target), _) => {
                        let at = position_of(frame, &target)?;
                        frame.detour = Some(frame.at);
                        frame.at = at;
                        Ok(())
                    }
                    // The same, for a workflow.
                    (None, Some(workflow_id)) => {
                        let workflow = self
                            .description
                            .workflows
                            .iter()
                            .find(|workflow| workflow.workflow_id == workflow_id)
                            .ok_or_else(|| ExecutionError::UnknownWorkflow(workflow_id.clone()))?;
                        let inputs = self.arguments(&arguments)?;
                        self.enter(workflow, inputs, Some((step_id, Then::Retry)))
                    }
                    // Nothing named: this step, again.
                    (None, None) => Ok(()),
                }
            }
            Action::Goto {
                step: Some(step_id),
                ..
            } => {
                frame.at = position_of(frame, &step_id)?;
                Ok(())
            }
            Action::Goto {
                workflow: Some(workflow_id),
                parameters: arguments,
                ..
            } => {
                let index = frame.order[frame.at];
                let step_id = frame.workflow.steps[index].step_id.clone();
                let workflow = self
                    .description
                    .workflows
                    .iter()
                    .find(|workflow| workflow.workflow_id == workflow_id)
                    .ok_or_else(|| ExecutionError::UnknownWorkflow(workflow_id.clone()))?;
                let inputs = self.arguments(&arguments)?;
                self.enter(workflow, inputs, Some((step_id, Then::EndCaller)))
            }
            Action::Goto { .. } => Ok(()),
        }
    }
}

/// Where a step sits in the order its workflow runs.
fn position_of(frame: &Frame<'_>, step_id: &str) -> Result<usize, ExecutionError> {
    let index = frame
        .workflow
        .steps
        .iter()
        .position(|step| step.step_id == step_id)
        .ok_or_else(|| ExecutionError::UnknownStep {
            workflow: frame.workflow.workflow_id.clone(),
            step: step_id.to_owned(),
        })?;
    Ok(frame
        .order
        .iter()
        .position(|&candidate| candidate == index)
        .unwrap_or(frame.order.len()))
}

/// What a step's outcome asks the run to do.
#[derive(Clone, Debug)]
enum Action {
    Advance,
    End(Outcome),
    Retry {
        /// Which failure action asked, so its own budget is the one
        /// that is spent.
        at: usize,
        after: Option<f64>,
        /// A step to run before trying again, if the action names one.
        step: Option<String>,
        /// A workflow to run before trying again, if it names one.
        workflow: Option<String>,
        parameters: Vec<ReusableOr<Parameter>>,
    },
    Goto {
        step: Option<String>,
        workflow: Option<String>,
        parameters: Vec<ReusableOr<Parameter>>,
    },
}

fn describe(action: &Action) -> Option<String> {
    match action {
        Action::Advance => None,
        Action::End(Outcome::Failed) => Some("ended, failed".to_owned()),
        Action::End(_) => Some("ended".to_owned()),
        Action::Retry {
            step: Some(step), ..
        } => Some(format!("retry via step `{step}`")),
        Action::Retry {
            workflow: Some(workflow),
            ..
        } => Some(format!("retry via workflow `{workflow}`")),
        Action::Retry { .. } => Some("retry".to_owned()),
        Action::Goto {
            step: Some(step), ..
        } => Some(format!("goto step `{step}`")),
        Action::Goto {
            workflow: Some(workflow),
            ..
        } => Some(format!("goto workflow `{workflow}`")),
        Action::Goto { .. } => None,
    }
}

/// A parameter, resolved to a name, a place and a value.
struct Resolved {
    name: String,
    location: ParameterLocation,
    value: Value,
}

/// Resolve a list of parameters, following `Reusable` references into
/// the description's components.
fn parameters(
    list: &[ReusableOr<Parameter>],
    description: &Description,
    scope: &Scope<'_>,
) -> Result<Vec<Resolved>, ExecutionError> {
    parameter_templates(list, description)?
        .into_iter()
        .map(|parameter| parameter.resolve(scope))
        .collect()
}

pub(crate) struct ParameterTemplate<'a> {
    pub(crate) parameter: &'a Parameter,
    pub(crate) overridden: Option<&'a Value>,
}

impl ParameterTemplate<'_> {
    pub(crate) fn overrides(&self, existing: &Self, workflow_call: bool) -> bool {
        self.parameter.name == existing.parameter.name
            && (workflow_call || self.location() == existing.location())
    }
    pub(crate) fn location(&self) -> ParameterLocation {
        self.parameter.in_.unwrap_or(ParameterLocation::Query)
    }

    fn resolve(&self, scope: &Scope<'_>) -> Result<Resolved, ExecutionError> {
        let value = match self.overridden {
            Some(value) => select::resolve(value, scope)?,
            None => select::value_of(&self.parameter.value, scope)?,
        };
        Ok(Resolved {
            name: self.parameter.name.clone(),
            location: self.location(),
            value,
        })
    }
}

pub(crate) fn parameter_templates<'a>(
    list: &'a [ReusableOr<Parameter>],
    description: &'a Description,
) -> Result<Vec<ParameterTemplate<'a>>, ExecutionError> {
    let mut templates = Vec::with_capacity(list.len());
    for entry in list {
        let (parameter, overridden) = match entry {
            ReusableOr::Item(parameter) => (parameter, None),
            ReusableOr::Reusable(reusable) => {
                let name = reusable
                    .reference
                    .strip_prefix("$components.parameters.")
                    .ok_or_else(|| {
                        ExecutionError::Unsupported(format!(
                            "`{}` is not a component this executor can follow",
                            reusable.reference
                        ))
                    })?;
                let parameter = description
                    .components
                    .as_ref()
                    .and_then(|components| components.parameters.get(name))
                    .ok_or_else(|| {
                        ExecutionError::Unsupported(format!(
                            "`{}` names a component the description has not got",
                            reusable.reference
                        ))
                    })?;
                (parameter, reusable.value.as_ref())
            }
        };
        templates.push(ParameterTemplate {
            parameter,
            overridden,
        });
    }
    Ok(templates)
}

/// Apply overrides before inspecting or evaluating their values. Dependency
/// discovery and request assembly must agree about which expressions survive.
pub(crate) fn effective_parameters<'a>(
    workflow: &'a Workflow,
    step: &'a Step,
    description: &'a Description,
) -> Result<Vec<ParameterTemplate<'a>>, ExecutionError> {
    let mut templates = parameter_templates(&workflow.parameters, description)?;
    for parameter in parameter_templates(&step.parameters, description)? {
        templates.retain(|existing| !parameter.overrides(existing, step.workflow_id.is_some()));
        templates.push(parameter);
    }
    Ok(templates)
}

/// A success action, following a `Reusable` into the components.
pub(crate) fn success_action<'a>(
    entry: &'a ReusableOr<roas_arazzo::v1_1::SuccessAction>,
    description: &'a Description,
) -> Result<&'a roas_arazzo::v1_1::SuccessAction, ExecutionError> {
    match entry {
        ReusableOr::Item(action) => Ok(action),
        ReusableOr::Reusable(reusable) => reusable
            .reference
            .strip_prefix("$components.successActions.")
            .and_then(|name| {
                description
                    .components
                    .as_ref()
                    .and_then(|components| components.success_actions.get(name))
            })
            .ok_or_else(|| {
                ExecutionError::Unsupported(format!(
                    "`{}` names a component the description has not got",
                    reusable.reference
                ))
            }),
    }
}

/// A failure action, following a `Reusable` into the components.
pub(crate) fn failure_action<'a>(
    entry: &'a ReusableOr<roas_arazzo::v1_1::FailureAction>,
    description: &'a Description,
) -> Result<&'a roas_arazzo::v1_1::FailureAction, ExecutionError> {
    match entry {
        ReusableOr::Item(action) => Ok(action),
        ReusableOr::Reusable(reusable) => reusable
            .reference
            .strip_prefix("$components.failureActions.")
            .and_then(|name| {
                description
                    .components
                    .as_ref()
                    .and_then(|components| components.failure_actions.get(name))
            })
            .ok_or_else(|| {
                ExecutionError::Unsupported(format!(
                    "`{}` names a component the description has not got",
                    reusable.reference
                ))
            }),
    }
}

/// Whether every criterion of an action holds. No criteria means the
/// action applies.
fn holds(
    name: &str,
    criteria: &[Criterion],
    scope: &Scope<'_>,
    outcomes: &mut Vec<ActionCriteriaOutcome>,
) -> Result<bool, ExecutionError> {
    outcomes.push(ActionCriteriaOutcome {
        name: name.to_owned(),
        criteria: Vec::with_capacity(criteria.len()),
        passed: false,
    });
    let outcome = outcomes.last_mut().expect("the action just recorded");
    for criterion in criteria {
        if !evaluate_criterion(criterion, scope, &mut outcome.criteria)? {
            return Ok(false);
        }
    }
    outcome.passed = true;
    Ok(true)
}

/// Evaluation errors fail a condition and retain its diagnostic. Unsupported
/// capabilities are engine failures, not ordinary false conditions to skip.
fn evaluate_criterion(
    criterion: &Criterion,
    scope: &Scope<'_>,
    outcomes: &mut Vec<CriterionOutcome>,
) -> Result<bool, ExecutionError> {
    let (passed, error) = match criterion::passes(criterion, scope) {
        Ok(passed) => (passed, None),
        Err(error) => (false, Some(error)),
    };
    let terminal = error
        .as_ref()
        .filter(|error| {
            matches!(
                error,
                CriterionError::Unsupported(_)
                    | CriterionError::Expression(ExpressionError::Unsupported(_))
                    | CriterionError::Select(SelectError::Unsupported(_))
                    | CriterionError::Select(SelectError::Expression(
                        ExpressionError::Unsupported(_)
                    ))
            )
        })
        .cloned();
    outcomes.push(CriterionOutcome {
        condition: criterion.condition.clone(),
        passed,
        error,
    });
    if let Some(error) = terminal {
        return Err(error.into());
    }
    Ok(passed)
}

/// The values a set of `outputs` names, for a workflow that stopped
/// early.
///
/// An output naming a step or a workflow that never ran is expected and
/// skipped — that is what stopping early means. Nothing else is: an
/// input that was never given, a pointer into a body that has not got
/// it, a malformed selector, an unsupported expression — each is a
/// fault in the description, and a failed workflow is no reason to keep
/// quiet about it.
fn evaluate_what_ran(
    outputs: &BTreeMap<String, ValueOrSelector>,
    scope: &Scope<'_>,
) -> Result<BTreeMap<String, Value>, ExecutionError> {
    let mut named = BTreeMap::new();
    for (name, value) in outputs {
        match select::value_of(value, scope) {
            Ok(value) => {
                named.insert(name.clone(), value);
            }
            Err(SelectError::Expression(ExpressionError::NotRun { .. })) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(named)
}

/// The values a set of `outputs` names.
fn evaluate_outputs(
    outputs: &BTreeMap<String, ValueOrSelector>,
    scope: &Scope<'_>,
) -> Result<BTreeMap<String, Value>, ExecutionError> {
    let mut resolved = BTreeMap::new();
    for (name, value) in outputs {
        resolved.insert(name.clone(), select::value_of(value, scope)?);
    }
    Ok(resolved)
}

/// The body a step sends: what goes on the wire, what it means, and
/// what to call it.
struct Body {
    bytes: Vec<u8>,
    value: Value,
    content_type: String,
}

/// The body a step sends, with its replacements applied.
fn body(step: &Step, scope: &Scope<'_>) -> Result<Option<Body>, ExecutionError> {
    let Some(request_body) = &step.request_body else {
        return Ok(None);
    };
    let mut payload = match &request_body.payload {
        Some(payload) => select::resolve(payload, scope)?,
        None => Value::Null,
    };
    for replacement in &request_body.replacements {
        let value = select::value_of(&replacement.value, scope)?;
        let language = match &replacement.target_selector_type {
            Some(type_) => select::kind_of(type_)?,
            None => select::Language::Pointer,
        };
        select::place(
            language,
            &replacement.target,
            &mut payload,
            value,
            scope.compiled,
        )
        .map_err(|reason| ExecutionError::BadRequest {
            step: step.step_id.clone(),
            reason,
        })?;
    }
    let content_type = request_body
        .content_type
        .clone()
        .unwrap_or_else(|| "application/json".to_owned());
    // A string payload sent as anything but JSON goes as it is written;
    // everything else is JSON on the wire.
    let bytes = match (&payload, content_type.contains("json")) {
        (Value::String(text), false) => text.clone().into_bytes(),
        _ => payload.to_string().into_bytes(),
    };
    Ok(Some(Body {
        bytes,
        value: payload,
        content_type,
    }))
}

/// The URL a request goes to: the server, the path with its parameters
/// filled in, and the query.
pub(crate) fn url(
    endpoint: &operation::Endpoint,
    path: &BTreeMap<String, Value>,
    query: &BTreeMap<String, Value>,
    querystring: Option<&str>,
) -> Result<String, String> {
    let mut filled = endpoint.path.clone();
    for (name, value) in path {
        filled = filled.replace(&format!("{{{name}}}"), &encode(&text(value)));
    }
    if let Some(start) = filled.find('{') {
        return Err(format!(
            "`{}` still has `{}` in it, which no parameter filled in",
            endpoint.path,
            &filled[start
                ..filled[start..]
                    .find('}')
                    .map_or(filled.len(), |end| start + end + 1)]
        ));
    }
    let mut url = format!("{}{filled}", endpoint.base);
    let pairs: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(&text(value))))
        .collect();
    let query = match (pairs.is_empty(), querystring) {
        (true, None) => String::new(),
        (true, Some(raw)) => raw.to_owned(),
        (false, None) => pairs.join("&"),
        (false, Some(raw)) => format!("{}&{raw}", pairs.join("&")),
    };
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query);
    }
    url::Url::parse(&url).map_err(|error| format!("`{url}` is not a URL: {error}"))?;
    Ok(url)
}

/// A value as the text that goes into a URL or a header: a string as it
/// stands, anything else as its JSON.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Percent-encode everything a URL does not leave alone.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The workflows to run, in an order that respects `dependsOn`, ending
/// with the one that was asked for.
pub(crate) fn ordered_workflows<'d>(
    description: &'d Description,
    wanted: &'d Workflow,
) -> Result<Vec<&'d Workflow>, ExecutionError> {
    let mut ordered = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut done = BTreeSet::new();
    visit(description, wanted, &mut ordered, &mut visiting, &mut done)?;
    Ok(ordered)
}

fn visit<'d>(
    description: &'d Description,
    workflow: &'d Workflow,
    ordered: &mut Vec<&'d Workflow>,
    visiting: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
) -> Result<(), ExecutionError> {
    if done.contains(&workflow.workflow_id) {
        return Ok(());
    }
    if !visiting.insert(workflow.workflow_id.clone()) {
        return Err(ExecutionError::Circular(workflow.workflow_id.clone()));
    }
    for id in &workflow.depends_on {
        let dependency = description
            .workflows
            .iter()
            .find(|candidate| &candidate.workflow_id == id)
            .ok_or_else(|| ExecutionError::UnknownWorkflow(id.clone()))?;
        visit(description, dependency, ordered, visiting, done)?;
    }
    visiting.remove(&workflow.workflow_id);
    done.insert(workflow.workflow_id.clone());
    ordered.push(workflow);
    Ok(())
}

/// Parse the expressions a value evaluates, with no runtime value lookups.
/// The visitor may collect dependencies or just leave syntax checked.
fn visit_value_expressions(
    value: &ValueOrSelector,
    visitor: &mut impl FnMut(Expression<'_>),
) -> Result<(), ExecutionError> {
    match value {
        ValueOrSelector::Literal(literal) => visit_literal_expressions(literal, visitor),
        ValueOrSelector::Selector(selector) => {
            visitor(runtime_syntax::parse(&selector.context)?);
            Ok(())
        }
    }
}

fn visit_literal_expressions(
    value: &Value,
    visitor: &mut impl FnMut(Expression<'_>),
) -> Result<(), ExecutionError> {
    match value {
        Value::String(text) => {
            for reference in expression::references(text) {
                visitor(runtime_syntax::parse(reference)?);
            }
        }
        Value::Array(items) => {
            for item in items {
                visit_literal_expressions(item, visitor)?;
            }
        }
        Value::Object(members) => {
            for member in members.values() {
                visit_literal_expressions(member, visitor)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn visit_parameter_expressions(
    parameters: Vec<ParameterTemplate<'_>>,
    visitor: &mut impl FnMut(Expression<'_>),
) -> Result<(), ExecutionError> {
    for template in parameters {
        match template.overridden {
            Some(value) => visit_literal_expressions(value, visitor)?,
            None => visit_value_expressions(&template.parameter.value, visitor)?,
        }
    }
    Ok(())
}

fn visit_action_parameter_expressions(
    list: &[ReusableOr<Parameter>],
    description: &Description,
    visitor: &mut impl FnMut(Expression<'_>),
) -> Result<(), ExecutionError> {
    for entry in list {
        // An action's arguments are only resolved when it is selected.
        // Still inspect available expressions independently, even when another
        // argument cannot be resolved until dispatch reports its error.
        if let Ok(parameters) = parameter_templates(std::slice::from_ref(entry), description) {
            visit_parameter_expressions(parameters, visitor)?;
        }
    }
    Ok(())
}

/// Every step id a step's *expressions* read, which is a dependency
/// whether or not `dependsOn` says so.
///
/// "Tools MUST also treat runtime expression output references (e.g.,
/// `$steps.stepId.outputs.field`) as implicit dependencies" — so two
/// things matter. Only the fields that hold expressions are read, and
/// within them only what the runtime would really evaluate: a whole
/// `$…` string, a `{$…}` inside one, and in a condition the bare
/// operands its parser reads. A payload that merely mentions a step in
/// its text goes on the wire as text, and is no dependency at all.
///
/// A `Reusable` is followed into the components: where a parameter or
/// an action is written makes no difference to what it reads.
fn steps_named_by(
    step: &Step,
    workflow: &Workflow,
    description: &Description,
) -> Result<BTreeSet<String>, ExecutionError> {
    let mut found = BTreeSet::new();
    let mut collect = |parsed: Expression<'_>| {
        if let Some(id) = parsed.step_id() {
            found.insert(id.to_owned());
        }
    };

    fn read_criteria(
        list: &[Criterion],
        visitor: &mut impl FnMut(Expression<'_>),
    ) -> Result<(), ExecutionError> {
        for criterion in list {
            for parsed in criterion::references(criterion)? {
                visitor(parsed);
            }
        }
        Ok(())
    }

    visit_parameter_expressions(
        effective_parameters(workflow, step, description)?,
        &mut collect,
    )?;
    read_criteria(&step.success_criteria, &mut collect)?;
    for output in step.outputs.values() {
        visit_value_expressions(output, &mut collect)?;
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            visit_literal_expressions(payload, &mut collect)?;
        }
        for replacement in &body.replacements {
            visit_value_expressions(&replacement.value, &mut collect)?;
        }
    }
    for entry in &step.on_success {
        // Keep reference-resolution failures lazy: the other outcome, or an
        // earlier action, may make this action unreachable. Syntax failures in
        // an available action remain errors rather than erased dependencies.
        if let Ok(action) = success_action(entry, description) {
            read_criteria(&action.criteria, &mut collect)?;
            visit_action_parameter_expressions(&action.parameters, description, &mut collect)?;
        }
    }
    for entry in &step.on_failure {
        if let Ok(action) = failure_action(entry, description) {
            read_criteria(&action.criteria, &mut collect)?;
            visit_action_parameter_expressions(&action.parameters, description, &mut collect)?;
        }
    }

    found.remove(&step.step_id);
    Ok(found)
}

/// Step indices in an order that respects `dependsOn` and the steps an
/// expression reads, keeping the document's order where neither says
/// anything.
fn ordered_steps(
    workflow: &Workflow,
    description: &Description,
) -> Result<Vec<usize>, ExecutionError> {
    // Shared actions run against the state available at dispatch. Attaching
    // their reads to every step manufactures edges (and even mutual cycles).
    // Validate known criteria and parameters once, without collecting reads.
    // An unresolved reusable action is still diagnosed if dispatch reaches it.
    for entry in &workflow.success_actions {
        if let Ok(action) = success_action(entry, description) {
            for criterion in &action.criteria {
                criterion::references(criterion)?;
            }
            visit_action_parameter_expressions(&action.parameters, description, &mut |_| {})?;
        }
    }
    for entry in &workflow.failure_actions {
        if let Ok(action) = failure_action(entry, description) {
            for criterion in &action.criteria {
                criterion::references(criterion)?;
            }
            visit_action_parameter_expressions(&action.parameters, description, &mut |_| {})?;
        }
    }
    order_steps(workflow, |step| steps_named_by(step, workflow, description))
}

/// Both lazy execution and checked preparation use this ordering algorithm.
pub(crate) fn order_steps(
    workflow: &Workflow,
    mut reads: impl FnMut(&Step) -> Result<BTreeSet<String>, ExecutionError>,
) -> Result<Vec<usize>, ExecutionError> {
    let index: BTreeMap<&str, usize> = workflow
        .steps
        .iter()
        .enumerate()
        .map(|(at, step)| (step.step_id.as_str(), at))
        .collect();
    let mut ordered = Vec::with_capacity(workflow.steps.len());
    let mut visiting = BTreeSet::new();
    let mut done = BTreeSet::new();
    for step in &workflow.steps {
        visit_step(
            workflow,
            &mut reads,
            &index,
            step,
            &mut ordered,
            &mut visiting,
            &mut done,
        )?;
    }
    Ok(ordered)
}

fn visit_step(
    workflow: &Workflow,
    reads: &mut impl FnMut(&Step) -> Result<BTreeSet<String>, ExecutionError>,
    index: &BTreeMap<&str, usize>,
    step: &Step,
    ordered: &mut Vec<usize>,
    visiting: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
) -> Result<(), ExecutionError> {
    if done.contains(&step.step_id) {
        return Ok(());
    }
    if !visiting.insert(step.step_id.clone()) {
        return Err(ExecutionError::Circular(step.step_id.clone()));
    }
    for id in &step.depends_on {
        let at = index
            .get(id.as_str())
            .ok_or_else(|| ExecutionError::UnknownStep {
                workflow: workflow.workflow_id.clone(),
                step: id.clone(),
            })?;
        visit_step(
            workflow,
            reads,
            index,
            &workflow.steps[*at],
            ordered,
            visiting,
            done,
        )?;
    }
    // The same for the steps this one reads. A name that is not a step
    // of this workflow is left alone: an expression may be wrong, and
    // saying so belongs where it is evaluated, with the whole context.
    for id in reads(step)? {
        let Some(at) = index.get(id.as_str()) else {
            continue;
        };
        visit_step(
            workflow,
            reads,
            index,
            &workflow.steps[*at],
            ordered,
            visiting,
            done,
        )?;
    }
    visiting.remove(&step.step_id);
    done.insert(step.step_id.clone());
    ordered.push(index[step.step_id.as_str()]);
    Ok(())
}
