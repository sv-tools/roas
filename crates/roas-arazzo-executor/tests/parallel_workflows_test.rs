//! The workflows the one asked for depends on, run together under
//! `Options::parallel`.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    AsyncHttpClient, ClientError, ExecutionError, ExecutionReport, HttpRequest, HttpResponse,
    Options, Outcome, Progress, RequestId, Run, SendFuture, SleepFuture, execute, execute_async,
    execute_async_with_report, testing::Fake,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn description(workflows: Value) -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": workflows,
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
            "paths": {
                "/a": { "get": { "operationId": "a" } },
                "/b": { "get": { "operationId": "b" } },
                "/c": { "get": { "operationId": "c" } },
                "/d": { "get": { "operationId": "d" } }
            }
        }),
    )
}

/// A workflow of one step calling `operation`, naming its body's `v`.
fn workflow(id: &str, operation: &str, depends_on: &[&str]) -> Value {
    json!({
        "workflowId": id,
        "dependsOn": depends_on,
        "steps": [{
            "stepId": operation,
            "operationId": operation,
            "outputs": { "v": "$response.body#/v" }
        }],
        "outputs": { "v": format!("$steps.{operation}.outputs.v") }
    })
}

fn send(run: &mut Run<'_>) -> HttpRequest {
    match run.advance() {
        Ok(Progress::Send(request)) => request,
        other => panic!("expected a request, got {other:?}"),
    }
}

fn path(request: &HttpRequest) -> &str {
    let last = request.url.rsplit('/').next().unwrap();
    last.split('?').next().unwrap()
}

fn reply(v: u64) -> HttpResponse {
    HttpResponse::json(200, &json!({ "v": v }))
}

fn done(run: &mut Run<'_>) -> ExecutionReport {
    match run.advance() {
        Ok(Progress::Done(report)) => *report,
        other => panic!("expected the end, got {other:?}"),
    }
}

fn steps(report: &ExecutionReport) -> Vec<(&str, &str)> {
    report
        .steps
        .iter()
        .map(|record| (record.workflow_id.as_str(), record.step_id.as_str()))
        .collect()
}

/// Two independent dependencies, and the workflow asked for after both.
fn two_roots() -> Description {
    description(json!([
        {
            "workflowId": "t",
            "dependsOn": ["one", "two"],
            "steps": [{ "stepId": "c", "operationId": "c" }],
            "outputs": {
                "one": "$workflows.one.outputs.v",
                "two": "$workflows.two.outputs.v"
            }
        },
        workflow("one", "a", &[]),
        workflow("two", "b", &[]),
    ]))
}

#[test]
fn independent_dependencies_are_in_flight_together() {
    let description = two_roots();
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    assert_eq!([path(&a), path(&b)], ["a", "b"]);
    assert_ne!(a.id, b.id);
    // The one asked for waits for both.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    assert_eq!(run.in_flight(), vec![a.id, b.id]);
    assert!(matches!(
        run.supply(reply(2)),
        Err(ExecutionError::InFlight { count: 2 })
    ));
    assert!(matches!(
        run.supply_to(RequestId::default(), reply(2)),
        Err(ExecutionError::NotWaiting)
    ));

    // Answered out of order, reported in the order they are listed.
    run.supply_to(b.id, reply(2)).unwrap();
    assert_eq!(run.in_flight(), vec![a.id]);
    assert!(matches!(
        run.supply_to(b.id, reply(2)),
        Err(ExecutionError::NotWaiting)
    ));
    // Shown once `one`, listed before it, is.
    assert!(steps(&run.partial_report()).is_empty());
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, reply(1)).unwrap();

    let c = send(&mut run);
    assert_eq!(path(&c), "c");
    assert!(c.id > b.id, "ids are not handed out twice");
    run.supply(reply(3)).unwrap();
    let report = done(&mut run);
    assert_eq!(steps(&report), [("one", "a"), ("two", "b"), ("t", "c")]);
    assert_eq!(report.outcome, Outcome::Succeeded);
    assert_eq!(report.outputs["one"], json!(1));
    assert_eq!(report.outputs["two"], json!(2));
}

