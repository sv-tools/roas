//! The engine: what to send, what it meant, and what to do next.
//!
//! [`Run`] performs no IO. It hands out a request, is handed a response,
//! and decides where that leaves the workflow — which is what lets one
//! engine serve a blocking caller, an async one, and a test with no
//! network at all.

use crate::criterion;
use crate::expression::{Exchange, Scope, StepState};
use crate::http::{HttpRequest, HttpResponse};
use crate::operation::{self, Source};
use crate::report::{CriterionOutcome, ExecutionError, ExecutionReport, Outcome, StepRecord};
use crate::select;
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

/// One workflow in progress.
struct Frame<'d> {
    workflow: &'d Workflow,
    inputs: Value,
    /// Step indices in the order `dependsOn` puts them.
    order: Vec<usize>,
    at: usize,
    steps: BTreeMap<String, StepState>,
    outputs: BTreeMap<String, Value>,
    attempts: BTreeMap<String, u32>,
    /// The step of the calling frame that is waiting for this one.
    caller: Option<String>,
    /// A `goto` to a workflow finishes the workflow it left.
    ends_caller: bool,
    outcome: Outcome,
}

/// The step waiting for a response.
struct Pending {
    step: usize,
    attempt: u32,
    exchange: Exchange,
    started: Instant,
}

