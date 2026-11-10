//! AsyncAPI steps: publishing, listening in the background, and joining a
//! receive where a later step needs it.

use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{
    ExecutionError, HttpResponse, IncomingMessage, Options, Outcome, Performed, Progress, Run,
    SubscriptionId, execute_with_messages, execute_with_messages_async, prepare, testing::Fake,
};
use serde_json::{Value, json};
use std::time::Duration;

fn asyncapi_3() -> Value {
    json!({
        "asyncapi": "3.0.0",
        "info": { "title": "Orders", "version": "1" },
        "servers": {
            "broker": { "host": "broker.example.com:9092", "protocol": "kafka" },
            "socket": { "host": "ws.example.com", "protocol": "ws" }
        },
        "channels": {
            "orders": {
                "address": "orders.{region}",
                "parameters": { "region": {} },
                "servers": [{ "$ref": "#/servers/broker" }],
                "messages": { "order": { "$ref": "#/components/messages/order" } },
                "bindings": { "kafka": { "partitions": 3 } }
            },
            "confirmations": {
                "address": "confirmations",
                "messages": { "confirmation": { "$ref": "#/components/messages/confirmation" } }
            },
            "plain": { "address": "plain", "messages": { "text": { "contentType": "text/plain" } } },
            "unknown": { "address": null, "messages": { "text": { "payload": {} } } },
            "traited": {
                "address": "traited",
                "messages": { "traited": {
                    "payload": {},
                    "traits": [
                        { "$ref": "#/components/messageTraits/plain" },
                        { "$ref": "#/components/messageTraits/correlated" }
                    ]
                } }
            }
        },
        "operations": {
            "placeOrder": { "action": "receive", "channel": { "$ref": "#/channels/orders" } },
            "confirmOrder": {
                "action": "send",
                "channel": { "$ref": "#/channels/confirmations" },
                "messages": [{ "$ref": "#/channels/confirmations/messages/confirmation" }]
            }
        },
        "components": {
            "messages": {
                "order": { "contentType": "application/json", "payload": {} },
                "confirmation": {
                    "correlationId": { "$ref": "#/components/correlationIds/byHeader" },
                    "payload": {}
                }
            },
            "messageTraits": {
                "plain": { "contentType": "text/plain" },
                "correlated": {
                    "contentType": "application/xml",
                    "correlationId": { "$ref": "#/components/correlationIds/byHeader" }
                }
            },
            "correlationIds": {
                "byHeader": { "location": "$message.header#/correlationId" }
            }
        }
    })
}

fn asyncapi_2() -> Value {
    json!({
        "asyncapi": "2.6.0",
        "info": { "title": "Legacy", "version": "1" },
        "defaultContentType": "application/json",
        "servers": { "mqtt": { "url": "mqtt://broker.example.com", "protocol": "mqtt" } },
        "channels": {
            "legacy/orders": {
                "publish": { "operationId": "submit", "message": { "payload": {} } },
                "subscribe": {
                    "operationId": "watch",
                    "message": { "oneOf": [
                        { "correlationId": { "location": "$message.payload#/meta/id" } },
                        { "payload": {} }
                    ] }
                }
            }
        }
    })
}

fn openapi() -> Value {
    json!({
        "openapi": "3.0.3",
        "servers": [{ "url": "https://api.example.com" }],
        "paths": { "/ping": { "get": { "operationId": "ping" } } }
    })
}

fn description(steps: Value, outputs: Value) -> Description {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "orders", "url": "https://example.com/asyncapi.json", "type": "asyncapi" },
            { "name": "legacy", "url": "https://example.com/legacy.json", "type": "asyncapi" },
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [{ "workflowId": "w", "steps": steps, "outputs": outputs }],
    }))
    .expect("a v1.1 description")
}

fn options() -> Options {
    Options::new()
        .source("orders", "https://example.com/asyncapi.json", asyncapi_3())
        .source("legacy", "https://example.com/legacy.json", asyncapi_2())
        .source("api", "https://api.example.com/openapi.json", openapi())
        .input("region", "eu")
        .input("id", "c-7")
}

fn send() -> Value {
    json!({
        "stepId": "send",
        "operationId": "$sourceDescriptions.orders.confirmOrder",
        "action": "send",
        "parameters": [{ "name": "correlationId", "in": "header", "value": "$inputs.id" }],
        "requestBody": { "payload": { "order": "{$inputs.region}-1" } },
        "outputs": { "sent": "$message.payload#/order" }
    })
}

fn receive() -> Value {
    json!({
        "stepId": "confirm",
        "operationId": "confirmOrder",
        "action": "receive",
        "correlationId": "$inputs.id",
        "timeout": 6000,
        "outputs": { "status": "$message.payload#/status" }
    })
}

fn confirmation(id: &str, status: &str) -> IncomingMessage {
    IncomingMessage::json(&json!({ "status": status })).header("correlationId", id)
}

