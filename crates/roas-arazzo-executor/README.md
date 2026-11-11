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
// The options every roas crate reads YAML with: YAML 1.2 booleans
// (`yes` stays a string), `017` / `1_000` stay strings, merge keys are
// expanded and duplicate keys rejected, and the budget raised so large
// specs fit.
let yaml = || serde_saphyr::options! {
    budget: serde_saphyr::budget! {
        max_nodes: 10_000_000,
        max_events: 25_000_000,
        max_aliases: 1_000_000,
        max_anchors: 1_000_000,
        max_merge_keys: 1_000_000,
        enforce_alias_anchor_ratio: false,
    },
    alias_limits: serde_saphyr::alias_limits! {
        max_total_replayed_events: 10_000_000,
    },
    strict_booleans: true,
    ignore_binary_tag_for_string: true,
    leading_zero_numbers_as_strings: true,
    underscored_numbers_as_strings: true,
    non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
    merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
    duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
};
let description: Description =
    serde_saphyr::from_str_with_options(include_str!("buy_pet.arazzo.yaml"), yaml())?;
let openapi = serde_saphyr::from_str_with_options(include_str!("petstore.openapi.yaml"), yaml())?;

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
| `execute_async` | `AsyncHttpClient` | the client's own `sleep`; requests in flight together under `Options::parallel` |
| `Run` | none — you drive it | you decide |

`Client` (behind the `reqwest` feature) implements both, over `reqwest::blocking::Client` and `reqwest::Client`. Implement the trait yourself to reuse your own client, authentication or middleware.