#[test]
fn the_parallel_limit_covers_every_workflow() {
    let roots = two_roots();
    let options = options().parallel(2);
    let mut run = Run::start(&roots, &options).unwrap();
    let a = send(&mut run);
    let _b = send(&mut run);
    run.supply_to(a.id, reply(1)).unwrap();
    // One is still in flight, and the one asked for waits for it.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));

    let three = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two", "three"], "steps": [{ "stepId": "d", "operationId": "d" }] },
        workflow("one", "a", &[]),
        workflow("two", "b", &[]),
        workflow("three", "c", &[]),
    ]));
    let mut run = Run::start(&three, &options).unwrap();
    let a = send(&mut run);
    let _b = send(&mut run);
    // Two in flight: `three` waits for room.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, reply(1)).unwrap();
    assert_eq!(path(&send(&mut run)), "c");
}

/// Whether the description's dependencies `one` and `two` are sent
/// together: `one` sends `a` first either way.
fn together(description: &Description) -> bool {
    let options = options().parallel(4);
    let mut run = Run::start(description, &options).unwrap();
    assert_eq!(path(&send(&mut run)), "a");
    match run.advance() {
        Ok(Progress::Send(request)) => {
            assert_eq!(path(&request), "b");
            true
        }
        Ok(Progress::Await(None)) => false,
        other => panic!("expected a request or to wait, got {other:?}"),
    }
}

#[test]
fn what_cannot_be_told_apart_runs_one_after_another() {
    let target = json!({
        "workflowId": "t",
        "dependsOn": ["one", "two"],
        "steps": [{ "stepId": "c", "operationId": "c" }]
    });
    // An AsyncAPI step.
    let mut messaging = workflow("two", "b", &[]);
    messaging["steps"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "stepId": "m", "channelPath": "$sourceDescriptions.events.channels.x", "action": "send" }));
    assert!(!together(&description(json!([
        target,
        workflow("one", "a", &[]),
        messaging
    ]))));
    // A call that names nothing: the run says so when it gets there.
    assert!(!together(&description(json!([
        target,
        workflow("one", "a", &[]),
        { "workflowId": "two", "steps": [{ "stepId": "call", "workflowId": "nope" }] }
    ]))));
    // Actions that can go to the same workflow, and to workflows of
    // their own.
    let actions = |shared: &str| {
        let mut one = workflow("one", "a", &[]);
        one["steps"][0]["onSuccess"] = json!([{ "name": "on", "type": "goto", "workflowId": "x" }]);
        one["failureActions"] = json!([{ "name": "off", "type": "goto", "workflowId": "y" }]);
        let mut two = workflow("two", "b", &[]);
        two["steps"][0]["onFailure"] =
            json!([{ "name": "off", "type": "goto", "workflowId": shared }]);
        two["successActions"] = json!([{ "name": "on", "type": "goto", "workflowId": "z" }]);
        description(json!([
            target,
            one,
            two,
            workflow("x", "c", &[]),
            workflow("y", "c", &[]),
            workflow("z", "d", &[]),
            workflow("w", "d", &[]),
        ]))
    };
    assert!(!together(&actions("x")));
    assert!(!together(&actions("y")));
    assert!(together(&actions("w")));
}

#[test]
fn a_workflow_reached_twice_is_looked_at_once() {
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
        {
            "workflowId": "one",
            "steps": [
                { "stepId": "first", "workflowId": "x" },
                { "stepId": "second", "workflowId": "x" }
            ]
        },
        workflow("x", "a", &[]),
        workflow("two", "b", &[]),
    ]));
    assert!(together(&description));
}

#[test]
fn one_after_another_without_parallel() {
    let description = two_roots();
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    assert_eq!(path(&a), "a");
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::Awaiting { .. })
    ));
}

