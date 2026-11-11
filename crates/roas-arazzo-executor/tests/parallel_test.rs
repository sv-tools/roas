//! Steps in flight together: `Options::parallel`.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    AsyncHttpClient, ExecutionError, HttpRequest, HttpResponse, Options, Outcome, Progress,
    RequestId, Run, SendFuture, SleepFuture, execute, execute_async, execute_async_with_report,
    testing::Fake,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn description(steps: Value, actions: Value) -> Description {
    let mut workflow = json!({ "workflowId": "w", "steps": steps });
    if let Value::Object(actions) = actions {
        workflow.as_object_mut().unwrap().extend(actions);
    }
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [workflow],
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
                "/c": { "get": { "operationId": "c" } }
            }
        }),
    )
}

fn step(id: &str) -> Value {
    json!({ "stepId": id, "operationId": id })
}

fn send(run: &mut Run<'_>) -> HttpRequest {
    match run.advance() {
        Ok(Progress::Send(request)) => request,
        other => panic!("expected a request, got {other:?}"),
    }
}

fn path(request: &HttpRequest) -> &str {
    request
        .url
        .rsplit('/')
        .next()
        .unwrap()
        .split('?')
        .next()
        .unwrap()
}

fn ok() -> HttpResponse {
    HttpResponse::json(200, &json!({ "v": 1 }))
}

#[test]
fn independent_steps_are_in_flight_together() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let options = options().parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    assert_eq!([path(&a), path(&b), path(&c)], ["a", "b", "c"]);
    assert_ne!(a.id, b.id);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    assert_eq!(run.in_flight(), [a.id, b.id, c.id]);
    // Answered in any order, judged in the order they are listed.
    run.supply_to(c.id, ok()).unwrap();
    run.supply_to(a.id, ok()).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(b.id, ok()).unwrap();
    let Ok(Progress::Done(report)) = run.advance() else {
        panic!("done");
    };
    assert!(report.is_success(), "{report}");
    let done: Vec<_> = report.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(done, ["a", "b", "c"]);
}

#[test]
fn no_more_than_the_limit_are_in_flight() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let options = options().parallel(2);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    send(&mut run);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, ok()).unwrap();
    assert_eq!(path(&send(&mut run)), "c");
}

#[test]
fn a_step_waits_for_what_it_depends_on_or_reads() {
    for second in [
        json!({ "stepId": "b", "operationId": "b", "dependsOn": ["a"] }),
        json!({ "stepId": "b", "operationId": "b",
                "parameters": [{ "name": "x", "in": "query", "value": "$steps.a.outputs.v" }] }),
    ] {
        let mut first = step("a");
        first["outputs"] = json!({ "v": "$response.body#/v" });
        let description = description(json!([first, second, step("c")]), json!({}));
        let options = options().parallel(3);
        let mut run = Run::start(&description, &options).unwrap();
        let a = send(&mut run);
        // `b` needs `a`, and `c` comes after `b`.
        assert!(matches!(run.advance(), Ok(Progress::Await(None))));
        run.supply_to(a.id, ok()).unwrap();
        assert_eq!(path(&send(&mut run)), "b");
        assert_eq!(path(&send(&mut run)), "c");
    }
}

#[test]
fn a_step_with_actions_runs_alone() {
    let mut middle = step("b");
    middle["onSuccess"] = json!([{ "name": "next", "type": "goto", "stepId": "c" }]);
    let description = description(json!([step("a"), middle, step("c")]), json!({}));
    let options = options().parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    // `b` waits for `a` to be answered…
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, ok()).unwrap();
    let b = send(&mut run);
    // …and nothing starts beside it.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(b.id, ok()).unwrap();
    assert_eq!(path(&send(&mut run)), "c");

    // Workflow actions apply to every step: none runs alongside another.
    let description = self::description(
        json!([step("a"), step("b")]),
        json!({ "failureActions": [{ "name": "stop", "type": "end" }] }),
    );
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, ok()).unwrap();
    assert_eq!(path(&send(&mut run)), "b");
}

