//! What a run did, and what can stop one.
//!
//! The split matters: a step whose criteria do not hold is an *outcome*,
//! not an error — the workflow said what to do about it. An error is
//! something the run could not answer at all, like an operation no
//! description holds.

use crate::criterion::CriterionError;
use crate::expression::ExpressionError;
use crate::http::ClientError;
use crate::operation::OperationError;
use crate::select::SelectError;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// How a workflow finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Every step that ran met its criteria.
    #[default]
    Succeeded,
    /// A step failed and nothing said to carry on.
    Failed,
    /// An action ended the workflow before its last step.
    Ended,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Outcome::Succeeded => "succeeded",
            Outcome::Failed => "failed",
            Outcome::Ended => "ended early",
        })
    }
}

/// One criterion, and whether it held.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CriterionOutcome {
    /// The condition as the description wrote it.
    pub condition: String,
    /// Whether it held.
    pub passed: bool,
}

/// What a step did — Arazzo has two kinds, and they leave different
/// traces.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Performed {
    /// The step called an API operation.
    Request {
        /// The method sent.
        method: String,
        /// The URL sent to.
        url: String,
        /// The status received.
        status: u16,
    },
    /// The step called another workflow.
    Workflow {
        /// The workflow it called.
        workflow_id: String,
        /// How that workflow finished.
        outcome: Outcome,
    },
}

/// One attempt at one step.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StepRecord {
    /// The workflow the step belongs to.
    pub workflow_id: String,
    /// The step's id.
    pub step_id: String,
    /// 1 for the first try, 2 for the first retry, and so on.
    pub attempt: u32,
    /// What the step did.
    pub performed: Performed,
    /// Each success criterion, in the order the step lists them.
    pub criteria: Vec<CriterionOutcome>,
    /// Whether the step, as a whole, succeeded.
    pub passed: bool,
    /// The outputs the step named.
    pub outputs: BTreeMap<String, Value>,
    /// The action the step's outcome triggered, if any.
    pub action: Option<String>,
    /// How long it took.
    pub elapsed: Duration,
}

impl StepRecord {
    /// The status the step's request came back with, for a step that
    /// sent one.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match &self.performed {
            Performed::Request { status, .. } => Some(*status),
            Performed::Workflow { .. } => None,
        }
    }

    /// The method the step sent, for a step that sent a request.
    #[must_use]
    pub fn method(&self) -> Option<&str> {
        match &self.performed {
            Performed::Request { method, .. } => Some(method),
            Performed::Workflow { .. } => None,
        }
    }

    /// The URL the step sent to, for a step that sent a request.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match &self.performed {
            Performed::Request { url, .. } => Some(url),
            Performed::Workflow { .. } => None,
        }
    }
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecutionReport {
    /// The workflow that was asked for.
    pub workflow_id: String,
    /// How it finished.
    pub outcome: Outcome,
    /// The outputs it named.
    pub outputs: BTreeMap<String, Value>,
    /// Every attempt at every step, in the order they were made —
    /// including the steps of workflows this one depended on or called.
    pub steps: Vec<StepRecord>,
}

impl ExecutionReport {
    /// Whether the workflow ran to a successful end.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.outcome != Outcome::Failed
    }
}

impl fmt::Display for ExecutionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "workflow `{}` {}", self.workflow_id, self.outcome)?;
        for step in &self.steps {
            match &step.performed {
                Performed::Request {
                    method,
                    url,
                    status,
                } => write!(f, "- {} {method} {url} → {status}", step.step_id)?,
                Performed::Workflow {
                    workflow_id,
                    outcome,
                } => write!(f, "- {} → workflow `{workflow_id}` {outcome}", step.step_id)?,
            }
            if step.attempt > 1 {
                write!(f, " (attempt {})", step.attempt)?;
            }
            if !step.passed {
                write!(f, " — failed")?;
            }
            if let Some(action) = &step.action {
                write!(f, " — {action}")?;
            }
            writeln!(f)?;
        }
        for (name, value) in &self.outputs {
            writeln!(f, "  {name} = {value}")?;
        }
        Ok(())
    }
}