#[test]
fn a_send_publishes_where_the_document_says_with_what_the_step_says() {
    let description = description(
        json!([send()]),
        json!({ "sent": "$steps.send.outputs.sent" }),
    );
    let mut client = Fake::new();
    let report =
        execute_with_messages(&description, &options(), &mut Fake::new(), &mut client).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(report.outputs["sent"], json!("eu-1"));
    let published = &client.published()[0];
    assert_eq!(published.channel.address, "confirmations");
    assert_eq!(published.channel.source, "orders");
    assert_eq!(published.channel.operation.as_deref(), Some("confirmOrder"));
    // No servers of its own: every server of the document.
    assert_eq!(published.channel.servers.len(), 2);
    assert_eq!(
        published.headers,
        [("correlationId".to_owned(), "c-7".to_owned())]
    );
    assert_eq!(
        published.payload.as_deref(),
        Some(br#"{"order":"eu-1"}"#.as_ref())
    );
    assert_eq!(published.content_type.as_deref(), Some("application/json"));
    assert_eq!(
        report.steps[0].performed,
        Performed::Message {
            action: "send".to_owned(),
            channel: "confirmations".to_owned(),
            delivered: true
        }
    );
    assert!(
        report.to_string().contains("- send send → `confirmations`"),
        "{report}"
    );
    assert_eq!(report.steps[0].status(), None);
    assert_eq!(report.steps[0].method(), None);
    assert_eq!(report.steps[0].url(), None);
}

#[test]
fn a_receive_listens_while_the_workflow_goes_on_and_joins_where_it_is_needed() {
    let description = description(
        json!([
            receive(),
            { "stepId": "ping", "operationId": "ping" },
            { "stepId": "after", "operationId": "ping", "dependsOn": ["confirm"] }
        ]),
        json!({ "status": "$steps.confirm.outputs.status" }),
    );
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    let Ok(Progress::Subscribe(subscription)) = run.advance() else {
        panic!("a subscription first");
    };
    assert_eq!(subscription.id, SubscriptionId(1));
    assert_eq!(subscription.id.to_string(), "#1");
    assert_eq!(subscription.channel.address, "confirmations");
    // The receive does not hold the next step up.
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    run.supply(HttpResponse::json(200, &json!({}))).unwrap();
    // `after` depends on it, so now its message is needed.
    let Ok(Progress::Receive {
        subscription: id,
        timeout,
    }) = run.advance()
    else {
        panic!("a receive");
    };
    assert_eq!(id, subscription.id);
    let timeout = timeout.expect("the step's timeout");
    assert!(timeout <= Duration::from_secs(6) && timeout > Duration::from_secs(5));
    // Asking again before answering is refused.
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::Awaiting { .. })
    ));
    // Another order's confirmation is passed over…
    run.deliver(Some(confirmation("c-other", "lost"))).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Receive { .. })));
    // …and this one completes the receive.
    run.deliver(Some(confirmation("c-7", "confirmed"))).unwrap();
    assert!(matches!(run.deliver(None), Err(ExecutionError::NotWaiting)));
    let Ok(Progress::Unsubscribe(closed)) = run.advance() else {
        panic!("the subscription is closed");
    };
    assert_eq!(closed, subscription.id);
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    run.supply(HttpResponse::json(200, &json!({}))).unwrap();
    let Ok(Progress::Done(report)) = run.advance() else {
        panic!("done");
    };
    assert!(report.is_success(), "{report}");
    assert_eq!(report.outputs["status"], json!("confirmed"));
    let order: Vec<_> = report
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect();
    assert_eq!(order, ["ping", "confirm", "after"]);
}

#[test]
fn a_receive_nothing_needs_is_joined_when_the_workflow_ends() {
    let description = description(
        json!([receive(), { "stepId": "ping", "operationId": "ping" }]),
        json!({ "status": "$steps.confirm.outputs.status" }),
    );
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let report = execute_with_messages(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})),
        &mut messages,
    )
    .unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(report.outputs["status"], json!("ok"));
    assert_eq!(messages.subscribed().len(), 1);
    assert_eq!(messages.unsubscribed(), [SubscriptionId(1)]);
    assert!(
        report
            .to_string()
            .contains("- confirm receive ← `confirmations`")
    );
}

#[test]
fn a_receive_that_times_out_fails_and_its_failure_actions_run() {
    let mut step = receive();
    step["onFailure"] = json!([{ "name": "give-up", "type": "goto", "stepId": "fallback" }]);
    let description = description(
        json!([
            step,
            { "stepId": "next", "operationId": "ping", "dependsOn": ["confirm"] },
            { "stepId": "fallback", "operationId": "ping" }
        ]),
        json!({}),
    );
    let mut messages = Fake::new();
    let mut http = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let report = execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
    let ran: Vec<_> = report
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect();
    // `fallback` runs where `next` would have, and `next` does not run.
    assert_eq!(ran, ["confirm", "fallback"], "{report}");
    assert!(!report.steps[0].passed);
    assert!(report.to_string().contains("(nothing arrived)"), "{report}");
    assert_eq!(
        report.steps[0].performed,
        Performed::Message {
            action: "receive".to_owned(),
            channel: "confirmations".to_owned(),
            delivered: false
        }
    );
}

#[test]
fn a_matching_message_that_fails_the_criteria_fails_the_step() {
    let mut step = receive();
    step["successCriteria"] = json!([{ "condition": "$message.payload#/status == 'confirmed'" }]);
    let description = description(json!([step]), json!({}));
    let mut messages = Fake::new()
        .message("confirmations", confirmation("c-7", "rejected"))
        .message("confirmations", confirmation("c-7", "confirmed"));
    let report =
        execute_with_messages(&description, &options(), &mut Fake::new(), &mut messages).unwrap();
    assert_eq!(report.outcome, Outcome::Failed, "{report}");
    assert_eq!(messages.received().len(), 1);
}

#[test]
fn a_retried_receive_listens_again() {
    let mut step = receive();
    step["successCriteria"] = json!([{ "condition": "$message.payload#/status == 'confirmed'" }]);
    step["onFailure"] = json!([{ "name": "again", "type": "retry", "retryLimit": 2 }]);
    let description = description(
        json!([step, { "stepId": "after", "operationId": "ping", "dependsOn": ["confirm"] }]),
        json!({ "status": "$steps.confirm.outputs.status" }),
    );
    let mut messages = Fake::new()
        .message("confirmations", confirmation("c-7", "pending"))
        .message("confirmations", confirmation("c-7", "confirmed"));
    let report = execute_with_messages(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})),
        &mut messages,
    )
    .unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(report.outputs["status"], json!("confirmed"));
    let ran: Vec<_> = report
        .steps
        .iter()
        .map(|step| (step.step_id.as_str(), step.attempt))
        .collect();
    assert_eq!(ran, [("confirm", 1), ("confirm", 2), ("after", 1)]);
    assert_eq!(messages.subscribed().len(), 2);
    assert_eq!(messages.unsubscribed().len(), 2);
}

