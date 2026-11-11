//! The workflows the one asked for depends on, run together under
//! [`Options::parallel`](super::Options::parallel).
//!
//! One after another, each of them starts once the one before it is done.
//! Run together, each is a run of its own — a branch — that starts once
//! every workflow listed before it that it could tell apart from running
//! first is done: one it shares a workflow with, through `dependsOn`, a
//! call, a `goto` or a `retry`, or whose workflows it reads with
//! `$workflows.<id>`, or which reads its. What is left can neither see nor
//! change what the others do, so the run ends as it would have one after
//! another: the steps are reported in the same order, and the first error
//! in that order is the one raised.

use super::{
    Run, failure_action, step_expressions, success_action, visit_value_expressions,
    workflow_action_expressions,
};
use crate::documents::{Doc, Documents};
use crate::http::{HttpResponse, RequestId};
use crate::report::{ExecutionError, ExecutionReport, StepRecord};
use crate::run::Progress;
use crate::runtime_syntax::Root;
use roas_arazzo::v1_1::Workflow;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// A hundred years.
const FOREVER: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// The root dependencies of a run, while they run.
pub(super) struct Roots<'d> {
    entries: Vec<Entry<'d>>,
    /// An entry's error, raised once every entry before it is done: one
    /// of those may yet stop the run first.
    held: Option<(usize, ExecutionError)>,
    /// The branches' requests in flight: the id the run handed out, the
    /// entry, and the id the branch gave it.
    routes: Vec<(RequestId, usize, RequestId)>,
}

/// One workflow the one asked for depends on.
pub(super) struct Entry<'d> {
    doc: Doc,
    workflow: &'d Workflow,
    /// The entries listed before it that it waits for.
    after: Vec<usize>,
    state: State<'d>,
}

enum State<'d> {
    Waiting,
    Running {
        run: Box<Run<'d>>,
        /// When the branch may go on: it asked to wait before a retry.
        wake: Option<Instant>,
    },
    /// What it recorded.
    Done(Vec<StepRecord>),
    /// It stopped the run: what it recorded until then.
    Failed(Vec<StepRecord>),
    /// An entry listed before it stopped the run, so it would not have run.
    Abandoned,
}

/// What a workflow can touch.
struct Reach {
    /// It and every workflow it can run.
    workflows: BTreeSet<(Doc, String)>,
    /// The workflows its expressions read.
    reads: BTreeSet<(Doc, String)>,
}

impl Reach {
    fn conflicts(&self, other: &Self) -> bool {
        !self.workflows.is_disjoint(&other.workflows)
            || !self.reads.is_disjoint(&other.workflows)
            || !other.reads.is_disjoint(&self.workflows)
    }
}

/// The entries of `queue` — the root dependencies, in the order they run
/// one after another — with what each waits for; `None` when none of
/// them could run together, or when it cannot be told: one of them can
/// reach an AsyncAPI step, or a reference that does not resolve, which
/// the run reports in its own time.
pub(super) fn plan<'d>(
    documents: &Documents<'d>,
    queue: &[(Doc, &'d Workflow)],
) -> Option<Vec<Entry<'d>>> {
    let reaches = queue
        .iter()
        .map(|(doc, workflow)| reach(documents, *doc, workflow).ok().flatten())
        .collect::<Option<Vec<_>>>()?;
    let mut together = false;
    let entries = queue
        .iter()
        .enumerate()
        .map(|(at, (doc, workflow))| {
            let after: Vec<usize> = (0..at)
                .filter(|&before| reaches[at].conflicts(&reaches[before]))
                .collect();
            together |= after.len() < at;
            Entry {
                doc: *doc,
                workflow,
                after,
                state: State::Waiting,
            }
        })
        .collect();
    together.then_some(entries)
}

