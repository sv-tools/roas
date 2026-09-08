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
//! ones.
//!
//! Source descriptions are loaded the same way: fetching them is IO, so
//! the caller supplies the parsed documents through
//! [`Options::source`].
//!
//! ## What it does not do yet
//!
//! AsyncAPI steps (`channelPath` / `action`), XPath criteria and
//! selectors, `inputs` schema validation, and parallel `dependsOn`
//! execution. Each is reported where it is met rather than passed over,
//! so a run never looks successful because something was skipped.

mod criterion;
mod expression;
mod http;
mod operation;
mod report;
mod run;
mod runtime_syntax;
mod select;

pub mod testing;

#[cfg(feature = "reqwest")]
mod client;

pub use criterion::CriterionError;
pub use expression::ExpressionError;
pub use http::{
    AsyncHttpClient, ClientError, HttpClient, HttpRequest, HttpResponse, SendFuture, SleepFuture,
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
    loop {
        match run.advance().map_err(|error| run.failure(error))? {
            Progress::Send(request) => {
                let response = client
                    .send(&request)
                    .map_err(|error| run.failure(error.into()))?;
                run.supply(response).map_err(|error| run.failure(error))?;
            }
            Progress::Wait(duration) => std::thread::sleep(duration),
            Progress::Done(report) => return Ok(*report),
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
    loop {
        match run.advance().map_err(|error| run.failure(error))? {
            Progress::Send(request) => {
                let response = client
                    .send(&request)
                    .await
                    .map_err(|error| run.failure(error.into()))?;
                run.supply(response).map_err(|error| run.failure(error))?;
            }
            Progress::Wait(duration) => client.sleep(duration).await,
            Progress::Done(report) => return Ok(*report),
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
