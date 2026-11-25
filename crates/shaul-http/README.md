# shaul-http

[![crates.io](https://img.shields.io/crates/v/shaul-http.svg)](https://crates.io/crates/shaul-http)
[![docs.rs](https://docs.rs/shaul-http/badge.svg)](https://docs.rs/shaul-http)

**English** | [简体中文](README.zh-CN.md)

Direct-link HTTP backend for [shaul](https://github.com/hushink/shaul), a
multi-protocol download engine.

## Features

- Segmented downloads over HTTP range requests
- Work-stealing range queue with adaptive segment sizing
- Resumable transfers with `ETag`, `Last-Modified` and size validators
- Positioned writes into one preallocated `.part` file
- `If-Range`, `416` and `200 + Content-Range` handling
- Single-stream fallback when a server ignores `Range`
- Manual redirect handling with cross-origin credential stripping
- Non-GET requests (`POST`/`PUT`/`PATCH`, ...) with an optional body; sent once
  without segmentation, resume or mirrors
- Custom headers and bearer, basic or header authentication
- HTTP/HTTPS/SOCKS5 proxy, global/per-host/per-task bandwidth limits and
  active request budgets
- SHA-256/SHA-512/SHA-1/MD5 checksums, `Digest` and `Content-MD5`
- Opt-in content-addressed cache hits when checksum and size are known
- `Content-Disposition` filename support
- Cancellation that pauses with a checkpoint instead of losing partial data
- Response-header timeouts and fsync-before-checkpoint durability
- Proxy modes that distinguish ambient settings from an explicit direct connection

## Usage

The easiest entry point is the [`shaul`](https://crates.io/crates/shaul)
facade. To embed only the HTTP stack:

```rust,no_run
use shaul_core::Config;

fn main() -> shaul_core::Result<()> {
    let config = Config::default();
    let router = shaul_http::default_router(&config)?;
    let _ = router;
    Ok(())
}
```

`HttpBackend` and `DirectLinkParser` can also be registered in a custom
`ParserRegistry` or `Router`.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [API reference](https://docs.rs/shaul-http)