/// Everything `workflow` can run and read; `None` when it can reach an
/// AsyncAPI step.
fn reach(
    documents: &Documents<'_>,
    doc: Doc,
    workflow: &Workflow,
) -> Result<Option<Reach>, ExecutionError> {
    let mut reach = Reach {
        workflows: BTreeSet::new(),
        reads: BTreeSet::new(),
    };
    let mut todo = vec![(doc, workflow)];
    while let Some((doc, workflow)) = todo.pop() {
        if !reach.workflows.insert((doc, workflow.workflow_id.clone())) {
            continue;
        }
        let description = documents.get(doc).description;
        let mut targets: Vec<&str> = workflow.depends_on.iter().map(String::as_str).collect();
        let mut read = |parsed: crate::runtime_syntax::Expression<'_>| {
            if parsed.root == Root::Workflows {
                reach.reads.insert((doc, parsed.parts[0].to_owned()));
            }
        };
        for output in workflow.outputs.values() {
            visit_value_expressions(output, &mut read)?;
        }
        workflow_action_expressions(workflow, description, &mut read)?;
        for step in &workflow.steps {
            if step.action.is_some() || step.channel_path.is_some() {
                return Ok(None);
            }
            step_expressions(step, workflow, description, &mut read)?;
            targets.extend(step.workflow_id.as_deref());
            for entry in &step.on_success {
                targets.extend(success_action(entry, description)?.workflow_id.as_deref());
            }
            for entry in &step.on_failure {
                targets.extend(failure_action(entry, description)?.workflow_id.as_deref());
            }
        }
        for entry in &workflow.success_actions {
            targets.extend(success_action(entry, description)?.workflow_id.as_deref());
        }
        for entry in &workflow.failure_actions {
            targets.extend(failure_action(entry, description)?.workflow_id.as_deref());
        }
        for target in targets {
            let (doc, at) = documents.resolve(doc, target)?;
            todo.push((doc, documents.workflow(doc, at)));
        }
    }
    Ok(Some(reach))
}

impl<'d> Roots<'d> {
    /// Requests the branches have in flight.
    pub(super) fn flying(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| match &entry.state {
                State::Running { run, .. } => run.flying(),
                _ => 0,
            })
            .sum()
    }

    /// The ids of the branches' requests in flight, oldest first.
    pub(super) fn in_flight(&self) -> Vec<RequestId> {
        self.routes
            .iter()
            .filter(|(_, at, local)| match &self.entries[*at].state {
                State::Running { run, .. } => run.in_flight().contains(local),
                _ => false,
            })
            .map(|(id, _, _)| *id)
            .collect()
    }

    /// What one after another would have recorded so far: every entry
    /// done in the order they are listed, then what the first that is not
    /// has. What the ones after it did is not shown — had the run stopped
    /// there, they would not have run.
    pub(super) fn records(&self) -> Vec<StepRecord> {
        let mut records = Vec::new();
        for entry in &self.entries {
            match &entry.state {
                State::Done(done) => records.extend(done.iter().cloned()),
                State::Running { run, .. } => {
                    records.extend(run.records.iter().cloned());
                    break;
                }
                State::Failed(done) => {
                    records.extend(done.iter().cloned());
                    break;
                }
                State::Waiting | State::Abandoned => break,
            }
        }
        records
    }

    /// The entry a request in flight belongs to.
    pub(super) fn route(&self, id: RequestId) -> Option<usize> {
        self.in_flight()
            .contains(&id)
            .then(|| {
                self.routes
                    .iter()
                    .find(|(route, _, _)| *route == id)
                    .map(|(_, at, _)| *at)
            })
            .flatten()
    }

    /// Whether `at` can start: every entry it waits for is done.
    fn ready(&self, at: usize) -> bool {
        self.held.as_ref().is_none_or(|(held, _)| at < *held)
            && self.entries[at]
                .after
                .iter()
                .all(|&before| matches!(self.entries[before].state, State::Done(_)))
    }

    /// Steps the running branches took.
    fn taken(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| match &entry.state {
                State::Running { run, .. } => run.taken,
                _ => 0,
            })
            .sum()
    }
}

impl<'d> Run<'d> {
    /// Run `entries` together; the first starts now, so that whatever
    /// keeps it from starting stops the run before anything is sent, as
    /// it does one after another.
    pub(super) fn start_roots(&mut self, entries: Vec<Entry<'d>>) -> Result<(), ExecutionError> {
        let (doc, workflow) = (entries[0].doc, entries[0].workflow);
        let mut roots = Roots {
            entries,
            held: None,
            routes: Vec::new(),
        };
        roots.entries[0].state = State::Running {
            run: Box::new(self.branch(doc, workflow)?),
            wake: None,
        };
        self.roots = Some(roots);
        Ok(())
    }