#[test]
fn a_failure_abandons_what_is_still_in_flight() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let options = options().parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    run.supply_to(a.id, ok()).unwrap();
    run.supply_to(b.id, HttpResponse::json(500, &json!({})))
        .unwrap();
    assert!(run.in_flight().is_empty());
    // Too late: `c` is no longer wanted.
    assert!(matches!(
        run.supply_to(c.id, ok()),
        Err(ExecutionError::NotWaiting)
    ));
    let Ok(Progress::Done(report)) = run.advance() else {
        panic!("done");
    };
    assert_eq!(report.outcome, Outcome::Failed);
    let done: Vec<_> = report.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(done, ["a", "b"]);
}

#[test]
fn supply_needs_an_id_when_several_are_in_flight() {
    let description = description(json!([step("a"), step("b")]), json!({}));
    let options = options().parallel(2);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    send(&mut run);
    let error = run.supply(ok()).unwrap_err();
    assert!(
        matches!(error, ExecutionError::InFlight { count: 2 }),
        "{error}"
    );
    assert!(error.to_string().contains("Run::supply_to"));
    // Not terminal.
    run.supply_to(a.id, ok()).unwrap();
    run.supply(ok()).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Done(_))));
    assert!(matches!(run.supply(ok()), Err(ExecutionError::NotWaiting)));
    assert!(matches!(
        run.supply_to(RequestId(9), ok()),
        Err(ExecutionError::NotWaiting)
    ));
}

#[test]
fn steps_run_one_at_a_time_by_default() {
    let description = description(json!([step("a"), step("b")]), json!({}));
    for options in [options(), options().parallel(0), options().parallel(1)] {
        let mut run = Run::start(&description, &options).unwrap();
        let a = send(&mut run);
        assert!(matches!(
            run.advance(),
            Err(ExecutionError::Awaiting { .. })
        ));
        run.supply(ok()).unwrap();
        assert_ne!(send(&mut run).id, a.id);
    }
}

#[test]
fn the_blocking_driver_sends_one_at_a_time() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let mut client = Fake::new()
        .reply(200, &json!({}))
        .reply(200, &json!({}))
        .reply(200, &json!({}));
    let report = execute(&description, &options().parallel(3), &mut client).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(client.sent().len(), 3);
}

#[test]
fn a_called_workflow_runs_its_own_steps_together() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [
            { "workflowId": "w", "steps": [
                { "stepId": "call", "workflowId": "inner" },
                step("c")
            ] },
            { "workflowId": "inner", "steps": [step("a"), step("b")] }
        ],
    }))
    .unwrap();
    let options = options().parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(b.id, ok()).unwrap();
    run.supply_to(a.id, ok()).unwrap();
    // The calling step is done only once its workflow is.
    assert_eq!(path(&send(&mut run)), "c");
}

/// Answers every request after `delay`, or at once with a 500 for a path
/// in `failing`, counting how many were in flight at most.
struct Slow {
    delay: Duration,
    failing: &'static str,
    flying: Mutex<(usize, usize)>,
}

impl Slow {
    fn new(delay: Duration, failing: &'static str) -> Self {
        Self {
            delay,
            failing,
            flying: Mutex::new((0, 0)),
        }
    }

    fn most(&self) -> usize {
        self.flying.lock().unwrap().1
    }
}

impl AsyncHttpClient for Slow {
    fn send<'a>(&'a self, request: &'a HttpRequest) -> SendFuture<'a> {
        Box::pin(async move {
            {
                let mut flying = self.flying.lock().unwrap();
                flying.0 += 1;
                flying.1 = flying.1.max(flying.0);
            }
            let failing = request.url.ends_with(self.failing);
            if !failing {
                tokio::time::sleep(self.delay).await;
            }
            self.flying.lock().unwrap().0 -= 1;
            Ok(HttpResponse::json(
                if failing { 500 } else { 200 },
                &json!({}),
            ))
        })
    }

    fn sleep(&self, duration: Duration) -> SleepFuture<'_> {
        Box::pin(tokio::time::sleep(duration))
    }
}

#[tokio::test]
async fn the_async_driver_sends_them_together() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let mut client = Slow::new(Duration::from_millis(50), "none");
    let report = execute_async(&description, &options().parallel(3), &mut client)
        .await
        .unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(client.most(), 3);
    let mut client = Slow::new(Duration::from_millis(5), "none");
    execute_async(&description, &options(), &mut client)
        .await
        .unwrap();
    assert_eq!(client.most(), 1);
}

