# roas-http-validator

Validates HTTP requests against an [OpenAPI](https://spec.openapis.org/oas/latest.html) description — framework-agnostic, with adapters for axum, actix-web, poem, salvo and rocket.

[![crates.io](https://img.shields.io/crates/v/roas-http-validator.svg)](https://crates.io/crates/roas-http-validator)
[![docs.rs](https://docs.rs/roas-http-validator/badge.svg)](https://docs.rs/roas-http-validator)

[`roas`](https://crates.io/crates/roas) checks that a *description* is well formed. This checks that a *request* is what the description says it should be: the path is one the description names, the method is one that path offers, every required parameter arrived, each one is the type its Schema Object declares, and the body is what the Request Body Object describes.

## Quick start

```rust
use roas_http_validator::{RequestView, Validator};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let spec = serde_yaml_ng::from_str(include_str!("petstore.openapi.yaml"))?;
let validator = Validator::new(spec);

let request = RequestView::new("GET", "/pets").with_query("limit=1000");
let report = validator.validate(&request)?;

assert!(!report.is_valid());
println!("{report}");
# Ok(()) }
```

```text
GET /pets (listPets): 1 error(s)
  - query parameter "limit": 1000 is above maximum 100
```

## Which request type?

None of them, and all of them.

Rust has no single HTTP request type to validate. `http::Request` comes closest — it is what hyper, tower, axum, warp and tonic all speak — but it is generic over a body that is usually a stream, and the crate itself is **version-split**: actix-web 4 still declares `http = "0.2"` while hyper 1, axum 0.8 and reqwest are on 1.x, so their `HeaderMap`s are different types that no single signature accepts. Rocket shares nothing with any of them.

So this crate takes `RequestView` — the small set of things an OpenAPI description actually talks about — and each framework gets a `ToRequestView` impl behind its own feature:

| Feature | Covers |
|---|---|
| `http` | `http::Request`, `http::request::Parts` — and so **axum**, warp, tonic, hyper |
| `actix-web` | `actix_web::HttpRequest` |
| `poem` | `poem::Request` |
| `salvo` | `salvo_core::http::Request` |
| `rocket` | `rocket::Request` |
| `reqwest` | `reqwest::Request` and its blocking twin — the *client's* side, for checking a call you are about to make |

```rust
use roas_http_validator::ToRequestView;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let validator = roas_http_validator::Validator::new(serde_json::from_str(
#     r#"{"openapi":"3.2.0","info":{"title":"t","version":"1"},"paths":{"/pets":{"post":{}}}}"#,
# )?);
# let body = b"{}";
let request = http::Request::builder()
    .method("POST")
    .uri("/pets")
    .header("content-type", "application/json")
    .body(())?;

let report = validator.validate(&request.request_view().with_body(body.as_slice()))?;
# Ok(()) }
```

The body is not part of that conversion, on purpose. A framework body is a stream, and validating one means buffering it — how much, and whether at all, is the caller's decision, so the adapters convert the head and `with_body` takes the bytes. `reqwest` is the exception: a non-streaming body is already bytes in memory, so that adapter supplies it and client-side validation is a one-liner.

## Routing is a different answer from validation

`validate` returns `Err(RoutingError)` when the description says nothing about the request, and `Ok(report)` when it does. A server usually turns the first into a 404 or a pass-through and the second into a 400, so they are not the same value:

```rust
# use roas_http_validator::{RequestView, RoutingError, Validator};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let validator = Validator::new(serde_json::from_str(
#     r#"{"openapi":"3.2.0","info":{"title":"t","version":"1"},"paths":{"/pets":{"get":{}}}}"#,
# )?);
match validator.validate(&RequestView::new("DELETE", "/pets")) {
    Err(RoutingError::PathNotFound { .. }) => { /* 404 */ }
    Err(RoutingError::MethodNotAllowed { allowed, .. }) => { /* 405, `Allow: {allowed}` */ }
    Ok(report) if report.is_valid() => { /* on you go */ }
    Ok(report) => { /* 400, and `report.errors` says why */ }
}
# Ok(()) }
```

Path matching follows the specification's own rule that a concrete segment outranks a templated one, so `/pets/mine` wins over `/pets/{petId}`. A Server Object's base path is stripped when the request carries one — resolved per *operation*, since a Server Object may sit on the Operation Object as well as the Path Item Object and the root and the innermost one wins, so `GET /pets` under `/v1` and `POST /pets` under `/v2` route separately — and the unstripped path is tried too, because an application behind a proxy sees the path without the prefix its own description advertises. `Options::base_path` overrides all of it.

OpenAPI 3.2's `additionalOperations` is looked up alongside the eight standard methods. Both are matched case-sensitively, as [RFC 9110 §9.1](https://www.rfc-editor.org/rfc/rfc9110#section-9.1) requires of a method token: `additionalOperations` keys match the capitalization the description wrote, and the eight standard ones match only their uppercase spelling — `get` is a different method from `GET`, and no Path Item Object describes it.

## Parameters arrive as text

`?limit=10` is the two characters `1` and `0`, not the number ten. Before a Schema Object can judge a parameter, the text has to be turned back into the value the description says it is — and `style` and `explode` say how it was flattened on the way out. All seven styles are handled:

| `in` | Styles |
|---|---|
| `path` | `simple`, `label`, `matrix` |
| `query` | `form`, `spaceDelimited`, `pipeDelimited`, `deepObject` |
| `header` | `simple` |
| `cookie` | `form` |
| `querystring` | `content` (OpenAPI 3.2) |

Splitting happens *before* decoding, so a percent-encoded delimiter stays data: in a non-exploded `form` array, `a%2Cb` is one item containing a comma rather than two items. (`spaceDelimited` has to be the exception — a literal space cannot appear in a query string at all, so `%20` is the only spelling its delimiter has.) The same `style`/`explode` machinery reads `application/x-www-form-urlencoded` bodies through their Encoding Object, so a repeated `tags=a&tags=b` field becomes the array it stands for.

A parameter whose schema this crate cannot read structurally — a composition, say — stays a string, so the schema still judges it and the verdict is at worst too strict, never too lax.

## Every error, not the first

`ValidationReport::errors` collects everything wrong with the request, the way `roas`'s own description validator collects diagnostics: a client that sent three bad parameters is better served by hearing about all three. Each error names where it was found, which parameter it is about, and — inside a body — a JSON Pointer to the value.

## Versions

The interpreter is v3.2. Enable `v3_1`, `v3_0` or `v2` to accept an older description: it is upconverted through `roas`'s own migrations first, so there is one interpreter rather than four.

```rust
# use roas_http_validator::{Options, Validator};
# #[cfg(feature = "v2")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let swagger = serde_json::from_str(r#"{"swagger":"2.0","info":{"title":"t","version":"1"}}"#)?;
let validator = Validator::from_v2(swagger, Options::new());
# Ok(()) }
# #[cfg(not(feature = "v2"))]
# fn main() {}
```

## What it does not check yet

Response validation, security requirements, `multipart/form-data` bodies, and XML. Anything a check could not judge is reported as `ErrorKind::Unsupported` rather than passed over, so a request never looks valid because nothing looked at it.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