    /// A run of one root dependency, beside the others: it sees what the
    /// run has finished so far, and nothing it does reaches the others
    /// until it is done.
    fn branch(&self, doc: Doc, workflow: &'d Workflow) -> Result<Run<'d>, ExecutionError> {
        let documents = self.documents.len();
        let mut run = Run {
            description: self.description,
            options: self.options,
            documents: self.documents.clone(),
            ambients: (0..documents).map(|_| None).collect(),
            prepared: self.prepared,
            root_inputs: self.root_inputs.clone(),
            frames: Vec::new(),
            queue: Vec::new(),
            dependencies_done: self.dependencies_done.clone(),
            deferred: Vec::new(),
            finished: self.finished.clone(),
            pending: Vec::new(),
            requests: 0,
            wait: None,
            records: Vec::new(),
            taken: 0,
            cap: 0,
            fresh: BTreeSet::new(),
            roots: None,
            report: None,
            halted: false,
            input_schemas: (0..documents).map(|_| Default::default()).collect(),
            resolvers: (0..documents).map(|_| None).collect(),
            deadline: self.deadline,
            sending: None,
            joining: None,
            closing: Vec::new(),
            subscriptions: 0,
        };
        // Its inputs were checked with the others' when the run started.
        run.enter(doc, workflow, Value::Object(self.root_inputs.clone()), None)?;
        Ok(run)
    }

    /// Advance whichever branch can go on, in the order they are listed;
    /// once every one is done, the workflow asked for starts.
    pub(super) fn advance_roots(&mut self) -> Result<Progress, ExecutionError> {
        'pass: loop {
            let roots = self.roots.as_mut().expect("root dependencies to run");
            if roots
                .entries
                .iter()
                .all(|entry| matches!(entry.state, State::Done(_)))
            {
                let roots = self.roots.take().expect("the roots just looked at");
                for entry in roots.entries {
                    if let State::Done(records) = entry.state {
                        self.records.extend(records);
                    }
                }
                let (doc, wanted) = self.queue.remove(0);
                let inputs = Value::Object(self.root_inputs.clone());
                self.enter(doc, wanted, inputs, None)?;
                return self.advance_inner();
            }
            let mut wake: Option<Instant> = None;
            for at in 0..roots.entries.len() {
                if self.start_root(at)? {
                    continue 'pass;
                }
                let flying = self.flying();
                let full = flying >= self.options.limits.parallel;
                let total = self.taken + self.roots.as_ref().map_or(0, Roots::taken);
                let left = self.options.limits.steps.saturating_sub(total);
                let roots = self.roots.as_mut().expect("root dependencies to run");
                let State::Running { run, wake: asleep } = &mut roots.entries[at].state else {
                    continue;
                };
                if let Some(until) = *asleep {
                    if until > Instant::now() {
                        wake = Some(wake.map_or(until, |wake| wake.min(until)));
                        continue;
                    }
                    *asleep = None;
                }
                // Out of steps, the branch waits while anything is in
                // flight: a workflow that ends gives back the steps it
                // still had in flight.
                if full || (left == 0 && flying > 0) {
                    continue;
                }
                run.cap = run.taken + left;
                match run.advance() {
                    Ok(Progress::Send(mut request)) => {
                        self.requests += 1;
                        let id = RequestId(self.requests);
                        roots.routes.push((id, at, request.id));
                        request.id = id;
                        return Ok(Progress::Send(request));
                    }
                    Ok(Progress::Await(_)) => {}
                    Ok(Progress::Wait(wait)) => {
                        // A wait too long to count to is one that does not end.
                        let until = Instant::now()
                            .checked_add(wait)
                            .or_else(|| Instant::now().checked_add(FOREVER))
                            .unwrap_or_else(Instant::now);
                        *asleep = Some(until);
                        wake = Some(wake.map_or(until, |wake| wake.min(until)));
                    }
                    Ok(Progress::Done(report)) => {
                        self.finish_root(at, *report)?;
                        continue 'pass;
                    }
                    // No branch reaches an AsyncAPI step: `plan` saw to it.
                    Ok(
                        Progress::Publish(_)
                        | Progress::Subscribe(_)
                        | Progress::Receive { .. }
                        | Progress::Unsubscribe(_),
                    ) => {
                        return Err(ExecutionError::Unsupported(
                            "an AsyncAPI step in a workflow run alongside others".into(),
                        ));
                    }
                    Err(error) => {
                        self.fail_root(at, error)?;
                        continue 'pass;
                    }
                }
            }
            let wake = wake.map(|wake| wake.saturating_duration_since(Instant::now()));
            return Ok(match wake {
                Some(wake) if self.flying() == 0 => Progress::Wait(wake),
                wake => Progress::Await(wake),
            });
        }
    }

