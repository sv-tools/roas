//! Executes OpenAPI Arazzo workflows.
//!
//! An Arazzo description is a program: ordered steps that call API
//! operations, assert on the responses, name outputs, and branch on
//! success or failure. [`roas-arazzo`](https://crates.io/crates/roas-arazzo)
//! parses and validates one; this crate runs it.
//!
//! ```no_run
//! # use roas_arazzo::v1_1::Description;
//! # use roas_arazzo_executor::{Options, execute};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let description: Description = serde_json::from_str("{}")?;
//! # let mut client = roas_arazzo_executor::testing::Fake::default();
//! let options = Options::new().workflow("buyPet");
//! let report = execute(&description, &options, &mut client)?;
//! println!("{report}");
//! # Ok(()) }
//! ```
//!
//! ## No IO of its own
//!
//! The engine decides *what* to send and asks a client to send it, so
//! the same engine runs under a blocking client, an async one, or a fake
//! that never touches a network. Implement [`HttpClient`] (or
//! [`AsyncHttpClient`]), or enable the `reqwest` feature for ready-made
//! ones. AsyncAPI steps publish and listen through a [`MessageClient`]
//! (or [`AsyncMessageClient`]) in the same way; see [`execute_with_messages`].
//!
//! Source descriptions are loaded the same way: fetching them is IO, so
//! the caller supplies the parsed documents through
//! [`Options::source`].
//! The optional `source-graph` feature adds a document registry and bounded
//! sync/async loading through explicitly configured `roas` fetchers. It retains
//! canonical Arazzo identities, retrieval metadata, local aliases and loading
//! diagnostics without adding IO to the execution state machine.
//! Workflows of linked Arazzo documents are callable as
//! `$sourceDescriptions.<name>.<workflowId>`; each runs in its own document's
//! scope (components, sources, `$self`) on the same engine as a local call.
//!
//! ## Checked preparation
//!
//! [`prepare`] validates static requirements before execution and returns a
//! reusable [`PreparedWorkflow`]. [`required_sources`] identifies which sources
//! to supply first. Existing execution functions retain lazy validation;
//! preparation is an explicit, stricter library path used by the CLI.
//!
//! ## What it does not do yet
//!
//! AsyncAPI steps without a message client — run them with
//! [`execute_with_messages`] — XPath criteria and
//! selectors without the `xpath` feature (and, with it, XPath versions
//! Arazzo does not name, XPath payload replacement targets, and the XPath
//! functions it refuses), and running the workflows a workflow depends on
//! together (steps run together under [`Options::parallel`]). Unsupported execution capabilities are reported when
//! reached, or statically by preparation. The optional `input-validation` feature supports
//! offline JSON Schema 2020-12 when [`Options::input_validation`] enables it.
//! Plans compile schemas; each run validates its actual bound inputs at entry.
//! The [README's support matrix](https://crates.io/crates/roas-arazzo-executor#support-matrix)
//! separates what is parsed, resolved, validated and executed.

mod channel;
mod criterion;
mod documents;
mod expression;
mod http;
mod input;
#[cfg(feature = "input-validation")]
mod input_catalog;
mod message;
mod operation;
mod operation_document;
#[cfg(feature = "source-graph")]
mod operation_graph;
mod operation_index;
mod prepare;
mod report;
mod run;
mod runtime_syntax;
mod select;
#[cfg(feature = "source-graph")]
mod source_graph;
#[cfg(feature = "source-graph")]
mod source_registry;
#[cfg(feature = "source-graph")]
pub use source_graph::{SourceCycle, SourceLoadOptions, SourceLoadReport};
#[cfg(feature = "source-graph")]
pub use source_registry::{
    DocumentId, SourceDiagnostic, SourceDocument, SourceError, SourceLink, SourceRegistry,
    SourceVersion,
};

pub mod testing;

#[cfg(feature = "reqwest")]
mod client;
#[cfg(feature = "xpath")]
mod xpath;