Source descriptions are the same story: fetching them is IO, so the caller passes the parsed documents to `Options::source`. [`roas-file-fetcher`](https://crates.io/crates/roas-file-fetcher) and [`roas-http-fetcher`](https://crates.io/crates/roas-http-fetcher) do that job for the loader and do it here just as well.

### Document identities and source graphs

The optional `source-graph` feature adds `SourceRegistry` and bounded sync/async
traversal through a caller-configured `roas::loader::Loader`. It registers no file
or network fetchers itself. The existing `Options::source` API remains available
without this feature.

```rust,no_run
use roas::loader::Loader;
use roas_arazzo_executor::{Options, SourceLoadOptions, SourceRegistry, prepare};
use serde_json::Value;

# fn example(root_json: Value, loader: &mut Loader) -> Result<(), Box<dyn std::error::Error>> {
let mut registry = SourceRegistry::new();
let root = registry.insert("https://example.test/workflows/root.json", root_json)?;
// Insert every other supplied document here, before loading any links.
let loading = registry.load_sources(root, loader, &SourceLoadOptions::default())?;
for diagnostic in &loading.diagnostics {
    eprintln!("{}: {diagnostic}", registry.document(diagnostic.owner)?.identity());
}
for diagnostic in &loading.reference_diagnostics {
    eprintln!("{diagnostic}");
}
let options = Options::new().source_registry(&registry, root)?;
let description = registry.document(root)?.arazzo().expect("an Arazzo root");
let plan = prepare(description, &options)?;
// plan.execute(&mut client), or plan.execute_async(&mut client).await
# Ok(()) }
```

Documents retain their original value, retrieval URI, canonical identity, effective
reference base, and written version. Arazzo is deserialized in full before its
references are resolved. A relative `$self` resolves against the retrieval URI
(the final redirect location when exposed by the fetcher); document references
then resolve against that identity. `$self` fragments and conflicting documents
claiming the same identity/location are rejected. URI normalization removes
fragments for document lookup and handles dot segments/default ports; queries
remain distinct. It does not canonicalize filesystem symlinks or all percent escapes.

Canonical Arazzo identities follow
[identity-based referencing](https://spec.openapis.org/arazzo/v1.1.0.html#identity-based-referencing).
An Arazzo retrieval URL different from its `$self` is accepted only with
`SourceLoadOptions::retrieval_aliases = true`, a compatibility extension. Explicit
`override_source(owner, name, target)` is also available. Aliases and
`override_base_url` are scoped to the owning document; identical names in different
documents never overwrite each other. Explicit `Options::source` / `base_url`
entries win over the registry adapter. `Options::source_document` exposes the
metadata of registry-backed sources, and returns `None` for legacy sources.
Registry-backed options (including cloned options and source aliases) share the
loader's immutable raw value. They do not keep another full JSON copy. Arazzo also
has its parsed typed model; API documents remain raw values with checked versions.
The adapter retains a registry snapshot, not a live view. After changing registry
links or overrides, construct fresh options; existing entries win on repeated
adapter calls too.

Cycles are retained as back edges, not recursively expanded documents. Shared
dependencies reuse handles and loaded resources. The default limits are 256
existing documents plus distinct loader attempts, and depth 32 (root depth zero).
Failed attempts and different retrieval aliases also consume the document budget;
known cycles/diamonds do not consume additional depth. `root_sources` selects root
aliases; linked Arazzo documents are traversed in full. These limits are independent
of workflow step/retry/call-depth limits. Supplied documents must be inserted before
loading; the caller is responsible for bounding those inputs and response sizes.

Loading failures carry owner/alias/field locations and leave readable documents
available. Preparation decides whether that partial graph is sufficient: an
unrelated missing source need not block a qualified operation, but a missing
candidate source still prevents proving a bare `operationId` unique. Loading does
not silently certify a partial graph as complete.

Recognized versions are Arazzo 1.0/1.1, OpenAPI 2.0/3.0/3.1/3.2, and AsyncAPI
2.6/3.0/3.1. Arazzo 1.0 retains its wire version and is upconverted for execution.
API documents retain complete raw values and model-checked versions; loading is
**not** API structural or schema validation. AsyncAPI steps run through a
message client the caller supplies ([AsyncAPI steps](#asyncapi-steps)). External OpenAPI Path Item resolution and API-server computation
have separate execution rules below; workflows of linked Arazzo documents are
callable as described next.

### Workflows of external Arazzo documents

A step, `dependsOn` entry, or `goto` / `retry` action names a workflow of another
Arazzo document as `$sourceDescriptions.<name>.<workflowId>`, where `<name>` is an
Arazzo source of the document that writes the reference:

```json
{ "stepId": "lookup", "workflowId": "$sourceDescriptions.shared.findPet",
  "parameters": [{ "name": "petId", "value": "$inputs.petId" }] }
```

External documents come from the source registry: `Options::source_registry`
indexes every Arazzo document reachable from the owner through linked Arazzo
sources (including links back to the owner, which name the entry description).
Without a registry, or before the source is linked, the reference fails with
`ExecutionError::ExternalWorkflow` naming the missing alias. `required_sources`
lists the entry's Arazzo aliases that the selected workflow reaches, and graph
loading traverses each linked Arazzo document's own sources in full. A linked
document that calls back into the entry can need more entry sources, so a
selective loader calls `required_sources` again after each load and loads the
additions until the set stops growing.

The called workflow runs on the same engine as a local call, in its own
document's scope:

- Its components, `$self`, `$sourceDescriptions`, operation sources and
  `$workflows.<id>` are those of the document that declares it. Two documents
  may both have a workflow `child`, a step `get` and a source `api`; neither
  shadows the other.
- Arguments are bound in the caller's scope, then validated against the callee's
  input schema (resolved against the callee's document) before it sends anything.
- `Options::source` / `Options::base_url` apply to the entry description only.
  Use `SourceRegistry::override_base_url(document, alias, url)` for another
  document's source; it applies to that document alone.
- Input binding, output projection, criteria, timeouts, retry detours and `goto`
  handoff follow the local-call rules unchanged.
- `StepRecord::document` carries the declaring document's canonical identity for
  steps of external workflows (`None` for the entry description), and the calling
  step's `Performed::Workflow` names the reference as written.
  `PreparationDiagnostic::document` locates findings the same way, and a
  runtime error raised in an external document comes wrapped in
  `ExecutionError::InDocument { document, error }`.

Preparation compiles the statically reachable workflow graph across documents and
structurally validates every external document it enters. A `dependsOn` cycle,
whether within one document or through several, fails preparation. Recursive
calls (`a` calls `b` calls `a`) are not static errors; they stop at the
workflow-depth limit with a partial report. Nor is a call into a workflow that
depends on one still running (`a` calls `b`, and `b` depends on `a`): the run
stops there with `ExecutionError::Circular`, named in the document of the
dependency. Source-graph cycles are harmless data,
and `goto` loops stop at the step limit. An external workflow entered by a
call, `goto` or `retry` runs its own `dependsOn` first, in its own document, as
a local one does.

Dependencies receive the root inputs, as local dependencies do. An external
dependency's outputs are recorded in its own document's `$workflows` namespace.
The runtime-expression grammar has no qualified `$workflows` form, so the dependent
cannot read them. A same-named local workflow is a different workflow: reading its
outputs before it has run is a runtime error, as for any workflow that has not run.

Each Arazzo document is upconverted independently: a v1.0 entry may call a v1.1
document and vice versa, and every document runs under the same condition profile
and v1.1 execution model.

### OpenAPI operation resolution

Preparation builds one borrowed operation index per needed source and retains
resolved endpoints for repeated runs. The lazy APIs use the same resolver,
indexing each source the first time a step of the run needs it and reusing that
index for the rest of the run (the indexes are kept until the run is dropped);
use preparation to reuse that work across runs.
Both resolve inline operations and local/external Path Item `$ref` chains while
preserving the path where each operation is mounted.
Source values are never rewritten and schemas are not dereferenced.

| Source version | Executable methods | Server selection |
| --- | --- | --- |
| Swagger 2.0 | GET, PUT, POST, DELETE, OPTIONS, HEAD, PATCH | Operation/root `schemes`, root `host` and `basePath`; omitted host/scheme use the retrieval URL when available |
| OpenAPI 3.0.x / 3.1.x | Swagger methods plus TRACE | First server at operation, Path Item, then root level; `/` when none is available |
| OpenAPI 3.2.x | 3.1 methods plus QUERY and `additionalOperations` | Same server selection; `$self` additionally establishes document identity/reference base |

An `additionalOperations` key is a case-sensitive HTTP method token, preserved in
the request; standard methods must use their fixed fields. Unversioned
`Options::source` values retain the legacy profile, including requiring a server
or `host`; registry source descriptions require a supported version discriminator.
This is operation resolution, not full OpenAPI validation or automatic parameter,
authentication, request-body or response-schema generation.

The resolver rejects duplicate operation IDs within a source, reference cycles,
missing/malformed targets, and overlapping fields in a Path Item plus its `$ref`
target, including `summary`, `description` and `x-*` annotations. These annotations
do not override their referenced counterparts. Non-overlapping siblings are combined.
Rejecting overlaps is an explicit policy for the
[specification's undefined merge case](https://spec.openapis.org/oas/v3.1.1.html#path-item-object),
not a claim that the specification mandates that policy. Indexing checks all
mounted paths in a needed source; even an unrelated broken mount in that source
prevents preparation. Qualified IDs avoid indexing unrelated sources, whereas
bare IDs require every candidate source to establish uniqueness.
Supplied Arazzo and AsyncAPI documents are not OpenAPI candidates and are skipped
by bare-ID lookup. Explicitly selecting one as an HTTP operation source is still
an error. Malformed or conflicting OpenAPI documents are never silently skipped;
unversioned OpenAPI values retain their legacy compatibility behavior.

For example, `paths: { /pets: { $ref: 'parts.yaml#/pet' } }` mounts the referenced
item at `/pets`. An operationPath ending in `#/paths/~1pets/get` selects that mount.
URI fragments are percent-decoded as UTF-8, then checked as JSON Pointers (only
`~0` and `~1` escapes). Invalid escapes produce errors, not partial lookups.
Use Arazzo's `{$sourceDescriptions.api.url}#/paths/~1pets/get` form; literal document
URIs are a compatibility extension and match normalized identities/retrieval URLs.
A pointer into a reusable item in the same source document also works if its
operation has exactly one mount. Multiple mounts require a specific `/paths`
pointer; unmounted components have no request path to infer.

With `source-graph`, `load_sources` and `load_sources_async` follow reachable Path
Item references using only the caller's registered fetchers and the existing shared
document/depth budgets. They never follow schema, parameter, response or callback
references. Preload standalone reference resources, without inventing an OpenAPI
version or source alias, with:

```rust,no_run
# use roas_arazzo_executor::SourceRegistry;
# use serde_json::json;
# fn example(registry: &mut SourceRegistry) -> Result<(), Box<dyn std::error::Error>> {
registry.insert_reference_document("https://example.test/parts.yaml", json!({
    "pet": { "get": { "operationId": "findPet" } }
}))?;
# Ok(()) }
```

Loading failures have document/pointer/reference locations in
`SourceLoadReport::reference_diagnostics`. Path Item cycles terminate loading and
are rejected by operation indexing. Prepare only after loading; preparation and
execution never fetch missing reference documents. Budgets bound loading, not
permission to execute supplied documents. Without `source-graph`, local references
and references to other explicitly supplied `Options::source` resources still work.

Relative Server Object URLs resolve against the **retrieval URI of the document
containing that Server Object**, including its final redirect location, never its
`$self`. Reference identity and server origin are distinct under the
[OpenAPI API-URL rules](https://spec.openapis.org/oas/v3.2.0.html#relative-references-in-api-urls).
For example, a server `./v1` in a referenced item retrieved from
`https://cdn.test/parts/item.yaml` becomes `https://cdn.test/parts/v1`, even if the
entry API has a different identity. The mounted path is appended to that base.
OpenAPI 3.2 retrieval aliases remain accepted as a compatibility extension;
`retrieval_aliases` controls the stricter Arazzo identity rule only.

Server variables use string defaults. An explicit absolute `base_url` override
wins over all document servers. Without one, a relative/default server needs a
usable HTTP(S) retrieval origin; file-based documents can supply an absolute server
or an override. Only HTTP(S) endpoints without server query strings/fragments are
supported. The existing empty-array policy is retained: an empty operation/Path
Item `servers` array falls through to its parent, and the first nonempty array wins.
This is a compatibility policy, not strict empty-array override semantics.

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

Driving `Run` directly goes one step further: `Progress::Wait` hands back the delay a `retry` asked for instead of spending it, so retry behaviour can be asserted in microseconds. One request is outstanding at a time — `advance` refuses to hand out another until `supply` has answered the first, so a driving loop cannot send the same request twice.

## What it runs

- **Steps** that name an operation by `operationId` (bare, or `$sourceDescriptions.<name>.<id>`) or by `operationPath`, and steps that call another `workflowId` — of the same document, or of an external Arazzo document as `$sourceDescriptions.<name>.<workflowId>` (the `source-graph` feature).
- **Parameters** in `path`, `query`, `querystring`, `header` and `cookie`, from the workflow and the step, with `$components.parameters` references and their `value` overrides.
- **Request bodies**, with runtime expressions anywhere inside the payload and `replacements` by JSON Pointer or JSONPath.
- **Criteria** — `simple` conditions (comparisons, `!`, `&&`, `||`, parentheses, property access and indexing), `regex`, `jsonpath`, `xpath` (the `xpath` feature), and v1.1 `jsonpointer` expression-type criteria (an extension). Pattern conditions interpolate `{$expressions}` before evaluation; simple conditions parse runtime expressions directly. A `jsonpath` condition passes on a non-empty nodelist, whatever the node holds.
- **Response bodies** — a JSON body is parsed; any other non-empty UTF-8 body is its text, a string (without a leading byte-order mark), so `$response.body` names an XML or plain-text body too. An empty body, or one in another encoding, is no body: it is not decoded lossily, which would change the data criteria and selectors judge.
- **Actions** — `end`, `goto` a step or a workflow, and `retry`, which honours `retryAfter`, defaults to a single attempt when `retryLimit` is absent, may send the run through another step or workflow before trying again, and gives way to the next failure action once its limit is spent.
- **Outputs** at step and workflow level, including [`Selector`](https://spec.openapis.org/arazzo/v1.1.0.html#selector-object)s, readable by later steps as `$steps.<id>.outputs.<name>`.
- **Runtime expressions** — `$url`, `$method`, `$statusCode`, `$request.*`, `$response.*`, `$inputs`, `$outputs`, `$steps`, `$workflows.<id>.inputs` / `.outputs`, `$sourceDescriptions`, `$components`, and `$self`.
- **`dependsOn`** between steps and between workflows, which orders them and rejects circles — as does reading another step's outputs, which orders the two without anyone having to say so. A workflow entered by a call, `goto` or `retry` runs what it depends on first, as the run's own workflow does: each dependency once in a run, with the run's inputs (not the arguments of the call), so one that already ran as a dependency — of the root or of an earlier entry — is not run again. A dependency that has not yet run as one, while an instance of it is still running further up (`a` calls `b`, and `b` depends on `a`), stops the run with `ExecutionError::Circular` before `b` or a second `a` sends anything — whatever inputs the running `a` has, so a dependency run is never started inside a running instance of itself. One that already finished as a dependency is met, so `b` then runs. As for the run's own workflow, a dependency that fails does not stop what depends on it: the target still runs. For a call, the calling step is judged on the target alone, and its `timeout` covers the dependencies too.

A step that calls a workflow is a step like any other: what it called becomes its outputs, its own `outputs` are named on top, its `successCriteria` are judged, its `timeout` covers the whole call, and its `onSuccess` / `onFailure` decide where the workflow goes next. It gets a record of its own in the report — `Performed::Workflow` rather than `Performed::Request`.

**AsyncAPI steps** send and receive messages through a client the caller supplies — see [AsyncAPI steps](#asyncapi-steps).

**Parallel steps**, when asked for — see [Parallel steps](#parallel-steps).

Both Arazzo versions: v1.1 directly, and v1.0 through `execute_v1_0` (the `v1_0` feature), which upconverts first so there is one interpreter.

### AsyncAPI steps

A step with an `action` publishes to (`send`) or listens on (`receive`) a channel of an AsyncAPI 2.6, 3.0 or 3.1 source. The engine still performs no IO: implement `MessageClient` (or `AsyncMessageClient`) for your broker — Kafka, MQTT, AMQP, WebSocket — and run with `execute_with_messages` (or `execute_with_messages_async`, or a plan's methods of the same names). `testing::Fake` scripts messages per channel address. The plain `execute` functions stop at an AsyncAPI step with an error, and `PreparedWorkflow::uses_messages` says up front whether a plan needs a message client; `roas arazzo run` has none and refuses such a workflow before sending anything.

```rust
# use roas_arazzo::v1_1::Description;
# use roas_arazzo_executor::{IncomingMessage, Options, execute_with_messages, testing::Fake};
# use serde_json::json;
# fn example(description: &Description, options: &Options) {
let mut messages = Fake::new().message(
    "confirmations",
    IncomingMessage::json(&json!({ "orderId": "o-1" })).header("correlationId", "c-7"),
);
let report = execute_with_messages(description, options, &mut Fake::new(), &mut messages);
# }
```

- **The channel** comes from `operationId` — bare, searched across the AsyncAPI sources, or `$sourceDescriptions.<name>.<id>` — or from `channelPath`, `$sourceDescriptions.<name>#/channels/<key>` (or `{$sourceDescriptions.<name>.url}#/…`). AsyncAPI 3.x operations point at their channel; 2.6 operations sit inside it, under `publish` or `subscribe`. Only references within the document are followed. A message's `contentType` and `correlationId` may come from its `traits`. A 3.x channel whose `address` is null or absent — unknown, as AsyncAPI defines it — is an error. The client is given the address with its `{parameters}` filled from `in: channel` parameters, the servers it is on (the channel's, else all of the document's) and the channel's bindings. The step's `action` decides the direction: an AsyncAPI operation's own `action` describes the application the document is about, and whether the workflow plays that application or talks to it is not something Arazzo says.
- **A send** publishes `in: header` parameters as headers and the `requestBody` as the payload, encoded as for a request, in its `contentType`, else the first message's `contentType`, else the document's `defaultContentType`, else JSON. The step is done once the client has sent it: Arazzo does not model broker acknowledgement. Other parameter locations are an error.
- **A receive does not hold the workflow up.** When it is reached the client is asked to subscribe, and the run goes on. Its message is asked for (`Progress::Receive`) when a later step depends on it — by `dependsOn` or by reading it, as do the workflow's own `successActions` / `failureActions`, which are judged after every step — or when the workflow runs out of steps. It completes there: its `successCriteria`, `outputs` and actions run at that point, and a `goto` or `end` it takes acts from there; the run then carries on with the step that needed it. A `retry` subscribes again. When an `end` (or a `goto` to another workflow) finishes the workflow, every receive still listening is closed unheard: it is not waited for, judged or recorded.
- **`correlationId`** is evaluated when the receive starts and compared with each message at the places its channel's messages keep a correlation id (AsyncAPI `correlationId.location`, `$message.header#/…` or `$message.payload#/…`; a string and a number with the same text match). Messages that do not carry it are passed over. A receive with a `correlationId` on a channel whose messages say nowhere where theirs is is an error.
- **The first matching message decides.** One that fails the step's `successCriteria` fails the step; the next message is not waited for. With no criteria, any matching message passes.
- **Timeouts**: a receive waits at most its `timeout`, counted from when it subscribed, and fails — its `onFailure` actions run — when nothing came; its criteria are not judged and it names no outputs. Without one it waits as long as the run may: forever, unless `Options::timeout` is set.
- **`$message.header.<name>`** and **`$message.payload`** (with `#/pointer`) read the message a receive got or a send sent, in the step's own criteria, outputs and actions and in the workflow's actions; `$steps.<id>.message.…` reads it later. Elsewhere — a step's parameters, payload or `correlationId`, which are evaluated before there is a message, or workflow outputs — `$message` is a preparation error. The payload is read as a response body is: JSON, else UTF-8 text. The ABNF's `#` form is the one parsed; the spec example's `$message.payload.orderId` is not.
- **Subscriptions** are closed (`Progress::Unsubscribe`) once their receive completes. When a run stops early, the `execute` functions close what is still open; a caller driving `Run` itself does so with `Run::open_subscriptions`.
- **Report**: an AsyncAPI step is a `Performed::Message` with its action, the channel address and whether a message went out or came in.

### Parallel steps

Steps run one at a time by default: Arazzo runs them in the order they are listed, and a step may rely on what an earlier one did without naming it (create a pet, then list pets). `Options::parallel(n)` lets up to `n` requests be in flight at once:

```rust
# use roas_arazzo::v1_1::Description;
# use roas_arazzo_executor::{Options, execute_async, testing::Fake};
# async fn example(description: &Description, options: Options) {
let report = execute_async(description, &options.parallel(4), &mut Fake::new())
    .await;
# }
```

- **Which steps run together.** A step starts while others are in flight when it is an HTTP step with no `onSuccess` or `onFailure` actions, in a workflow with no `successActions` or `failureActions`, and none of the steps it names in `dependsOn` or reads (`$steps.<id>…`) is still in flight. Steps start in the run's order, so a step that must wait holds back the ones after it.
- **Everything else runs alone**: a step with actions (they may jump or retry), one that calls a workflow (its own steps may run together inside it), and an AsyncAPI step start once everything before them is done, and nothing after them starts until they are.
- **Failure.** When a step fails and its workflow ends, requests still in flight in that workflow are abandoned: they are not recorded, and their responses are not wanted. A step that could not start — the run's step limit is reached, or its request cannot be built — waits for what is in flight first, and so does an error in judging a step that finished before the steps listed ahead of it: if one of those ends the workflow, the error is dropped. Either way a run stops where it would have one step at a time. A client that fails to carry out a request still stops the run at once, whichever step sent it.
- **The report** lists steps in the order they finished, which is not always the order they are written in.
- **Drivers.** `execute_async` and the other async functions send the requests in flight together. `execute` and the other blocking functions still send one at a time. A caller driving `Run` itself gets several `Progress::Send`s in a row, then `Progress::Await` when nothing more can start; it answers each with `Run::supply_to(request.id, response)`, and `Run::in_flight` says which requests are still wanted.

The workflows a workflow depends on (`dependsOn`) still run one after another.

### XPath

With the `xpath` feature, XPath criteria and selectors are evaluated by [`oxixml-xpath`](https://crates.io/crates/oxixml-xpath), as the version they name: `xpath-10`, `xpath-20`, `xpath-30` or `xpath-31`, and plain `xpath` as XPath 3.1. Each version is its own language, so `let` does not compile as XPath 2.0 and `'3' + 4` is `7` in XPath 1.0 but a type error later. It is off by default. The context must name XML text — a response body that is not JSON is one. Unprefixed names match no namespace, and no prefix is bound, so a document with a default namespace is read with wildcards such as `/*:order/*:total` or braced URIs such as `/Q{urn:x}order`.

A response body is untrusted, so the engine runs under limits:

- Every compile and evaluation runs on a thread of its own with a 16 MiB stack, which the engine is told it has: a recursion that would overflow it stops with `XPDY0130` instead of aborting the process. An evaluation that runs past 1 second, parsing included, stops with `XPDY0130` too. The engine checks that deadline only between steps and calls, so parsing and a single built-in run to completion, and so does a general comparison such as `//a = //b`, which compares every pair of items: over two sets of 50,000 nodes from a 1.2 MB body that takes about three minutes. A run waits for the engine at most 2 seconds and then fails the evaluation; the engine's thread finishes on its own, and until it does every other XPath evaluation in the process fails at once rather than start another, so retries cannot pile them up. Compiling, which preparation does, is not held up.
- A context is refused before it is parsed when it is larger than 4 MiB, holds a document type declaration (its internal entities would expand into a far larger document; no external entity is ever read either way), nests elements more than 1,024 deep, has more than 100,000 elements, comments and processing instructions, has an element with more than 256 attributes or more than 100,000 attributes in all, binds more than 100,000 namespaces across its elements (each element counts every prefix in scope on it besides `xml`, because the engine gives each its own node), has text that, counted once for each element around it, comes to more than 64 MiB (every element's string value repeats the text inside it, and `//a` builds them all), or has nodes that, counted the same way, come to more than 10,000,000 (building an element's string value walks every node under it).
- The engine spends a few kilobytes on each node, so a context at these limits takes up to about 300 MB parsed. An evaluation allocates as it goes and is bounded only by its deadline: an expression that builds a large sequence, such as `//l/following::l`, can reach a gigabyte or two before it is stopped.
- An expression that calls or names `fn:parse-xml`, `fn:parse-xml-fragment`, `fn:json-to-xml`, `fn:parse-json`, `fn:distinct-values`, `fn:id`, `fn:idref`, `fn:element-with-id`, `fn:innermost`, `fn:outermost`, `fn:function-lookup`, a fold (`fn:fold-left`, `fn:fold-right`, `array:fold-left`, `array:fold-right`) or a regular-expression function (`fn:matches`, `fn:replace`, `fn:tokenize`, `fn:analyze-string`) is refused as unsupported before it is compiled. Text parsed into XML would escape the limits on a context; the engine clones and drops nested maps and arrays recursively, beyond the stack it measures, and `fn:parse-json` or a fold can nest them as deep as a response says; `fn:distinct-values`, the ID functions, `fn:innermost` and `fn:outermost` compare every value with every other in one built-in; and a regular expression runs to completion past the deadline. Match with an Arazzo `regex` criterion instead. `fn:doc`, `fn:unparsed-text` and `fn:collection` find nothing, and `fn:environment-variable` sees no variables.
- An expression can build strings and sequences as large as the deadline lets it, whatever the context. A value interpolated with `{$...}` becomes part of the expression's text, not a string it compares, so interpolate only values the description controls, such as its inputs, never text taken from a response.

- A criterion holds by the expression's effective boolean value: a non-empty node sequence, `true`, a non-zero number or a non-empty string. A null context fails it. A context that is not XML, a sequence with no effective boolean value (`(1, 2)`) or a dynamic error fails it with a diagnostic, like any other criterion evaluation error.
- A selector's result becomes JSON item by item: a node is its string value, a boolean stays a boolean, a number is a JSON number when JSON can write it (`NaN`, `INF`, and an integer or decimal JSON cannot hold exactly, stay strings), a whole number is written as an integer (`number(/id)` over `<id>7</id>` is `7`, not `7.0`), and any other value is its string value. A function, map or array item has no JSON value, and selecting one is an error. One item is the value, several are an array of them, and none is `SelectError::Empty`.
- Any version Arazzo does not name is refused. A payload replacement whose `targetSelectorType` is XPath is refused too, because the engine does not write into XML.
- Preparation compiles constant expressions as their version, so a syntax error is reported before anything is sent. An expression with `{$...}` in it is compiled once it is filled in.
- As in every typed criterion, `{$` always starts an interpolated runtime expression. XPath 3.1 braces that begin with a variable, such as `array{$x}` or `map{$k: 1}`, are written with a space: `array{ $x }`.
- A panic inside the engine is caught, and the expression is refused as unsupported, which is terminal like any other unsupported capability. The panic hook still prints its message, and a binary built with `panic = "abort"` still aborts.

## Support matrix

Each column is a separate layer: [`roas-arazzo`](https://crates.io/crates/roas-arazzo) parses, the source registry and preparation resolve references, `roas-arazzo`'s `Validate` and checked preparation (`prepare`) validate, and a run executes. A capability marked *Not run* (XPath without its feature, AsyncAPI steps without a message client) is an error where it is met, never skipped; `x-` extensions of Arazzo objects are ignored. A check named in the validation column is made by `prepare`, and the lazy `execute` / `Run::start` path makes only some of them: it does not see unknown members at all.

| Feature | Parsing | Reference resolution | Validation | Execution |
| --- | --- | --- | --- | --- |
| Arazzo 1.0 / 1.1 | Typed models; v1.0 keeps its written version | `$self` and Arazzo `sourceDescriptions` through `SourceRegistry` (`source-graph`) | Structural checks; preparation checks the selected workflow's reachable graph | v1.1 directly; v1.0 after upconversion |
| `x-` extensions of Arazzo objects | Kept on every object but a Reusable Object | — | Not interpreted | Ignored |
| Other unknown members | Dropped by the model; listed by `Description::unknown_members` | — | Preparation error (entry when given `Options::unknown_members`, external documents always) | A checked run refuses them; the lazy path never sees them |
| OpenAPI 2.0 / 3.0 / 3.1 / 3.2 sources | Raw values with a checked version | `operationId`, `operationPath`, Path Item `$ref` chains, servers | Duplicate IDs, reference cycles, Path Item overlaps; no structural or schema validation | HTTP requests ([methods and servers](#openapi-operation-resolution)) |
| AsyncAPI 2.6 / 3.0 / 3.1 sources | Raw values with a checked version | Operations and channels by `operationId` / `channelPath`, local `$ref`s | Channels, parameter locations and sources checked; `$message` outside an AsyncAPI step is an error | Messages sent and received through a `MessageClient` ([details](#asyncapi-steps)) |
| External Arazzo workflows | Typed models from the registry | `$sourceDescriptions.<name>.<workflowId>` | Structure, unknown members and the cross-document workflow graph | In their own document's scope |
| Components | Typed models | `$components.<kind>.<name>` by name | A missing component is a preparation error | Resolved where used |
| Runtime expressions | Strings | `$sourceDescriptions`, `$components`, `$self`, `$workflows.<id>` | Syntax and references checked, with byte offsets | Evaluated |
| `simple` conditions | Strings | — | Parsed under the [condition profile](#condition-profile) | Evaluated, short-circuiting |
| `regex` criteria | Typed criterion types | — | Constant patterns compiled | Evaluated |
| JSONPath criteria, and selectors (v1.1) | Typed | — | Constant paths compiled; only version `rfc9535`, so a v1.0 criterion that names the Goessner draft is refused | Evaluated as RFC 9535; a lazy run refuses any other version when it reaches it |
| JSON Pointer selectors and criteria (v1.1) | Typed | — | Constant pointers' syntax checked | Evaluated; the criterion type is an extension |
| XPath criteria and selectors | Typed | — | With the `xpath` feature: constant expressions compiled as their version, unknown versions, refused functions and XPath replacement targets refused; without it, a preparation error | With the `xpath` feature, evaluated as XPath 1.0 to 3.1 over XML text ([details](#xpath)); without it, not run |
| Workflow input schemas | JSON values | Offline only: `Options::schema_document`, the registry, `Options::source` documents, local references ([details](#workflow-input-validation)) | Compiled when the `input-validation` feature is on and `Options::input_validation` selects `Draft202012` (off by default) | When enabled, inputs validated at every start, call and transfer |
| `dependsOn` | Typed | Step and workflow ids | Cycles rejected | One at a time, except that an AsyncAPI receive listens in the background until a step depends on it; a call, `goto` or `retry` target's dependencies run first, each once in a run, with the run's inputs |

## Condition profile

The same profile applies to v1.0 and v1.1. Arazzo specifies the operators but does
not yet fully specify their grammar or bare-value truthiness (upstream issues
[#518](https://github.com/OAI/Arazzo-Specification/issues/518) and
[#517](https://github.com/OAI/Arazzo-Specification/issues/517)). These are explicit
executor policies, not a claim that every evaluator behaves identically.

| Behavior | Rule | Status |
| --- | --- | --- |
| Navigation | `.member` accesses an object; `[0]` indexes an array | Specified operators; identifier and index syntax is executor policy |
| Strings | Single quotes; a doubled quote escapes itself: `'Rex''s'` | Specified |
| Comparisons | Case-insensitive strings, numeric-string coercion against a number, structural collection equality | String comparison specified; coercion recommended; collection equality retained policy |
| Null / missing | `null == null` passes; null differs from other values; a missing value is an error, not null | Null equality specified; inequality and missing-value handling explicit policy |
| Bare values | `false`, `null`, zero, `''`, empty arrays and objects fail; other values pass | Boolean/null behavior specified; remaining truthiness retained policy |
| Evaluation order | Parse everything and check referenced step/workflow declarations first; `&&` and `||` short-circuit runtime value lookup; dependency analysis visits both sides | Executor policy; changed from eager evaluation |
| Precedence | Navigation, unary `!`, one comparison, `&&`, then `||`; parentheses group conditions | Executor policy; chained comparisons rejected |
| Numbers | JSON number syntax; finite values only; integral comparisons retain integer precision | Executor policy |
| Extensions | Double-quoted strings (doubled quote escaping), unquoted non-numeric words, whole `$inputs`/component collections, workflow-output shorthand, step exchange access | Compatibility extensions; whole `$outputs` and `$workflows.<id>.outputs` collections also supported |

Standalone runtime expressions must consume their entire field. In a simple
condition, whitespace and `()&|=!<>` delimit an operand. This supports both
`$request.query.limit == 10` and `$statusCode==200`. Names containing these
characters may be valid standalone but cannot always be expressed in a simple
condition. A lone `=` receives a boundary diagnostic; the parser never guesses a
name from live data. Quoted strings are literals, not an expression escape hatch.

Runtime-expression identifiers take precedence over condition navigation:
`$inputs.auth.token` names the single input `auth.token`, while
`$response.body.pets[0].name` navigates the body. Use `$inputs.auth#/token` to read
a nested input. Likewise, component/output names and header names retain dots.
This corrects the old nested-object interpretation of dotted input/output names.
Query/path names and source-reference names consume their entire bounded token.

Whole `$workflows.<id>.outputs` reads return an object, just like `$outputs`.
An empty output collection is a value (false as a bare simple condition), not a
missing output. Named reads such as `$workflows.inner.outputs.code` and pointers
such as `$workflows.inner.outputs#/code` still work. An unknown workflow remains
a missing-reference error; a declared workflow that has not run remains a
not-run error, rather than yielding an empty object.

JSON Pointers are distinct from navigation: in `$response.body#/data.name[0]`,
`data.name[0]` is a literal property name. `#/` is not a closing delimiter, so
`$response.body#/a=b == 1` is not supported. Use the standalone
`context: $response.body#/a=b` with a typed criterion, or a Selector. Invalid
pointer escapes and ignored suffixes now produce diagnostics with byte offsets.
No URI-percent decoding is performed on runtime JSON Pointers.

Integer-to-integer comparisons are exact within signed/unsigned 64-bit range.
Floating-point or mixed comparisons still use binary64 and can round; this is
not an arbitrary-precision numeric evaluator. Numeric strings compare numerically
against numbers, but two strings retain case-insensitive lexical ordering.

Legacy syntax checking during step ordering does not replace checked preparation.
Known workflow-wide action criteria and parameter expressions are syntax-checked
once, using effective parameter overrides, but their reads do not add prerequisites
to every step: they use the state available when an action is considered.
Step-local expression dependencies still affect ordering. Missing
reusable actions and action-argument components are diagnosed only when dispatch
reaches them; they do not prevent an unrelated outcome from running.
Runtime errors evaluated in criteria follow the failure/reporting policy below.
Errors in parameters and outputs still stop execution. Use `prepare` for the
strict pre-execution checks described below.

### Checked preparation

Use `prepare(&description, &options)` to check a workflow before sending requests.
It returns an immutable `PreparedWorkflow` or a `PreparationError` containing
deterministically ordered diagnostics (numeric array indices, lexical field names).
Findings carry a field path, workflow and
step context, a byte offset where available, and the original model/executor error.
Paths use the model validator's human-readable notation, not JSON Pointer; a
reusable value's path names its component while the workflow/step identifies its use.

Give `Options::source_map` a `roas_arazzo::source_map::SourceMap` (built with
`roas-arazzo`'s `source-map` feature) of the text the description was parsed from,
and each finding also carries `span` (where its field's value is written, or its
name for a finding about the name) and `position` (where `offset` falls in that text, through escapes
and YAML folding); its display then reads `… at line 12, column 31: …` instead of
`… at byte 9`. A finding inside an external Arazzo document uses the map given for
that document's identity with `Options::document_source_map(identity, map)`, where
`identity` is what the finding's `document` names. Without a map, or for a value the map cannot decode exactly, nothing
is guessed: `position` stays `None` and the byte offset is shown.
Give `Options::unknown_members` what `roas_arazzo`'s `Description::unknown_members`
listed for the description, and each member is a `PreparationIssue::UnknownMember`
error located at its name: the specification allows only its fields and `x-`
extensions (a Reusable Object allows none), and a member the model dropped, such as a misspelt `successCriteria`,
would otherwise be silently ignored by the run. External Arazzo documents in the
`SourceRegistry` are checked the same way without being asked. Only checked
preparation locates findings; errors of the lazy `execute` / `Run::start` path carry
no field paths.

```rust,no_run
# use roas_arazzo_executor::{prepare, Options, testing::Fake};
# fn example(description: &roas_arazzo::v1_1::Description) -> Result<(), Box<dyn std::error::Error>> {
let options = Options::new().workflow("buyPet"); // also supply source documents
let plan = prepare(description, &options)?;
let report = plan.execute(&mut Fake::default())?;
// Or: plan.execute_async(&mut client).await, or manually drive plan.start().
# Ok(()) }
```

Preparation validates document structure globally, then checks the selected
workflow's potential steps, calls, recovery targets and dependencies. It follows
both action outcomes and all criteria, including branches that runtime dispatch
could skip. Shared action reads are validated without becoming step dependencies.
Effective parameter overrides use the same matching rules as execution. Missing
components, unknown step/workflow IDs, unsupported capabilities, constant invalid
patterns, unresolved operations, and dependency cycles prevent a checked run.
Static URL/path-parameter errors are checked with placeholder values, without
evaluating inputs or responses.

This is a strict **preparation policy**, separate from Arazzo's runtime criterion
failure rules. For example, `true || $steps.typo.outputs.value` and a typed criterion
missing `context` cannot be recovered into a checked success. An absent response
property or an invalid runtime-generated regex/JSONPath still follows ordinary
criterion recovery. Constant malformed regex/JSONPath patterns are rejected during
preparation, even on an action that might not be selected.

`required_sources(&description, &options)` performs source-independent checks and
returns the source names needed before full preparation. Qualified operation IDs
and operation paths do not require unrelated sources; bare IDs still need every
non-Arazzo source to establish uniqueness. Neither API fetches anything. The CLI
uses this discovery before `--load`, then executes a checked plan. Explicit
`--source` files are still read, and `--ignore` retains its existing shallow model
validation exceptions. Terminal CLI errors print available partial history unless
`--quiet` is set, and still exit unsuccessfully.

The plan borrows immutable description/options and reuses parsed runtime
expressions, simple-condition ASTs, interpolation templates, constant regex and
JSONPath programs, resolved endpoints and dependency ordering. Parser-call
instrumentation tests verify reuse across repeated executions; no wall-clock
speedup is claimed. Dynamic patterns are compiled per evaluation without a cache
keyed by runtime values. Every run has separate state; `start_with_inputs` replaces
the selected workflow's and root dependencies' initial inputs with a fresh map.

`condition_profile()` returns `CONDITION_PROFILE` (`roas-arazzo-conditions-1`).
The profile is fixed, not inferred from response values. With
`Options::portability_lints(true)`, bare-value truthiness produces advisory findings
in `plan.diagnostics()`; it neither invalidates the document nor changes evaluation.
No persistent cache is provided; any future cache must bind the profile,
description, sources and relevant options together.

Existing `execute`, `execute_async`, v1.0 wrappers and `Run::start` retain their
lazy validation behavior and signatures. Library callers opt into strictness by
preparing a plan. For v1.0, upconvert to a retained v1.1 description before preparing.
The CLI now opts in by default, so previously skipped document defects can cause an
earlier, nonzero exit instead of a recovered success.

Input-schema compilation is optional; see the input-validation policy below.
Preparation does not add XPath execution; XPath runs only with the `xpath` feature. AsyncAPI steps run only with a message client.
Source identity loading is opt-in and occurs before preparation;
referenced OpenAPI resolution uses the already-supplied document graph.
JSONPath execution supports `rfc9535`, not the alternate Goessner draft: preparation
refuses another version up front, and a lazy run refuses it when it reaches it.
The dependencies of the run's workflow, and of a workflow a call or recovery
transfer enters, are scheduled on both paths; a dependency a nested entry needs
is checked against its input schema when it starts, as a call's target is.

### Locations and offsets

Offsets into a string are zero-based UTF-8 byte offsets into the decoded string:
`CriterionError::Syntax::offset` into the condition, `ExpressionError::Syntax` /
`Navigation::offset` into the expression or operand held in its `expression`
field, and `PreparationDiagnostic::offset` into the field's value as parsed, after
quotes and escapes. Only preparation findings are mapped back to the original
text, when the document has a source map: `PreparationDiagnostic::span` is where
the field's value (or, for a finding about a name, the name) is written, and
`PreparationDiagnostic::position` is where `offset` falls, when the finding has
one. Both are `roas_arazzo::source_map` positions: a zero-based UTF-8 byte offset
into the original text plus a one-based line and a one-based column counted in
Unicode scalar values. Runtime errors carry only the string offset. An editor
speaking UTF-16, such as an LSP client, converts at its own boundary. A diagnostic
about an operand boundary, such as a lone `=` or `&` inside a simple-condition
operand, points at that operator character, so an editor can mark where the
parser split the name.

### Workflow input validation

Enable the `input-validation` Cargo feature and explicitly select the policy:

```rust
use roas_arazzo_executor::{InputValidation, Options};

let options = Options::new()
    .input_validation(InputValidation::Draft202012)
    .input("petId", "7");
```

Library defaults remain `InputValidation::Disabled`; enabling a Cargo feature
through dependency unification does not change existing callers. Requesting
validation without the feature fails explicitly. The configured mode is available
from `Options::input_validation_mode()`, `PreparedWorkflow::input_validation()`
and `ExecutionReport::input_validation`. Disabled runs do not certify conformance.

`prepare` compiles schemas, not input instances. `start`, `execute`, and
`start_with_inputs` validate the actual input object each time. Initial root and
dependency inputs are checked before **any** request; workflow calls and recovery
transfers validate their explicit bound arguments before the child sends anything.
Invalid child inputs are terminal engine errors, not response-criterion failures:
`onFailure` cannot turn them into a success. Partial reports retain completed
parent attempts, without inventing an attempt for a rejected child. The same
entry checks apply to lazy, prepared, sync, async, and v1.0-upconverted runs.

The profile is JSON Schema 2020-12, including local reusable inputs, JSON Pointer
references, `$id`, `$anchor`, recursive schemas and schema-valued applicators.
There is no coercion, default insertion, or format assertion (`format` remains
an annotation). Other explicitly declared dialects in selected schemas are
rejected. Numeric values retain `serde_json`'s normal representation; this is not
arbitrary-precision JSON.

Resolution is **offline**. Supply complete resources with
`Options::schema_document(uri, value)` or through a source registry (including
`insert_reference_document`). Legacy absolute `Options::source` documents are
also available. `input_schema_base(retrieval_uri)` supplies an Arazzo retrieval
base without the registry. `$self` resolves against retrieval and becomes the
document base; nested schema `$id` establishes its own scope. Without either an
absolute `$self` or retrieval metadata, an isolated internal base permits local
references only; supply a real base for relative external references. Endpoint
base-URL overrides do not affect schema resolution.

The schema catalog discovers embedded input/schema identities and anchors without
compiling unused schemas. Compilation selects each reachable workflow input and
the reusable inputs, OpenAPI component schemas, and standalone schema documents
reached through its references. Unused components/documents cannot fail a run just
because they declare another dialect or refer to an unavailable resource. Selection
is by schema root, not by individual assertion: the entire definition tree within
a selected root is checked, and its references must resolve. Conflicting claims on
a selected identity, invalid URI scopes and unsupported dialects are errors.
No schema reference starts a file or network fetch, including when other
dependencies enable the backend's fetch features. A private normalized indexing
view preserves schema URI scope across Arazzo/OpenAPI container fields; supplied
documents and runtime expression contexts are unchanged. Compilation caches
belong to each prepared plan (or lazy run), not a process-global cache.

`ExecutionError::Input` distinguishes malformed configuration/schema failures
from `InputError::Invalid`, which carries all instance violations with schema and
instance locations. Displayed instance values are masked. Schema processing is
not a CPU/memory sandbox: use trusted schemas and bound input/document sizes in
the calling application.

### Criterion recovery and partial reports

A runtime criterion error is a failed condition with a typed diagnostic in
`CriterionOutcome.error`, following the [Arazzo evaluation-error rules](https://spec.openapis.org/arazzo/v1.1.0.html#evaluation-errors).
Missing values, invalid navigation, and malformed runtime-generated regex/JSONPath
patterns can therefore reach `onFailure` retries or recovery targets. All success
criteria are recorded. An action's criteria stop at the first failure, and later
eligible actions can still match; `StepRecord.action_criteria` records the actions
actually considered. Unreached operands/actions produce no runtime diagnostics.
The report's text display includes criterion diagnostics.

An ordinary false condition has no error. Explicit null is distinct from a missing
value: a null regex/JSONPath context fails without a missing-value diagnostic,
whereas a missing context value retains its expression error. A JSONPath node
containing null selected from a non-null document still counts as a match.
The JSON Pointer criterion extension retains its node-existence behavior.

Unsupported XPath/AsyncAPI capabilities (XPath without its feature, XPath versions Arazzo does not name, XPath replacement targets, XPath functions the executor refuses) remain terminal errors, not false
conditions silently skipped in favor of another action. Preparation-time syntax
errors also remain terminal. Runtime failures outside criteria (including output,
parameter, operation, action-reference, client, and limit errors) retain their
original `ExecutionError` categories. This policy also applies to v1.0 descriptions
after upconversion; it does not claim that v1.0 defines the v1.1 evaluation rules.

Existing `execute`, `execute_async`, and `execute_v1_0` signatures are unchanged.
Use `execute_with_report`, `execute_async_with_report`, or (with `v1_0` enabled)
`execute_v1_0_with_report` to receive an `ExecutionFailure` containing the original
error and an optional partial report. No report exists if `Run::start` failed.
Once started, an interrupted report has `Outcome::Incomplete`, which is never
successful. It retains previous attempts and actual responses/completed workflow
calls, including an attempt whose output evaluation or action dispatch failed;
it does not invent responses for requests that failed or were never sent.
Workflow outputs are available only after successful output evaluation at completion.

For a custom driver, `Run::partial_report()` returns a snapshot without evaluating
more expressions. A terminal engine error leaves history inspectable but stops
further execution (`ExecutionError::Stopped`). `Awaiting` and `NotWaiting` remain
correctable driving errors. Completed reports remain inspectable, and repeated
`advance` calls return the same completed report.

### Migrating from 0.1.x

The grammar and diagnostic changes are released on the **0.2** line, not as a
0.1.x patch. `ExpressionError` adds `Syntax` and `Navigation`. `ExpressionError`,
`CriterionError`, and `SelectError` are now `#[non_exhaustive]`; downstream matches
on these enums must include a fallback arm. Adding the attribute is itself a
breaking change and does not make the new variants compatible with 0.1.x.
`CriterionError::Syntax` also gains a structured `offset` field (a zero-based
UTF-8 byte offset into `condition`). Its `message` now contains only the parser's
reason; use `offset` instead of parsing a location out of that string. Update
explicit construction/destructuring of this variant for 0.2; matches can use `..`
to ignore fields. Its standalone display still includes the location, while a
preparation diagnostic prints the field-relative location once.
Also review the dotted-name, short-circuit, and numeric-literal changes described
above when migrating workflow documents. No public variants are removed.
Callers that previously expected runtime criterion errors in `Err` must now inspect
the report's outcome and criterion diagnostics; successful recovery can produce a
successful run containing earlier failed attempts. The report fields, partial-report
APIs, and non-exhaustive enum variants added for recovery are additive API changes.
`OperationError` is now re-exported at the crate root and is `#[non_exhaustive]`,
with structured document, reference, duplicate-ID and server errors. The new
resolver rejects ambiguous or malformed operation documents that older versions
could accept by taking the first match. Path Item overlap handling, strict pointer
decoding and HTTP(S)-only server validation can therefore reject existing documents.

`Options::inputs` no longer silently ignores non-object values: preparation or
start reports `InputError::NotObject`, even with schema validation disabled.
Passing a subsequent object batch replaces the inputs and clears that error.
The CLI now validates input schemas by default; `--skip-input-validation` is the
explicit compatibility opt-out. `arazzo validate` remains structural validation.

External workflow references (`$sourceDescriptions.<name>.<workflowId>`) used to
fail with `ExecutionError::Unsupported`. They now resolve through a source registry
and fail with the new `ExecutionError::ExternalWorkflow` when the source is
undeclared, not Arazzo, unsupplied, or lacks the workflow. `StepRecord::document`,
`PreparationDiagnostic::document` and the `document` field of
`InputError::Schema` / `InputError::Invalid` (set on errors from a run; a
preparation diagnostic names the document itself) are new, as is
`ExecutionError::InDocument`, which wraps operation, expression, criterion and
request errors raised in an external document (entry-description errors stay
unwrapped). Patterns that list every
field of those variants need `..`. Code that builds these structs or matches
every enum variant must be updated. A workflow id missing from an external
document stays bare and is reported as `ExternalWorkflow` inside `InDocument`
naming that document.
An explicit `Options::source` entry for an Arazzo alias also decides which
document `$sourceDescriptions.<alias>.<id>` runs. An owned value cannot hold an
Arazzo document, so it is reported as an error. Local calls, their reports and their diagnostics are
unchanged.

A lazy run (`execute`, `Run::start`) used to enter a called, `goto` or `retry`
target that declares its own `dependsOn` without running those dependencies.
Both paths now run them first, each once in a run and with the run's inputs, as
the run's own workflow's dependencies run; such a description is no longer
refused.

A lazy run also used to evaluate a JSONPath criterion, selector or replacement
target of any `version` as RFC 9535. It now stops at any version but `rfc9535`,
as preparation already did: with `ExecutionError::Criterion(CriterionError::Unsupported(_))`
for a criterion and `ExecutionError::Select(SelectError::Unsupported(_))` for a
selector or replacement target (preparation reports a criterion's version as
`ExecutionError::Unsupported`, at `type.version`). That includes v1.0
criteria that name the Goessner draft, the only JSONPath version v1.0 allows; a
v1.0 `jsonpath` criterion with no version still runs.

### AsyncAPI steps and `Progress`

AsyncAPI steps now run ([AsyncAPI steps](#asyncapi-steps)). `Progress` gains `Publish`, `Subscribe`, `Receive` and `Unsubscribe`, so a loop that drives `Run` itself needs arms for them (or a panicking catch-all if it runs no AsyncAPI steps); `Run::deliver` answers a `Receive`. `Performed` gains `Message`. `$message` is now evaluated, and is a preparation error in an HTTP step instead of an unsupported capability; `ExpressionError::Unsupported` is no longer raised. Preparation no longer refuses AsyncAPI steps, so check `PreparedWorkflow::uses_messages` before running a plan without a message client.

### Parallel steps and the clients

`Options::parallel` is new ([Parallel steps](#parallel-steps)); without it runs behave as before. To make it possible:

- `AsyncHttpClient::send` takes `&self` instead of `&mut self`, so several sends can be awaited at once. A client that kept state behind `&mut self` keeps it behind a lock, as `testing::Fake` now does.
- `HttpRequest` has an `id` (`RequestId`). `Progress` gains `Await`, which only a run with `Options::parallel` above one returns, and `Run` gains `supply_to` and `in_flight`. `Run::supply` still answers the one request in flight, and returns `ExecutionError::InFlight` when there are several.
- `Fake::sent` and `Fake::waited` return owned `Vec`s rather than slices.

### XPath and response bodies

A response body that is not JSON used to leave `$response.body` empty, so naming it failed with a missing-body error. It is now the body's text, as a string, when the body is UTF-8. This applies with or without the `xpath` feature: a criterion such as `$response.body == 'pong'` can now pass, and an output of `$response.body` now holds the text instead of failing. An empty body still has no `$response.body`. `SelectError` gains an `Evaluation` variant for an expression that is valid but cannot be evaluated against its data; matches already need a fallback arm. Criteria that read such a body now judge its text rather than failing for want of a body: a JSONPath or JSON Pointer criterion over it matches nothing except `$` or the empty pointer, which match the whole string and pass, and a simple condition such as `$response.body != null` passes. `$response.body#/id` now reports that the value has no `/id`.

## What it does not run

Each of these is reported where it is met, never passed over — a run should not look successful because something was skipped.

- **AsyncAPI steps without a message client**: the plain `execute` functions, and `roas arazzo run`, stop at them.
- **XPath** criteria and selectors without the `xpath` feature; with it, versions Arazzo does not name, XPath payload replacement targets, and the functions listed under [XPath](#xpath).
- **Parallel workflows**: the workflows a workflow depends on run one after another. Steps run together only under `Options::parallel` (see [Parallel steps](#parallel-steps)).

## Safety rails

A description can loop — `goto` is a jump. `Options` caps the number of steps (1000), the depth of workflow calls (8) and the retries of one step (10); each raises `ExecutionError::Limit` rather than running forever. Time is not limited unless you ask: `Options::timeout` bounds the whole run. Each request's timeout is cut to the time left, a retry wait that would outlast it is not taken, and a response that comes back after the time is spent — or a client that gives up then — stops the run with `ExecutionError::TimedOut` and a partial report (a caller driving `Run` itself maps its client's failures with `Run::client_failure`). Running out of time ends the run; it is not a failed step, so `onFailure` actions do not run.

## License

`MIT OR Apache-2.0`, as the rest of the workspace.