#[test]
fn a_dependency_that_reads_another_waits_for_it() {
    let mut reader = workflow("two", "b", &[]);
    reader["steps"][0]["parameters"] =
        json!([{ "name": "x", "in": "query", "value": "$workflows.one.outputs.v" }]);
    // The first one reads it.
    let mut reads = true;
    for workflows in [
        // Reads it.
        json!([
            { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
            workflow("one", "a", &[]),
            reader,
        ]),
        // Depends on it.
        json!([
            { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
            workflow("one", "a", &[]),
            workflow("two", "b", &["one"]),
        ]),
        // Calls a workflow that depends on it.
        json!([
            { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
            workflow("one", "a", &[]),
            { "workflowId": "two", "steps": [{ "stepId": "call", "workflowId": "x" }] },
            workflow("x", "b", &["one"]),
        ]),
    ] {
        let description = description(workflows);
        let options = options().parallel(4);
        let mut run = Run::start(&description, &options).unwrap();
        let a = send(&mut run);
        assert_eq!(path(&a), "a");
        assert!(matches!(run.advance(), Ok(Progress::Await(None))));
        run.supply_to(a.id, reply(1)).unwrap();
        let b = send(&mut run);
        assert_eq!(path(&b), "b");
        if reads {
            assert!(b.url.ends_with("/b?x=1"), "{}", b.url);
        }
        reads = false;
    }
}

#[test]
fn a_dependency_one_ran_on_its_way_is_not_run_again() {
    // `two` reaches `one` through a call, so `one` waits for it — and
    // then has nothing left to do. `three` runs alongside both.
    let description = description(json!([
        {
            "workflowId": "t",
            "dependsOn": ["two", "one", "three"],
            "steps": [{ "stepId": "d", "operationId": "d" }]
        },
        workflow("one", "a", &[]),
        { "workflowId": "two", "steps": [{ "stepId": "call", "workflowId": "x" }] },
        workflow("x", "b", &["one"]),
        workflow("three", "c", &[]),
    ]));
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let c = send(&mut run);
    assert_eq!([path(&a), path(&c)], ["a", "c"]);
    run.supply_to(a.id, reply(1)).unwrap();
    let b = send(&mut run);
    assert_eq!(path(&b), "b");
    run.supply_to(c.id, reply(3)).unwrap();
    run.supply_to(b.id, reply(2)).unwrap();
    let d = send(&mut run);
    assert_eq!(path(&d), "d");
    run.supply(reply(4)).unwrap();
    let report = done(&mut run);
    assert_eq!(
        steps(&report),
        [
            ("one", "a"),
            ("x", "b"),
            ("two", "call"),
            ("three", "c"),
            ("t", "d")
        ]
    );
}

/// `one` and `two` together; `two` stops the run once answered.
fn a_failing_second() -> Description {
    let mut failing = workflow("two", "b", &[]);
    failing["steps"][0]["onSuccess"] = json!([{ "name": "go", "type": "goto", "stepId": "nope" }]);
    description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two", "three"], "steps": [{ "stepId": "d", "operationId": "d" }] },
        workflow("one", "a", &[]),
        failing,
        workflow("three", "c", &[]),
    ]))
}

#[test]
fn an_error_waits_for_the_workflows_listed_before_it() {
    let description = a_failing_second();
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    // `one` may yet stop the run first: the error is held for it.
    run.supply_to(b.id, reply(2)).unwrap();
    // The workflow listed after the failing one would not have run.
    assert_eq!(run.in_flight(), vec![a.id]);
    assert!(matches!(
        run.supply_to(c.id, reply(3)),
        Err(ExecutionError::NotWaiting)
    ));
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, reply(1)).unwrap();
    // Once `one` is done, nothing can stop the run before `two`.
    let error = run.advance().unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownStep { ref step, .. } if step == "nope"),
        "got: {error}"
    );
    assert_eq!(steps(&run.partial_report()), [("one", "a"), ("two", "b")]);
    assert!(run.in_flight().is_empty());
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
}

