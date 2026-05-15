# roas-file-fetcher

Filesystem [`ResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.ResourceFetcher.html) and
[`AsyncResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.AsyncResourceFetcher.html) for the
[`roas`](https://crates.io/crates/roas) OpenAPI loader.

[![crates.io](https://img.shields.io/crates/v/roas-file-fetcher.svg)](https://crates.io/crates/roas-file-fetcher)
[![docs.rs](https://docs.rs/roas-file-fetcher/badge.svg)](https://docs.rs/roas-file-fetcher)

Two zero-sized fetcher types share the same API:

- `FileFetcher` — blocking, backed by `std::fs::read`, for `Loader::register_fetcher`.
- `AsyncFileFetcher` — async, backed by `tokio::fs::read`, for `Loader::register_async_fetcher`. Requires a tokio runtime when the returned future is awaited.

Both reject anything other than `file://` URIs with `LoaderError::UnsupportedFetcherUri`. I/O errors surface as `LoaderError::ReadFile`; body parse errors surface as `LoaderError::Parse`.

## Features

- (default) JSON file bodies are parsed with `serde_json`.
- `yaml` — also accept YAML file bodies. Format selection is by file path extension (`.yaml` / `.yml`). Pulls in `serde_yaml_ng`.

```toml
[dependencies]
roas-file-fetcher = { version = "0.1", features = ["yaml"] }
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

```rust
use roas::loader::Loader;
use roas_file_fetcher::AsyncFileFetcher;

let mut loader = Loader::new();
loader.register_async_fetcher("file://", AsyncFileFetcher::new());
```

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at
your option.
