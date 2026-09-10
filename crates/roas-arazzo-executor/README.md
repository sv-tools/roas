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
**not** API structural or schema validation. AsyncAPI loading does not enable
broker execution. Cross-document workflow execution, external OpenAPI Path Item
resolution, and relative API-server computation are not added by this feature.
Document bases are distinct from API endpoint overrides.

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

- **Steps** that name an operation by `operationId` (bare, or `$sourceDescriptions.<name>.<id>`) or by `operationPath`, and steps that call another `workflowId`.
- **Parameters** in `path`, `query`, `querystring`, `header` and `cookie`, from the workflow and the step, with `$components.parameters` references and their `value` overrides.
- **Request bodies**, with runtime expressions anywhere inside the payload and `replacements` by JSON Pointer or JSONPath.
- **Criteria** — `simple` conditions (comparisons, `!`, `&&`, `||`, parentheses, property access and indexing), `regex`, and `jsonpath`. Pattern conditions interpolate `{$expressions}` before evaluation; simple conditions parse runtime expressions directly. A `jsonpath` condition passes on a non-empty nodelist, whatever the node holds.
- **Actions** — `end`, `goto` a step or a workflow, and `retry`, which honours `retryAfter`, defaults to a single attempt when `retryLimit` is absent, may send the run through another step or workflow before trying again, and gives way to the next failure action once its limit is spent.
- **Outputs** at step and workflow level, including [`Selector`](https://spec.openapis.org/arazzo/v1.1.0.html#selector-object)s, readable by later steps as `$steps.<id>.outputs.<name>`.
- **Runtime expressions** — `$url`, `$method`, `$statusCode`, `$request.*`, `$response.*`, `$inputs`, `$outputs`, `$steps`, `$workflows.<id>.inputs` / `.outputs`, `$sourceDescriptions`, `$components`, and `$self`.
- **`dependsOn`** between steps and between workflows, which orders them and rejects circles — as does reading another step's outputs, which orders the two without anyone having to say so.

A step that calls a workflow is a step like any other: what it called becomes its outputs, its own `outputs` are named on top, its `successCriteria` are judged, its `timeout` covers the whole call, and its `onSuccess` / `onFailure` decide where the workflow goes next. It gets a record of its own in the report — `Performed::Workflow` rather than `Performed::Request`.

Both Arazzo versions: v1.1 directly, and v1.0 through `execute_v1_0` (the `v1_0` feature), which upconverts first so there is one interpreter.

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

Preparation is not input-schema validation: `plan.workflow().inputs` exposes the
opaque schema for caller integration. It does not add XPath, AsyncAPI, external
workflow execution, source identity loading or referenced OpenAPI resolution.
Checked JSONPath execution supports `rfc9535`, not the alternate Goessner draft.
Entry-workflow dependencies are supported; calls/recovery transfers to workflows
with their own `dependsOn` are rejected by the checked path because the engine
does not yet schedule those nested dependencies.

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

Unsupported XPath/AsyncAPI capabilities remain terminal errors, not false
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