#[test]
fn a_receive_without_a_timeout_waits_as_long_as_the_run_may() {
    let mut step = receive();
    step.as_object_mut().unwrap().remove("timeout");
    let description = description(json!([step]), json!({}));
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    assert!(matches!(
        run.advance(),
        Ok(Progress::Receive { timeout: None, .. })
    ));
    let options = self::options().timeout(Duration::from_secs(60));
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    let Ok(Progress::Receive {
        timeout: Some(timeout),
        ..
    }) = run.advance()
    else {
        panic!("a receive cut to the run's time");
    };
    assert!(timeout <= Duration::from_secs(60));
    // Nothing came because the run's time was up: the run times out.
    let options = self::options().timeout(Duration::from_millis(20));
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    assert!(matches!(run.advance(), Ok(Progress::Receive { .. })));
    std::thread::sleep(Duration::from_millis(40));
    assert!(matches!(
        run.deliver(None),
        Err(ExecutionError::TimedOut { .. })
    ));
    assert!(matches!(run.advance(), Err(ExecutionError::Stopped)));
}

#[test]
fn asyncapi_2_channels_are_found_by_operation_and_by_path() {
    let description = description(
        json!([
            {
                "stepId": "submit",
                "operationId": "submit",
                "action": "send",
                "requestBody": { "contentType": "text/plain", "payload": "hello" }
            },
            {
                "stepId": "watch",
                "channelPath": "$sourceDescriptions.legacy#/channels/legacy~1orders",
                "action": "receive",
                "correlationId": 7,
                "outputs": { "body": "$message.payload" }
            }
        ]),
        json!({ "body": "$steps.watch.outputs.body" }),
    );
    let mut messages = Fake::new()
        .message(
            "legacy/orders",
            IncomingMessage::json(&json!({ "meta": { "id": 8 } })),
        )
        .message(
            "legacy/orders",
            IncomingMessage::new(Vec::new(), br#"{"meta":{"id":"7"},"n":1}"#.to_vec()),
        );
    let report =
        execute_with_messages(&description, &options(), &mut Fake::new(), &mut messages).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(
        report.outputs["body"],
        json!({ "meta": { "id": "7" }, "n": 1 })
    );
    let published = &messages.published()[0];
    assert_eq!(published.channel.address, "legacy/orders");
    assert_eq!(published.payload.as_deref(), Some(b"hello".as_ref()));
    assert_eq!(published.content_type.as_deref(), Some("text/plain"));
    assert_eq!(published.channel.servers.len(), 1);
    assert_eq!(messages.subscribed()[0].channel.operation, None);
}

#[test]
fn channel_parameters_servers_and_bindings_come_from_the_document() {
    let description = description(
        json!([{
            "stepId": "listen",
            "channelPath": "{$sourceDescriptions.orders.url}#/channels/orders",
            "action": "receive",
            "parameters": [{ "name": "region", "in": "channel", "value": "$inputs.region" }]
        }]),
        json!({}),
    );
    let mut messages = Fake::new().message("orders.eu", IncomingMessage::json(&json!(1)));
    let options = options();
    let plan = prepare(&description, &options).unwrap();
    let report = plan
        .execute_with_messages(&mut Fake::new(), &mut messages)
        .unwrap();
    assert!(report.is_success(), "{report}");
    let channel = &messages.subscribed()[0].channel;
    assert_eq!(channel.address, "orders.eu");
    assert_eq!(channel.servers.keys().collect::<Vec<_>>(), ["broker"]);
    assert_eq!(
        channel.bindings,
        Some(json!({ "kafka": { "partitions": 3 } }))
    );
    // A channel with no operation: the message says what it is.
    let description = self::description(
        json!([{ "stepId": "say", "channelPath": "$sourceDescriptions.orders#/channels/plain", "action": "send", "requestBody": { "payload": "hi" } }]),
        json!({}),
    );
    let mut messages = Fake::new();
    execute_with_messages(
        &description,
        &self::options(),
        &mut Fake::new(),
        &mut messages,
    )
    .unwrap();
    assert_eq!(messages.published()[0].channel.address, "plain");
    assert_eq!(
        messages.published()[0].content_type.as_deref(),
        Some("text/plain")
    );
    assert_eq!(
        messages.published()[0].payload.as_deref(),
        Some(b"hi".as_ref())
    );
}

#[test]
fn what_cannot_be_sent_or_listened_for_is_an_error() {
    let cases = [
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/orders", "action": "send" }),
            "no `channel` parameter gives `{region}`",
        ),
        (
            json!({ "stepId": "s", "operationId": "confirmOrder", "action": "send",
                    "parameters": [{ "name": "q", "in": "query", "value": 1 }] }),
            "`query`, which an AsyncAPI step has not",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/plain",
                    "action": "receive", "correlationId": "x" }),
            "no message of channel `plain` says where its correlation id is",
        ),
        (
            json!({ "stepId": "s", "operationId": "nope", "action": "send" }),
            "operation `nope` is in none of the source descriptions",
        ),
        (
            json!({ "stepId": "s", "operationId": "$sourceDescriptions.api.ping", "action": "send" }),
            "an AsyncAPI step needs an AsyncAPI source description",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/none", "action": "send" }),
            "the document has nothing at `/channels/none`",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders", "action": "send" }),
            "names no channel inside the document",
        ),
        (
            json!({ "stepId": "s", "channelPath": "https://elsewhere#/channels/x", "action": "send" }),
            "does not start with `$sourceDescriptions.<name>`",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.gone#/channels/x", "action": "send" }),
            "source description `gone` was not supplied",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/unknown", "action": "send" }),
            "the channel's address is unknown",
        ),
    ];
    for (step, expected) in cases {
        let description = description(json!([step]), json!({}));
        let failure =
            execute_with_messages(&description, &options(), &mut Fake::new(), &mut Fake::new())
                .unwrap_err();
        assert!(
            failure.error.to_string().contains(expected),
            "{}",
            failure.error
        );
    }
}