#[test]
fn an_error_listed_earlier_wins() {
    let mut failing = workflow("one", "a", &[]);
    failing["steps"][0]["onSuccess"] = json!([{ "name": "go", "type": "goto", "stepId": "first" }]);
    let mut later = workflow("two", "b", &[]);
    later["steps"][0]["onSuccess"] = json!([{ "name": "go", "type": "goto", "stepId": "second" }]);
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two", "three"], "steps": [{ "stepId": "d", "operationId": "d" }] },
        failing,
        later,
        workflow("three", "c", &[]),
    ]));
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    run.supply_to(c.id, reply(3)).unwrap();
    run.supply_to(b.id, reply(2)).unwrap();
    let error = run.supply_to(a.id, reply(1)).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownStep { ref step, .. } if step == "first"),
        "got: {error}"
    );
    assert_eq!(steps(&run.partial_report()), [("one", "a")]);
}

#[test]
fn an_error_with_nothing_before_it_stops_the_run_at_once() {
    let description = a_failing_second();
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let _c = send(&mut run);
    run.supply_to(a.id, reply(1)).unwrap();
    // `one` is done once the run is advanced.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    let error = run.supply_to(b.id, reply(2)).unwrap_err();
    assert!(
        matches!(error, ExecutionError::UnknownStep { .. }),
        "got: {error}"
    );
}

#[test]
fn a_dependency_waiting_to_retry_says_when() {
    let description = retrying(0.05);
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    run.supply_to(a.id, HttpResponse::json(503, &json!({})))
        .unwrap();
    // `two` is still in flight: wait for it, or for the retry.
    match run.advance() {
        Ok(Progress::Await(Some(wake))) => assert!(wake <= Duration::from_millis(50)),
        other => panic!("expected to wait, got {other:?}"),
    }
    run.supply_to(b.id, reply(2)).unwrap();
    // Nothing in flight: a plain wait.
    match run.advance() {
        Ok(Progress::Wait(wake)) => {
            assert!(wake <= Duration::from_millis(50));
            std::thread::sleep(wake);
        }
        other => panic!("expected to wait, got {other:?}"),
    }
    let again = send(&mut run);
    assert_eq!(path(&again), "a");
    run.supply(reply(1)).unwrap();
    assert_eq!(path(&send(&mut run)), "c");
    run.supply(reply(3)).unwrap();
    let report = done(&mut run);
    assert_eq!(
        steps(&report),
        [("one", "a"), ("one", "a"), ("two", "b"), ("t", "c")]
    );
}

#[test]
fn the_step_limit_counts_every_workflow() {
    let description = two_roots();
    let options = options().parallel(4).max_steps(2);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    run.supply_to(a.id, reply(1)).unwrap();
    run.supply_to(b.id, reply(2)).unwrap();
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::Limit {
            limit: "step",
            at: 2
        })
    ));
    assert_eq!(steps(&run.partial_report()), [("one", "a"), ("two", "b")]);
}

#[test]
fn a_step_limit_reached_alongside_stops_the_run() {
    let description = two_roots();
    let options = options().parallel(4).max_steps(1);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    // `two` is out of steps: it waits while `one` may yet give some back.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    assert_eq!(run.in_flight(), vec![a.id]);
    let error = run.supply_to(a.id, reply(1));
    assert!(error.is_ok());
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::Limit {
            limit: "step",
            at: 1
        })
    ));
}

#[test]
fn a_dependency_that_cannot_start_stops_the_run_before_anything_is_sent() {
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
        workflow("one", "a", &[]),
        workflow("two", "b", &[]),
    ]));
    let options = options().parallel(4).max_depth(0);
    assert!(matches!(
        Run::start(&description, &options),
        Err(ExecutionError::Limit {
            limit: "workflow depth",
            ..
        })
    ));
}