/// Why a run could not continue.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExecutionError {
    /// The description holds no workflow by that name.
    #[error("the description has no workflow `{0}`")]
    UnknownWorkflow(String),
    /// A `goto` named a step the workflow does not have.
    #[error("workflow `{workflow}` has no step `{step}` to go to")]
    UnknownStep {
        /// The workflow the `goto` was in.
        workflow: String,
        /// The step it named.
        step: String,
    },
    /// `dependsOn` describes a circle.
    #[error("`dependsOn` is circular: {0}")]
    Circular(String),
    /// The step's operation could not be found.
    #[error(transparent)]
    Operation(#[from] OperationError),
    /// A runtime expression could not be evaluated.
    #[error(transparent)]
    Expression(#[from] ExpressionError),
    /// A value or selector could not be resolved.
    #[error(transparent)]
    Select(#[from] SelectError),
    /// A criterion could not be decided.
    #[error(transparent)]
    Criterion(#[from] CriterionError),
    /// The client could not carry a request out.
    #[error("the request could not be sent: {0}")]
    Client(#[from] ClientError),
    /// The request could not be assembled.
    #[error("step `{step}` cannot be turned into a request: {reason}")]
    BadRequest {
        /// The step being built.
        step: String,
        /// What went wrong.
        reason: String,
    },
    /// A limit stopped the run — most likely a loop.
    #[error("the run stopped after reaching its {limit} limit of {at}")]
    Limit {
        /// Which limit.
        limit: &'static str,
        /// What it was set to.
        at: usize,
    },
    /// Something Arazzo allows that this crate does not execute.
    #[error("{0}")]
    Unsupported(String),
    /// `supply` was called when no request was outstanding.
    #[error("a response arrived when no request was outstanding")]
    NotWaiting,
    /// `advance` was called again before the outstanding request was
    /// answered.
    #[error("the run is waiting for a response to `{method} {url}` — supply it before advancing")]
    Awaiting {
        /// The method of the request still outstanding.
        method: String,
        /// Its URL.
        url: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(step_id: &str, status: u16, passed: bool) -> StepRecord {
        StepRecord {
            workflow_id: "buyPet".to_owned(),
            step_id: step_id.to_owned(),
            attempt: 1,
            performed: Performed::Request {
                method: "GET".to_owned(),
                url: format!("https://api.example.com/{step_id}"),
                status,
            },
            criteria: vec![CriterionOutcome {
                condition: "$statusCode == 200".to_owned(),
                passed,
            }],
            passed,
            outputs: BTreeMap::new(),
            action: None,
            elapsed: Duration::from_millis(12),
        }
    }

    #[test]
    fn a_report_reads_as_what_happened() {
        let report = ExecutionReport {
            workflow_id: "buyPet".to_owned(),
            outcome: Outcome::Succeeded,
            outputs: BTreeMap::from([("pet".to_owned(), json!({ "id": 7 }))]),
            steps: vec![record("findPet", 200, true)],
        };
        assert_eq!(
            report.to_string(),
            "workflow `buyPet` succeeded\n\
             - findPet GET https://api.example.com/findPet → 200\n  \
             pet = {\"id\":7}\n"
        );
        assert!(report.is_success());
    }

    #[test]
    fn a_failure_and_a_retry_show_in_the_line() {
        let mut failed = record("findPet", 503, false);
        failed.attempt = 2;
        failed.action = Some("retry".to_owned());
        let report = ExecutionReport {
            workflow_id: "buyPet".to_owned(),
            outcome: Outcome::Failed,
            outputs: BTreeMap::new(),
            steps: vec![failed],
        };
        let text = report.to_string();
        assert!(text.contains("workflow `buyPet` failed"), "{text}");
        assert!(text.contains("(attempt 2)"), "{text}");
        assert!(text.contains("— failed"), "{text}");
        assert!(text.contains("— retry"), "{text}");
        assert!(!report.is_success());
    }

    #[test]
    fn a_step_that_called_a_workflow_reads_as_what_it_called() {
        let record = StepRecord {
            workflow_id: "buyPet".to_owned(),
            step_id: "authenticate".to_owned(),
            attempt: 1,
            performed: Performed::Workflow {
                workflow_id: "login".to_owned(),
                outcome: Outcome::Succeeded,
            },
            criteria: Vec::new(),
            passed: true,
            outputs: BTreeMap::from([("token".to_owned(), json!("t-1"))]),
            action: None,
            elapsed: Duration::from_millis(30),
        };
        // It sent no request, so it has no method, URL or status to
        // report — and says so rather than inventing them.
        assert_eq!(record.status(), None);
        assert_eq!(record.method(), None);
        assert_eq!(record.url(), None);

        let report = ExecutionReport {
            workflow_id: "buyPet".to_owned(),
            outcome: Outcome::Succeeded,
            outputs: BTreeMap::new(),
            steps: vec![record],
        };
        assert_eq!(
            report.to_string(),
            "workflow `buyPet` succeeded\n\
             - authenticate → workflow `login` succeeded\n"
        );
    }

    #[test]
    fn a_step_that_sent_a_request_says_what_it_sent() {
        let record = record("findPet", 200, true);
        assert_eq!(record.status(), Some(200));
        assert_eq!(record.method(), Some("GET"));
        assert_eq!(record.url(), Some("https://api.example.com/findPet"));
    }

    #[test]
    fn a_run_waiting_for_a_response_says_which_one() {
        assert_eq!(
            ExecutionError::Awaiting {
                method: "GET".to_owned(),
                url: "https://api.example.com/pets".to_owned(),
            }
            .to_string(),
            "the run is waiting for a response to `GET https://api.example.com/pets` — supply it before advancing"
        );
    }

    #[test]
    fn an_outcome_reads_as_a_word() {
        assert_eq!(Outcome::Succeeded.to_string(), "succeeded");
        assert_eq!(Outcome::Failed.to_string(), "failed");
        assert_eq!(Outcome::Ended.to_string(), "ended early");
    }

    #[test]
    fn an_error_says_which_thing_was_missing() {
        assert_eq!(
            ExecutionError::UnknownWorkflow("nope".to_owned()).to_string(),
            "the description has no workflow `nope`"
        );
        assert_eq!(
            ExecutionError::UnknownStep {
                workflow: "buyPet".to_owned(),
                step: "nope".to_owned(),
            }
            .to_string(),
            "workflow `buyPet` has no step `nope` to go to"
        );
        assert_eq!(
            ExecutionError::Limit {
                limit: "step",
                at: 1000
            }
            .to_string(),
            "the run stopped after reaching its step limit of 1000"
        );
    }
}