#[test]
fn documents_the_executor_cannot_follow_are_refused() {
    let mut external = asyncapi_3();
    external["operations"]["confirmOrder"]["channel"] = json!({ "$ref": "other.json#/channels/x" });
    external["channels"]["loop"] = json!({ "$ref": "#/channels/loop" });
    let mut old = asyncapi_3();
    old["asyncapi"] = json!("1.2.0");
    let options = options().source("orders", "https://example.com/asyncapi.json", external);
    for (step, expected) in [
        (
            json!({ "stepId": "s", "operationId": "confirmOrder", "action": "send" }),
            "is not a reference within the document",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/loop", "action": "send" }),
            "chain of more than 32 references",
        ),
    ] {
        let description = description(json!([step]), json!({}));
        let failure =
            execute_with_messages(&description, &options, &mut Fake::new(), &mut Fake::new())
                .unwrap_err();
        assert!(
            failure.error.to_string().contains(expected),
            "{}",
            failure.error
        );
    }
    let options = self::options().source("orders", "https://example.com/asyncapi.json", old);
    let description = description(json!([send()]), json!({}));
    let failure = execute_with_messages(&description, &options, &mut Fake::new(), &mut Fake::new())
        .unwrap_err();
    assert!(
        failure
            .error
            .to_string()
            .contains("AsyncAPI 1.2.0 is not supported")
    );
}

#[test]
fn a_bare_operation_id_must_name_one_asyncapi_operation() {
    let description = description(
        json!([{ "stepId": "s", "operationId": "confirmOrder", "action": "send" }]),
        json!({}),
    );
    let both = options().source("legacy", "https://example.com/legacy.json", asyncapi_3());
    let failure =
        execute_with_messages(&description, &both, &mut Fake::new(), &mut Fake::new()).unwrap_err();
    assert!(
        failure
            .error
            .to_string()
            .contains("more than one source description")
    );
    // With a source missing, uniqueness cannot be shown.
    let partial =
        Options::new().source("orders", "https://example.com/asyncapi.json", asyncapi_3());
    let failure = execute_with_messages(&description, &partial, &mut Fake::new(), &mut Fake::new())
        .unwrap_err();
    assert!(
        failure.error.to_string().contains("was not supplied"),
        "{}",
        failure.error
    );
}

