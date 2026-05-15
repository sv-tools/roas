# roas-http-fetcher

HTTP/HTTPS [`ResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.ResourceFetcher.html) and
[`AsyncResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.AsyncResourceFetcher.html) for the
[`roas`](https://crates.io/crates/roas) OpenAPI loader.

[![crates.io](https://img.shields.io/crates/v/roas-http-fetcher.svg)](https://crates.io/crates/roas-http-fetcher)
[![docs.rs](https://docs.rs/roas-http-fetcher/badge.svg)](https://docs.rs/roas-http-fetcher)

Built on `reqwest` with `rustls-tls`. The crate exposes two fetchers:

- `HttpFetcher` — `reqwest::blocking::Client`, for `Loader::register_fetcher`.
- `AsyncHttpFetcher` — `reqwest::Client`, for `Loader::register_async_fetcher`. Requires a tokio runtime when awaited.

Both are `Clone` so a single underlying connection pool can be shared across `http://` and `https://` registrations.

## Features

- (default) JSON response bodies are parsed with `serde_json`.
- `yaml` — also accept YAML response bodies. The fetcher sniffs `Content-Type`
  first (`application/yaml`, `application/x-yaml`, `text/yaml`, etc.) and falls
  back to the URL path extension (`.yaml` / `.yml`). Pulls in `serde_yaml_ng`.

```toml
[dependencies]
roas-http-fetcher = { version = "0.1", features = ["yaml"] }
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

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at
your option.
