# Spec JSON Schemas

Vendored copies of the authoritative JSON Schemas for every specification version implemented in this
workspace. They are reference material for the typed models and validators — nothing in the build reads
them.

Each file is the latest published iteration as of 2026-08-02, downloaded verbatim from its source URL.

## OpenAPI (`roas`)

| Version | File                            | Source                                                       |
|---------|---------------------------------|--------------------------------------------------------------|
| 2.0     | `oas/2.0/schema.json`           | <https://raw.githubusercontent.com/OAI/OpenAPI-Specification/main/_archive_/schemas/v2.0/schema.json> |
| 3.0.x   | `oas/3.0/schema.json`           | <https://spec.openapis.org/oas/3.0/schema/2024-10-18>         |
| 3.1.x   | `oas/3.1/schema.json`           | <https://spec.openapis.org/oas/3.1/schema/2025-11-23>         |
| 3.1.x   | `oas/3.1/schema-base.json`      | <https://spec.openapis.org/oas/3.1/schema-base/2025-11-23>    |
| 3.1.x   | `oas/3.1/dialect.json`          | <https://spec.openapis.org/oas/3.1/dialect/2024-11-10>        |
| 3.1.x   | `oas/3.1/meta.json`             | <https://spec.openapis.org/oas/3.1/meta/2024-11-10>           |
| 3.2.x   | `oas/3.2/schema.json`           | <https://spec.openapis.org/oas/3.2/schema/2025-11-23>         |
| 3.2.x   | `oas/3.2/schema-base.json`      | <https://spec.openapis.org/oas/3.2/schema-base/2025-11-23>    |
| 3.2.x   | `oas/3.2/dialect.json`          | <https://spec.openapis.org/oas/3.2/dialect/2025-09-17>        |
| 3.2.x   | `oas/3.2/meta.json`             | <https://spec.openapis.org/oas/3.2/meta/2025-09-17>           |

OpenAPI 2.0 has no schema published on `spec.openapis.org`; the copy above is the archived
`swagger.io/v2/schema.json` kept in the OpenAPI-Specification repository.

For 3.1 and 3.2, `schema.json` validates a description against the base vocabulary, while
`schema-base.json` additionally requires the OAS dialect (`dialect.json`, whose vocabulary is described
by `meta.json`) for every embedded Schema Object.

## Arazzo (`roas-arazzo`)

| Version | File                      | Source                                                  |
|---------|---------------------------|----------------------------------------------------------|
| 1.0.x   | `arazzo/1.0/schema.json`  | <https://spec.openapis.org/arazzo/1.0/schema/2025-10-15> |
| 1.1.x   | `arazzo/1.1/schema.json`  | <https://spec.openapis.org/arazzo/1.1/schema/2026-04-15> |

## Overlay (`roas-overlay`)

| Version | File                       | Source                                                    |
|---------|----------------------------|------------------------------------------------------------|
| 1.0.x   | `overlay/1.0/schema.json`  | <https://spec.openapis.org/overlay/1.0/schema/2026-04-01>  |
| 1.1.x   | `overlay/1.1/schema.json`  | <https://spec.openapis.org/overlay/1.1/schema/2026-04-01>  |

## AsyncAPI

Reference material for the AsyncAPI documents that Arazzo v1.1 workflows can point at (AsyncAPI source
descriptions and channel steps); no crate here parses AsyncAPI itself.

| Version | File                        | Source                                                              |
|---------|-----------------------------|----------------------------------------------------------------------|
| 2.6.0   | `asyncapi/2.6.0/schema.json` | `asyncapi/spec-json-schemas` → `schemas/2.6.0.json` |
| 3.0.0   | `asyncapi/3.0.0/schema.json` | `asyncapi/spec-json-schemas` → `schemas/3.0.0.json` |
| 3.1.0   | `asyncapi/3.1.0/schema.json` | `asyncapi/spec-json-schemas` → `schemas/3.1.0.json` |

Taken from [`asyncapi/spec-json-schemas`](https://github.com/asyncapi/spec-json-schemas) at commit
`61cc6add7cf3467f56d1fbb55b1a2b78b4ae6323`; byte-identical to what
<https://asyncapi.com/schema-store/> serves. These are the `$id`-bearing documents
(`http://asyncapi.com/definitions/<version>/asyncapi.json`) — the repository also carries
`-without-$id` twins, which are what
[`all.schema-store.json`](https://www.asyncapi.com/schema-store/all.schema-store.json) `$ref`s so that
schemastore.org can bundle every version in one file.

Each document is self-contained: unlike the OpenAPI schemas, all bindings and Schema Object definitions
are inlined, which is why they are an order of magnitude larger.

## Refreshing

The OpenAPI Initiative publishes dated schema *iterations* — the spec version stays fixed while the
schema is corrected. Newly published iterations for a given version are listed in the
[`OAI/spec.openapis.org`](https://github.com/OAI/spec.openapis.org) repository (e.g.
`oas/3.2/schema/`); to update a file here, download the newest date from that directory and refresh the
matching row above.

AsyncAPI has no dated iterations — a given version's schema is corrected in place — so refreshing means
re-downloading from a newer commit of `asyncapi/spec-json-schemas` and updating the pinned SHA above.