pub use criterion::CriterionError;
pub use expression::ExpressionError;
pub use http::{
    AsyncHttpClient, ClientError, HttpClient, HttpRequest, HttpResponse, RequestId, SendFuture,
    SleepFuture,
};
pub use input::{InputError, InputValidation, InputViolation};
pub use message::{
    AsyncMessageClient, Channel, DoneFuture, IncomingMessage, MessageClient, OutgoingMessage,
    ReceiveFuture, Subscription, SubscriptionId,
};
pub use operation::OperationError;
#[cfg(feature = "source-graph")]
pub use operation_graph::ReferenceDiagnostic;
pub use prepare::{
    CONDITION_PROFILE, PreparationDiagnostic, PreparationError, PreparationIssue, PreparedWorkflow,
    prepare, required_sources,
};
pub use report::{
    ActionCriteriaOutcome, CriterionOutcome, ExecutionError, ExecutionFailure, ExecutionReport,
    Outcome, Performed, StepRecord,
};
pub use run::{Options, Progress, Run};
pub use select::SelectError;

#[cfg(feature = "reqwest")]
pub use client::Client;

use roas_arazzo::v1_1::Description;
use std::pin::Pin;

/// Run a workflow, performing every request with `client`.
///
/// The workflow is [`Options::workflow`], or the first one in the
/// description. The report says what each step did; a step that fails
/// its criteria is part of the report, not an error.
/// Runtime criterion diagnostics are retained in the report. Use
/// [`execute_with_report`] to also retain history on terminal engine errors.
///
/// # Errors
///
/// [`ExecutionError`] when the run cannot continue: an unknown workflow,
/// an operation that cannot be resolved, an expression that names
/// nothing, a client failure, or a limit reached.
pub fn execute<C: HttpClient + ?Sized>(
    description: &Description,
    options: &Options,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionError> {
    execute_with_report(description, options, client).map_err(|failure| failure.error)
}

/// Run a workflow, retaining a partial report when an engine or client error stops it.
///
/// ```no_run
/// # use roas_arazzo::v1_1::Description;
/// # use roas_arazzo_executor::{Options, HttpClient, execute_with_report};
/// # fn inspect(description: &Description, options: &Options, client: &mut dyn HttpClient) {
/// match execute_with_report(description, options, client) {
///     Ok(report) => println!("{report}"),
///     Err(failure) => {
///         eprintln!("{}", failure.error);
///         if let Some(report) = failure.report {
///             eprintln!("{report}");
///         }
///     }
/// }
/// # }
/// ```
///
/// # Errors
///
/// [`ExecutionFailure`] contains the original [`ExecutionError`] and a partial
/// report with [`Outcome::Incomplete`]. Its report is `None` when preparation
/// failed before a [`Run`] could be created. Criterion evaluation failures alone
/// are normal failed outcomes and may be recovered by the workflow's actions.
/// Unsupported criterion capabilities remain terminal errors.
pub fn execute_with_report<C: HttpClient + ?Sized>(
    description: &Description,
    options: &Options,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionFailure> {
    let mut run = Run::start(description, options).map_err(|error| ExecutionFailure {
        error,
        report: None,
    })?;
    drive(&mut run, client)
}

pub(crate) fn drive<C: HttpClient + ?Sized>(
    run: &mut Run<'_>,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionFailure> {
    drive_with(run, client, None)
}

/// Run a workflow whose AsyncAPI steps publish to and listen on channels
/// through `messages`, and whose other steps send requests with `client`.
///
/// [`execute`] and its kin refuse an AsyncAPI step; this is the same
/// engine with somewhere for its messages to go.
///
/// # Errors
///
/// As [`execute_with_report`]; a failure of either client stops the run.
pub fn execute_with_messages<C: HttpClient + ?Sized, M: MessageClient + ?Sized>(
    description: &Description,
    options: &Options,
    client: &mut C,
    messages: &mut M,
) -> Result<ExecutionReport, ExecutionFailure> {
    let mut run = Run::start(description, options).map_err(|error| ExecutionFailure {
        error,
        report: None,
    })?;
    drive_with(&mut run, client, Some(&mut Messages(messages)))
}

/// Async counterpart of [`execute_with_messages`].
///
/// # Errors
///
/// As [`execute_with_messages`].
pub async fn execute_with_messages_async<
    C: AsyncHttpClient + ?Sized,
    M: AsyncMessageClient + ?Sized,
>(
    description: &Description,
    options: &Options,
    client: &mut C,
    messages: &mut M,
) -> Result<ExecutionReport, ExecutionFailure> {
    let mut run = Run::start(description, options).map_err(|error| ExecutionFailure {
        error,
        report: None,
    })?;
    drive_async_with(&mut run, client, Some(&mut AsyncMessages(messages))).await
}

/// A message client behind one object-safe face.
pub(crate) struct Messages<'m, M: MessageClient + ?Sized>(pub(crate) &'m mut M);

pub(crate) trait Carrier {
    fn carry(&mut self, run: &Run<'_>, progress: Progress) -> Result<Carried, ExecutionError>;
    fn close(&mut self, subscription: SubscriptionId);
}

/// What came of a message progress.
pub(crate) enum Carried {
    Nothing,
    Message(Option<IncomingMessage>),
}

impl<M: MessageClient + ?Sized> Carrier for Messages<'_, M> {
    fn carry(&mut self, run: &Run<'_>, progress: Progress) -> Result<Carried, ExecutionError> {
        let client = &mut *self.0;
        Ok(match progress {
            Progress::Publish(message) => {
                client
                    .publish(&message)
                    .map_err(|error| run.client_failure(error))?;
                Carried::Nothing
            }
            Progress::Subscribe(subscription) => {
                client
                    .subscribe(&subscription)
                    .map_err(|error| run.client_failure(error))?;
                Carried::Nothing
            }
            Progress::Receive {
                subscription,
                timeout,
            } => Carried::Message(
                client
                    .receive(subscription, timeout)
                    .map_err(|error| run.client_failure(error))?,
            ),
            Progress::Unsubscribe(subscription) => {
                client.unsubscribe(subscription);
                Carried::Nothing
            }
            Progress::Send(_) | Progress::Await | Progress::Wait(_) | Progress::Done(_) => {
                Carried::Nothing
            }
        })
    }

    fn close(&mut self, subscription: SubscriptionId) {
        self.0.unsubscribe(subscription);
    }
}

/// Why a run without a message client cannot carry on.
fn no_messages() -> ExecutionError {
    ExecutionError::Unsupported(
        "an AsyncAPI step needs a message client — run it with `execute_with_messages`".into(),
    )
}

pub(crate) fn drive_with<C: HttpClient + ?Sized>(
    run: &mut Run<'_>,
    client: &mut C,
    mut messages: Option<&mut dyn Carrier>,
) -> Result<ExecutionReport, ExecutionFailure> {
    let result = drive_loop(
        run,
        client,
        messages
            .as_mut()
            .map(|messages| &mut **messages as &mut dyn Carrier),
    );
    // A run that stopped early leaves its subscriptions open; close them.
    if result.is_err()
        && let Some(messages) = messages
    {
        for subscription in run.open_subscriptions() {
            messages.close(subscription);
        }
    }
    result
}

fn drive_loop<C: HttpClient + ?Sized>(
    run: &mut Run<'_>,
    client: &mut C,
    mut messages: Option<&mut dyn Carrier>,
) -> Result<ExecutionReport, ExecutionFailure> {
    loop {
        match run.advance().map_err(|error| run.failure(error))? {
            // One at a time: each request is answered before the next
            // advance, so the run never has to wait on one in flight.
            Progress::Send(request) => {
                let response = client
                    .send(&request)
                    .map_err(|error| run.failure(run.client_failure(error)))?;
                run.supply_to(request.id, response)
                    .map_err(|error| run.failure(error))?;
            }
            Progress::Await => unreachable!("every request is answered before the next advance"),
            Progress::Wait(duration) => std::thread::sleep(duration),
            Progress::Done(report) => return Ok(*report),
            progress => {
                let Some(messages) = messages.as_deref_mut() else {
                    return Err(run.failure(no_messages()));
                };
                if let Carried::Message(message) = messages
                    .carry(run, progress)
                    .map_err(|error| run.failure(error))?
                {
                    run.deliver(message).map_err(|error| run.failure(error))?;
                }
            }
        }
    }
}

/// Run a workflow, performing every request with an async `client`.
///
/// The same engine as [`execute`]; only the waiting differs.
///
/// # Errors
///
/// As [`execute`].
pub async fn execute_async<C: AsyncHttpClient + ?Sized>(
    description: &Description,
    options: &Options,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionError> {
    execute_async_with_report(description, options, client)
        .await
        .map_err(|failure| failure.error)
}

/// Async counterpart of [`execute_with_report`], preserving the same diagnostics
/// and partial history. Retry waits use the client's async sleep implementation.
///
/// # Errors
///
/// As [`execute_with_report`].
pub async fn execute_async_with_report<C: AsyncHttpClient + ?Sized>(
    description: &Description,
    options: &Options,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionFailure> {
    let mut run = Run::start(description, options).map_err(|error| ExecutionFailure {
        error,
        report: None,
    })?;
    drive_async(&mut run, client).await
}

pub(crate) async fn drive_async<C: AsyncHttpClient + ?Sized>(
    run: &mut Run<'_>,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionFailure> {
    drive_async_with(
        run,
        client,
        None::<&mut AsyncMessages<'_, dyn AsyncMessageClient + Send>>,
    )
    .await
}

/// An async message client, as the async driver uses it.
pub(crate) struct AsyncMessages<'m, M: AsyncMessageClient + ?Sized>(pub(crate) &'m mut M);

pub(crate) async fn drive_async_with<
    C: AsyncHttpClient + ?Sized,
    M: AsyncMessageClient + ?Sized,
>(
    run: &mut Run<'_>,
    client: &mut C,
    mut messages: Option<&mut AsyncMessages<'_, M>>,
) -> Result<ExecutionReport, ExecutionFailure> {
    let result = drive_async_loop(run, client, messages.as_deref_mut()).await;
    // A run that stopped early leaves its subscriptions open; close them.
    if result.is_err()
        && let Some(AsyncMessages(messages)) = messages
    {
        for subscription in run.open_subscriptions() {
            messages.unsubscribe(subscription);
        }
    }
    result
}

/// Hand the run the first response to come back, and stop waiting for
/// whatever the run then abandoned.
async fn answer_one<F: Future<Output = Result<HttpResponse, ClientError>>>(
    run: &mut Run<'_>,
    flying: &mut Vec<(RequestId, Pin<Box<F>>)>,
) -> Result<(), ExecutionFailure> {
    let (id, result) = first_answered(flying).await;
    let response = result.map_err(|error| run.failure(run.client_failure(error)))?;
    run.supply_to(id, response)
        .map_err(|error| run.failure(error))?;
    let wanted = run.in_flight();
    flying.retain(|(id, _)| wanted.contains(id));
    Ok(())
}

/// Wait for whichever request in flight is answered first, and take it out.
async fn first_answered<F: Future<Output = Result<HttpResponse, ClientError>>>(
    flying: &mut Vec<(RequestId, Pin<Box<F>>)>,
) -> (RequestId, Result<HttpResponse, ClientError>) {
    std::future::poll_fn(|context| {
        for at in 0..flying.len() {
            if let std::task::Poll::Ready(result) = flying[at].1.as_mut().poll(context) {
                // In order: the ones still in flight are polled, and so
                // sent, in the order they were handed out.
                let (id, _) = flying.remove(at);
                return std::task::Poll::Ready((id, result));
            }
        }
        std::task::Poll::Pending
    })
    .await
}

async fn drive_async_loop<C: AsyncHttpClient + ?Sized, M: AsyncMessageClient + ?Sized>(
    run: &mut Run<'_>,
    client: &mut C,
    mut messages: Option<&mut AsyncMessages<'_, M>>,
) -> Result<ExecutionReport, ExecutionFailure> {
    // Requests in flight, with the id of each.
    let mut flying = Vec::new();
    loop {
        match run.advance().map_err(|error| run.failure(error))? {
            Progress::Send(request) => {
                let id = request.id;
                let client = &*client;
                flying.push((id, Box::pin(async move { client.send(&request).await })));
                // Ask for another while the run may have one more in flight.
                if !run.takes_more() {
                    answer_one(run, &mut flying).await?;
                }
            }
            Progress::Await => answer_one(run, &mut flying).await?,
            Progress::Wait(duration) => client.sleep(duration).await,
            Progress::Done(report) => return Ok(*report),
            progress => {
                let Some(AsyncMessages(messages)) = messages.as_deref_mut() else {
                    return Err(run.failure(no_messages()));
                };
                match progress {
                    Progress::Publish(message) => messages
                        .publish(&message)
                        .await
                        .map_err(|error| run.failure(run.client_failure(error)))?,
                    Progress::Subscribe(subscription) => messages
                        .subscribe(&subscription)
                        .await
                        .map_err(|error| run.failure(run.client_failure(error)))?,
                    Progress::Receive {
                        subscription,
                        timeout,
                    } => {
                        let message = messages
                            .receive(subscription, timeout)
                            .await
                            .map_err(|error| run.failure(run.client_failure(error)))?;
                        run.deliver(message).map_err(|error| run.failure(error))?;
                    }
                    Progress::Unsubscribe(subscription) => messages.unsubscribe(subscription),
                    Progress::Send(_) | Progress::Await | Progress::Wait(_) | Progress::Done(_) => {
                    }
                }
            }
        }
    }
}

/// Run an Arazzo v1.0 description, upconverting it to v1.1 first.
///
/// # Errors
///
/// As [`execute`].
#[cfg(feature = "v1_0")]
pub fn execute_v1_0<C: HttpClient + ?Sized>(
    description: &roas_arazzo::v1_0::Description,
    options: &Options,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionError> {
    execute(&Description::from(description.clone()), options, client)
}

/// Run an Arazzo v1.0 description with the report-preserving API after upconversion.
/// Runtime criterion recovery follows the same policy as v1.1 execution.
///
/// # Errors
///
/// As [`execute_with_report`].
#[cfg(feature = "v1_0")]
pub fn execute_v1_0_with_report<C: HttpClient + ?Sized>(
    description: &roas_arazzo::v1_0::Description,
    options: &Options,
    client: &mut C,
) -> Result<ExecutionReport, ExecutionFailure> {
    execute_with_report(&Description::from(description.clone()), options, client)
}

#[cfg(test)]
mod send_tests {
    /// The async entry points stay usable from a spawned task.
    #[test]
    fn futures_are_send() {
        fn send<T: Send>(_: &T) {}
        let description: roas_arazzo::v1_1::Description =
            serde_json::from_value(serde_json::json!({
                "arazzo": "1.1.0",
                "info": { "title": "T", "version": "1" },
                "sourceDescriptions": [],
                "workflows": [{ "workflowId": "w", "steps": [] }]
            }))
            .expect("a description");
        let options = crate::Options::new();
        let mut client = crate::testing::Fake::new();
        let mut messages = crate::testing::Fake::new();
        send(&crate::execute_async(&description, &options, &mut client));
        send(&crate::execute_with_messages_async(
            &description,
            &options,
            &mut client,
            &mut messages,
        ));
    }
}
