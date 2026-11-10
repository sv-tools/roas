//! A time limit on a whole run: `Options::timeout`.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    ClientError, ExecutionError, HttpClient, HttpRequest, HttpResponse, Options, Outcome, Progress,
    Run, execute, execute_async_with_report, execute_with_report, prepare, testing::Fake,
};
use serde_json::{Value, json};
use std::time::Duration;

fn workflow(steps: Value) -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [{ "workflowId": "w", "steps": steps }],
    }))
    .expect("a v1.1 description")
}

fn options() -> Options {
    Options::new().source(
        "api",
        "https://api.example.com/openapi.json",
        json!({
            "openapi": "3.0.3",
            "servers": [{ "url": "https://api.example.com" }],
            "paths": { "/pets": { "get": { "operationId": "listPets" } } }
        }),
    )
}

fn list(timeout: Option<u64>) -> Value {
    let mut step = json!({ "stepId": "list", "operationId": "listPets" });
    if let Some(timeout) = timeout {
        step["timeout"] = json!(timeout);
    }
    step
}

fn first_request(description: &Description, options: &Options) -> HttpRequest {
    let mut run = Run::start(description, options).expect("a run");
    match run.advance().expect("progress") {
        Progress::Send(request) => request,
        other => panic!("expected a request, got {other:?}"),
    }
}

#[test]
fn a_request_waits_no_longer_than_the_run_has_left() {
    let minute = Duration::from_secs(60);
    let options = options().timeout(minute);
    // No timeout of its own: the time the run has left.
    let open = first_request(&workflow(json!([list(None)])), &options)
        .timeout
        .expect("a timeout");
    assert!(
        open <= minute && open > minute - Duration::from_secs(5),
        "{open:?}"
    );
    // A longer one of its own is cut to it.
    let long = first_request(&workflow(json!([list(Some(120_000))])), &options)
        .timeout
        .expect("a timeout");
    assert!(long <= minute, "{long:?}");
    // A shorter one is kept.
    let short = first_request(&workflow(json!([list(Some(1_000))])), &options).timeout;
    assert_eq!(short, Some(Duration::from_secs(1)));
    // Without a run limit, nothing changes.
    let unlimited = first_request(&workflow(json!([list(None)])), &self::options()).timeout;
    assert_eq!(unlimited, None);
}

#[test]
fn a_run_that_is_out_of_time_sends_nothing_more() {
    let description = workflow(json!([list(None)]));
    let mut client = Fake::new().reply(200, &json!({}));
    let failure = execute_with_report(
        &description,
        &options().timeout(Duration::ZERO),
        &mut client,
    )
    .unwrap_err();
    assert!(matches!(
        failure.error,
        ExecutionError::TimedOut { after } if after.is_zero()
    ));
    assert!(client.sent().is_empty());
    let report = failure.report.expect("a partial report");
    assert_eq!(report.outcome, Outcome::Incomplete);
    assert!(report.steps.is_empty());
}

#[test]
fn a_retry_wait_that_would_outlast_the_run_is_not_taken() {
    let mut retrying = list(None);
    retrying["successCriteria"] = json!([{ "condition": "$statusCode == 200" }]);
    retrying["onFailure"] =
        json!([{ "name": "again", "type": "retry", "retryAfter": 120, "retryLimit": 1 }]);
    let description = workflow(json!([retrying]));
    let options = options().timeout(Duration::from_secs(60));
    let mut run = Run::start(&description, &options).expect("a run");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    run.supply(HttpResponse::json(503, &json!({})))
        .expect("the response is understood");
    let error = run.advance().unwrap_err();
    assert!(
        matches!(error, ExecutionError::TimedOut { after } if after == Duration::from_secs(60))
    );
    // The attempt that came back is kept; the run is over.
    assert_eq!(run.partial_report().steps.len(), 1);
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
    // A wait the run has time for is asked for as written.
    let options = self::options().timeout(Duration::from_secs(600));
    let mut run = Run::start(&description, &options).expect("a run");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    run.supply(HttpResponse::json(503, &json!({})))
        .expect("the response is understood");
    assert!(matches!(
        run.advance(),
        Ok(Progress::Wait(wait)) if wait == Duration::from_secs(120)
    ));
}

/// Fails every request, after taking `delay` over it.
struct Failing {
    delay: Duration,
}

impl HttpClient for Failing {
    fn send(&mut self, _: &HttpRequest) -> Result<HttpResponse, ClientError> {
        std::thread::sleep(self.delay);
        Err(ClientError("timed out".to_owned()))
    }
}

