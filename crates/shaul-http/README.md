# shaul-http

Direct-link HTTP backend for [shaul](https://github.com/hushink/shaul), a
multi-protocol download engine.

## Features

- segmented downloads over HTTP range requests
- resumable transfers with ETag / Last-Modified / size validators
- positioned writes into a single `.part` file (no fragment merge step)
- `If-Range`, `416`, and `200 + Content-Range` handling
- single-stream fallback when a server ignores `Range`
- manual redirect handling with cross-origin credential stripping
- custom headers, bearer/basic/header authentication
- HTTP/HTTPS/SOCKS5 proxy support, global/per-task bandwidth limits and
  global/per-host active request budgets
- SHA-256/SHA-512/SHA-1/MD5 checksums, `Digest` and `Content-MD5`
- `Content-Disposition` filename support

## Usage

The easiest entry point is the [`shaul`](https://crates.io/crates/shaul)
facade. To embed only the HTTP stack:

```rust,no_run
use shaul_core::Config;

# fn main() -> shaul_core::Result<()> {
let config = Config::default();
let router = shaul_http::default_router(&config)?;
# Ok(())
# }
```

`HttpBackend` and `DirectLinkParser` can also be registered in a custom
`ParserRegistry` / `Router`.

## Links

- [Project README](https://github.com/hushink/shaul/blob/main/README.md)
- [API audit](https://github.com/hushink/shaul/blob/main/docs/api-audit.md)

MSRV is Rust 1.92. License: Apache-2.0.