#[test]
fn preparation_resolves_channels_and_checks_sources() {
    let description = description(json!([send(), receive()]), json!({}));
    let options = options();
    let plan = prepare(&description, &options).unwrap();
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let report = plan
        .execute_with_messages(&mut Fake::new(), &mut messages)
        .unwrap();
    assert!(report.is_success(), "{report}");
    // A plain run of the plan has nowhere to send messages.
    let failure = plan.execute(&mut Fake::new()).unwrap_err();
    assert!(failure.error.to_string().contains("needs a message client"));
    for (step, expected) in [
        (
            json!({ "stepId": "s", "operationId": "$sourceDescriptions.api.ping", "action": "send" }),
            "source `api` is not an AsyncAPI description",
        ),
        (
            json!({ "stepId": "s", "operationId": "$sourceDescriptions.orders.confirmOrder" }),
            "source `orders` does not describe supported HTTP operations",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/none", "action": "send" }),
            "channelPath",
        ),
        (
            json!({ "stepId": "s", "operationId": "nope", "action": "send" }),
            "operationId",
        ),
        (
            json!({ "stepId": "s", "operationId": "confirmOrder", "action": "receive", "correlationId": "$steps.nope.outputs.x" }),
            "correlationId",
        ),
        (
            json!({ "stepId": "s", "operationId": "ping", "successCriteria": [{ "condition": "$message.payload == 1" }] }),
            "can read",
        ),
    ] {
        let description = self::description(json!([step]), json!({}));
        let error = prepare(&description, &self::options()).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    // An unnamed operation of an AsyncAPI step needs every source to be
    // supplied, as a bare HTTP operation does.
    let description = self::description(
        json!([{ "stepId": "s", "operationId": "confirmOrder", "action": "send" }]),
        json!({}),
    );
    let required = roas_arazzo_executor::required_sources(&description, &Options::new()).unwrap();
    assert_eq!(required.len(), 3, "{required:?}");
}

#[tokio::test]
async fn an_async_run_carries_messages_the_same_way() {
    let description = description(
        json!([send(), receive()]),
        json!({ "status": "$steps.confirm.outputs.status" }),
    );
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let report =
        execute_with_messages_async(&description, &options(), &mut Fake::new(), &mut messages)
            .await
            .unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(report.outputs["status"], json!("ok"));
    assert_eq!(messages.published().len(), 1);
    assert_eq!(messages.unsubscribed().len(), 1);
    let options = options();
    let plan = prepare(&description, &options).unwrap();
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let report = plan
        .execute_with_messages_async(&mut Fake::new(), &mut messages)
        .await
        .unwrap();
    assert!(report.is_success(), "{report}");
    // Without a message client, an async run refuses the step too.
    let failure = roas_arazzo_executor::execute_async_with_report(
        &description,
        &self::options(),
        &mut Fake::new(),
    )
    .await
    .unwrap_err();
    assert!(failure.error.to_string().contains("needs a message client"));
}

#[test]
fn message_in_an_http_step_is_missing_at_runtime() {
    let description = description(
        json!([{
            "stepId": "ping",
            "operationId": "ping",
            "successCriteria": [{ "condition": "$message.header.x == 'y'" }]
        }]),
        json!({}),
    );
    let report = roas_arazzo_executor::execute(
        &description,
        &options(),
        &mut Fake::new().reply(200, &json!({})),
    )
    .unwrap();
    assert!(!report.is_success());
    assert!(report.steps[0].criteria[0].error.is_some());
}

#[test]
fn a_receive_reached_again_replaces_the_one_still_listening() {
    let description = description(
        json!([
            receive(),
            {
                "stepId": "ping",
                "operationId": "ping",
                "onSuccess": [{ "name": "again", "type": "goto", "stepId": "confirm",
                                "criteria": [{ "condition": "$statusCode == 201" }] }]
            }
        ]),
        json!({ "status": "$steps.confirm.outputs.status" }),
    );
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let mut http = Fake::new().reply(201, &json!({})).reply(200, &json!({}));
    let report = execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(messages.subscribed().len(), 2);
    assert_eq!(
        messages.unsubscribed(),
        [SubscriptionId(1), SubscriptionId(2)]
    );
    assert_eq!(messages.received()[0].0, SubscriptionId(2));
}

#[test]
fn a_correlation_id_may_be_the_whole_payload_and_an_unknown_location_matches_nothing() {
    let mut document = asyncapi_3();
    document["components"]["correlationIds"]["byHeader"]["location"] = json!("$message.payload");
    let options = options().source("orders", "https://example.com/asyncapi.json", document);
    let mut step = receive();
    step.as_object_mut().unwrap().remove("outputs");
    let description = description(json!([step]), json!({}));
    let mut messages = Fake::new()
        .message("confirmations", IncomingMessage::json(&json!("c-8")))
        .message("confirmations", IncomingMessage::json(&json!("c-7")));
    let report =
        execute_with_messages(&description, &options, &mut Fake::new(), &mut messages).unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(messages.received().len(), 2);
    let mut document = asyncapi_3();
    document["components"]["correlationIds"]["byHeader"]["location"] = json!("$message.other#/x");
    let options = self::options().source("orders", "https://example.com/asyncapi.json", document);
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let report =
        execute_with_messages(&description, &options, &mut Fake::new(), &mut messages).unwrap();
    assert!(!report.is_success(), "{report}");
    assert!(report.to_string().contains("(nothing arrived)"));
}

#[test]
fn more_ways_a_channel_is_found_or_not() {
    // An operation that lists no messages takes its channel's.
    let description = description(
        json!([{
            "stepId": "place",
            "operationId": "$sourceDescriptions.orders.placeOrder",
            "action": "send",
            "parameters": [{ "name": "region", "in": "channel", "value": "us" }],
            "requestBody": { "payload": { "n": 1 } }
        }]),
        json!({}),
    );
    let mut messages = Fake::new();
    execute_with_messages(&description, &options(), &mut Fake::new(), &mut messages).unwrap();
    assert_eq!(messages.published()[0].channel.address, "orders.us");
    assert_eq!(
        messages.published()[0].content_type.as_deref(),
        Some("application/json")
    );
    // Broken references inside a channel: messages and servers that lead
    // nowhere are passed over; bindings that do are an error.
    let mut document = asyncapi_3();
    document["channels"]["odd"] = json!({
        "address": "odd",
        "servers": [7, { "$ref": "#/servers/missing" }],
        "messages": { "gone": { "$ref": "#/components/messages/gone" } }
    });
    document["channels"]["bound"] = json!({ "address": "bound", "bindings": { "$ref": "#/nope" } });
    let options = self::options().source("orders", "https://example.com/asyncapi.json", document);
    let odd = self::description(
        json!([{ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/odd", "action": "send" }]),
        json!({}),
    );
    let mut messages = Fake::new();
    execute_with_messages(&odd, &options, &mut Fake::new(), &mut messages).unwrap();
    assert!(messages.published()[0].channel.servers.is_empty());
    assert_eq!(messages.published()[0].payload, None);
    let bound = self::description(
        json!([{ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/bound", "action": "send" }]),
        json!({}),
    );
    let failure =
        execute_with_messages(&bound, &options, &mut Fake::new(), &mut Fake::new()).unwrap_err();
    assert!(
        failure.error.to_string().contains("nothing at"),
        "{}",
        failure.error
    );
    for (step, expected) in [
        (
            json!({ "stepId": "s", "action": "send" }),
            "names neither an operation nor a workflow",
        ),
        (
            json!({ "stepId": "s", "operationId": "$sourceDescriptions.orders.nope", "action": "send" }),
            "operation `$sourceDescriptions.orders.nope` is in none",
        ),
    ] {
        let description = self::description(json!([step]), json!({}));
        let failure = execute_with_messages(
            &description,
            &self::options(),
            &mut Fake::new(),
            &mut Fake::new(),
        )
        .unwrap_err();
        assert!(
            failure.error.to_string().contains(expected),
            "{}",
            failure.error
        );
    }
}

#[test]
fn a_plan_says_whether_it_needs_a_message_client_and_a_run_that_cannot_start_says_why() {
    let options = options();
    let asynchronous = description(json!([send()]), json!({}));
    assert!(prepare(&asynchronous, &options).unwrap().uses_messages());
    let plain = description(
        json!([{ "stepId": "ping", "operationId": "ping" }]),
        json!({}),
    );
    assert!(!prepare(&plain, &options).unwrap().uses_messages());
    let error = prepare(
        &description(
            json!([{ "stepId": "s", "operationId": "confirmOrder", "action": "send",
                     "parameters": [{ "name": "q", "in": "query", "value": 1 }] }]),
            json!({}),
        ),
        &options,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("in `channel` or `header`"),
        "{error}"
    );
    // A depth limit of zero cannot even enter the workflow.
    let options = self::options().max_depth(0);
    let failure =
        execute_with_messages(&asynchronous, &options, &mut Fake::new(), &mut Fake::new())
            .unwrap_err();
    assert!(failure.report.is_none());
    let plan = prepare(&asynchronous, &options).unwrap();
    assert!(
        plan.execute_with_messages(&mut Fake::new(), &mut Fake::new())
            .is_err()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert!(
            execute_with_messages_async(
                &asynchronous,
                &options,
                &mut Fake::new(),
                &mut Fake::new()
            )
            .await
            .is_err()
        );
        assert!(
            plan.execute_with_messages_async(&mut Fake::new(), &mut Fake::new())
                .await
                .is_err()
        );
    });
}

#[test]
fn a_stopped_run_takes_no_message() {
    let description = description(json!([receive()]), json!({}));
    let options = options().timeout(Duration::ZERO);
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(
        run.advance(),
        Err(ExecutionError::TimedOut { .. })
    ));
    assert!(matches!(run.deliver(None), Err(ExecutionError::Stopped)));
}

#[test]
fn a_receive_that_gets_nothing_fails_whatever_its_criteria_and_outputs_say() {
    let mut step = receive();
    step["successCriteria"] = json!([{ "condition": "$inputs.id == 'c-7'" }]);
    step["onFailure"] = json!([{ "name": "give-up", "type": "end" }]);
    let description = description(json!([step]), json!({}));
    let report =
        execute_with_messages(&description, &options(), &mut Fake::new(), &mut Fake::new())
            .unwrap();
    assert_eq!(report.outcome, Outcome::Failed, "{report}");
    assert!(report.steps[0].criteria.is_empty());
    assert!(report.steps[0].outputs.is_empty());
    assert_eq!(report.steps[0].action.as_deref(), Some("ended, failed"));
}

#[test]
fn joins_inside_a_retry_detour_come_back_to_the_right_steps() {
    let mut confirm = receive();
    confirm["successCriteria"] =
        json!([{ "condition": "$message.payload#/status == 'confirmed'" }]);
    confirm["onFailure"] = json!([{ "name": "again", "type": "retry", "stepId": "t" }]);
    let mut second = receive();
    second["stepId"] = json!("r2");
    second["operationId"] = json!("$sourceDescriptions.orders.placeOrder");
    second["parameters"] = json!([{ "name": "region", "in": "channel", "value": "eu" }]);
    second.as_object_mut().unwrap().remove("correlationId");
    second.as_object_mut().unwrap().remove("outputs");
    let description = description(
        json!([
            confirm,
            second,
            { "stepId": "b", "operationId": "ping", "dependsOn": ["confirm"] },
            { "stepId": "t", "operationId": "ping", "dependsOn": ["r2"] }
        ]),
        json!({}),
    );
    let mut messages = Fake::new()
        .message("confirmations", confirmation("c-7", "pending"))
        .message("confirmations", confirmation("c-7", "confirmed"))
        .message("orders.eu", IncomingMessage::json(&json!(1)));
    let mut http = Fake::new()
        .reply(200, &json!({}))
        .reply(200, &json!({}))
        .reply(200, &json!({}));
    let report = execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
    let ran: Vec<_> = report
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect();
    assert_eq!(ran, ["confirm", "r2", "t", "confirm", "b", "t"], "{report}");
    // `r2` listened once.
    assert_eq!(messages.subscribed().len(), 3);
    assert!(report.is_success(), "{report}");
}

#[test]
fn a_run_that_stops_early_closes_its_subscriptions() {
    let description = description(
        json!([receive(), { "stepId": "ping", "operationId": "ping" }]),
        json!({}),
    );
    // The HTTP client has no reply scripted, so the run fails.
    let mut messages = Fake::new();
    let failure = execute_with_messages(&description, &options(), &mut Fake::new(), &mut messages)
        .unwrap_err();
    assert!(matches!(failure.error, ExecutionError::Client(_)));
    assert_eq!(messages.unsubscribed(), [SubscriptionId(1)]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut messages = Fake::new();
    runtime.block_on(async {
        execute_with_messages_async(&description, &options(), &mut Fake::new(), &mut messages)
            .await
            .unwrap_err();
    });
    assert_eq!(messages.unsubscribed(), [SubscriptionId(1)]);
    // A manual driver can ask which are open.
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    assert_eq!(run.open_subscriptions(), [SubscriptionId(1)]);
}

#[test]
fn preparation_refuses_what_a_run_would() {
    for (step, expected) in [
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/plain",
                    "action": "receive", "correlationId": "x" }),
            "says where its correlation id is",
        ),
        (
            json!({ "stepId": "s", "operationId": "placeOrder", "action": "send" }),
            "no `channel` parameter gives `{region}`",
        ),
        (
            json!({ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/unknown", "action": "receive" }),
            "the channel's address is unknown",
        ),
    ] {
        let description = description(json!([step]), json!({}));
        let error = prepare(&description, &options()).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    // A lazy run refuses a channel with no action.
    let description = description(
        json!([{ "stepId": "s", "channelPath": "$sourceDescriptions.orders#/channels/plain" }]),
        json!({}),
    );
    let failure =
        execute_with_messages(&description, &options(), &mut Fake::new(), &mut Fake::new())
            .unwrap_err();
    assert!(
        failure.error.to_string().contains("no `action`"),
        "{}",
        failure.error
    );
}

#[test]
fn a_workflow_action_that_reads_a_receive_joins_it() {
    let mut description = description(
        json!([receive(), { "stepId": "ping", "operationId": "ping" }, { "stepId": "ping2", "operationId": "ping" }]),
        json!({}),
    );
    description.workflows[0].success_actions = serde_json::from_value(json!([
        { "name": "done", "type": "end", "criteria": [{ "condition": "$steps.confirm.outputs.status == 'ok'" }] }
    ]))
    .unwrap();
    let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
    let mut http = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let report = execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
    let ran: Vec<_> = report
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect();
    assert_eq!(ran, ["confirm"], "{report}");
    assert_eq!(report.outcome, Outcome::Ended);
}

#[test]
fn referenced_operations_and_channel_items_are_followed() {
    let mut v3 = asyncapi_3();
    v3["operations"]["refd"] = json!({ "$ref": "#/components/operations/refd" });
    v3["components"]["operations"] = json!({
        "refd": { "action": "send", "channel": { "$ref": "#/channels/confirmations" } }
    });
    let mut v2 = asyncapi_2();
    v2["channels"]["shared"] = json!({ "$ref": "#/components/channels/shared" });
    v2["channels"]["broken"] = json!({ "$ref": "#/nowhere" });
    v2["components"] =
        json!({ "channels": { "shared": { "publish": { "operationId": "share" } } } });
    let options = options()
        .source("orders", "https://example.com/asyncapi.json", v3)
        .source("legacy", "https://example.com/legacy.json", v2);
    let description = description(
        json!([
            { "stepId": "a", "operationId": "$sourceDescriptions.orders.refd", "action": "send" },
            { "stepId": "b", "operationId": "$sourceDescriptions.legacy.share", "action": "send" }
        ]),
        json!({}),
    );
    let mut messages = Fake::new();
    execute_with_messages(&description, &options, &mut Fake::new(), &mut messages).unwrap();
    let addresses: Vec<_> = messages
        .published()
        .iter()
        .map(|message| message.channel.address.as_str())
        .collect();
    assert_eq!(addresses, ["confirmations", "shared"]);
}

#[test]
fn a_retry_wait_comes_after_the_old_subscription_is_closed() {
    let mut step = receive();
    step["onFailure"] = json!([{ "name": "again", "type": "retry", "retryAfter": 5 }]);
    let description = description(json!([step]), json!({}));
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    assert!(matches!(run.advance(), Ok(Progress::Receive { .. })));
    run.deliver(None).unwrap();
    assert!(matches!(
        run.advance(),
        Ok(Progress::Unsubscribe(SubscriptionId(1)))
    ));
    assert!(matches!(run.advance(), Ok(Progress::Wait(_))));
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
}

#[test]
fn a_message_trait_gives_what_the_message_leaves_out() {
    let description = description(
        json!([{
            "stepId": "listen",
            "channelPath": "$sourceDescriptions.orders#/channels/traited",
            "action": "receive",
            "correlationId": "$inputs.id",
            "timeout": 0,
            "outputs": { "got": "$message.payload" }
        }, {
            "stepId": "say",
            "channelPath": "$sourceDescriptions.orders#/channels/traited",
            "action": "send",
            "requestBody": { "payload": "hi" }
        }]),
        json!({ "got": "$steps.listen.outputs.got" }),
    );
    let mut messages = Fake::new()
        .message(
            "traited",
            IncomingMessage::json(&json!("other")).header("correlationId", "nope"),
        )
        .message(
            "traited",
            IncomingMessage::json(&json!("mine")).header("correlationId", "c-7"),
        );
    let report =
        execute_with_messages(&description, &options(), &mut Fake::new(), &mut messages).unwrap();
    assert!(report.is_success(), "{report}");
    // The correlation id comes from the second trait.
    assert_eq!(report.outputs["got"], json!("mine"));
    // The first trait that sets a field wins.
    assert_eq!(
        messages.published()[0].content_type.as_deref(),
        Some("text/plain")
    );
}

#[test]
fn a_goto_out_of_a_retry_detour_still_comes_back_to_the_receive() {
    let mut confirm = receive();
    confirm["timeout"] = json!(0);
    confirm["onFailure"] =
        json!([{ "name": "again", "type": "retry", "stepId": "d", "retryLimit": 1 }]);
    let description = description(
        json!([
            confirm,
            { "stepId": "c", "operationId": "ping", "dependsOn": ["confirm"] },
            { "stepId": "d", "operationId": "ping",
              "onSuccess": [{ "name": "jump", "type": "goto", "stepId": "x" }] },
            { "stepId": "x", "operationId": "ping" }
        ]),
        json!({}),
    );
    let mut http = Fake::new()
        .reply(200, &json!({}))
        .reply(200, &json!({}))
        .reply(200, &json!({}))
        .reply(200, &json!({}))
        .reply(200, &json!({}));
    let mut messages = Fake::new();
    let report = execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
    let ran: Vec<_> = report
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect();
    // The detour jumps on to `x`, and the retry still comes after it; the
    // second attempt gets nothing either, so the workflow fails before `c`.
    assert_eq!(ran, ["confirm", "d", "x", "confirm"], "{report}");
    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(messages.subscribed().len(), 2);
}

#[test]
fn a_message_is_read_only_where_there_is_one() {
    for (steps, outputs) in [
        (
            json!([{
                "stepId": "s",
                "operationId": "confirmOrder",
                "action": "send",
                "parameters": [{ "name": "h", "in": "header", "value": "$message.header.x" }]
            }]),
            json!({}),
        ),
        (
            json!([{ "stepId": "s", "operationId": "confirmOrder", "action": "send" }]),
            json!({ "o": "$message.payload" }),
        ),
        (
            json!([{ "stepId": "s", "operationId": "confirmOrder", "action": "send" }]),
            json!({ "successActions": "$message.payload" }),
        ),
    ] {
        let description = description(steps, outputs);
        let error = prepare(&description, &options()).unwrap_err();
        assert!(error.to_string().contains("can read"), "{error}");
    }
    // A workflow action may read it.
    let mut confirm = receive();
    confirm["timeout"] = json!(0);
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "orders", "url": "https://example.com/asyncapi.json", "type": "asyncapi" }
        ],
        "workflows": [{
            "workflowId": "w",
            "steps": [confirm],
            "failureActions": [{
                "name": "stop", "type": "end",
                "criteria": [{ "condition": "$message.payload != 1" }]
            }]
        }],
    }))
    .unwrap();
    prepare(&description, &options()).expect("a plan");
    // So may a reusable action of an AsyncAPI step.
    let mut confirm = receive();
    confirm["onSuccess"] = json!([{ "reference": "$components.successActions.check" }]);
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "orders", "url": "https://example.com/asyncapi.json", "type": "asyncapi" },
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [{ "workflowId": "w", "steps": [
            confirm,
            { "stepId": "p", "operationId": "ping",
              "onSuccess": [{ "reference": "$components.successActions.check" }] }
        ] }],
        "components": { "successActions": { "check": {
            "name": "check", "type": "end",
            "criteria": [{ "condition": "$message.payload#/status == 'ok'" }]
        } } }
    }))
    .unwrap();
    let error = prepare(&description, &options()).unwrap_err().to_string();
    // Only the HTTP step's use is refused.
    assert!(error.contains("can read"), "{error}");
    assert_eq!(error.matches("can read").count(), 1, "{error}");
}

