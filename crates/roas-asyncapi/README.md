# roas-asyncapi

Rust implementation of the [AsyncAPI Specification](https://www.asyncapi.com/docs/reference/specification/v3.0.0): parse and validate AsyncAPI documents.

[![crates.io](https://img.shields.io/crates/v/roas-asyncapi.svg)](https://crates.io/crates/roas-asyncapi)
[![docs.rs](https://docs.rs/roas-asyncapi/badge.svg)](https://docs.rs/roas-asyncapi)

An *AsyncAPI document* describes an event-driven API: the channels an application sends to and receives from, the messages that travel over them, and the servers and protocols that carry them. It is the event-driven counterpart to an OpenAPI description.

This crate is a sibling of [`roas`](https://crates.io/crates/roas) (the typed parser / validator / merger for OpenAPI 2.0–3.2), [`roas-overlay`](https://crates.io/crates/roas-overlay), and [`roas-arazzo`](https://crates.io/crates/roas-arazzo) — whose v1.1 workflows can point at the AsyncAPI documents this crate models. It provides the typed document model plus a `Validate` framework that collects every diagnostic in one pass.

## Versions

| AsyncAPI version | Feature flag     | Status          | Notes                                                     |
|------------------|------------------|-----------------|-----------------------------------------------------------|
| 3.0              | `v3_0` (default) | ✅ implemented  | —                                                          |
| 3.1              | —                | 🚧 planned      | A thin delta over 3.0: new `schemaFormat` values, `ros2` bindings |
| 2.6              | —                | 🚧 planned      | The pre-v3 model: channels keyed by path, `publish` / `subscribe` |

## Quick start

```rust
use enumset::EnumSet;
use roas_asyncapi::v3_0::Document;
use roas_asyncapi::validation::Validate;

let doc: Document = serde_json::from_str(r##"{
    "asyncapi": "3.0.0",
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

YAML documents work the same way — parse with `serde_yaml_ng` (or any other YAML crate) into `Document`.

## Validation

`Validate::validate` returns every diagnostic it finds rather than failing on the first one. Diagnostics carry a JSONPath-flavor `path` (e.g. `#.channels.lightMeasured.parameters`). Beyond required / non-empty fields and the component-key pattern (`^[A-Za-z0-9\.\-_]+$`), the checks are:

- **Cross-reference integrity** — an operation's `channel` names a declared channel; its `messages` are a subset of *that* channel's messages (a message borrowed from another channel is reported, as is a component message the channel does not list, one that is not declared at all, or a local pointer that names something other than a message); a channel's `servers` name declared servers; the same for an operation's `reply`.
- **Runtime expressions** — `correlationId.location`, `parameter.location`, and `reply.address.location` are parsed against the `$message.header#/…` / `$message.payload#/…` grammar. The `#` is mandatory, per the schema pattern; `$message.payload#` selects the whole payload.
- **`schemaFormat`** — required to be non-empty. It is *not* checked against a fixed list: the specification types it as `anyOf: [string, <enum>]`, so a custom dialect is legal and simply keeps its schema as raw JSON. The documented formats (every 2.0.0–2.6.0 and 3.0.0 AsyncAPI dialect, OpenAPI 3.0.0, Avro 1.9.0, RAML 1.0, JSON Schema draft-07) are exposed as `SUPPORTED_SCHEMA_FORMATS` / `is_supported_schema_format` for callers that want to ask.
- **Channel address ↔ parameters** — every `{placeholder}` in an address is declared, and every declared parameter is used. Server `host` / `pathname` placeholders are checked against `variables` the same way.
- **Security-scheme variants** — each `type` gets both halves of its contract: the fields it requires (`http` needs `scheme`, `oauth2` needs `flows`, `httpApiKey` needs `name` + `in`) and the fields it forbids, since every branch of the specification's `oneOf` is `additionalProperties: false`. `bearerFormat` is accepted only alongside `scheme: bearer`. Each OAuth grant type is checked the same way — required URLs plus `availableScopes`, and the URL its grant type does *not* use (`implicit` forbids `tokenUrl`; `password` and `clientCredentials` forbid `authorizationUrl`).
- **Schema keyword constraints** — from the draft-07 meta-schema: `type` names a real JSON Schema type and its array form is non-empty and duplicate-free, `enum` is non-empty and duplicate-free under draft-07 instance equality (so `1` and `1.0` collide), `allOf` / `anyOf` / `oneOf` are non-empty, `multipleOf` is positive, tuple-form `items` is non-empty, `required` entries are unique, a `discriminator` names a property that this schema itself declares and requires (a composition keyword does not delegate that to its subschemas), and bounds are not inverted.
- **Other coherence** — a `default` is one of the `enum` values, and a message example defines `headers` and/or `payload`.

`ValidationOptions` (EnumSet): `IgnoreEmptyInfoTitle`, `IgnoreEmptyInfoVersion`, `IgnoreUnusedChannelParameter`, and `ErrorOnExternalReference`. Behind the `clap` feature, the enum implements `clap::ValueEnum` so downstream CLIs can surface it directly.

## Scope

The model and its validators are the whole surface. Out of scope for this release:

- **`$ref` resolution across documents.** Cross-reference checks run on document-local pointers; an external `$ref` is accepted without further checking unless `ErrorOnExternalReference` asks for a self-contained document.
- **Trait merging.** `message.traits` and `operation.traits` are parsed and validated, not applied.
- **Typed protocol bindings.** AsyncAPI 3.0 types ~17 protocols across server / channel / operation / message, each with its own independently versioned `bindingVersion`. Bindings are held as raw JSON keyed by protocol, so they round-trip losslessly and typed accessors can be layered on later without a breaking change.
- **Payload dialects other than the default.** Only the AsyncAPI Schema Object dialect — JSON Schema draft-07 plus `discriminator` / `externalDocs` / `deprecated` — is typed, and every draft-07 keyword is modeled so a schema round-trips unchanged. A payload carrying a `schema` property is a Multi Format Schema Object (that presence is the discriminator, exactly as the specification's `anySchema` defines it, not the optional `schemaFormat`), and its `schema` stays raw JSON whatever dialect it names.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