#[test]
fn a_client_that_gives_up_at_the_deadline_times_the_run_out() {
    let description = workflow(json!([list(None)]));
    let error = execute(
        &description,
        &options().timeout(Duration::from_millis(20)),
        &mut Failing {
            delay: Duration::from_millis(40),
        },
    )
    .unwrap_err();
    assert!(matches!(error, ExecutionError::TimedOut { .. }), "{error}");
    // Before the deadline, a client failure is the client's.
    let error = execute(
        &description,
        &options().timeout(Duration::from_secs(60)),
        &mut Failing {
            delay: Duration::ZERO,
        },
    )
    .unwrap_err();
    assert!(matches!(error, ExecutionError::Client(_)), "{error}");
}

#[test]
fn each_run_of_a_plan_has_its_own_time() {
    let description = workflow(json!([list(None)]));
    let options = options().timeout(Duration::from_millis(100));
    let plan = prepare(&description, &options).expect("a plan");
    // Longer than the limit passes between preparing and running.
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..2 {
        let report = plan
            .execute(&mut Fake::new().reply(200, &json!({})))
            .expect("a report");
        assert!(report.is_success(), "{report}");
    }
    let options = self::options().timeout(Duration::ZERO);
    let plan = prepare(&description, &options).expect("a plan");
    let error = plan.execute(&mut Fake::new()).unwrap_err();
    assert!(
        matches!(error.error, ExecutionError::TimedOut { .. }),
        "{error}"
    );
}

#[test]
fn a_response_after_the_deadline_is_not_judged() {
    let mut ending = list(None);
    ending["successCriteria"] = json!([{ "condition": "$statusCode == 200" }]);
    ending["onFailure"] = json!([{ "name": "stop", "type": "end" }]);
    let description = workflow(json!([ending]));
    let options = options().timeout(Duration::from_millis(20));
    let mut run = Run::start(&description, &options).expect("a run");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    std::thread::sleep(Duration::from_millis(40));
    let error = run.supply(HttpResponse::json(503, &json!({}))).unwrap_err();
    assert!(matches!(error, ExecutionError::TimedOut { .. }), "{error}");
    // Not recorded as a failed step, and nothing more happens.
    assert!(run.partial_report().steps.is_empty());
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
    // A caller driving the run maps a late client failure the same way.
    let mut run = Run::start(&description, &options).expect("a run");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    let early = run.client_failure(ClientError("refused".to_owned()));
    assert!(matches!(early, ExecutionError::Client(_)), "{early}");
    std::thread::sleep(Duration::from_millis(40));
    let late = run.client_failure(ClientError("timed out".to_owned()));
    assert!(matches!(late, ExecutionError::TimedOut { .. }), "{late}");
}

#[test]
fn a_later_request_gets_what_is_left() {
    let description = workflow(json!([
        list(None),
        { "stepId": "again", "operationId": "listPets", "dependsOn": ["list"] }
    ]));
    let options = options().timeout(Duration::from_millis(500));
    let mut run = Run::start(&description, &options).expect("a run");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    std::thread::sleep(Duration::from_millis(200));
    run.supply(HttpResponse::json(200, &json!({})))
        .expect("the response is understood");
    let Ok(Progress::Send(request)) = run.advance() else {
        panic!("a second request");
    };
    let left = request.timeout.expect("a timeout");
    assert!(left <= Duration::from_millis(300), "{left:?}");
}

#[test]
fn a_retry_wait_in_a_called_workflow_counts_too() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [
            { "workflowId": "w", "steps": [{ "stepId": "call", "workflowId": "inner" }] },
            { "workflowId": "inner", "steps": [{
                "stepId": "list",
                "operationId": "listPets",
                "successCriteria": [{ "condition": "$statusCode == 200" }],
                "onFailure": [{ "name": "again", "type": "retry", "retryAfter": 120 }]
            }] }
        ],
    }))
    .expect("a v1.1 description");
    let options = options().timeout(Duration::from_secs(60));
    let mut run = Run::start(&description, &options).expect("a run");
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    run.supply(HttpResponse::json(503, &json!({})))
        .expect("the response is understood");
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::TimedOut { .. })
    ));
}

#[tokio::test]
async fn an_async_run_is_timed_the_same_way() {
    let description = workflow(json!([list(None)]));
    let mut client = Fake::new().reply(200, &json!({}));
    let failure = execute_async_with_report(
        &description,
        &options().timeout(Duration::ZERO),
        &mut client,
    )
    .await
    .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::TimedOut { .. }));
    assert!(client.sent().is_empty());
}
