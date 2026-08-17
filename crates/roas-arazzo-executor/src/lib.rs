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
mod select;

pub mod testing;

#[cfg(feature = "reqwest")]
mod client;

pub use criterion::CriterionError;
pub use expression::ExpressionError;
pub use http::{
    AsyncHttpClient, ClientError, HttpClient, HttpRequest, HttpResponse, SendFuture, SleepFuture,
};
pub use report::{CriterionOutcome, ExecutionError, ExecutionReport, Outcome, StepRecord};
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
    let mut run = Run::start(description, options)?;
    loop {
        match run.advance()? {
            Progress::Send(request) => {
                let response = client.send(&request).map_err(ExecutionError::from)?;
                run.supply(response)?;
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
    let mut run = Run::start(description, options)?;
    loop {
        match run.advance()? {
            Progress::Send(request) => {
                let response = client.send(&request).await.map_err(ExecutionError::from)?;
                run.supply(response)?;
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