#[test]
fn every_driver_reports_what_one_after_another_does() {
    let description = two_roots();
    let replies = || {
        Fake::new()
            .reply(200, &json!({ "v": 1 }))
            .reply(200, &json!({ "v": 2 }))
            .reply(200, &json!({ "v": 3 }))
    };
    let one_by_one = execute(&description, &options(), &mut replies()).unwrap();
    let blocking = execute(&description, &options().parallel(4), &mut replies()).unwrap();
    // `a` is answered after `b`.
    let mut late = Late::slow("a", Duration::from_millis(50));
    let parallel = block_on(execute_async(
        &description,
        &options().parallel(4),
        &mut late,
    ))
    .unwrap();
    assert_eq!(late.sent(), ["a", "b", "c"]);
    assert_eq!(one_by_one.outputs["one"], json!(1));
    assert_eq!(one_by_one.outputs["two"], json!(2));
    for report in [&blocking, &parallel] {
        assert_eq!(steps(report), steps(&one_by_one));
        assert_eq!(report.outputs, one_by_one.outputs);
        assert_eq!(report.outcome, one_by_one.outcome);
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

/// Answers `b` late, and `a` with a failure the first time — unless told
/// otherwise.
struct Late {
    slow: &'static str,
    late: Duration,
    retried: bool,
    /// A request the client fails to carry out, after `late`.
    broken: Option<&'static str>,
    sent: Mutex<Vec<(String, Instant)>>,
}

impl Late {
    fn new(late: Duration) -> Self {
        Self {
            slow: "b",
            late,
            retried: true,
            broken: None,
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Answers `slow` late, and everything at once and well.
    fn slow(slow: &'static str, late: Duration) -> Self {
        Self {
            slow,
            retried: false,
            ..Self::new(late)
        }
    }

    fn sent(self) -> Vec<String> {
        let sent = self.sent.into_inner().unwrap();
        sent.into_iter().map(|(path, _)| path).collect()
    }
}

impl AsyncHttpClient for Late {
    fn send<'a>(&'a self, request: &'a HttpRequest) -> SendFuture<'a> {
        Box::pin(async move {
            let path = path(request).to_owned();
            let first = {
                let mut sent = self.sent.lock().unwrap();
                sent.push((path.clone(), Instant::now()));
                sent.iter().filter(|(sent, _)| *sent == path).count() == 1
            };
            if path == self.slow || Some(path.as_str()) == self.broken {
                tokio::time::sleep(self.late).await;
            }
            if Some(path.as_str()) == self.broken {
                return Err(ClientError::new("boom"));
            }
            let status = if self.retried && path == "a" && first {
                503
            } else {
                200
            };
            // `a` is 1, `b` 2 and so on.
            let v = u64::from(path.as_bytes()[0] - b'a' + 1);
            Ok(HttpResponse::json(status, &json!({ "v": v })))
        })
    }

    fn sleep(&self, duration: Duration) -> SleepFuture<'_> {
        Box::pin(tokio::time::sleep(duration))
    }
}

fn retrying(after: f64) -> Description {
    let mut retrying = workflow("one", "a", &[]);
    retrying["steps"][0]["successCriteria"] = json!([{ "condition": "$statusCode == 200" }]);
    retrying["steps"][0]["onFailure"] =
        json!([{ "name": "again", "type": "retry", "retryAfter": after, "retryLimit": 1 }]);
    description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
        retrying,
        workflow("two", "b", &[]),
    ]))
}

#[tokio::test]
async fn the_async_driver_retries_while_another_workflow_waits() {
    let description = retrying(0.01);
    let mut client = Late::new(Duration::from_millis(500));
    let report = execute_async(&description, &options().parallel(4), &mut client)
        .await
        .unwrap();
    assert!(report.is_success(), "{report}");
    let sent = client.sent.into_inner().unwrap();
    let order: Vec<_> = sent.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(order, ["a", "b", "a", "c"]);
    // The retry did not wait for `b`'s answer.
    assert!(sent[2].1 - sent[1].1 < Duration::from_millis(400));
}

