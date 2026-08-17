# roas-arazzo-executor

Executes [OpenAPI Arazzo](https://spec.openapis.org/arazzo/v1.1.0.html) workflows: runs every step's request, follows the description's success and failure actions, and reports what happened.

[![crates.io](https://img.shields.io/crates/v/roas-arazzo-executor.svg)](https://crates.io/crates/roas-arazzo-executor)
[![docs.rs](https://docs.rs/roas-arazzo-executor/badge.svg)](https://docs.rs/roas-arazzo-executor)

An Arazzo description is a program: ordered steps that call API operations, assert on the responses, name outputs, and branch on success or failure. [`roas-arazzo`](https://crates.io/crates/roas-arazzo) parses and validates one; this crate runs it.

## Quick start

```rust
use roas_arazzo::v1_1::Description;
use roas_arazzo_executor::{Client, Options, execute};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let description: Description = serde_yaml_ng::from_str(include_str!("buy_pet.arazzo.yaml"))?;
let openapi = serde_yaml_ng::from_str(include_str!("petstore.openapi.yaml"))?;

let options = Options::new()
    .workflow("buyPet")
    .source("petStore", "https://api.example.com/openapi.yaml", openapi)
    .input("petId", "7");

let report = execute(&description, &options, &mut Client::blocking())?;
println!("{report}");
# Ok(()) }
```

```text
workflow `buyPet` succeeded
- findPet GET https://api.example.com/v1/pets/7 → 200
- orderPet POST https://api.example.com/v1/pets/7/order → 201
  orderId = "o-1"
  petName = "fluffy"
```

## It performs no IO of its own

The engine decides *what* to send and asks a client to send it. That is what lets one engine serve a blocking caller, an async one, and a test with no network at all:

| Entry point | Client trait | Waiting |
|---|---|---|
| `execute` | `HttpClient` | `std::thread::sleep` |
| `execute_async` | `AsyncHttpClient` | the client's own `sleep` |
| `Run` | none — you drive it | you decide |

`Client` (behind the `reqwest` feature) implements both, over `reqwest::blocking::Client` and `reqwest::Client`. Implement the trait yourself to reuse your own client, authentication or middleware.

Source descriptions are the same story: fetching them is IO, so the caller passes the parsed documents to `Options::source`. [`roas-file-fetcher`](https://crates.io/crates/roas-file-fetcher) and [`roas-http-fetcher`](https://crates.io/crates/roas-http-fetcher) do that job for the loader and do it here just as well.

## Testing a workflow

`testing::Fake` answers from a script and keeps what it was asked, so a workflow can be tested without a server:

```rust
use roas_arazzo_executor::{Options, execute, testing::Fake};
use serde_json::json;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let description: roas_arazzo::v1_1::Description = serde_json::from_str("{}")?;
# let options = Options::new();
let mut client = Fake::new()
    .reply(200, &json!({ "id": 7, "name": "fluffy" }))
    .reply(201, &json!({ "orderId": "o-1" }));

let report = execute(&description, &options, &mut client)?;

assert_eq!(client.sent()[1].method, "POST");
assert!(report.is_success());
# Ok(()) }
```

Driving `Run` directly goes one step further: `Progress::Wait` hands back the delay a `retry` asked for instead of spending it, so retry behaviour can be asserted in microseconds.

## What it runs

- **Steps** that name an operation by `operationId` (bare, or `$sourceDescriptions.<name>.<id>`) or by `operationPath`, and steps that call another `workflowId`.
- **Parameters** in `path`, `query`, `querystring`, `header` and `cookie`, from the workflow and the step, with `$components.parameters` references and their `value` overrides.
- **Request bodies**, with runtime expressions anywhere inside the payload and `replacements` by JSON Pointer or JSONPath.
- **Criteria** — `simple` conditions (comparisons, `&&`, `||`, parentheses), `regex`, and `jsonpath`.
- **Actions** — `end`, `goto` a step or a workflow, and `retry` with `retryAfter` / `retryLimit`.
- **Outputs** at step and workflow level, including [`Selector`](https://spec.openapis.org/arazzo/v1.1.0.html#selector-object)s, readable by later steps as `$steps.<id>.outputs.<name>`.
- **`dependsOn`** between steps and between workflows, which orders them and rejects circles.

Both Arazzo versions: v1.1 directly, and v1.0 through `execute_v1_0` (the `v1_0` feature), which upconverts first so there is one interpreter.

## What it does not run

Each of these is reported where it is met, never passed over — a run should not look successful because something was skipped.

- **AsyncAPI steps** (`channelPath` / `action` / `correlationId`): they need a broker client, not an HTTP one.
- **XPath** criteria and selectors: JSON Pointer and JSONPath are supported.
- **`inputs` schema validation**: inputs are passed through as given.
- **Parallel execution**: `dependsOn` orders steps and workflows; they still run one at a time.

## Safety rails

A description can loop — `goto` is a jump. `Options` caps the number of steps (1000), the depth of workflow calls (8) and the retries of one step (10); each raises `ExecutionError::Limit` rather than running forever.

## License

`MIT OR Apache-2.0`, as the rest of the workspace.