#[tokio::test]
async fn the_async_driver_does_not_wait_for_what_was_abandoned() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let mut client = Slow::new(Duration::from_secs(5), "/a");
    let started = Instant::now();
    let report = execute_async(&description, &options().parallel(3), &mut client)
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(report.outcome, Outcome::Failed);
    let done: Vec<_> = report.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(done, ["a"]);
}

#[test]
fn a_receive_is_joined_with_nothing_else_in_flight() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" },
            { "name": "events", "url": "https://example.com/events.json", "type": "asyncapi" }
        ],
        "workflows": [{ "workflowId": "w", "steps": [
            { "stepId": "r", "channelPath": "$sourceDescriptions.events#/channels/news",
              "action": "receive", "timeout": 100 },
            step("a"),
            { "stepId": "b", "operationId": "b", "dependsOn": ["r"] }
        ] }],
    }))
    .unwrap();
    let options = options()
        .source(
            "events",
            "https://example.com/events.json",
            json!({ "asyncapi": "3.0.0", "info": { "title": "E", "version": "1" },
                    "channels": { "news": { "address": "news" } } }),
        )
        .parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    let a = send(&mut run);
    // `b` needs the receive, which is not joined while `a` is in flight.
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, ok()).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Receive { .. })));
}

#[tokio::test]
async fn a_step_that_cannot_start_waits_for_what_is_in_flight() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    // Out of steps at `c`: `a` and `b` are still answered and recorded.
    let mut client = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let failure = execute_async_with_report(
        &description,
        &options().parallel(3).max_steps(2),
        &mut client,
    )
    .await
    .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::Limit { .. }));
    let report = failure.report.unwrap();
    let done: Vec<_> = report.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(done, ["a", "b"]);
    // An earlier failure ends the workflow before the limit is reached,
    // as it would one step at a time.
    let mut client = Fake::new().reply(500, &json!({})).reply(200, &json!({}));
    let report = execute_async(
        &description,
        &options().parallel(3).max_steps(2),
        &mut client,
    )
    .await
    .unwrap();
    assert_eq!(report.outcome, Outcome::Failed);

    // The same for a request that cannot be built.
    let mut broken = step("c");
    broken["parameters"] = json!([{ "name": "x", "in": "query", "value": "$inputs.missing" }]);
    let description = self::description(json!([step("a"), step("b"), broken]), json!({}));
    let options = options().parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, HttpResponse::json(500, &json!({})))
        .unwrap();
    assert!(matches!(
        run.supply_to(b.id, ok()),
        Err(ExecutionError::NotWaiting)
    ));
    let Ok(Progress::Done(report)) = run.advance() else {
        panic!("done");
    };
    assert_eq!(report.outcome, Outcome::Failed);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, ok()).unwrap();
    run.supply_to(b.id, ok()).unwrap();
    assert!(run.advance().is_err());
    assert_eq!(run.partial_report().steps.len(), 2);
}

#[test]
fn a_stopped_run_wants_nothing_in_flight() {
    let mut first = step("a");
    first["outputs"] = json!({ "x": "$steps.nowhere.outputs.x" });
    let description = description(json!([first, step("b"), step("c")]), json!({}));
    let options = options().parallel(3);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    send(&mut run);
    send(&mut run);
    assert!(run.supply_to(a.id, ok()).is_err());
    assert!(run.in_flight().is_empty());
    assert!(matches!(run.supply(ok()), Err(ExecutionError::Stopped)));
}

#[tokio::test]
async fn the_async_driver_sends_in_the_order_the_run_asked() {
    let description = description(json!([step("a"), step("b"), step("c")]), json!({}));
    let mut client = Fake::new()
        .reply(200, &json!({}))
        .reply(200, &json!({}))
        .reply(500, &json!({}));
    let report = execute_async(&description, &options().parallel(3), &mut client)
        .await
        .unwrap();
    let sent: Vec<_> = client.sent().iter().map(|r| path(r).to_owned()).collect();
    assert_eq!(sent, ["a", "b", "c"]);
    assert_eq!(report.outcome, Outcome::Failed);
    let failed: Vec<_> = report
        .steps
        .iter()
        .filter(|s| !s.passed)
        .map(|s| s.step_id.as_str())
        .collect();
    assert_eq!(failed, ["c"]);
}

