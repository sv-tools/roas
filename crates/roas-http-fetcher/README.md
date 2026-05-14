# roas-http-fetcher

HTTP / HTTPS resource fetcher for [`roas`](https://crates.io/crates/roas)'s external-reference loader.

[![crates.io](https://img.shields.io/crates/v/roas-http-fetcher.svg)](https://crates.io/crates/roas-http-fetcher)

Wraps `reqwest::blocking::Client` and implements `roas::loader::ResourceFetcher`, so external `$ref`s with
`http://` or `https://` schemes can be resolved by `roas::loader::Loader`.

## Usage

```shell
cargo add roas-http-fetcher
```

```rust
use roas::loader::Loader;
use roas_http_fetcher::HttpFetcher;

let mut loader = Loader::new();
loader.register_fetcher("https://", HttpFetcher::new());
loader.register_fetcher("http://", HttpFetcher::new());
```

For custom headers, timeouts, proxies, or alternative TLS configuration, build a `reqwest::blocking::Client`
yourself and hand it to `HttpFetcher::with_client`:

```rust
use std::time::Duration;
use roas_http_fetcher::HttpFetcher;

let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(10))
    .build()
    .unwrap();
let fetcher = HttpFetcher::with_client(client);
```

## See also

- [`roas`](https://crates.io/crates/roas) — the library this fetcher plugs into.
- [`roas-cli`](https://crates.io/crates/roas-cli) — the command-line front-end (uses this fetcher for `--load http`).