/// A workflow run, one request at a time.
pub struct Run<'d> {
    description: &'d Description,
    options: &'d Options,
    /// `sourceDescriptions` as JSON, for `$sourceDescriptions.…`.
    sources: Value,
    /// `components` as JSON, for `$components.…`.
    components: Value,
    frames: Vec<Frame<'d>>,
    /// Workflows still to run — dependencies first, then the one asked
    /// for.
    queue: Vec<&'d Workflow>,
    finished: BTreeMap<String, BTreeMap<String, Value>>,
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
            sources,
            components,
            frames: Vec::new(),
            queue,
            finished: BTreeMap::new(),
            pending: None,
            wait: None,
            records: Vec::new(),
            taken: 0,
            report: None,
        };
        run.enter(first, Value::Object(options.inputs.clone()), None, false)?;
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

        let Some(frame) = self.frames.last_mut() else {
            return Err(ExecutionError::NotWaiting);
        };
        let step = &frame.workflow.steps[pending.step];
        let step_id = step.step_id.clone();
        let workflow_id = frame.workflow.workflow_id.clone();

        let scope = Scope {
            inputs: &frame.inputs,
            outputs: &frame.outputs,
            steps: &frame.steps,
            workflows: &self.finished,
            here: Some(&pending.exchange),
            sources: &self.sources,
            components: &self.components,
        };

        // Did the step do what it said it would?
        let mut criteria = Vec::with_capacity(step.success_criteria.len());
        let mut passed = true;
        if step.success_criteria.is_empty() {
            passed = (200..400).contains(&status);
        }
        for criterion in &step.success_criteria {
            let holds = criterion::passes(criterion, &scope)?;
            criteria.push(CriterionOutcome {
                condition: criterion.condition.clone(),
                passed: holds,
            });
            passed = passed && holds;
        }

        // Only a step that did what it said can name what it produced:
        // a failed one is about to be retried or given up on, and its
        // outputs would name parts of a response that is not there.
        let outputs = if passed {
            evaluate_outputs(&step.outputs, &scope)?
        } else {
            BTreeMap::new()
        };

        // What the step's outcome says to do next.
        let action = self.decide(pending.step, passed, &pending.exchange)?;
        let described = describe(&action);

        let frame = self.frames.last_mut().expect("the frame is still there");
        frame.steps.insert(
            step_id.clone(),
            StepState {
                exchange: Some(pending.exchange.clone()),
                outputs: outputs.clone(),
            },
        );
        self.records.push(StepRecord {
            workflow_id,
            step_id,
            attempt: pending.attempt,
            method: pending.exchange.request.method.clone(),
            url: pending.exchange.request.url.clone(),
            status: Some(status),
            criteria,
            passed,
            outputs,
            action: described,
            elapsed,
        });

        self.apply(action)
    }

    // ---- the steps of a run -----------------------------------------

    /// Push a frame for `workflow`.
    fn enter(
        &mut self,
        workflow: &'d Workflow,
        inputs: Value,
        caller: Option<String>,
        ends_caller: bool,
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
            order: ordered_steps(workflow)?,
            at: 0,
            steps: BTreeMap::new(),
            outputs: BTreeMap::new(),
            attempts: BTreeMap::new(),
            caller,
            ends_caller,
            outcome: Outcome::Succeeded,
        });
        Ok(())
    }

    /// Finish the top frame: name its outputs and give them to whoever
    /// is waiting.
    fn leave(&mut self) -> Result<(), ExecutionError> {
        let frame = self.frames.pop().expect("a frame to leave");
        let outputs = {
            let scope = Scope {
                inputs: &frame.inputs,
                outputs: &frame.outputs,
                steps: &frame.steps,
                workflows: &self.finished,
                here: None,
                sources: &self.sources,
                components: &self.components,
            };
            if frame.outcome == Outcome::Succeeded {
                evaluate_outputs(&frame.workflow.outputs, &scope)?
            } else {
                // A workflow that stopped early names outputs from steps
                // that never ran. What can be named is worth reporting;
                // the rest went with the steps.
                evaluate_what_it_can(&frame.workflow.outputs, &scope)
            }
        };
        self.finished
            .insert(frame.workflow.workflow_id.clone(), outputs.clone());

        match (frame.caller, self.frames.last_mut()) {
            // A called workflow hands its outputs to the step that
            // called it.
            (Some(step_id), Some(parent)) => {
                parent.steps.insert(
                    step_id,
                    StepState {
                        exchange: None,
                        outputs: outputs.clone(),
                    },
                );
                if frame.ends_caller {
                    parent.at = parent.order.len();
                    parent.outcome = frame.outcome;
                } else {
                    parent.at += 1;
                }
            }
            // A root workflow: its outputs are the run's, unless it was
            // only a dependency of the one that was asked for.
            _ => {
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
                    self.enter(next, inputs, None, false)?;
                }
            }
        }
        Ok(())
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
        let inputs = {
            let scope = Scope {
                inputs: &frame.inputs,
                outputs: &frame.outputs,
                steps: &frame.steps,
                workflows: &self.finished,
                here: None,
                sources: &self.sources,
                components: &self.components,
            };
            let mut inputs = Map::new();
            for parameter in parameters(&step.parameters, self.description, &scope)? {
                inputs.insert(parameter.name, parameter.value);
            }
            Value::Object(inputs)
        };
        self.enter(workflow, inputs, Some(step_id), false)
    }

    /// Assemble the request a step wants sent.
    fn build(&self, index: usize) -> Result<(HttpRequest, Exchange), ExecutionError> {
        let frame = self.frames.last().expect("a frame to build in");
        let step = &frame.workflow.steps[index];
        let endpoint = operation::resolve(step, &self.options.sources, &self.options.base_urls)?;
        let scope = Scope {
            inputs: &frame.inputs,
            outputs: &frame.outputs,
            steps: &frame.steps,
            workflows: &self.finished,
            here: None,
            sources: &self.sources,
            components: &self.components,
        };

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
        exchange: &Exchange,
    ) -> Result<Action, ExecutionError> {
        let frame = self.frames.last().expect("a frame to decide in");
        let step = &frame.workflow.steps[index];
        let scope = Scope {
            inputs: &frame.inputs,
            outputs: &frame.outputs,
            steps: &frame.steps,
            workflows: &self.finished,
            here: Some(exchange),
            sources: &self.sources,
            components: &self.components,
        };

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
        for action in actions {
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
                FailureActionType::Retry => Action::Retry {
                    after: action.retry_after,
                    limit: action.retry_limit,
                },
            });
        }
        // Nothing said what to do about a failure, so the workflow stops
        // where it is.
        Ok(Action::End(Outcome::Failed))
    }

    /// Carry an action out.
    fn apply(&mut self, action: Action) -> Result<(), ExecutionError> {
        let limit = self.options.limits.retries;
        let frame = self.frames.last_mut().expect("a frame to act in");
        match action {
            Action::Advance => {
                frame.at += 1;
                Ok(())
            }
            Action::End(outcome) => {
                frame.outcome = outcome;
                frame.at = frame.order.len();
                // An ended workflow ends what called it, too: its caller
                // was waiting on this answer.
                if outcome == Outcome::Failed {
                    for frame in &mut self.frames {
                        frame.outcome = Outcome::Failed;
                    }
                }
                Ok(())
            }
            Action::Retry { after, limit: cap } => {
                let index = frame.order[frame.at];
                let step_id = frame.workflow.steps[index].step_id.clone();
                let attempts = frame.attempts.entry(step_id).or_insert(0);
                *attempts += 1;
                let cap = cap.map_or(limit, |cap| {
                    u32::try_from(cap).unwrap_or(u32::MAX).min(limit)
                });
                if *attempts > cap {
                    frame.outcome = Outcome::Failed;
                    frame.at = frame.order.len();
                    return Ok(());
                }
                if let Some(after) = after.filter(|after| *after > 0.0) {
                    self.wait = Some(Duration::from_secs_f64(after));
                }
                Ok(())
            }
            Action::Goto {
                step: Some(step_id),
                ..
            } => {
                let index = frame
                    .workflow
                    .steps
                    .iter()
                    .position(|step| step.step_id == step_id)
                    .ok_or_else(|| ExecutionError::UnknownStep {
                        workflow: frame.workflow.workflow_id.clone(),
                        step: step_id.clone(),
                    })?;
                frame.at = frame
                    .order
                    .iter()
                    .position(|&candidate| candidate == index)
                    .unwrap_or(frame.order.len());
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
                let inputs = {
                    let frame = self.frames.last().expect("a frame to leave");
                    let scope = Scope {
                        inputs: &frame.inputs,
                        outputs: &frame.outputs,
                        steps: &frame.steps,
                        workflows: &self.finished,
                        here: None,
                        sources: &self.sources,
                        components: &self.components,
                    };
                    let mut inputs = self.options.inputs.clone();
                    for parameter in parameters(&arguments, self.description, &scope)? {
                        inputs.insert(parameter.name, parameter.value);
                    }
                    Value::Object(inputs)
                };
                self.enter(workflow, inputs, Some(step_id), true)
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

/// What a step's outcome asks the run to do.
#[derive(Clone, Debug)]
enum Action {
    Advance,
    End(Outcome),
    Retry {
        after: Option<f64>,
        limit: Option<u64>,
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

/// The values a set of `outputs` names, skipping the ones that name
/// nothing — for a workflow that did not get far enough to have them.
fn evaluate_what_it_can(
    outputs: &BTreeMap<String, ValueOrSelector>,
    scope: &Scope<'_>,
) -> BTreeMap<String, Value> {
    outputs
        .iter()
        .filter_map(|(name, value)| {
            select::value_of(value, scope)
                .ok()
                .map(|value| (name.clone(), value))
        })
        .collect()
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

/// Step indices in an order that respects `dependsOn`, keeping the
/// document's order where it says nothing.
fn ordered_steps(workflow: &Workflow) -> Result<Vec<usize>, ExecutionError> {
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
