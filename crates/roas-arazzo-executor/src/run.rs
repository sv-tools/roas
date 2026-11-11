//! The engine: what to send, what it meant, and what to do next.
//!
//! [`Run`] performs no IO. It hands out a request, is handed a response,
//! and decides where that leaves the workflow — which is what lets one
//! engine serve a blocking caller, an async one, and a test with no
//! network at all.

use crate::criterion::{self, CriterionError};
use crate::documents::{Doc, Document, Documents};
use crate::expression::{self, Exchange, ExpressionError, Scope, StepState, WorkflowState};
use crate::http::{HttpRequest, HttpResponse, RequestId};
use crate::message::{IncomingMessage, OutgoingMessage, Subscription, SubscriptionId};
use crate::operation::{self, Source};
use crate::report::{
    ActionCriteriaOutcome, CriterionOutcome, ExecutionError, ExecutionFailure, ExecutionReport,
    Outcome, Performed, StepRecord,
};
use crate::runtime_syntax::{self, Expression};
use crate::select;
use crate::select::SelectError;
use roas_arazzo::v1_1::{
    Criterion, Description, FailureActionType, Parameter, ParameterLocation, ResolveError,
    ReusableOr, SourceType, Step, StepAction, SuccessActionType, ValueOrSelector, Workflow,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

mod roots;

/// How far a run can go before it is treated as looping.
#[derive(Clone, Copy, Debug)]
struct Limits {
    steps: usize,
    depth: usize,
    retries: u32,
    /// How long the whole run may take; `None` is no limit.
    time: Option<Duration>,
    /// How many requests may be in flight at once.
    parallel: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: 1_000,
            depth: 8,
            retries: 10,
            time: None,
            parallel: 1,
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
    pub(crate) invalid_inputs: bool,
    pub(crate) input_validation: crate::InputValidation,
    pub(crate) input_schema_base: Option<url::Url>,
    pub(crate) schema_documents: BTreeMap<url::Url, Value>,
    pub(crate) sources: BTreeMap<String, Source>,
    pub(crate) base_urls: BTreeMap<String, String>,
    #[cfg(feature = "source-graph")]
    pub(crate) registry: Option<crate::source_registry::OperationContext>,
    #[cfg(feature = "source-graph")]
    pub(crate) workflow_documents:
        Option<std::sync::Arc<crate::source_registry::WorkflowDocuments>>,
    pub(crate) validation: enumset::EnumSet<roas_arazzo::validation::ValidationOptions>,
    pub(crate) portability_lints: bool,
    pub(crate) source_map: Option<std::sync::Arc<roas_arazzo::source_map::SourceMap>>,
    pub(crate) unknown_members: Option<roas_arazzo::unknown::UnknownMembers>,
    pub(crate) document_source_maps:
        BTreeMap<String, std::sync::Arc<roas_arazzo::source_map::SourceMap>>,
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

    /// Where the description's fields sit in the text it was read from.
    /// [`crate::prepare`] then gives each finding the line and column of
    /// its field ([`PreparationDiagnostic::span`]) and of the offending
    /// character inside a string ([`PreparationDiagnostic::position`]).
    /// Without a map, findings carry the model path and string offset only.
    ///
    /// The map must be of the text the description was parsed from: it is
    /// looked up by path and is never checked against the model.
    ///
    /// [`PreparationDiagnostic::span`]: crate::PreparationDiagnostic::span
    /// [`PreparationDiagnostic::position`]: crate::PreparationDiagnostic::position
    #[must_use]
    pub fn source_map(
        mut self,
        map: impl Into<std::sync::Arc<roas_arazzo::source_map::SourceMap>>,
    ) -> Self {
        self.source_map = Some(map.into());
        self
    }

    /// The members of the description that its model dropped, from
    /// `Description::unknown_members` on the value it was read from:
    /// [`crate::prepare`] reports each as an error, since the executor
    /// cannot honour what an unknown member means. Without them, nothing
    /// is known to have been dropped. External documents of a source
    /// registry carry their own
    /// (`SourceDocument::unknown_members`, with the `source-graph` feature).
    #[must_use]
    pub fn unknown_members(mut self, members: roas_arazzo::unknown::UnknownMembers) -> Self {
        self.unknown_members = Some(members);
        self
    }

    /// [`Self::source_map`] for an external Arazzo document: where the
    /// fields of the document with this canonical `identity` sit in the
    /// text it was read from. `identity` is compared as written with
    /// [`PreparationDiagnostic::document`]: the document's resolved
    /// `$self`, or the URI it was retrieved from (`SourceDocument::identity`
    /// with the `source-graph` feature). A later map for the same identity
    /// replaces the earlier one. External documents exist only with the
    /// `source-graph` feature; without it this map is never consulted.
    ///
    /// Set it on the options passed to [`crate::prepare`], before or after
    /// `source_registry`: every document's map is read from there.
    ///
    /// [`PreparationDiagnostic::document`]: crate::PreparationDiagnostic::document
    #[must_use]
    pub fn document_source_map(
        mut self,
        identity: impl Into<String>,
        map: impl Into<std::sync::Arc<roas_arazzo::source_map::SourceMap>>,
    ) -> Self {
        self.document_source_maps
            .insert(identity.into(), map.into());
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

    /// Set one workflow input. This does not clear an earlier non-object
    /// [`Self::inputs`] batch error; replace that batch with a JSON object first.
    #[must_use]
    pub fn input(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.inputs.insert(name.into(), value.into());
        self
    }

    /// Set every input at once, from a JSON object.
    /// A non-object is rejected by preparation/start, even when schema validation
    /// is disabled. A later object batch replaces it and clears that error.
    #[must_use]
    pub fn inputs(mut self, inputs: Value) -> Self {
        match inputs {
            Value::Object(inputs) => {
                self.inputs = inputs;
                self.invalid_inputs = false;
            }
            _ => self.invalid_inputs = true,
        }
        self
    }

    /// Supply a source description: the `name` it was declared with, the
    /// `url` it was declared with, and the parsed document.
    /// An absolute URL also supplies the document's retrieval base for relative
    /// references and servers. Use a source registry when identity and retrieval
    /// differ (for example after an HTTP redirect).
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
        let name = name.into();
        // An owned value is an API document, never a workflow source: an
        // earlier registry adaptation must not keep calling the old document.
        #[cfg(feature = "source-graph")]
        if let Some(table) = &mut self.workflow_documents
            && table.entry.contains_key(&name)
        {
            std::sync::Arc::make_mut(table).entry.remove(&name);
        }
        self.sources.insert(
            name,
            Source {
                url: url.into(),
                data: crate::operation::SourceData::Owned(document),
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

    /// The longest the whole run may take, from [`Run::start`] (or a
    /// prepared plan's `execute`) to its report. No limit by default.
    ///
    /// Each request's [`HttpRequest::timeout`](crate::HttpRequest::timeout)
    /// is cut to the time left, and a retry wait that would outlast it is
    /// not taken: the run stops with [`ExecutionError::TimedOut`] instead.
    /// So does a response that comes back after the time is spent, and a
    /// client failure then (see [`Run::client_failure`]). The limit is
    /// checked whenever the run hands control back or is given a response,
    /// so work in between — such as an XPath evaluation, which may take up
    /// to 2 seconds — can run past it before the run stops.
    /// A step that runs out of the run's time is not a failed step, so its
    /// `onFailure` actions do not run.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.limits.time = Some(timeout);
        self
    }

    /// Let up to `requests` steps be in flight at once (`0` counts as
    /// `1`). Steps run one at a time by default: Arazzo runs them in the
    /// order they are listed, and a step may rely on what an earlier one
    /// did without naming it.
    ///
    /// With a limit above one, a step starts while earlier ones are still
    /// in flight when it is an HTTP step with no `onSuccess` or
    /// `onFailure` actions of its own, in a workflow with no
    /// `successActions` or `failureActions`, and none of the steps it
    /// depends on (`dependsOn`) or reads (`$steps.<id>`) is still in
    /// flight. Every other step — one with actions, one that calls a
    /// workflow, an AsyncAPI step — starts alone, once everything before
    /// it is done, and nothing after it starts until it is done. Steps are
    /// judged in the order they are listed, whichever response comes back
    /// first, so a run ends or stops where it would one at a time. When a
    /// step fails and its workflow ends, requests still in flight in that
    /// workflow are abandoned: they are not recorded.
    ///
    /// The workflows the one asked for depends on run together as well,
    /// unless they could tell which ran first — they can run a common
    /// workflow, or one reads another's with `$workflows.<id>` — or any
    /// can reach an AsyncAPI step. Their steps are reported, and an error
    /// among them raised, as one after another; the step limit counts all
    /// of their steps.
    ///
    /// [`execute_async`](crate::execute_async) and its kin send requests
    /// in flight together; [`execute`](crate::execute) and its kin still
    /// send one at a time. A caller driving [`Run`] itself hands each
    /// response back with [`Run::supply_to`].
    #[must_use]
    pub fn parallel(mut self, requests: usize) -> Self {
        self.limits.parallel = requests.max(1);
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
    /// [`Run::supply`] — or, when several are in flight under
    /// [`Options::parallel`], to [`Run::supply_to`] with the request's
    /// `id`. With requests in flight, advance again before answering to
    /// see whether another can start.
    Send(HttpRequest),
    /// Nothing more can start until a request in flight is answered:
    /// hand one of their responses to [`Run::supply_to`], then advance.
    /// Only a run with [`Options::parallel`] above one asks for this.
    ///
    /// With a duration, a workflow running alongside is waiting that long
    /// to retry a step: advance once it has passed, even if no response
    /// came yet. Waiting for a response anyway only makes the retry later.
    Await(Option<Duration>),
    /// Wait this long — a retry asked for it — then carry on.
    Wait(Duration),
    /// Publish this message — an AsyncAPI `send` step — then advance. The
    /// step is done once it is sent.
    Publish(OutgoingMessage),
    /// Start listening — an AsyncAPI `receive` step — then advance. The
    /// run carries on while messages arrive; keep them until they are
    /// asked for.
    Subscribe(Subscription),
    /// A step now needs the next message on `subscription`: wait for it
    /// at most `timeout` (`None`: as long as it takes), then hand it — or
    /// `None` if nothing came — to [`Run::deliver`].
    Receive {
        /// The subscription to read from.
        subscription: SubscriptionId,
        /// How long the receive step still waits.
        timeout: Option<Duration>,
    },
    /// The run is done with this subscription: stop listening, then
    /// advance.
    Unsubscribe(SubscriptionId),
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
    /// Run the next workflow the waiting entry depends on, or, after the
    /// last, that entry itself.
    Dependency,
}

/// A call, `goto` or `retry` target waiting for the workflows it depends on.
struct Deferred<'d> {
    /// Dependencies still to run, in `dependsOn` order.
    remaining: Vec<(Doc, &'d Workflow)>,
    doc: Doc,
    workflow: &'d Workflow,
    inputs: Value,
    caller: (String, Then),
    /// When the entry was asked for: the calling step's `timeout` covers
    /// the dependencies too.
    started: Instant,
}

/// One workflow in progress.
struct Frame<'d> {
    /// The document declaring `workflow`, whose scope its expressions,
    /// components, sources and workflow references resolve in.
    doc: Doc,
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
    /// Receive steps listening in the background, oldest first.
    listening: Vec<Listening>,
    /// Where to come back to once a receive that was joined is done — the
    /// step that needed it, or the end of the workflow — and the detour
    /// that was pending there.
    /// A stack: a step a joined receive goes back to may itself join one.
    /// Each entry: the receive step joined, where to go back to, and the
    /// detour pending there. Only that receive's completion pops it.
    resume: Vec<(usize, usize, Option<usize>)>,
    /// An `end`, or a `goto` to another workflow, finished it.
    ended: bool,
    outcome: Outcome,
}

/// A receive step waiting for its message.
struct Listening {
    step: usize,
    subscription: SubscriptionId,
    attempt: u32,
    exchange: Exchange,
    started: Instant,
    /// The step's own `timeout`.
    timeout: Option<Duration>,
    /// The `correlationId` a message must carry, when the step gives one.
    expected: Option<Value>,
    /// Where the channel's messages carry their correlation id.
    locations: Vec<String>,
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
    /// The step failed whatever its criteria say — a receive that got no
    /// message — so they are not evaluated, and it names no outputs.
    failed: bool,
    /// It ran alongside other steps: the frame has already moved past it.
    alongside: bool,
    attempt: u32,
    performed: Performed,
    elapsed: Duration,
}

/// A step waiting for a response.
struct Pending {
    id: RequestId,
    /// It started while others were in flight; its completion does not
    /// move the frame on, which already moved past it.
    alongside: bool,
    /// Its response, kept until every step listed before it is judged:
    /// steps are judged in the order they are listed, as one at a time.
    answer: Option<(HttpResponse, Duration)>,
    step: usize,
    attempt: u32,
    exchange: Exchange,
    started: Instant,
}

/// The parts of one document every expression in it can see, whichever of
/// its workflows is running.
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

impl<'d> Ambient<'d> {
    fn of(description: &Description, compiled: Option<&'d crate::prepare::Compiled<'d>>) -> Self {
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
        Self {
            compiled,
            sources,
            components,
            self_: description.self_.clone(),
            workflows: description
                .workflows
                .iter()
                .map(|workflow| workflow.workflow_id.clone())
                .collect(),
        }
    }
}

/// A workflow run, one request at a time.
pub struct Run<'d> {
    description: &'d Description,
    options: &'d Options,
    documents: Documents<'d>,
    /// Per document, built when one of its workflows is first entered.
    ambients: Vec<Option<Ambient<'d>>>,
    prepared: Option<&'d crate::PreparedWorkflow<'d>>,
    root_inputs: Map<String, Value>,
    frames: Vec<Frame<'d>>,
    /// Workflows still to run — dependencies first, then the one asked
    /// for.
    queue: Vec<(Doc, &'d Workflow)>,
    /// Workflows that finished as a dependency — of the root, or of a
    /// call, `goto` or `retry` target — each of which runs once in a run.
    dependencies_done: BTreeSet<(Doc, String)>,
    /// Entries waiting for their dependencies, innermost last.
    deferred: Vec<Deferred<'d>>,
    /// Finished workflows, per document: `$workflows.<id>` names a workflow
    /// of the document the expression is written in.
    finished: Vec<BTreeMap<String, WorkflowState>>,
    /// Requests in flight, oldest first. More than one only under
    /// [`Options::parallel`], and all in the innermost frame.
    pending: Vec<Pending>,
    /// Requests handed out so far, for their ids.
    requests: u64,
    wait: Option<Duration>,
    records: Vec<StepRecord>,
    taken: usize,
    /// How many steps this run may take: [`Options::max_steps`], less
    /// what workflows running alongside it took.
    cap: usize,
    /// Workflows that finished in this run, as `finished` names them —
    /// what a workflow run alongside others hands back.
    fresh: BTreeSet<(Doc, String)>,
    /// The workflows the one asked for depends on, run together.
    roots: Option<roots::Roots<'d>>,
    report: Option<Box<ExecutionReport>>,
    halted: bool,
    /// Per document; validators are keyed by workflow id within it.
    input_schemas: Vec<crate::input::InputSchemas>,
    /// Per document, made when one of its steps first resolves an
    /// operation without a plan: each source is indexed once in a run, not
    /// once per request.
    resolvers: Vec<Option<crate::operation_index::Resolver<'d>>>,
    /// When the run's [`Options::timeout`] runs out.
    deadline: Option<Instant>,
    /// A published message whose step completes on the next advance.
    sending: Option<Pending>,
    /// The subscription a [`Progress::Receive`] is outstanding for.
    joining: Option<SubscriptionId>,
    /// Subscriptions to close, handed out before anything else.
    closing: Vec<SubscriptionId>,
    /// Subscriptions made so far, for the next one's id.
    subscriptions: u64,
}

/// The ambient scope of a document that has an entered workflow.
fn ambient<'a, 'd>(ambients: &'a [Option<Ambient<'d>>], doc: Doc) -> &'a Ambient<'d> {
    ambients[doc.0]
        .as_ref()
        .expect("a document's ambient scope exists once a workflow of it is entered")
}

/// Source descriptions a document was not given, and which could hold an
/// operation — a bare `operationId` cannot be shown to be unique while one of
/// these is missing.
pub(crate) fn unsupplied(document: &Document<'_>) -> Vec<String> {
    document
        .description
        .source_descriptions
        .iter()
        // An Arazzo document holds workflows, not operations, so its absence
        // cannot make an operation ambiguous.
        .filter(|source| source.type_ != Some(SourceType::Arazzo))
        .filter(|source| !document.options.sources.contains_key(&source.name))
        .map(|source| source.name.clone())
        .collect()
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
        // Started first, so the time spent ordering and checking inputs
        // counts too.
        let deadline = options
            .limits
            .time
            .and_then(|time| Instant::now().checked_add(time));
        options.check_inputs()?;
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

        let documents = Documents::new(description, options);
        let queue = match prepared {
            Some(prepared) => prepared.queue.clone(),
            None => ordered_workflows(&documents, Doc::ENTRY, wanted)?,
        };
        let mut input_schemas: Vec<_> = (0..documents.len())
            .map(|_| crate::input::InputSchemas::default())
            .collect();
        let initial_inputs = Value::Object(root_inputs.clone());
        // Fail before any dependency can send when the selected workflow (or a
        // root dependency) cannot accept the shared initial argument object.
        for (doc, workflow) in &queue {
            let document = documents.get(*doc);
            match prepared {
                Some(prepared) => prepared.input_schemas[doc.0].validate(workflow, &initial_inputs),
                None => input_schemas[doc.0]
                    .compile(document.description, options, document.options, workflow)
                    .and_then(|()| input_schemas[doc.0].validate(workflow, &initial_inputs)),
            }
            .map_err(|error| error.in_document(document.identity))?;
        }
        // Under `parallel`, the workflows the one asked for depends on run
        // together where nothing one does can change another.
        let roots = (options.limits.parallel > 1 && queue.len() > 2)
            .then(|| roots::plan(&documents, &queue[..queue.len() - 1]))
            .flatten();
        let mut run = Self {
            description,
            options,
            prepared,
            root_inputs,
            ambients: (0..documents.len()).map(|_| None).collect(),
            resolvers: (0..documents.len()).map(|_| None).collect(),
            finished: vec![BTreeMap::new(); documents.len()],
            documents,
            frames: Vec::new(),
            queue,
            dependencies_done: BTreeSet::new(),
            deferred: Vec::new(),
            pending: Vec::new(),
            requests: 0,
            wait: None,
            records: Vec::new(),
            taken: 0,
            cap: options.limits.steps,
            fresh: BTreeSet::new(),
            roots: None,
            report: None,
            halted: false,
            input_schemas,
            deadline,
            sending: None,
            joining: None,
            closing: Vec::new(),
            subscriptions: 0,
        };
        match roots {
            Some(entries) => {
                // Only the one asked for is left to run after them.
                run.queue.drain(..run.queue.len() - 1);
                run.start_roots(entries)?;
            }
            None => {
                let (doc, first) = run.queue.remove(0);
                run.enter(doc, first, initial_inputs, None)?;
            }
        }
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
            input_validation: self.options.input_validation,
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
            steps: match &self.roots {
                Some(roots) => roots.records(),
                None => self.records.clone(),
            },
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
        let result = self
            .advance_inner()
            .and_then(|progress| self.in_time(progress));
        if let Err(error) = &result
            && !matches!(error, ExecutionError::Awaiting { .. })
        {
            self.halted = true;
        }
        result
    }

    /// Fit what the run asks for into the time it has left.
    fn in_time(&self, progress: Progress) -> Result<Progress, ExecutionError> {
        let Some(deadline) = self.deadline else {
            return Ok(progress);
        };
        let left = deadline.saturating_duration_since(Instant::now());
        match progress {
            Progress::Done(_) | Progress::Unsubscribe(_) | Progress::Await(_) => Ok(progress),
            Progress::Wait(wait) if wait < left => Ok(Progress::Wait(wait)),
            Progress::Send(mut request) if !left.is_zero() => {
                request.timeout = Some(request.timeout.map_or(left, |own| own.min(left)));
                Ok(Progress::Send(request))
            }
            Progress::Receive {
                subscription,
                timeout,
            } if !left.is_zero() => Ok(Progress::Receive {
                subscription,
                timeout: Some(timeout.map_or(left, |own| own.min(left))),
            }),
            Progress::Publish(_) | Progress::Subscribe(_) if !left.is_zero() => Ok(progress),
            Progress::Wait(_)
            | Progress::Send(_)
            | Progress::Receive { .. }
            | Progress::Publish(_)
            | Progress::Subscribe(_) => Err(self.timed_out()),
        }
    }

    fn timed_out(&self) -> ExecutionError {
        ExecutionError::TimedOut {
            after: self.options.limits.time.unwrap_or_default(),
        }
    }

    fn out_of_time(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// What a client that could not carry out the request
    /// [`Run::advance`] asked for stops the run with, for a caller driving
    /// the run itself: [`ExecutionError::TimedOut`] once the run's
    /// [`Options::timeout`] is spent — the request's timeout was cut to
    /// it, so that is most likely why the client gave up — and
    /// [`ExecutionError::Client`] otherwise. The `execute` functions use it.
    #[must_use]
    pub fn client_failure(&self, error: crate::ClientError) -> ExecutionError {
        if self.out_of_time() {
            self.timed_out()
        } else {
            error.into()
        }
    }

    /// Every subscription still open — listening, or done with but not yet
    /// handed out as [`Progress::Unsubscribe`]. A caller driving the run
    /// itself closes these when it stops early, as the `execute` functions
    /// do. One may never have been subscribed — its `Subscribe` failed, or
    /// the run stopped before handing it out.
    #[must_use]
    pub fn open_subscriptions(&self) -> Vec<SubscriptionId> {
        let mut open = self.closing.clone();
        for frame in &self.frames {
            open.extend(
                frame
                    .listening
                    .iter()
                    .map(|listening| listening.subscription),
            );
        }
        open.sort();
        open
    }

    fn advance_inner(&mut self) -> Result<Progress, ExecutionError> {
        if self.roots.is_some() {
            return self.advance_roots();
        }
        // A subscription that is done with is closed before a retry waits.
        if self.pending.is_empty()
            && self.joining.is_none()
            && let Some(subscription) = self.closing.pop()
        {
            return Ok(Progress::Unsubscribe(subscription));
        }
        if let Some(wait) = self.wait.take() {
            return Ok(Progress::Wait(wait));
        }
        // One request is outstanding at a time. Handing out another
        // would send it twice and lose the exchange the first one is
        // waiting to be judged by.
        if self.options.limits.parallel <= 1
            && let Some(pending) = self.pending.first()
        {
            return Err(ExecutionError::Awaiting {
                method: pending.exchange.request.method.clone(),
                url: pending.exchange.request.url.clone(),
            });
        }
        if let Some(subscription) = self.joining {
            return Err(ExecutionError::Awaiting {
                method: "receive".to_owned(),
                url: format!("subscription {subscription}"),
            });
        }
        // Asking for more means the message went out.
        if let Some(sent) = self.sending.take() {
            let performed = Performed::Message {
                action: "send".to_owned(),
                channel: sent.exchange.request.url.clone(),
                delivered: true,
            };
            let doc = self.frames.last().expect("the sending frame").doc;
            self.complete(
                sent.step,
                Completion {
                    exchange: Some(sent.exchange),
                    given: BTreeMap::new(),
                    default_pass: true,
                    failed: false,
                    alongside: false,
                    attempt: sent.attempt,
                    performed,
                    elapsed: sent.started.elapsed(),
                },
            )
            .map_err(|error| locate(&self.documents, doc, error))?;
        }
        loop {
            if let Some(report) = &self.report {
                return Ok(Progress::Done(report.clone()));
            }
            let Some(frame) = self.frames.last() else {
                return Err(ExecutionError::Stopped);
            };
            // The frame is spent: name its outputs and hand them back —
            // once every receive it started has its message. After an
            // `end` nothing more is judged: whatever is still listening is
            // closed unheard.
            if frame.at >= frame.order.len() {
                if !self.pending.is_empty() {
                    return Ok(Progress::Await(None));
                }
                if frame.ended && !frame.listening.is_empty() {
                    let frame = self.frames.last_mut().expect("the frame is still there");
                    let closed: Vec<_> = frame
                        .listening
                        .drain(..)
                        .map(|listening| listening.subscription)
                        .collect();
                    self.closing.extend(closed.into_iter().rev());
                    continue;
                }
                if let Some(listening) = frame.listening.first() {
                    return Ok(self.join(listening.subscription));
                }
                if let Some(subscription) = self.closing.pop() {
                    return Ok(Progress::Unsubscribe(subscription));
                }
                self.leave()?;
                continue;
            }
            let index = frame.order[frame.at];
            // A step starts while others are in flight only when it may
            // run alongside them and does not need what they produce.
            let alongside = self.options.limits.parallel > 1 && alongside(frame, index);
            if !self.pending.is_empty() {
                if !alongside || self.flying() >= self.options.limits.parallel {
                    return Ok(Progress::Await(None));
                }
                let step = &frame.workflow.steps[index];
                let description = self.documents.get(frame.doc).description;
                let mut needs = steps_named_by(step, frame.workflow, description)
                    .map_err(|error| locate(&self.documents, frame.doc, error))?;
                needs.extend(step.depends_on.iter().cloned());
                if self
                    .pending
                    .iter()
                    .any(|pending| needs.contains(&frame.workflow.steps[pending.step].step_id))
                {
                    return Ok(Progress::Await(None));
                }
            }
            // A step that depends on a receive still listening waits for it.
            if !frame.listening.is_empty() {
                let step = &frame.workflow.steps[index];
                let description = self.documents.get(frame.doc).description;
                let mut needs = steps_named_by(step, frame.workflow, description)
                    .map_err(|error| locate(&self.documents, frame.doc, error))?;
                needs.extend(step.depends_on.iter().cloned());
                // The workflow's own actions are judged after this step.
                needs.extend(
                    workflow_action_reads(frame.workflow, description)
                        .map_err(|error| locate(&self.documents, frame.doc, error))?,
                );
                if let Some(listening) = frame
                    .listening
                    .iter()
                    .find(|listening| needs.contains(&frame.workflow.steps[listening.step].step_id))
                {
                    // Joined with nothing else in flight.
                    if !self.pending.is_empty() {
                        return Ok(Progress::Await(None));
                    }
                    return Ok(self.join(listening.subscription));
                }
            }

            // A step that cannot start while others are in flight waits
            // for them: one of them may yet end the workflow before it,
            // as it would have one at a time.
            if self.taken >= self.cap {
                if !self.pending.is_empty() {
                    return Ok(Progress::Await(None));
                }
                return Err(ExecutionError::Limit {
                    limit: "step",
                    at: self.options.limits.steps,
                });
            }

            let doc = frame.doc;
            let step = &frame.workflow.steps[index];
            if step.workflow_id.is_some() {
                self.taken += 1;
                self.call(index)?;
                continue;
            }
            if step.action.is_some() || step.channel_path.is_some() {
                self.taken += 1;
                return self
                    .message_step(index)
                    .map_err(|error| locate(&self.documents, doc, error));
            }

            let (mut request, exchange) = match self
                .endpoint(index)
                .and_then(|endpoint| self.build(index, endpoint))
            {
                Ok(built) => built,
                Err(_) if !self.pending.is_empty() => return Ok(Progress::Await(None)),
                Err(error) => return Err(locate(&self.documents, doc, error)),
            };
            self.taken += 1;
            let frame = self.frames.last_mut().expect("the frame to send from");
            let attempt = frame.attempts.get(&step.step_id).copied().unwrap_or(0) + 1;
            // A step that runs alongside others is passed at once: the
            // next one may start before it is answered.
            if alongside {
                frame.at += 1;
            }
            self.requests += 1;
            let id = RequestId(self.requests);
            request.id = id;
            self.pending.push(Pending {
                id,
                alongside,
                answer: None,
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
        match self.in_flight().as_slice() {
            [] => Err(ExecutionError::NotWaiting),
            [id] => self.supply_to(*id, response),
            many => Err(ExecutionError::InFlight { count: many.len() }),
        }
    }

    /// Hand back the response to request `id`, one of those in flight.
    ///
    /// # Errors
    ///
    /// As [`Run::supply`]; [`ExecutionError::NotWaiting`] when `id` is not
    /// in flight — it was answered already, or abandoned when its
    /// workflow ended (see [`Run::in_flight`]).
    pub fn supply_to(
        &mut self,
        id: RequestId,
        response: HttpResponse,
    ) -> Result<(), ExecutionError> {
        if self.halted {
            return Err(ExecutionError::Stopped);
        }
        let result = self.supply_inner(id, response);
        if let Err(error) = &result
            && !matches!(error, ExecutionError::NotWaiting)
        {
            self.halted = true;
        }
        result
    }

    /// Whether another request may be in flight beside those already
    /// there.
    pub(crate) fn takes_more(&self) -> bool {
        self.flying() < self.options.limits.parallel
    }

    /// Requests not yet answered.
    fn flying(&self) -> usize {
        self.pending
            .iter()
            .filter(|pending| pending.answer.is_none())
            .count()
            + self.roots.as_ref().map_or(0, roots::Roots::flying)
    }

    /// The requests in flight, oldest first. A request that was handed out
    /// and is no longer here was abandoned: its workflow ended while it
    /// was in flight, or the run stopped, so its response is not wanted.
    #[must_use]
    pub fn in_flight(&self) -> Vec<RequestId> {
        // A stopped run wants nothing more.
        if self.halted {
            return Vec::new();
        }
        if let Some(roots) = &self.roots {
            return roots.in_flight();
        }
        self.pending
            .iter()
            .filter(|pending| pending.answer.is_none())
            .map(|pending| pending.id)
            .collect()
    }

    fn supply_inner(
        &mut self,
        id: RequestId,
        response: HttpResponse,
    ) -> Result<(), ExecutionError> {
        if self.roots.is_some() {
            return self.supply_root(id, response);
        }
        if self.frames.last().is_none() {
            return Err(ExecutionError::NotWaiting);
        }
        let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.id == id && pending.answer.is_none())
        else {
            return Err(ExecutionError::NotWaiting);
        };
        pending.answer = Some((response, pending.started.elapsed()));
        // A response that came back after the run's time is not judged:
        // running out of time ends the run, it does not fail the step.
        if self.out_of_time() {
            return Err(self.timed_out());
        }
        // Judged in the order the steps are listed: a step listed before
        // this one may yet end the workflow, and then this one would not
        // have run. One that ends it abandons the rest.
        while self
            .pending
            .first()
            .is_some_and(|pending| pending.answer.is_some())
        {
            let mut pending = self.pending.remove(0);
            let (response, elapsed) = pending.answer.take().expect("an answer");
            let status = response.status;
            pending.exchange.response_body = response.body_value();
            pending.exchange.response = Some(response);
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
                    failed: false,
                    alongside: pending.alongside,
                    attempt: pending.attempt,
                    performed,
                    elapsed,
                },
            )?;
        }
        Ok(())
    }

    /// Hand back what came of the [`Progress::Receive`] [`Run::advance`]
    /// asked for: the next message on the subscription, or `None` when
    /// nothing came within the timeout.
    ///
    /// A message whose correlation id is not the one the step waits for
    /// is passed over: the next advance asks for another. A message that
    /// is, or none at all, completes the receive step — its criteria,
    /// outputs and actions — at the point the run needed it.
    ///
    /// # Errors
    ///
    /// As [`Run::supply`]; [`ExecutionError::NotWaiting`] when no message
    /// was asked for.
    pub fn deliver(&mut self, message: Option<IncomingMessage>) -> Result<(), ExecutionError> {
        if self.halted {
            return Err(ExecutionError::Stopped);
        }
        let result = self.deliver_inner(message);
        if let Err(error) = &result
            && !matches!(error, ExecutionError::NotWaiting)
        {
            self.halted = true;
        }
        result
    }

    fn deliver_inner(&mut self, message: Option<IncomingMessage>) -> Result<(), ExecutionError> {
        let Some(subscription) = self.joining.take() else {
            return Err(ExecutionError::NotWaiting);
        };
        if self.out_of_time() {
            return Err(self.timed_out());
        }
        let frame = self.frames.last_mut().expect("a frame that is listening");
        let Some(at) = frame
            .listening
            .iter()
            .position(|listening| listening.subscription == subscription)
        else {
            return Err(ExecutionError::NotWaiting);
        };
        let message = message.map(|message| expression::Message {
            payload: message.payload_value(),
            headers: message.headers,
        });
        if let Some(message) = &message
            && !correlates(&frame.listening[at], message)
        {
            // Not the one this step waits for: ask again.
            return Ok(());
        }
        let mut listening = frame.listening.remove(at);
        self.closing.push(subscription);
        let delivered = message.is_some();
        listening.exchange.message = message;
        // The receive completes where it was needed, then the run comes
        // back there.
        frame
            .resume
            .push((listening.step, frame.at, frame.detour.take()));
        frame.at = frame
            .order
            .iter()
            .position(|&index| index == listening.step)
            .unwrap_or(frame.at);
        let doc = frame.doc;
        let performed = Performed::Message {
            action: "receive".to_owned(),
            channel: listening.exchange.request.url.clone(),
            delivered,
        };
        self.complete(
            listening.step,
            Completion {
                exchange: Some(listening.exchange),
                given: BTreeMap::new(),
                default_pass: delivered,
                failed: !delivered,
                alongside: false,
                attempt: listening.attempt,
                performed,
                elapsed: listening.started.elapsed(),
            },
        )
        .map_err(|error| locate(&self.documents, doc, error))
    }

    /// Ask for the next message on a listening receive.
    fn join(&mut self, subscription: SubscriptionId) -> Progress {
        let frame = self.frames.last().expect("a frame that is listening");
        let listening = frame
            .listening
            .iter()
            .find(|listening| listening.subscription == subscription)
            .expect("the subscription is listening");
        let timeout = listening
            .timeout
            .map(|timeout| timeout.saturating_sub(listening.started.elapsed()));
        self.joining = Some(subscription);
        Progress::Receive {
            subscription,
            timeout,
        }
    }

    /// Start an AsyncAPI step: publish its message, or subscribe and move
    /// on.
    fn message_step(&mut self, index: usize) -> Result<Progress, ExecutionError> {
        let resolved = self.channel(index)?;
        let frame = self.frames.last().expect("a frame to send from");
        let step = &frame.workflow.steps[index];
        let scope = scope(
            frame,
            &frame.steps,
            None,
            &self.finished[frame.doc.0],
            ambient(&self.ambients, frame.doc),
        );
        let description = self.documents.get(frame.doc).description;
        let mut channel_values = BTreeMap::new();
        let mut headers = Vec::new();
        for parameter in effective_parameters(frame.workflow, step, description)? {
            let parameter = parameter.resolve(&scope)?;
            match parameter.location {
                ParameterLocation::Channel => {
                    channel_values.insert(parameter.name, text(&parameter.value));
                }
                ParameterLocation::Header => headers.push((parameter.name, text(&parameter.value))),
                other => {
                    return Err(ExecutionError::BadRequest {
                        step: step.step_id.clone(),
                        reason: format!(
                            "parameter `{}` is in `{}`, which an AsyncAPI step has not",
                            parameter.name,
                            serde_json::to_value(other)
                                .ok()
                                .and_then(|value| value.as_str().map(str::to_owned))
                                .unwrap_or_default()
                        ),
                    });
                }
            }
        }
        let mut channel = resolved.channel;
        channel.address =
            crate::channel::address(&channel.address, &channel_values).map_err(|reason| {
                ExecutionError::BadRequest {
                    step: step.step_id.clone(),
                    reason,
                }
            })?;
        let Some(action) = step.action else {
            return Err(ExecutionError::BadRequest {
                step: step.step_id.clone(),
                reason: "it names a channel but no `action`".into(),
            });
        };
        let receive = action == StepAction::Receive;
        let payload = body(step, &scope, &resolved.content_type)?;
        let attempt = frame.attempts.get(&step.step_id).copied().unwrap_or(0) + 1;
        let timeout = step
            .timeout
            .and_then(|timeout| u64::try_from(timeout).ok())
            .map(Duration::from_millis);
        let expected = match &step.correlation_id {
            Some(id) if receive => {
                if resolved.correlation.is_empty() {
                    return Err(ExecutionError::BadRequest {
                        step: step.step_id.clone(),
                        reason: format!(
                            "it waits for a `correlationId`, but no message of channel `{}` \
                             says where its correlation id is",
                            channel.address
                        ),
                    });
                }
                Some(select::resolve(id, &scope)?)
            }
            _ => None,
        };
        let (bytes, value, content_type) = match payload {
            Some(payload) => (
                Some(payload.bytes),
                Some(payload.value),
                Some(payload.content_type),
            ),
            None => (None, None, None),
        };
        let exchange = Exchange {
            request: HttpRequest {
                id: RequestId::default(),
                method: if receive { "receive" } else { "send" }.to_owned(),
                url: channel.address.clone(),
                headers: headers.clone(),
                body: bytes.clone(),
                timeout,
            },
            body: value.clone(),
            message: (!receive).then(|| expression::Message {
                headers: headers.clone(),
                payload: value,
            }),
            ..Exchange::default()
        };
        if !receive {
            self.sending = Some(Pending {
                id: RequestId::default(),
                alongside: false,
                answer: None,
                step: index,
                attempt,
                exchange,
                started: Instant::now(),
            });
            return Ok(Progress::Publish(OutgoingMessage {
                channel,
                headers,
                content_type,
                payload: bytes,
            }));
        }
        self.subscriptions += 1;
        let subscription = SubscriptionId(self.subscriptions);
        let frame = self.frames.last_mut().expect("a frame to listen in");
        // A receive that runs again replaces the one still listening.
        if let Some(at) = frame
            .listening
            .iter()
            .position(|listening| listening.step == index)
        {
            let old = frame.listening.remove(at);
            self.closing.push(old.subscription);
        }
        frame.listening.push(Listening {
            step: index,
            subscription,
            attempt,
            exchange,
            started: Instant::now(),
            timeout,
            expected,
            locations: resolved.correlation,
        });
        self.apply(Action::Advance)?;
        Ok(Progress::Subscribe(Subscription {
            id: subscription,
            channel,
            headers,
        }))
    }

    /// The channel an AsyncAPI step names: the plan's, or resolved in the
    /// step's own document.
    fn channel(&self, index: usize) -> Result<crate::channel::Resolved, ExecutionError> {
        let frame = self.frames.last().expect("a frame to resolve in");
        let (doc, workflow) = (frame.doc, frame.workflow);
        let step = &workflow.steps[index];
        if let Some(prepared) = self.prepared {
            return prepared
                .channels
                .get(&(doc, workflow.workflow_id.as_str(), step.step_id.as_str()))
                .cloned()
                .ok_or_else(|| {
                    ExecutionError::Unsupported("channel is outside the prepared plan".into())
                });
        }
        let document = *self.documents.get(doc);
        Ok(crate::channel::resolve(
            step,
            &document.options.sources,
            &unsupplied(&document),
        )?)
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
        let here = frame.doc;
        let step = &frame.workflow.steps[index];
        let step_id = step.step_id.clone();
        let workflow_id = frame.workflow.workflow_id.clone();

        // A real response/completed call already exists. Retain this attempt
        // before output evaluation or action selection can stop the engine.
        let record = self.records.len();
        self.records.push(StepRecord {
            document: self.documents.get(frame.doc).identity.map(str::to_owned),
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
                &self.finished[frame.doc.0],
                ambient(&self.ambients, frame.doc),
            );

            // Criteria, where a step states them, are the whole
            // judgement: a step that says `$statusCode == 404` means it.
            let mut passed = if done.failed {
                false
            } else if step.success_criteria.is_empty() {
                done.default_pass
            } else {
                true
            };
            let judged = if done.failed {
                &[][..]
            } else {
                &step.success_criteria[..]
            };
            for criterion in judged {
                let holds =
                    evaluate_criterion(criterion, &scope, &mut self.records[record].criteria)
                        .map_err(|error| locate(&self.documents, here, error))?;
                passed = passed && holds;
            }
            // Only a step that did what it said can name what it
            // produced: a failed one is about to be retried or given up
            // on, and its outputs would name what is not there.
            let outputs = if passed {
                let mut outputs = done.given.clone();
                outputs.extend(
                    evaluate_outputs(&step.outputs, &scope)
                        .map_err(|error| locate(&self.documents, here, error))?,
                );
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
        let action = self
            .decide(index, passed, done.exchange.as_ref(), record)
            .map_err(|error| locate(&self.documents, here, error))?;
        self.records[record].action = describe(&action);
        if done.alongside && matches!(action, Action::Advance) {
            return Ok(());
        }
        self.apply(action)
    }

    /// Push a frame for `workflow`, declared by document `doc`.
    fn enter(
        &mut self,
        doc: Doc,
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
        let document = *self.documents.get(doc);
        // Caller-free entries are the initial root/dependency queue: start_inner
        // already checked every one against the immutable root_inputs snapshot.
        // Calls and recovery entries have newly bound arguments, even when they
        // re-enter a workflow that was also in the initial queue.
        if caller.is_some() {
            match self.prepared {
                Some(prepared) => prepared.input_schemas[doc.0].validate(workflow, &inputs),
                None => self.input_schemas[doc.0]
                    .compile(
                        document.description,
                        self.options,
                        document.options,
                        workflow,
                    )
                    .and_then(|()| self.input_schemas[doc.0].validate(workflow, &inputs)),
            }
            .map_err(|error| error.in_document(document.identity))?;
        }
        let order = match self.prepared {
            Some(prepared) => prepared
                .orders
                .get(&(doc, workflow.workflow_id.as_str()))
                .cloned()
                .ok_or_else(|| {
                    ExecutionError::Unsupported("workflow is outside the prepared plan".into())
                }),
            None => ordered_steps(workflow, document.description),
        }
        .map_err(|error| locate(&self.documents, doc, error))?;
        if self.ambients[doc.0].is_none() {
            self.ambients[doc.0] = Some(Ambient::of(
                document.description,
                self.prepared.map(|prepared| &prepared.compiled),
            ));
        }
        self.frames.push(Frame {
            doc,
            workflow,
            inputs,
            order,
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
            listening: Vec::new(),
            resume: Vec::new(),
            ended: false,
            outcome: Outcome::Succeeded,
        });
        Ok(())
    }

    /// Enter a call, `goto` or `retry` target after the workflows it
    /// depends on. Those run as the root's dependencies do: each once in a
    /// run, with the run's inputs, so one that already finished as a
    /// dependency is not run again. One still to run with any instance
    /// running further up — whatever its inputs, that instance is, directly
    /// or not, entering the target — is a `dependsOn` cycle: a dependency
    /// run is never started inside a running instance of itself.
    fn enter_after_dependencies(
        &mut self,
        doc: Doc,
        workflow: &'d Workflow,
        inputs: Value,
        caller: (String, Then),
    ) -> Result<(), ExecutionError> {
        let mut remaining = ordered_workflows(&self.documents, doc, workflow)?;
        // The target itself comes last.
        remaining.pop();
        remaining.retain(|(doc, dependency)| {
            !self
                .dependencies_done
                .contains(&(*doc, dependency.workflow_id.clone()))
        });
        if remaining.is_empty() {
            return self.enter(doc, workflow, inputs, Some(caller));
        }
        // However it was entered, a running workflow waits for this entry.
        if let Some((running_doc, running)) = remaining.iter().find(|(doc, dependency)| {
            self.frames.iter().any(|frame| {
                frame.doc == *doc && frame.workflow.workflow_id == dependency.workflow_id
            })
        }) {
            // Named in the dependency's own document, as a static cycle is.
            return Err(locate(
                &self.documents,
                *running_doc,
                ExecutionError::Circular(running.workflow_id.clone()),
            ));
        }
        self.deferred.push(Deferred {
            remaining,
            doc,
            workflow,
            inputs,
            caller,
            started: Instant::now(),
        });
        self.next_dependency()
    }

    /// Enter the next dependency the innermost deferred entry waits for,
    /// skipping one a deeper entry ran meanwhile; after the last, the
    /// entry itself.
    fn next_dependency(&mut self) -> Result<(), ExecutionError> {
        loop {
            let deferred = self
                .deferred
                .last_mut()
                .expect("an entry waiting for its dependencies");
            if deferred.remaining.is_empty() {
                let Deferred {
                    doc,
                    workflow,
                    inputs,
                    caller,
                    started,
                    ..
                } = self.deferred.pop().expect("the entry just looked at");
                self.enter(doc, workflow, inputs, Some(caller))?;
                self.frames
                    .last_mut()
                    .expect("the entry just entered")
                    .started = started;
                return Ok(());
            }
            let (doc, dependency) = deferred.remaining.remove(0);
            if self
                .dependencies_done
                .contains(&(doc, dependency.workflow_id.clone()))
            {
                continue;
            }
            let step_id = deferred.caller.0.clone();
            let inputs = Value::Object(self.root_inputs.clone());
            return self.enter(doc, dependency, inputs, Some((step_id, Then::Dependency)));
        }
    }

    /// Finish the top frame: name its outputs and give them to whoever
    /// is waiting.
    fn leave(&mut self) -> Result<(), ExecutionError> {
        let frame = self.frames.pop().expect("a frame to leave");
        let outputs = {
            let scope = scope(
                &frame,
                &frame.steps,
                None,
                &self.finished[frame.doc.0],
                ambient(&self.ambients, frame.doc),
            );
            if frame.outcome == Outcome::Succeeded {
                evaluate_outputs(&frame.workflow.outputs, &scope)
            } else {
                // A workflow that stopped early names outputs from steps
                // that never ran. Those go with the steps; anything else
                // wrong with an output is still worth saying.
                evaluate_what_ran(&frame.workflow.outputs, &scope)
            }
            .map_err(|error| locate(&self.documents, frame.doc, error))?
        };
        self.finished[frame.doc.0].insert(
            frame.workflow.workflow_id.clone(),
            WorkflowState {
                inputs: frame.inputs.clone(),
                outputs: outputs.clone(),
            },
        );
        self.fresh
            .insert((frame.doc, frame.workflow.workflow_id.clone()));
        if is_dependency(&frame) {
            self.dependencies_done
                .insert((frame.doc, frame.workflow.workflow_id.clone()));
        }

        let Some((step_id, then)) = frame.caller else {
            // A root workflow: its outputs are the run's, unless it was
            // only a dependency of the one that was asked for.
            if self.queue.is_empty() {
                self.report = Some(Box::new(ExecutionReport {
                    input_validation: self.options.input_validation,
                    workflow_id: frame.workflow.workflow_id.clone(),
                    outcome: frame.outcome,
                    outputs,
                    steps: std::mem::take(&mut self.records),
                }));
            } else {
                // A dependency a nested entry already ran is not run again.
                // The one asked for comes last and is never skipped: every
                // workflow it depends on runs before it, so no nested entry
                // can depend on it while one of those is still running.
                while self.queue.len() > 1
                    && self
                        .dependencies_done
                        .contains(&(self.queue[0].0, self.queue[0].1.workflow_id.clone()))
                {
                    self.queue.remove(0);
                }
                let (doc, next) = self.queue.remove(0);
                let inputs = Value::Object(self.root_inputs.clone());
                self.enter(doc, next, inputs, None)?;
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
                parent.resume.clear();
                parent.ended = true;
                parent.outcome = frame.outcome;
                Ok(())
            }
            // A `retry` sent the run through another workflow first;
            // now the step that failed is tried again.
            Then::Retry => Ok(()),
            // One of the workflows an entry waits for is done.
            Then::Dependency => self.next_dependency(),
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
                        failed: false,
                        alongside: false,
                        attempt,
                        performed: Performed::Workflow {
                            // The reference as the calling step wrote it: a
                            // local id, or `$sourceDescriptions.<name>.<id>`.
                            workflow_id: parent.workflow.steps[index]
                                .workflow_id
                                .clone()
                                .unwrap_or_else(|| frame.workflow.workflow_id.clone()),
                            outcome: frame.outcome,
                        },
                        elapsed,
                    },
                )
            }
        }
    }

    /// A step that calls a workflow, of this document or of another
    /// Arazzo document it names as a source.
    fn call(&mut self, index: usize) -> Result<(), ExecutionError> {
        let here = self.frames.last().expect("a frame to call from").doc;
        let (doc, workflow, step_id, inputs) = self
            .bind_call(index)
            .map_err(|error| locate(&self.documents, here, error))?;
        // The callee locates its own errors.
        self.enter_after_dependencies(doc, workflow, inputs, (step_id, Then::Advance))
    }

    /// Resolve a workflow step's target and bind its arguments, in the
    /// caller's document.
    fn bind_call(
        &self,
        index: usize,
    ) -> Result<(Doc, &'d Workflow, String, Value), ExecutionError> {
        let frame = self.frames.last().expect("a frame to call from");
        let step = &frame.workflow.steps[index];
        let step_id = step.step_id.clone();
        let wanted = step.workflow_id.clone().unwrap_or_default();
        let (doc, at) = self.documents.resolve(frame.doc, &wanted)?;
        let workflow = self.documents.workflow(doc, at);
        let description = self.documents.get(frame.doc).description;

        // A workflow step's parameters are the workflow's inputs.
        // "When the step... specifies a `workflowId`, then all
        // parameters map to workflow inputs", and a workflow's own
        // parameters are "applicable for all steps described under this
        // workflow... can be overridden at the step level but cannot be
        // removed there" — so both lists go, the step's last. They are
        // evaluated in the caller's document; the callee sees only them.
        let scope = scope(
            frame,
            &frame.steps,
            None,
            &self.finished[frame.doc.0],
            ambient(&self.ambients, frame.doc),
        );
        let mut inputs = Map::new();
        for parameter in effective_parameters(frame.workflow, step, description)? {
            let parameter = parameter.resolve(&scope)?;
            inputs.insert(parameter.name, parameter.value);
        }
        Ok((doc, workflow, step_id, Value::Object(inputs)))
    }

    /// The inputs a called workflow starts with: the caller's own,
    /// with whatever parameters were passed to it written over them.
    fn arguments(&self, arguments: &[ReusableOr<Parameter>]) -> Result<Value, ExecutionError> {
        let frame = self.frames.last().expect("a frame to pass arguments from");
        let scope = scope(
            frame,
            &frame.steps,
            None,
            &self.finished[frame.doc.0],
            ambient(&self.ambients, frame.doc),
        );
        // A called workflow starts with what it was passed and nothing
        // else: only the parameters are forwarded, not the caller's
        // whole input context, so a child reading `$inputs.x` is asking
        // for something the calling step gave it.
        let mut inputs = Map::new();
        let description = self.documents.get(frame.doc).description;
        for parameter in parameters(arguments, description, &scope)? {
            inputs.insert(parameter.name, parameter.value);
        }
        Ok(Value::Object(inputs))
    }

    /// The operation a step sends to: the plan's, or resolved in the step's
    /// own document — one of its sources, with its base-URL overrides — by
    /// that document's resolver, which indexes each source once in a run.
    fn endpoint(&mut self, index: usize) -> Result<operation::Endpoint, ExecutionError> {
        let frame = self.frames.last().expect("a frame to resolve in");
        let (doc, workflow) = (frame.doc, frame.workflow);
        let step = &workflow.steps[index];
        if let Some(prepared) = self.prepared {
            return prepared
                .endpoints
                .get(&(doc, workflow.workflow_id.as_str(), step.step_id.as_str()))
                .cloned()
                .ok_or_else(|| {
                    ExecutionError::Unsupported("operation is outside the prepared plan".into())
                });
        }
        let document = *self.documents.get(doc);
        let resolver = self.resolvers[doc.0]
            .get_or_insert_with(|| crate::operation_index::Resolver::new(document.options));
        Ok(resolver.resolve(step, &unsupplied(&document))?)
    }

    /// Assemble the request a step wants sent to `endpoint`.
    fn build(
        &self,
        index: usize,
        endpoint: operation::Endpoint,
    ) -> Result<(HttpRequest, Exchange), ExecutionError> {
        let frame = self.frames.last().expect("a frame to build in");
        let step = &frame.workflow.steps[index];
        let document = self.documents.get(frame.doc);
        let scope = scope(
            frame,
            &frame.steps,
            None,
            &self.finished[frame.doc.0],
            ambient(&self.ambients, frame.doc),
        );

        // The workflow's parameters first, so a step's own override them.
        let resolved = effective_parameters(frame.workflow, step, document.description)?
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

        let body = body(step, &scope, "application/json")?;
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
            id: RequestId::default(),
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
                message: None,
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
        let description = self.documents.get(frame.doc).description;
        let scope = scope(
            frame,
            &frame.steps,
            exchange,
            &self.finished[frame.doc.0],
            ambient(&self.ambients, frame.doc),
        );
        let outcomes = &mut self.records[record].action_criteria;

        if passed {
            // A step's own actions first, then the workflow's.
            let actions = step
                .on_success
                .iter()
                .chain(frame.workflow.success_actions.iter());
            for action in actions {
                let action = success_action(action, description)?;
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
            let action = failure_action(action, description)?;
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
        let here = frame.doc;
        match action {
            Action::Advance => {
                // A step run as a retry's detour hands control back to
                // the step that asked for it, which is tried again.
                // A receive completed where it was needed hands control
                // back there.
                if let Some(back) = frame.detour.take() {
                    frame.at = back;
                } else if let Some((_, back, detour)) = pop_resume(frame, frame.order[frame.at]) {
                    frame.at = back;
                    frame.detour = detour;
                } else {
                    frame.at += 1;
                }
                Ok(())
            }
            Action::End(outcome) => {
                // What is still in flight is abandoned: nothing will judge
                // it, and it never counted as a step taken.
                self.taken = self.taken.saturating_sub(self.pending.len());
                self.pending.clear();
                frame.outcome = outcome;
                frame.at = frame.order.len();
                frame.resume.clear();
                frame.ended = true;
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
                        let at = position_of(frame, &target)
                            .map_err(|error| locate(&self.documents, here, error))?;
                        frame.detour = Some(frame.at);
                        frame.at = at;
                        Ok(())
                    }
                    // The same, for a workflow.
                    (None, Some(workflow_id)) => {
                        let (doc, at) = self
                            .documents
                            .resolve(frame.doc, &workflow_id)
                            .map_err(|error| locate(&self.documents, here, error))?;
                        let workflow = self.documents.workflow(doc, at);
                        let inputs = self
                            .arguments(&arguments)
                            .map_err(|error| locate(&self.documents, here, error))?;
                        self.enter_after_dependencies(doc, workflow, inputs, (step_id, Then::Retry))
                    }
                    // Nothing named: this step, again.
                    (None, None) => Ok(()),
                }
            }
            Action::Goto {
                step: Some(step_id),
                ..
            } => {
                let current = frame.order.get(frame.at).copied();
                frame.at = position_of(frame, &step_id)
                    .map_err(|error| locate(&self.documents, here, error))?;
                // A joined receive that goes elsewhere does not come back.
                if let Some(current) = current
                    && let Some((_, _, detour)) = pop_resume(frame, current)
                {
                    frame.detour = detour;
                }
                Ok(())
            }
            Action::Goto {
                workflow: Some(workflow_id),
                parameters: arguments,
                ..
            } => {
                let index = frame.order[frame.at];
                let step_id = frame.workflow.steps[index].step_id.clone();
                let (doc, at) = self
                    .documents
                    .resolve(frame.doc, &workflow_id)
                    .map_err(|error| locate(&self.documents, here, error))?;
                let workflow = self.documents.workflow(doc, at);
                let inputs = self
                    .arguments(&arguments)
                    .map_err(|error| locate(&self.documents, here, error))?;
                self.enter_after_dependencies(doc, workflow, inputs, (step_id, Then::EndCaller))
            }
            Action::Goto { .. } => Ok(()),
        }
    }
}

/// Whether a message carries the correlation id its receive step waits
/// for, at any of the places the channel's messages keep one.
fn correlates(listening: &Listening, message: &expression::Message) -> bool {
    let Some(expected) = &listening.expected else {
        return true;
    };
    listening.locations.iter().any(|location| {
        correlation(location, message)
            .is_some_and(|found| found == *expected || text(&found) == text(expected))
    })
}

/// The value at an AsyncAPI correlation-id location —
/// `$message.header#/<name>` or `$message.payload#/<pointer>` — in a
/// message.
fn correlation(location: &str, message: &expression::Message) -> Option<Value> {
    let (source, pointer) = match location.split_once('#') {
        Some((source, pointer)) => (source, pointer),
        None => (location, ""),
    };
    match source.trim() {
        "$message.header" => {
            let name = pointer.strip_prefix('/')?;
            let name = name.replace("~1", "/").replace("~0", "~");
            message
                .headers
                .iter()
                .find(|(header, _)| header.eq_ignore_ascii_case(&name))
                .map(|(_, value)| Value::String(value.clone()))
        }
        "$message.payload" => message.payload.as_ref()?.pointer(pointer).cloned(),
        _ => None,
    }
}

/// The innermost resume entry, if it belongs to the receive `step`.
fn pop_resume(frame: &mut Frame<'_>, step: usize) -> Option<(usize, usize, Option<usize>)> {
    if frame
        .resume
        .last()
        .is_some_and(|(receive, _, _)| *receive == step)
    {
        frame.resume.pop()
    } else {
        None
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

/// Whether `frame` runs as a dependency — of the root, or of a nested entry.
fn is_dependency(frame: &Frame<'_>) -> bool {
    matches!(frame.caller, None | Some((_, Then::Dependency)))
}

/// Name the external document an error was raised in, so an operation,
/// expression or criterion error of a linked document is not mistaken for
/// one of the entry's. Each error is located where it arises, by the
/// document of the frame it belongs to: an outer site never re-locates an
/// error a callee raised, because the entry's own errors stay unwrapped.
fn locate(documents: &Documents<'_>, doc: Doc, error: ExecutionError) -> ExecutionError {
    let Some(identity) = documents.get(doc).identity else {
        return error;
    };
    match error {
        ExecutionError::Input(_)
        | ExecutionError::InDocument { .. }
        | ExecutionError::Limit { .. }
        | ExecutionError::TimedOut { .. }
        | ExecutionError::Client(_)
        | ExecutionError::NotWaiting
        | ExecutionError::Awaiting { .. }
        | ExecutionError::Stopped => error,
        error => ExecutionError::InDocument {
            document: identity.to_owned(),
            error: Box::new(error),
        },
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
        templates.push(ParameterTemplate {
            parameter: entry.get_item(description).map_err(component_error)?,
            overridden: entry.value_override(),
        });
    }
    Ok(templates)
}

/// Why a `Reusable` could not be followed into the components.
fn component_error(error: ResolveError) -> ExecutionError {
    let problem = match error {
        ResolveError::NotFound(_) => "names a component the description has not got",
        _ => "is not a component this executor can follow",
    };
    ExecutionError::Unsupported(format!("`{}` {problem}", error.reference()))
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
    entry.get_item(description).map_err(component_error)
}

/// A failure action, following a `Reusable` into the components.
pub(crate) fn failure_action<'a>(
    entry: &'a ReusableOr<roas_arazzo::v1_1::FailureAction>,
    description: &'a Description,
) -> Result<&'a roas_arazzo::v1_1::FailureAction, ExecutionError> {
    entry.get_item(description).map_err(component_error)
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
/// `default_type`: the media type when the step's `requestBody` names none.
fn body(
    step: &Step,
    scope: &Scope<'_>,
    default_type: &str,
) -> Result<Option<Body>, ExecutionError> {
    let Some(request_body) = &step.request_body else {
        return Ok(None);
    };
    let mut payload = match &request_body.payload {
        Some(payload) => select::resolve(payload, scope)?,
        None => Value::Null,
    };
    for replacement in &request_body.replacements {
        // A language the engine cannot evaluate is refused first, as a
        // selector's is.
        let language = match &replacement.target_selector_type {
            Some(type_) => select::target_kind_of(type_)?,
            None => select::Language::Pointer,
        };
        let value = select::value_of(&replacement.value, scope)?;
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
        .unwrap_or_else(|| default_type.to_owned());
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
    documents: &Documents<'d>,
    doc: Doc,
    wanted: &'d Workflow,
) -> Result<Vec<(Doc, &'d Workflow)>, ExecutionError> {
    let mut ordered = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut done = BTreeSet::new();
    visit(
        documents,
        doc,
        wanted,
        &mut ordered,
        &mut visiting,
        &mut done,
    )?;
    Ok(ordered)
}

/// `dependsOn` may name `$sourceDescriptions.<name>.<workflowId>`, so the
/// walk is over (document, workflow) pairs: a dependency cycle through
/// another document is still a cycle, while two documents' same-named
/// workflows are different nodes.
fn visit<'d>(
    documents: &Documents<'d>,
    doc: Doc,
    workflow: &'d Workflow,
    ordered: &mut Vec<(Doc, &'d Workflow)>,
    visiting: &mut BTreeSet<(Doc, String)>,
    done: &mut BTreeSet<(Doc, String)>,
) -> Result<(), ExecutionError> {
    let key = (doc, workflow.workflow_id.clone());
    if done.contains(&key) {
        return Ok(());
    }
    if !visiting.insert(key.clone()) {
        // The cycle closes in `doc`: it names that document, as any other
        // error raised there does.
        return Err(locate(
            documents,
            doc,
            ExecutionError::Circular(workflow.workflow_id.clone()),
        ));
    }
    for reference in &workflow.depends_on {
        let (target, at) = documents
            .resolve(doc, reference)
            .map_err(|error| locate(documents, doc, error))?;
        visit(
            documents,
            target,
            documents.workflow(target, at),
            ordered,
            visiting,
            done,
        )?;
    }
    visiting.remove(&key);
    done.insert(key);
    ordered.push((doc, workflow));
    Ok(())
}

/// Parse the expressions a value evaluates, with no runtime value lookups.
/// The visitor may collect dependencies or just leave syntax checked.
pub(super) fn visit_value_expressions(
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

/// Whether step `index` may start while other steps are in flight: an
/// HTTP step whose completion can only move on or end the workflow — no
/// actions of its own or its workflow's — in a frame that is not on a
/// retry's detour or back from a receive.
fn alongside(frame: &Frame<'_>, index: usize) -> bool {
    let step = &frame.workflow.steps[index];
    step.workflow_id.is_none()
        && step.action.is_none()
        && step.channel_path.is_none()
        && step.on_success.is_empty()
        && step.on_failure.is_empty()
        && frame.workflow.success_actions.is_empty()
        && frame.workflow.failure_actions.is_empty()
        && frame.detour.is_none()
        && frame.resume.is_empty()
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
    step_expressions(step, workflow, description, &mut |parsed| {
        if let Some(id) = parsed.step_id() {
            found.insert(id.to_owned());
        }
    })?;
    found.remove(&step.step_id);
    Ok(found)
}

/// Visit every expression a step evaluates — see [`steps_named_by`].
pub(super) fn step_expressions(
    step: &Step,
    workflow: &Workflow,
    description: &Description,
    visitor: &mut impl FnMut(Expression<'_>),
) -> Result<(), ExecutionError> {
    visit_parameter_expressions(effective_parameters(workflow, step, description)?, visitor)?;
    read_criteria(&step.success_criteria, visitor)?;
    for output in step.outputs.values() {
        visit_value_expressions(output, visitor)?;
    }
    if let Some(id) = &step.correlation_id {
        visit_literal_expressions(id, visitor)?;
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            visit_literal_expressions(payload, visitor)?;
        }
        for replacement in &body.replacements {
            visit_value_expressions(&replacement.value, visitor)?;
        }
    }
    for entry in &step.on_success {
        // Keep reference-resolution failures lazy: the other outcome, or an
        // earlier action, may make this action unreachable. Syntax failures in
        // an available action remain errors rather than erased dependencies.
        if let Ok(action) = success_action(entry, description) {
            read_criteria(&action.criteria, visitor)?;
            visit_action_parameter_expressions(&action.parameters, description, visitor)?;
        }
    }
    for entry in &step.on_failure {
        if let Ok(action) = failure_action(entry, description) {
            read_criteria(&action.criteria, visitor)?;
            visit_action_parameter_expressions(&action.parameters, description, visitor)?;
        }
    }
    Ok(())
}

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

/// The steps a workflow's own `successActions` / `failureActions` read.
fn workflow_action_reads(
    workflow: &Workflow,
    description: &Description,
) -> Result<BTreeSet<String>, ExecutionError> {
    let mut found = BTreeSet::new();
    workflow_action_expressions(workflow, description, &mut |parsed| {
        if let Some(id) = parsed.step_id() {
            found.insert(id.to_owned());
        }
    })?;
    Ok(found)
}

/// Visit every expression a workflow's own `successActions` /
/// `failureActions` evaluate.
pub(super) fn workflow_action_expressions(
    workflow: &Workflow,
    description: &Description,
    visitor: &mut impl FnMut(Expression<'_>),
) -> Result<(), ExecutionError> {
    for entry in &workflow.success_actions {
        if let Ok(action) = success_action(entry, description) {
            read_criteria(&action.criteria, visitor)?;
            visit_action_parameter_expressions(&action.parameters, description, visitor)?;
        }
    }
    for entry in &workflow.failure_actions {
        if let Ok(action) = failure_action(entry, description) {
            read_criteria(&action.criteria, visitor)?;
            visit_action_parameter_expressions(&action.parameters, description, visitor)?;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpResponse;
    use serde_json::json;

    #[test]
    fn a_lazy_run_indexes_each_source_once() {
        let description: Description = serde_json::from_value(json!({
            "arazzo": "1.1.0", "info": { "title": "T", "version": "1" },
            "sourceDescriptions": [{ "name": "api", "url": "https://x.test/api.json", "type": "openapi" }],
            "workflows": [{ "workflowId": "w", "steps": [
                { "stepId": "a", "operationId": "list" },
                { "stepId": "b", "operationId": "$sourceDescriptions.api.list" },
                { "stepId": "c", "operationId": "list" }] }]
        }))
        .unwrap();
        let options = Options::new().source(
            "api",
            "https://x.test/api.json",
            json!({ "openapi": "3.1.0", "servers": [{ "url": "https://x.test" }],
                "paths": { "/items": { "get": { "operationId": "list" } } } }),
        );
        let mut run = Run::start(&description, &options).unwrap();
        for _ in 0..3 {
            assert!(matches!(run.advance().unwrap(), Progress::Send(_)));
            run.supply(HttpResponse::json(200, &json!([]))).unwrap();
        }
        assert!(matches!(run.advance().unwrap(), Progress::Done(_)));
        let resolver = run.resolvers[0].as_ref().expect("the entry's resolver");
        assert_eq!(resolver.builds, 1);
    }

    #[test]
    fn a_reusable_that_cannot_be_followed_says_which_way_it_failed() {
        let description: Description = serde_json::from_value(json!({
            "arazzo": "1.1.0", "info": { "title": "T", "version": "1" },
            "sourceDescriptions": [{ "name": "api", "url": "https://x.test/api.json" }],
            "workflows": [{ "workflowId": "w", "steps": [{ "stepId": "a", "operationId": "list" }] }],
            "components": { "successActions": { "done": { "name": "done", "type": "end" } } }
        }))
        .unwrap();
        let message = |entry: Value| {
            let entry: ReusableOr<Parameter> = serde_json::from_value(entry).unwrap();
            match parameter_templates(std::slice::from_ref(&entry), &description) {
                Err(ExecutionError::Unsupported(message)) => message,
                _ => panic!("the reference must not resolve"),
            }
        };
        assert_eq!(
            message(json!({ "reference": "$components.parameters.page" })),
            "`$components.parameters.page` names a component the description has not got"
        );
        assert_eq!(
            message(json!({ "reference": "$components.successActions.done" })),
            "`$components.successActions.done` is not a component this executor can follow"
        );

        let action: ReusableOr<roas_arazzo::v1_1::SuccessAction> =
            serde_json::from_value(json!({ "reference": "$components.successActions.done" }))
                .unwrap();
        assert_eq!(success_action(&action, &description).unwrap().name, "done");
    }
}