#[test]
fn requests_in_flight_together_share_the_run_time() {
    let description = description(json!([step("a"), step("b")]), json!({}));
    let options = options().parallel(2).timeout(Duration::from_millis(20));
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    assert!(a.timeout.is_some_and(|t| t <= Duration::from_millis(20)));
    assert!(b.timeout.is_some_and(|t| t <= Duration::from_millis(20)));
    std::thread::sleep(Duration::from_millis(40));
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    assert!(matches!(
        run.supply_to(a.id, ok()),
        Err(ExecutionError::TimedOut { .. })
    ));
    assert!(run.in_flight().is_empty());
}

fn broken(id: &str) -> Value {
    let mut step = step(id);
    step["outputs"] = json!({ "x": "$steps.nowhere.outputs.x" });
    step
}

fn judged(run: &Run<'_>) -> Vec<String> {
    run.partial_report()
        .steps
        .iter()
        .map(|step| step.step_id.clone())
        .collect()
}

#[test]
fn responses_are_judged_in_the_order_the_steps_are_listed() {
    let options = options().parallel(3);
    let description = description(json!([step("a"), broken("b"), step("c")]), json!({}));
    // `b` waits for `a`, whose failure ends the workflow: `b` never was.
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    run.supply_to(b.id, ok()).unwrap();
    assert_eq!(run.in_flight(), [a.id, c.id]);
    assert!(matches!(
        run.supply_to(b.id, ok()),
        Err(ExecutionError::NotWaiting)
    ));
    assert!(matches!(run.advance(), Ok(Progress::Await(None))));
    run.supply_to(a.id, HttpResponse::json(500, &json!({})))
        .unwrap();
    assert!(run.in_flight().is_empty());
    let Ok(Progress::Done(report)) = run.advance() else {
        panic!("done");
    };
    assert_eq!(report.outcome, Outcome::Failed);
    let done: Vec<_> = report.steps.iter().map(|s| s.step_id.as_str()).collect();
    assert_eq!(done, ["a"]);
    // `a` passes: `b`'s error stops the run before `c`'s failure is judged.
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    let c = send(&mut run);
    run.supply_to(b.id, ok()).unwrap();
    run.supply_to(c.id, HttpResponse::json(500, &json!({})))
        .unwrap();
    assert!(matches!(
        run.supply_to(a.id, ok()),
        Err(ExecutionError::Select(_))
    ));
    assert_eq!(judged(&run), ["a", "b"]);
    assert!(run.in_flight().is_empty());
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
    // A later failure waits for an earlier error, as one at a time.
    let description = self::description(json!([broken("a"), step("b")]), json!({}));
    let options = self::options().parallel(2);
    let mut run = Run::start(&description, &options).unwrap();
    let a = send(&mut run);
    let b = send(&mut run);
    run.supply_to(b.id, HttpResponse::json(500, &json!({})))
        .unwrap();
    assert!(matches!(
        run.supply_to(a.id, ok()),
        Err(ExecutionError::Select(_))
    ));
    assert_eq!(judged(&run), ["a"]);
}

#[test]
fn abandoned_requests_do_not_count_as_steps_taken() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [
            { "workflowId": "w", "steps": [
                { "stepId": "call", "workflowId": "inner",
                  "onFailure": [{ "name": "on", "type": "goto", "stepId": "c" }] },
                step("c")
            ] },
            { "workflowId": "inner", "steps": [step("a"), step("b")] }
        ],
    }))
    .unwrap();
    for parallel in [1, 2] {
        let options = options().parallel(parallel).max_steps(3);
        let mut run = Run::start(&description, &options).unwrap();
        let a = send(&mut run);
        if parallel > 1 {
            send(&mut run);
        }
        run.supply_to(a.id, HttpResponse::json(500, &json!({})))
            .unwrap();
        assert_eq!(path(&send(&mut run)), "c", "parallel {parallel}");
    }
}
