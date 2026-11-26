# roas-file-fetcher

Filesystem [`ResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.ResourceFetcher.html) and
[`AsyncResourceFetcher`](https://docs.rs/roas/latest/roas/loader/trait.AsyncResourceFetcher.html) for the
[`roas`](https://crates.io/crates/roas) OpenAPI loader.

[![crates.io](https://img.shields.io/crates/v/roas-file-fetcher.svg)](https://crates.io/crates/roas-file-fetcher)
[![docs.rs](https://docs.rs/roas-file-fetcher/badge.svg)](https://docs.rs/roas-file-fetcher)

`FileFetcher` is blocking and reads regular files up to a configurable byte cap (default 64 MiB), for `Loader::register_fetcher`. Non-`file://` URIs are rejected with `LoaderError::UnsupportedFetcherUri`; invalid or remote-host `file://` URIs (such as UNC hosts) are rejected with `LoaderError::InvalidFileUri`. I/O and policy failures (missing files, non-regular files, size limits, root escapes, refused symlinks) surface as `LoaderError::ReadFile`; body parse errors surface as `LoaderError::Parse`. Use `with_root`, `follow_symlinks(false)`, and `with_max_bytes` to harden reads for untrusted inputs.

## Features

- (default) Sync-only, JSON bodies, no tokio dep.
- `async` — also expose `AsyncFileFetcher`, running filesystem I/O and JSON/YAML parsing on Tokio's blocking pool via `tokio::task::spawn_blocking`, for `Loader::register_async_fetcher`. Requires an active tokio runtime when the returned future is awaited. Pulls in `tokio` with the `rt` feature.
- `yaml` — accept YAML file bodies in addition to JSON. Selection is by decoded file path extension (`.yaml` / `.yml`). Pulls in `serde-saphyr`.

```toml
[dependencies]
roas-file-fetcher = { version = "0.3", features = ["async", "yaml"] }
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

`fetch_document_text` (on both fetchers) returns the parsed document together with
the text it was parsed from, for callers that report positions in it.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT) at your option.