#[tokio::test]
async fn the_async_driver_takes_an_answer_that_comes_before_the_retry() {
    let description = retrying(0.2);
    let mut client = Late::new(Duration::from_millis(10));
    let report = execute_async(&description, &options().parallel(4), &mut client)
        .await
        .unwrap();
    assert!(report.is_success(), "{report}");
    let sent = client.sent.into_inner().unwrap();
    let order: Vec<_> = sent.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(order, ["a", "b", "a", "c"]);
    // The retry still waited as long as it asked.
    assert!(sent[2].1 - sent[0].1 >= Duration::from_millis(200));
}

#[test]
fn a_client_failure_waits_for_the_workflows_listed_before_it() {
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two", "three"], "steps": [{ "stepId": "d", "operationId": "d" }] },
        workflow("one", "a", &[]),
        workflow("two", "b", &[]),
        workflow("three", "c", &[]),
    ]));
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    run.supply_to(c.id, reply(3)).unwrap();
    run.supply_failure(b.id, ClientError::new("boom")).unwrap();
    assert_eq!(run.in_flight(), vec![a.id]);
    // What one after another would have recorded: `three` would not run.
    assert!(steps(&run.partial_report()).is_empty());
    run.supply_to(a.id, reply(1)).unwrap();
    let error = run.advance().unwrap_err();
    assert!(matches!(error, ExecutionError::Client(_)), "got: {error}");
    assert_eq!(steps(&run.partial_report()), [("one", "a")]);

    // Outside the workflows run together, it stops the run at once.
    let mut run = Run::start(&description, &options).unwrap();
    for _ in 0..3 {
        let request = send(&mut run);
        run.supply_to(request.id, reply(1)).unwrap();
    }
    let d = send(&mut run);
    assert!(matches!(
        run.supply_failure(d.id, ClientError::new("boom")),
        Err(ExecutionError::Client(_))
    ));
    assert!(matches!(
        run.supply_failure(d.id, ClientError::new("boom")),
        Err(ExecutionError::Stopped)
    ));
}

#[tokio::test]
async fn the_async_driver_reports_the_error_one_after_another_would() {
    // `two` stops the run when advanced past `b`, while `three` is in flight.
    let mut failing = workflow("two", "b", &[]);
    failing["steps"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "stepId": "b2", "operationId": "b", "parameters": [{ "name": "x", "in": "query", "value": "$inputs.nope" }] }));
    let stopping = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two", "three"], "steps": [{ "stepId": "d", "operationId": "d" }] },
        workflow("one", "a", &[]),
        failing,
        workflow("three", "c", &[]),
    ]));
    let mut late = Late::slow("c", Duration::from_millis(100));
    let failure = execute_async_with_report(&stopping, &options().parallel(4), &mut late)
        .await
        .unwrap_err();
    assert!(
        failure.error.to_string().contains("nope"),
        "got: {}",
        failure.error
    );

    // A client that fails: what one after another would have recorded.
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two", "three"], "steps": [{ "stepId": "d", "operationId": "d" }] },
        workflow("one", "a", &[]),
        workflow("two", "b", &[]),
        workflow("three", "c", &[]),
    ]));
    let mut late = Late::slow("a", Duration::from_millis(100));
    late.broken = Some("b");
    late.late = Duration::from_millis(30);
    let failure = execute_async_with_report(&description, &options().parallel(4), &mut late)
        .await
        .unwrap_err();
    assert!(
        matches!(failure.error, ExecutionError::Client(_)),
        "got: {}",
        failure.error
    );
    let report = failure.report.expect("a partial report");
    assert_eq!(steps(&report), [("one", "a")]);
}

