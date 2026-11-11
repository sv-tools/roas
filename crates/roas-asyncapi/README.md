# roas-asyncapi

Rust implementation of the AsyncAPI Specification ([v2.6](https://www.asyncapi.com/docs/reference/specification/v2.6.0) / [v3.0](https://www.asyncapi.com/docs/reference/specification/v3.0.0) / [v3.1](https://www.asyncapi.com/docs/reference/specification/v3.1.0)): parse and validate AsyncAPI documents.

[![crates.io](https://img.shields.io/crates/v/roas-asyncapi.svg)](https://crates.io/crates/roas-asyncapi)
[![docs.rs](https://docs.rs/roas-asyncapi/badge.svg)](https://docs.rs/roas-asyncapi)

An *AsyncAPI document* describes an event-driven API: the channels an application sends to and receives from, the messages that travel over them, and the servers and protocols that carry them. It is the event-driven counterpart to an OpenAPI description.

This crate is a sibling of [`roas`](https://crates.io/crates/roas) (the typed parser / validator / merger for OpenAPI 2.0–3.2), [`roas-overlay`](https://crates.io/crates/roas-overlay), and [`roas-arazzo`](https://crates.io/crates/roas-arazzo) — whose v1.1 workflows can point at the AsyncAPI documents this crate models. It provides the typed document model plus a `Validate` framework that collects every diagnostic in one pass.

## Versions

| AsyncAPI version | Feature flag     | Status          | Notes                                                     |
|------------------|------------------|-----------------|-----------------------------------------------------------|
| 3.0              | `v3_0`           | ✅ implemented  | —                                                          |
| 3.1              | `v3_1` (default) | ✅ implemented  | Adds its own `schemaFormat` values and the `ros2` bindings |
| 2.6              | `v2_6`           | ✅ implemented  | The pre-v3 model: channels keyed by path, `publish` / `subscribe` |

`v2_6`, `v3_0`, and `v3_1` are independent — enable whichever you need. With `v3_0` and `v3_1` both enabled, an `impl From<v3_0::Document> for v3_1::Document` upconverts a 3.0 document; since 3.1 left the object model untouched, nothing is dropped or approximated and only the `asyncapi` version string changes. With `v2_6` and `v3_0` both enabled, `v3_0::from_v2_6::convert` converts a 2.6 document — v3 reorganized the document rather than extending it, so that conversion is genuinely lossy and returns a `ConversionReport` saying where a name had to be invented and what had nowhere to go.

## Quick start

```rust
use enumset::EnumSet;
use roas_asyncapi::v3_1::Document;
use roas_asyncapi::validation::Validate;

let doc: Document = serde_json::from_str(r##"{
    "asyncapi": "3.1.0",
    "info": { "title": "Streetlights", "version": "1.0.0" },
    "servers": {
        "production": { "host": "broker.example.com:9092", "protocol": "kafka" }
    },
    "channels": {
        "lightMeasured": {
            "address": "smartylighting/streetlights/{streetlightId}/lighting/measured",
            "parameters": { "streetlightId": { "description": "The streetlight id" } },
            "messages": { "lightMeasured": { "name": "LightMeasured" } }
        }
    },
    "operations": {
        "receiveLightMeasurement": {
            "action": "receive",
            "channel": { "$ref": "#/channels/lightMeasured" },
            "messages": [ { "$ref": "#/channels/lightMeasured/messages/lightMeasured" } ]
        }
    }
}"##).unwrap();

doc.validate(EnumSet::empty()).expect("document is well-formed");
assert_eq!(doc.channels.len(), 1);
```

YAML documents work the same way — parse with `serde-saphyr` (or any other YAML crate) into `Document`.

## Validation

`Validate::validate` returns every diagnostic it finds rather than failing on the first one. Diagnostics carry a JSONPath-flavor `path` (e.g. `#.channels.lightMeasured.parameters`). Beyond required / non-empty fields and the component-key pattern (`^[A-Za-z0-9\.\-_]+$`), the checks are:

- **Cross-reference integrity** — an operation's `channel` names a declared channel; its `messages` are a subset of *that* channel's messages (a message borrowed from another channel is reported, as is a component message the channel does not list, one that is not declared at all, or a local pointer that names something other than a message); a channel's `servers` name declared servers; the same for an operation's `reply`.
- **Runtime expressions** — `correlationId.location`, `parameter.location`, and `reply.address.location` are parsed against the `$message.header#/…` / `$message.payload#/…` grammar. The `#` is mandatory, per the schema pattern; `$message.payload#` selects the whole payload.
- **`schemaFormat`** — required to be non-empty. It is *not* checked against a fixed list: the specification types it as `anyOf: [string, <enum>]`, so a custom dialect is legal and simply keeps its schema as raw JSON. The documented formats are exposed per version as `SUPPORTED_SCHEMA_FORMATS` / `is_supported_schema_format` for callers that want to ask: every 2.0.0–2.6.0 and 3.0.0 AsyncAPI dialect plus OpenAPI 3.0.0, Avro 1.9.0, RAML 1.0 and JSON Schema draft-07, and in `v3_1` the three `version=3.1.0` dialects on top.
- **Channel address ↔ parameters** — every `{placeholder}` in an address is declared, and every declared parameter is used. Server `host` / `pathname` placeholders are checked against `variables` the same way.
- **Security-scheme variants** — each `type` gets both halves of its contract: the fields it requires (`http` needs `scheme`, `oauth2` needs `flows`, `httpApiKey` needs `name` + `in`) and the fields it forbids, since every branch of the specification's `oneOf` is `additionalProperties: false`. `bearerFormat` is accepted only alongside `scheme: bearer`. Each OAuth grant type is checked the same way — required URLs plus `availableScopes`, and the URL its grant type does *not* use (`implicit` forbids `tokenUrl`; `password` and `clientCredentials` forbid `authorizationUrl`).
- **Schema keyword constraints** — from the draft-07 meta-schema: `type` names a real JSON Schema type and its array form is non-empty and duplicate-free, `enum` is non-empty and duplicate-free under draft-07 instance equality (so `1` and `1.0` collide), `allOf` / `anyOf` / `oneOf` are non-empty, `multipleOf` is positive, tuple-form `items` is non-empty, `required` entries are unique, a `discriminator` names a property that this schema itself declares and requires (a composition keyword does not delegate that to its subschemas), and bounds are not inverted.
- **Other coherence** — a `default` is one of the `enum` values, and a message example defines `headers` and/or `payload`.

One limit is worth stating: `enum` uniqueness compares numbers after the parser has rounded non-integer literals to `f64`, so two decimals differing only past `f64`'s precision (17+ significant digits) are treated as one. Preserving the exact text would need `serde_json`'s `arbitrary_precision`, which changes how every `serde_json::Value` serializes through other serializers — YAML output becomes `$serde_json::private::Number` maps — and Cargo feature unification would impose that on the sibling crates as well.

`ValidationOptions` (EnumSet): `IgnoreEmptyInfoTitle`, `IgnoreEmptyInfoVersion`, `IgnoreUnusedChannelParameter`, and `ErrorOnExternalReference`. Behind the `clap` feature, the enum implements `clap::ValueEnum` so downstream CLIs can surface it directly.

## Resolving references

Each version's `Document` can follow a `$ref` to the object it names, as long as the chain stays inside that document:

```rust
use roas_asyncapi::v3_1::{Document, Resolution};

let doc: Document = serde_json::from_str(r##"{
    "asyncapi": "3.1.0",
    "info": { "title": "Streetlights", "version": "1.0.0" },
    "channels": {
        "lightMeasured": {
            "address": "lighting/measured",
            "messages": { "lightMeasured": { "name": "LightMeasured" } }
        }
    },
    "operations": {
        "receiveLightMeasurement": {
            "action": "receive",
            "channel": { "$ref": "#/channels/lightMeasured" },
            "messages": [ { "$ref": "#/channels/lightMeasured/messages/lightMeasured" } ]
        }
    }
}"##).unwrap();

// A `RefOr<T>` entry: the item itself, or what its `$ref` leads to.
let operation = doc.operations["receiveLightMeasurement"]
    .get_item(&doc)
    .found()
    .expect("declared inline");

// A bare `Reference`, which says nothing of its kind: the document's
// method for that kind follows it.
match doc.resolve_channel(&operation.channel) {
    Resolution::Found(channel) => assert_eq!(channel.messages.len(), 1),
    // Another document (nothing is loaded), or a place in this one
    // the channel resolver does not hand back an object from.
    Resolution::Opaque => {}
    other => panic!("channel {}", other.problem().unwrap_or("did not resolve")),
}

let message = doc.resolve_message(&operation.messages[0]).found().unwrap();
assert_eq!(message.name.as_deref(), Some("LightMeasured"));
```

`RefOr::get_item(&doc)` takes the entry the model holds, where an inline item resolves to itself, and returns a `Resolution<T>`: `Found`, `Opaque` (the chain leaves the document, or ends at a place the resolver for that kind does not hand back an object from), `Missing`, `Cycle`, `Unrecognized`, or `WrongKind`. `found()` gives the object and `problem()` says why a reference is a document bug, if it is one. `Resolution` is `Clone` and comparable, and each version module re-exports it beside `RefOr`.

`get_item` works for the kinds a version's `Document` implements `common::reference::ResolveReference<T>` for. Where the model holds a bare `Reference` instead of a `RefOr`, the `Document` has a method per kind:

| Version | `RefOr<T>::get_item(&doc)` for `T` | `Document` methods |
|---|---|---|
| 3.0 / 3.1 | `Server`, `Channel`, `Operation`, `Message`, `OperationReply` | `resolve_server(&Reference)` (a channel's `servers`), `resolve_channel(&Reference)` (an operation's or reply's `channel`), `resolve_message(&Reference)` (an operation's or reply's `messages`) |
| 2.6 | `Server`, `Message`, `SecurityScheme` | `resolve_channel(&ChannelItem)` (a channel item carries its own `$ref`) |

Every other kind has no resolver. That includes `SecurityScheme` in 3.x, which only 2.6 covers.

`RefOr::resolved_ref(&doc)` returns where the item was found: `None` for an inline item, otherwise the last `$ref` the chain followed — the target of an alias, the reference into another document when the chain leaves it, or the last reference reached when the chain dangles or loops. It takes the same documents and kinds as `get_item`. For the references the model holds outside a `RefOr`, the 3.x `Document` has `resolved_server_ref`, `resolved_channel_ref` and `resolved_message_ref`, each taking a bare `Reference`; the 2.6 `Document` has `resolved_channel_ref`, taking a `ChannelItem`.

## What 2.6 does differently

2.6 is a different document rather than an earlier draft of v3, so `v2_6` is its own model rather than a variation on `v3_0`:

| | 2.6 | 3.x |
|---|---|---|
| Channels | required, keyed by the channel *path* | keyed by a name, with a separate `address` |
| Operations | `publish` / `subscribe` under a channel, from the *consumer's* point of view | a top-level map, `send` / `receive` from the *application's* point of view |
| Messages | one message, or `{ "oneOf": [...] }`, on the operation | the channel's `messages` map, referenced by the operation |
| Payload dialect | `schemaFormat` on the message, payload alongside it | a Multi Format Schema Object wrapping both |
| Parameters | carry a full `Schema` | strings constrained by `enum` / `default` / `examples` |
| Security | OpenAPI-style requirement maps (name → scopes) | inline schemes; OAuth's `scopes` renamed `availableScopes` |
| `tags` / `externalDocs` | at the document root | under `info` |
| Servers | one `url` | `host` + `pathname` |

The `publish` / `subscribe` inversion is the migration trap worth naming: 2.6's `publish` describes messages *others* publish to the channel, so the application being described receives them — which is why it maps to v3's `receive`, not `send`.

## Scope

Out of scope for this release:

- **`$ref` resolution across documents.** No other document is ever loaded, by the validator or by `RefOr::get_item` and the `resolve_*` methods. A resolver returns `Resolution::Opaque` for an external `$ref` it follows (the one given, or an alias it looks up), or `Resolution::Unrecognized` when its fragment is not a JSON Pointer. `ErrorOnExternalReference` asks the validator for a self-contained document.
- **Trait merging.** `message.traits` and `operation.traits` are parsed and validated, not applied.
- **Typed protocol bindings.** AsyncAPI 3.0 types ~17 protocols across server / channel / operation / message, each with its own independently versioned `bindingVersion`. Bindings are held as raw JSON keyed by protocol, so they round-trip losslessly and typed accessors can be layered on later without a breaking change.
- **Payload dialects other than the default.** Only the AsyncAPI Schema Object dialect — JSON Schema draft-07 plus `discriminator` / `externalDocs` / `deprecated` — is typed, and every draft-07 keyword is modeled so a schema round-trips unchanged. A payload carrying a `schema` property is a Multi Format Schema Object (that presence is the discriminator, exactly as the specification's `anySchema` defines it, not the optional `schemaFormat`), and its `schema` stays raw JSON whatever dialect it names.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
