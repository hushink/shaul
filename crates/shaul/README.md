# shaul

[![crates.io](https://img.shields.io/crates/v/shaul.svg)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](https://github.com/hushink/shaul/blob/main/Cargo.toml)

A unified, resumable download engine for direct HTTP links, Hugging Face,
ModelScope, BitTorrent, FTP and optional Hugging Face Xet.

`shaul` is the facade crate. It assembles the parser → router → backend
pipeline from [`shaul-core`](https://crates.io/crates/shaul-core),
[`shaul-http`](https://crates.io/crates/shaul-http),
[`shaul-hub`](https://crates.io/crates/shaul-hub) and, when enabled,
[`shaul-bt`](https://crates.io/crates/shaul-bt) and
[`shaul-ftp`](https://crates.io/crates/shaul-ftp).

## Install

```toml
[dependencies]
shaul = "1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

# Optional protocol support:
# shaul = { version = "1.2", features = ["bt", "ftp", "xet"] }
```

| Feature | Adds |
| --- | --- |
| `bt` | BitTorrent, magnet links and `.torrent` files |
| `ftp` | Single-stream resumable `ftp://` downloads |
| `xet` | Hugging Face Xet downloads with HTTP fallback |

## Example

```rust,no_run
use shaul::{Config, Link, TaskOptions};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = shaul::default_engine(Config::default())?;

    let options = TaskOptions {
        connections: Some(8),
        extract: Some(true),
        ..TaskOptions::default()
    };

    let task = engine
        .add(
            Link::new("https://example.com/data.tar.gz")?,
            PathBuf::from("./downloads"),
            options,
        )
        .await?;

    engine.run_task(task.id).await?;
    Ok(())
}
```

Subscribe to `engine.subscribe()` for typed progress events. If you do not
want to own a Tokio runtime, use `default_blocking_engine`.

## Capabilities

- Segmented HTTP downloads with a work-stealing range queue and adaptive
  segment sizing
- Safe resume with `ETag`, `Last-Modified` and size validation
- Custom headers and bearer, basic or header authentication
- HTTP/HTTPS/SOCKS5 proxy with `NO_PROXY`
- Global, per-host and per-task bandwidth limits; active request budgets
- Compatible mirror fallback
- SHA-256/SHA-512/SHA-1/MD5 verification, toggleable per download
- Archive extraction after verification, with a pluggable extractor API
- Hugging Face and ModelScope repositories with optional provider caches
- Optional Xet downloads for Hugging Face files with `xetHash`
- BitTorrent, magnet and `.torrent` support through the `bt` feature
- Single-stream `ftp://` downloads with `SIZE`/`MDTM`/`REST` resume
- Concurrent multi-file downloads
- Opt-in content-addressed cache for verified files
- Persistent queue with priorities, pause/resume/cancel and retry events
- Typed events, NDJSON output and low-cardinality telemetry metrics

## Extending

Implement `LinkParser` and/or `DownloadBackend`, then assemble a `Router` with
`ParserRegistry`. Archive formats are extensible through `ArchiveExtractor` and
`ExtractorRegistry`. Extension is compile-time; no unstable plugin ABI is used.

## CLI

The companion [`shaul-cli`](https://crates.io/crates/shaul-cli) crate installs
the `shaul` command:

```console
$ cargo install shaul-cli
```

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [Extension guide](https://github.com/hushink/shaul/blob/main/docs/extending.md)
- [Resume and hardening](https://github.com/hushink/shaul/blob/main/docs/resume.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul)