#[test]
fn a_retry_too_far_off_to_count_to_does_not_panic() {
    let description = retrying(1e19);
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let _b = send(&mut run);
    run.supply_to(a.id, HttpResponse::json(503, &json!({})))
        .unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Await(Some(_)))));
}

#[test]
fn steps_given_back_count_for_the_others() {
    // `one` sends three steps together; the first fails and ends it,
    // giving back the two still in flight. One after another, the run
    // takes 1 + 2 + 1 = 4 steps.
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "t1", "operationId": "d" }] },
        {
            "workflowId": "one",
            "steps": [
                { "stepId": "s1", "operationId": "a" },
                { "stepId": "s2", "operationId": "c" },
                { "stepId": "s3", "operationId": "d" }
            ]
        },
        {
            "workflowId": "two",
            "steps": [
                { "stepId": "b1", "operationId": "b", "outputs": { "v": "$response.body#/v" } },
                { "stepId": "b2", "operationId": "b", "parameters": [{ "name": "x", "in": "query", "value": "$steps.b1.outputs.v" }] }
            ]
        },
    ]));
    let options = options().parallel(8).max_steps(4);
    let mut run = Run::start(&description, &options).unwrap();
    let s1 = send(&mut run);
    let _s2 = send(&mut run);
    let _s3 = send(&mut run);
    let b1 = send(&mut run);
    assert_eq!(path(&b1), "b");
    run.supply_to(b1.id, reply(2)).unwrap();
    // Out of steps while `one` may yet give some back.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(s1.id, HttpResponse::json(500, &json!({})))
        .unwrap();
    let b2 = send(&mut run);
    assert!(b2.url.ends_with("/b?x=2"), "{}", b2.url);
    run.supply(reply(2)).unwrap();
    assert_eq!(path(&send(&mut run)), "d");
    run.supply(reply(4)).unwrap();
    let report = done(&mut run);
    assert_eq!(
        steps(&report),
        [("one", "s1"), ("two", "b1"), ("two", "b2"), ("t", "t1")]
    );
}

#[test]
fn a_failure_of_a_request_no_longer_wanted_is_not_the_runs() {
    let description = a_failing_second();
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    run.supply_to(b.id, reply(2)).unwrap();
    // `three` was abandoned: its client's failure does not stop the run.
    assert!(matches!(
        run.supply_failure(c.id, ClientError::new("boom")),
        Err(ExecutionError::NotWaiting)
    ));
    run.supply_to(a.id, reply(1)).unwrap();
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::UnknownStep { .. })
    ));
}

#[test]
fn a_dependency_that_cannot_start_later_waits_for_the_ones_before_it() {
    let description = description(json!([
        { "workflowId": "t", "dependsOn": ["one", "two"], "steps": [{ "stepId": "c", "operationId": "c" }] },
        workflow("one", "a", &[]),
        {
            "workflowId": "two",
            "steps": [
                { "stepId": "x", "operationId": "b", "dependsOn": ["y"] },
                { "stepId": "y", "operationId": "b", "dependsOn": ["x"] }
            ]
        },
    ]));
    let options = options().parallel(4);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, reply(1)).unwrap();
    let error = run.advance().unwrap_err();
    assert!(matches!(error, ExecutionError::Circular(_)), "got: {error}");
    assert_eq!(steps(&run.partial_report()), [("one", "a")]);
}

#[tokio::test]
async fn a_retry_waits_by_the_clients_clock() {
    // `Fake` does not spend the waits it is asked for.
    let description = retrying(1.0);
    let mut client = Fake::new()
        .reply(503, &json!({}))
        .reply(200, &json!({ "v": 2 }))
        .reply(200, &json!({ "v": 1 }))
        .reply(200, &json!({ "v": 3 }));
    let started = Instant::now();
    let report = execute_async(&description, &options().parallel(4), &mut client)
        .await
        .unwrap();
    assert!(report.is_success(), "{report}");
    assert!(started.elapsed() < Duration::from_millis(500));
}