    /// Start entry `at` if it is ready; whether it changed.
    fn start_root(&mut self, at: usize) -> Result<bool, ExecutionError> {
        let roots = self.roots.as_ref().expect("root dependencies to run");
        if !matches!(roots.entries[at].state, State::Waiting) || !roots.ready(at) {
            return Ok(false);
        }
        let Entry { doc, workflow, .. } = roots.entries[at];
        // A workflow an entry before it ran on its way is not run again.
        if self
            .dependencies_done
            .contains(&(doc, workflow.workflow_id.clone()))
        {
            self.roots.as_mut().expect("the roots").entries[at].state = State::Done(Vec::new());
            return Ok(true);
        }
        match self.branch(doc, workflow) {
            Ok(run) => {
                self.roots.as_mut().expect("the roots").entries[at].state = State::Running {
                    run: Box::new(run),
                    wake: None,
                };
            }
            Err(error) => self.fail_root(at, error)?,
        }
        Ok(true)
    }

    /// Entry `at` is done: what it finished is the run's now.
    fn finish_root(&mut self, at: usize, report: ExecutionReport) -> Result<(), ExecutionError> {
        let roots = self.roots.as_mut().expect("root dependencies to run");
        let State::Running { run, .. } =
            std::mem::replace(&mut roots.entries[at].state, State::Done(report.steps))
        else {
            unreachable!("only a running entry finishes");
        };
        roots.routes.retain(|(_, entry, _)| *entry != at);
        self.taken += run.taken;
        for (doc, workflow_id) in &run.fresh {
            if let Some(state) = run.finished[doc.0].get(workflow_id) {
                self.finished[doc.0].insert(workflow_id.clone(), state.clone());
            }
            self.fresh.insert((*doc, workflow_id.clone()));
        }
        self.dependencies_done.extend(run.dependencies_done);
        self.raise_held()
    }

    /// Entry `at` stopped with `error`. Those listed after it would not
    /// have run; those before it still might stop the run first.
    pub(super) fn fail_root(
        &mut self,
        at: usize,
        error: ExecutionError,
    ) -> Result<(), ExecutionError> {
        let roots = self.roots.as_mut().expect("root dependencies to run");
        let records = match std::mem::replace(&mut roots.entries[at].state, State::Abandoned) {
            State::Running { run, .. } => run.records,
            _ => Vec::new(),
        };
        roots.entries[at].state = State::Failed(records);
        for entry in &mut roots.entries[at + 1..] {
            entry.state = State::Abandoned;
        }
        roots.routes.retain(|(_, entry, _)| *entry < at);
        roots.held = Some((at, error));
        self.raise_held()
    }

    /// Raise the held error once nothing listed before it can stop the
    /// run first.
    fn raise_held(&mut self) -> Result<(), ExecutionError> {
        let roots = self.roots.as_mut().expect("root dependencies to run");
        let Some((at, _)) = roots.held else {
            return Ok(());
        };
        if roots.entries[..at]
            .iter()
            .all(|entry| matches!(entry.state, State::Done(_)))
        {
            let (_, error) = roots.held.take().expect("the error just looked at");
            return Err(error);
        }
        Ok(())
    }

    /// Hand a branch the response to its request.
    pub(super) fn supply_root(
        &mut self,
        id: RequestId,
        response: HttpResponse,
    ) -> Result<(), ExecutionError> {
        let roots = self.roots.as_mut().expect("root dependencies to run");
        let Some(&(_, at, local)) = roots.routes.iter().find(|(route, _, _)| *route == id) else {
            return Err(ExecutionError::NotWaiting);
        };
        let State::Running { run, .. } = &mut roots.entries[at].state else {
            return Err(ExecutionError::NotWaiting);
        };
        match run.supply_to(local, response) {
            Ok(()) => {
                let wanted = run.in_flight();
                roots
                    .routes
                    .retain(|(_, entry, local)| *entry != at || wanted.contains(local));
                Ok(())
            }
            Err(ExecutionError::NotWaiting) => Err(ExecutionError::NotWaiting),
            Err(error) => self.fail_root(at, error),
        }
    }
}
