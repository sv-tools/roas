# roas-http-fetcher

HTTP/HTTPS [`ResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.ResourceFetcher.html) and
[`AsyncResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.AsyncResourceFetcher.html) for the
[`roas`](https://crates.io/crates/roas) OpenAPI loader.

[![crates.io](https://img.shields.io/crates/v/roas-http-fetcher.svg)](https://crates.io/crates/roas-http-fetcher)
[![docs.rs](https://docs.rs/roas-http-fetcher/badge.svg)](https://docs.rs/roas-http-fetcher)

Built on `reqwest` with `rustls-tls`. A single generic `Fetcher<C>` underlies two type aliases:

- `HttpFetcher` (= `Fetcher<reqwest::blocking::Client>`) — implements `ResourceFetcher` for `Loader::register_fetcher`.
- `AsyncHttpFetcher` (= `Fetcher<reqwest::Client>`) — implements `AsyncResourceFetcher` for `Loader::register_async_fetcher`. A tokio runtime must be active when the returned future is awaited.

Both forms are `Clone` so a single fetcher can be registered for both `http://` and `https://` prefixes on the same `Loader`, sharing one underlying connection pool. Schemes other than `http` / `https` are rejected with `LoaderError::UnsupportedFetcherUri`. The default constructor builds a client with a 30-second request timeout; `try_new()` is the fallible variant that surfaces TLS / IO environment failures from `reqwest::ClientBuilder`.

## Features

- (default) JSON response bodies are parsed with `serde_json`.
- `yaml` — also accept YAML response bodies. The fetcher sniffs `Content-Type`
  first (`application/yaml`, `application/x-yaml`, `text/yaml`, etc.) and falls
  back to the URL path extension (`.yaml` / `.yml`). Pulls in `serde_yaml_ng`.

```toml
[dependencies]
roas-http-fetcher = { version = "0.2.5", features = ["yaml"] }
```

## Usage

```shell
cargo add roas-http-fetcher
```

```rust
use roas::loader::Loader;
use roas_http_fetcher::HttpFetcher;

let mut loader = Loader::new();
let http = HttpFetcher::new();
loader.register_fetcher("https://", http.clone());
loader.register_fetcher("http://", http);
```

For async loaders use `AsyncHttpFetcher` with `register_async_fetcher` instead — same shape:

```rust
use roas::loader::Loader;
use roas_http_fetcher::AsyncHttpFetcher;

let mut loader = Loader::new();
let http = AsyncHttpFetcher::new();
loader.register_async_fetcher("https://", http.clone());
loader.register_async_fetcher("http://", http);
```

A non-2xx HTTP response, transport failure, or unreadable body is surfaced through
[`LoaderError::Fetch`](https://docs.rs/roas/latest/roas/loader/enum.LoaderError.html) with a
[`HttpFetchError`](https://docs.rs/roas-http-fetcher/latest/roas_http_fetcher/enum.HttpFetchError.html) source.

The fetchers also implement `fetch_document`, used by `Loader::load_document` and
`load_document_async`, to preserve the final response URL after redirects alongside
the unchanged value. With `yaml`, a missing, empty or octet-stream Content-Type
uses a YAML extension on either the requested or final URL. Thus a `.yaml` URL
redirecting to an extensionless blob retains its original format hint, and an
extensionless URL redirecting to `.yaml` works too. Explicit Content-Type still
takes precedence over both URLs; disabling `yaml` still means JSON only. `with_client`
continues to honor the supplied client's redirect, timeout, proxy and TLS policies;
metadata collection does not install a different client or enable more URI schemes.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
