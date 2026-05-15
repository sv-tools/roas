# roas-file-fetcher

Filesystem [`ResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.ResourceFetcher.html) and
[`AsyncResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.AsyncResourceFetcher.html) for the
[`roas`](https://crates.io/crates/roas) OpenAPI loader.

[![crates.io](https://img.shields.io/crates/v/roas-file-fetcher.svg)](https://crates.io/crates/roas-file-fetcher)
[![docs.rs](https://docs.rs/roas-file-fetcher/badge.svg)](https://docs.rs/roas-file-fetcher)

`FileFetcher` is blocking, backed by `std::fs::read`, for `Loader::register_fetcher`. Non-`file://` URIs are rejected with `LoaderError::UnsupportedFetcherUri`. I/O errors surface as `LoaderError::ReadFile`; body parse errors surface as `LoaderError::Parse`.

## Features

- (default) Sync-only, JSON bodies, no tokio dep.
- `async` — also expose `AsyncFileFetcher`, backed by `tokio::fs::read`, for `Loader::register_async_fetcher`. Requires an active tokio runtime when the returned future is awaited. Pulls in `tokio` with `fs` + `rt` features.
- `yaml` — accept YAML file bodies in addition to JSON. Selection is by file path extension (`.yaml` / `.yml`). Pulls in `serde_yaml_ng`.

```toml
[dependencies]
roas-file-fetcher = { version = "0.1", features = ["async", "yaml"] }
```

## Usage

```shell
cargo add roas-file-fetcher
```

```rust
use roas::loader::Loader;
use roas_file_fetcher::FileFetcher;

let mut loader = Loader::new();
loader.register_fetcher("file://", FileFetcher::new());
```

With the `async` feature enabled:

```rust
use roas::loader::Loader;
use roas_file_fetcher::AsyncFileFetcher;

let mut loader = Loader::new();
loader.register_async_fetcher("file://", AsyncFileFetcher::new());
```

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at
your option.