#[test]
fn an_untyped_source_may_hold_asyncapi_operations() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [{ "name": "orders", "url": "https://example.com/asyncapi.json" }],
        "workflows": [{ "workflowId": "w", "steps": [send()] }],
    }))
    .unwrap();
    let options = options();
    let plan = prepare(&description, &options).expect("a plan");
    assert!(plan.uses_messages());
    let mut messages = Fake::new();
    let report = plan
        .execute_with_messages(&mut Fake::new(), &mut messages)
        .unwrap();
    assert!(report.is_success(), "{report}");
    assert_eq!(messages.published().len(), 1);
}

#[test]
fn an_end_closes_the_receives_still_listening_unheard() {
    // No timeout and no time limit: joined, it would wait forever.
    let mut confirm = receive();
    confirm.as_object_mut().unwrap().remove("timeout");
    confirm["onSuccess"] = json!([{ "name": "late", "type": "goto", "stepId": "late" }]);
    let description = description(
        json!([
            confirm,
            { "stepId": "x", "operationId": "ping",
              "successCriteria": [{ "condition": "$statusCode == 200" }],
              "onSuccess": [{ "name": "stop", "type": "end" }],
              "onFailure": [{ "name": "give-up", "type": "end" }] },
            { "stepId": "late", "operationId": "ping" }
        ]),
        json!({}),
    );
    for (status, outcome) in [(200, Outcome::Ended), (500, Outcome::Failed)] {
        // A message is waiting, but nothing asks for it.
        let mut messages = Fake::new().message("confirmations", confirmation("c-7", "ok"));
        let mut http = Fake::new().reply(status, &json!({}));
        let report =
            execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
        let ran: Vec<_> = report
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect();
        assert_eq!(ran, ["x"], "{report}");
        assert_eq!(report.outcome, outcome, "{report}");
        assert!(messages.received().is_empty());
        assert_eq!(messages.unsubscribed(), [SubscriptionId(1)]);
    }
    // Driven by hand: the subscription is closed, then the run is done.
    let options = options();
    let mut run = Run::start(&description, &options).unwrap();
    assert!(matches!(run.advance(), Ok(Progress::Subscribe(_))));
    assert!(matches!(run.advance(), Ok(Progress::Send(_))));
    run.supply(HttpResponse::json(200, &json!({}))).unwrap();
    assert!(matches!(
        run.advance(),
        Ok(Progress::Unsubscribe(SubscriptionId(1)))
    ));
    assert!(run.open_subscriptions().is_empty());
    assert!(matches!(run.advance(), Ok(Progress::Done(_))));
}

#[test]
fn an_end_in_a_called_workflow_closes_its_receives() {
    let description: Description = serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": { "title": "T", "version": "1.0.0" },
        "sourceDescriptions": [
            { "name": "orders", "url": "https://example.com/asyncapi.json", "type": "asyncapi" },
            { "name": "api", "url": "https://api.example.com/openapi.json", "type": "openapi" }
        ],
        "workflows": [
            { "workflowId": "w", "steps": [
                { "stepId": "call", "workflowId": "inner",
                  "parameters": [{ "name": "id", "value": "$inputs.id" }] },
                { "stepId": "after", "operationId": "ping" }
            ] },
            { "workflowId": "inner", "steps": [
                receive(),
                { "stepId": "x", "operationId": "ping",
                  "onSuccess": [{ "name": "stop", "type": "end" }] }
            ] }
        ],
    }))
    .unwrap();
    let mut messages = Fake::new();
    let mut http = Fake::new().reply(200, &json!({})).reply(200, &json!({}));
    let report = execute_with_messages(&description, &options(), &mut http, &mut messages).unwrap();
    assert!(report.is_success(), "{report}");
    assert!(messages.received().is_empty());
    assert_eq!(messages.unsubscribed(), [SubscriptionId(1)]);
}
