//! The engine: what to send, what it meant, and what to do next.
//!
//! [`Run`] performs no IO. It hands out a request, is handed a response,
//! and decides where that leaves the workflow — which is what lets one
//! engine serve a blocking caller, an async one, and a test with no
//! network at all.

use crate::criterion;
use crate::expression::{self, Exchange, ExpressionError, Scope, StepState, WorkflowState};
use crate::http::{HttpRequest, HttpResponse};
use crate::operation::{self, Source};
use crate::report::{
    CriterionOutcome, ExecutionError, ExecutionReport, Outcome, Performed, StepRecord,
};
use crate::select;
use crate::select::SelectError;
use roas_arazzo::v1_1::{
    Criterion, Description, FailureActionType, Parameter, ParameterLocation, ReusableOr, Step,
    SuccessActionType, ValueOrSelector, Workflow,
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
    workflow: Option<String>,
    inputs: Map<String, Value>,
    sources: BTreeMap<String, Source>,
    base_urls: BTreeMap<String, String>,
    headers: Vec<(String, String)>,
    limits: Limits,
}

impl Options {
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
    ambient: &'s Ambient,
) -> Scope<'s> {
    Scope {
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
struct Ambient {
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
    ambient: Ambient,
    frames: Vec<Frame<'d>>,
    /// Workflows still to run — dependencies first, then the one asked
    /// for.
    queue: Vec<&'d Workflow>,
    finished: BTreeMap<String, WorkflowState>,
    pending: Option<Pending>,
    wait: Option<Duration>,
    records: Vec<StepRecord>,
    taken: usize,
    report: Option<Box<ExecutionReport>>,
}

impl<'d> Run<'d> {
    /// Prepare a run: pick the workflow, order what it depends on, and
    /// stop before anything is sent.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::UnknownWorkflow`] or
    /// [`ExecutionError::Circular`] when the description does not
    /// describe a runnable order.
    pub fn start(
        description: &'d Description,
        options: &'d Options,
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

        let mut queue = ordered_workflows(description, wanted)?;
        let first = queue.remove(0);
        let mut run = Self {
            description,
            options,
            ambient: Ambient {
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
            taken: 0,
            report: None,
        };
        run.enter(first, Value::Object(options.inputs.clone()), None)?;
        Ok(run)
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
    pub fn advance(&mut self) -> Result<Progress, ExecutionError> {
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
            if let Some(report) = self.report.take() {
                return Ok(Progress::Done(report));
            }
            let Some(frame) = self.frames.last() else {
                return Ok(Progress::Done(Box::new(self.finish())));
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

    /// Hand back the response to the request [`Run::next`] asked for.
    ///
    /// # Errors
    ///
    /// Whatever the response made impossible — a criterion that cannot
    /// be decided, an output that names nothing, a `goto` with no
    /// target.
    pub fn supply(&mut self, response: HttpResponse) -> Result<(), ExecutionError> {
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

        // What the step produced is in scope while its own outputs are
        // named — that is how a workflow step reads what it called.
        let mut state = StepState {
            exchange: done.exchange.clone(),
            outputs: done.given.clone(),
            passed: true,
        };
        let (passed, criteria, outputs) = {
            let mut ahead = frame.steps.clone();
            ahead.insert(step_id.clone(), state.clone());
            let scope = scope(
                frame,
                &ahead,
                done.exchange.as_ref(),
                &self.finished,
                &self.ambient,
            );

            let mut criteria = Vec::with_capacity(step.success_criteria.len());
            // Criteria, where a step states them, are the whole
            // judgement: a step that says `$statusCode == 404` means it.
            let mut passed = if step.success_criteria.is_empty() {
                done.default_pass
            } else {
                true
            };
            for criterion in &step.success_criteria {
                let holds = criterion::passes(criterion, &scope)?;
                criteria.push(CriterionOutcome {
                    condition: criterion.condition.clone(),
                    passed: holds,
                });
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
            (passed, criteria, outputs)
        };
        state.outputs = outputs.clone();
        state.passed = passed;

        // The step is in scope before its actions are chosen: an
        // `onSuccess` criterion reading `$steps.<this step>.outputs` is
        // asking about the step that just finished.
        let frame = self.frames.last_mut().expect("the frame is still there");
        frame.steps.insert(step_id.clone(), state);

        let action = self.decide(index, passed, done.exchange.as_ref())?;
        let described = describe(&action);

        self.records.push(StepRecord {
            workflow_id,
            step_id,
            attempt: done.attempt,
            performed: done.performed,
            criteria,
            passed,
            outputs,
            action: described,
            elapsed: done.elapsed,
        });
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
            order: ordered_steps(workflow, self.description)?,
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
                let inputs = Value::Object(self.options.inputs.clone());
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
        let mut arguments = frame.workflow.parameters.clone();
        arguments.extend(step.parameters.iter().cloned());
        let inputs = self.arguments(&arguments)?;
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
        let endpoint = operation::resolve(step, &self.options.sources, &self.options.base_urls)?;
        let scope = scope(frame, &frame.steps, None, &self.finished, &self.ambient);

        // The workflow's parameters first, so a step's own override them.
        let mut resolved = parameters(&frame.workflow.parameters, self.description, &scope)?;
        for parameter in parameters(&step.parameters, self.description, &scope)? {
            resolved.retain(|existing| {
                !(existing.name == parameter.name && existing.location == parameter.location)
            });
            resolved.push(parameter);
        }

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
    ) -> Result<Action, ExecutionError> {
        let frame = self.frames.last().expect("a frame to decide in");
        let step = &frame.workflow.steps[index];
        let scope = scope(frame, &frame.steps, exchange, &self.finished, &self.ambient);

        if passed {
            // A step's own actions first, then the workflow's.
            let actions = step
                .on_success
                .iter()
                .chain(frame.workflow.success_actions.iter());
            for action in actions {
                let action = success_action(action, self.description)?;
                if !holds(&action.criteria, &scope)? {
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
            if !holds(&action.criteria, &scope)? {
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

    /// The report for a run that has nothing left to do.
    fn finish(&mut self) -> ExecutionReport {
        ExecutionReport {
            workflow_id: self
                .options
                .workflow
                .clone()
                .or_else(|| {
                    self.description
                        .workflows
                        .first()
                        .map(|workflow| workflow.workflow_id.clone())
                })
                .unwrap_or_default(),
            outcome: Outcome::Succeeded,
            outputs: BTreeMap::new(),
            steps: std::mem::take(&mut self.records),
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
    let mut resolved = Vec::with_capacity(list.len());
    for entry in list {
        let (parameter, overridden) = match entry {
            ReusableOr::Item(parameter) => (parameter.clone(), None),
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
                (parameter.clone(), reusable.value.clone())
            }
        };
        let value = match overridden {
            Some(value) => select::resolve(&value, scope)?,
            None => select::value_of(&parameter.value, scope)?,
        };
        resolved.push(Resolved {
            name: parameter.name,
            location: parameter.in_.unwrap_or(ParameterLocation::Query),
            value,
        });
    }
    Ok(resolved)
}

/// A success action, following a `Reusable` into the components.
fn success_action(
    entry: &ReusableOr<roas_arazzo::v1_1::SuccessAction>,
    description: &Description,
) -> Result<roas_arazzo::v1_1::SuccessAction, ExecutionError> {
    match entry {
        ReusableOr::Item(action) => Ok(action.clone()),
        ReusableOr::Reusable(reusable) => reusable
            .reference
            .strip_prefix("$components.successActions.")
            .and_then(|name| {
                description
                    .components
                    .as_ref()
                    .and_then(|components| components.success_actions.get(name))
            })
            .cloned()
            .ok_or_else(|| {
                ExecutionError::Unsupported(format!(
                    "`{}` names a component the description has not got",
                    reusable.reference
                ))
            }),
    }
}

/// A failure action, following a `Reusable` into the components.
fn failure_action(
    entry: &ReusableOr<roas_arazzo::v1_1::FailureAction>,
    description: &Description,
) -> Result<roas_arazzo::v1_1::FailureAction, ExecutionError> {
    match entry {
        ReusableOr::Item(action) => Ok(action.clone()),
        ReusableOr::Reusable(reusable) => reusable
            .reference
            .strip_prefix("$components.failureActions.")
            .and_then(|name| {
                description
                    .components
                    .as_ref()
                    .and_then(|components| components.failure_actions.get(name))
            })
            .cloned()
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
fn holds(criteria: &[Criterion], scope: &Scope<'_>) -> Result<bool, ExecutionError> {
    for criterion in criteria {
        if !criterion::passes(criterion, scope)? {
            return Ok(false);
        }
    }
    Ok(true)
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
        select::place(language, &replacement.target, &mut payload, value).map_err(|reason| {
            ExecutionError::BadRequest {
                step: step.step_id.clone(),
                reason,
            }
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
fn url(
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
fn ordered_workflows<'d>(
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
fn steps_named_by(step: &Step, description: &Description) -> BTreeSet<String> {
    let mut found = BTreeSet::new();

    /// The step id an expression names, if it names one.
    fn named_in(expression: &str) -> Option<String> {
        let rest = expression.strip_prefix("$steps.")?;
        let (id, _) = rest.split_once('.')?;
        Some(id.to_owned())
    }
    fn read(text: &str, found: &mut BTreeSet<String>) {
        found.extend(
            expression::references(text)
                .into_iter()
                .filter_map(named_in),
        );
    }
    fn read_condition(text: &str, found: &mut BTreeSet<String>) {
        found.extend(
            expression::references_in_condition(text)
                .into_iter()
                .filter_map(named_in),
        );
    }
    fn read_value(value: &ValueOrSelector, found: &mut BTreeSet<String>) {
        match value {
            ValueOrSelector::Literal(literal) => read_literal(literal, found),
            // A selector's context is an expression; its selector is a
            // JSONPath or a pointer, which the runtime does not
            // evaluate as one.
            ValueOrSelector::Selector(selector) => read(&selector.context, found),
        }
    }
    fn read_literal(value: &Value, found: &mut BTreeSet<String>) {
        match value {
            Value::String(text) => read(text, found),
            Value::Array(items) => items.iter().for_each(|item| read_literal(item, found)),
            Value::Object(members) => members
                .values()
                .for_each(|member| read_literal(member, found)),
            _ => {}
        }
    }
    fn read_parameters(
        list: &[ReusableOr<Parameter>],
        description: &Description,
        found: &mut BTreeSet<String>,
    ) {
        for entry in list {
            match entry {
                ReusableOr::Item(parameter) => read_value(&parameter.value, found),
                ReusableOr::Reusable(reusable) => {
                    // The override, and the component it names.
                    if let Some(overridden) = &reusable.value {
                        read_literal(overridden, found);
                    }
                    if let Some(parameter) = reusable
                        .reference
                        .strip_prefix("$components.parameters.")
                        .and_then(|name| {
                            description
                                .components
                                .as_ref()
                                .and_then(|components| components.parameters.get(name))
                        })
                    {
                        read_value(&parameter.value, found);
                    }
                }
            }
        }
    }
    fn read_criteria(list: &[Criterion], found: &mut BTreeSet<String>) {
        for criterion in list {
            if let Some(context) = &criterion.context {
                read(context, found);
            }
            read_condition(&criterion.condition, found);
        }
    }

    read_parameters(&step.parameters, description, &mut found);
    read_criteria(&step.success_criteria, &mut found);
    for output in step.outputs.values() {
        read_value(output, &mut found);
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            read_literal(payload, &mut found);
        }
        for replacement in &body.replacements {
            read_value(&replacement.value, &mut found);
        }
    }
    for entry in &step.on_success {
        if let Ok(action) = success_action(entry, description) {
            read_criteria(&action.criteria, &mut found);
            read_parameters(&action.parameters, description, &mut found);
        }
    }
    for entry in &step.on_failure {
        if let Ok(action) = failure_action(entry, description) {
            read_criteria(&action.criteria, &mut found);
            read_parameters(&action.parameters, description, &mut found);
        }
    }

    found.remove(&step.step_id);
    found
}

/// Step indices in an order that respects `dependsOn` and the steps an
/// expression reads, keeping the document's order where neither says
/// anything.
fn ordered_steps(
    workflow: &Workflow,
    description: &Description,
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
            description,
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
    description: &Description,
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
            description,
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
    for id in steps_named_by(step, description) {
        let Some(at) = index.get(id.as_str()) else {
            continue;
        };
        visit_step(
            workflow,
            description,
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
