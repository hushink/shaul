# shaul

A unified, resumable download engine for direct HTTP links, Hugging Face,
ModelScope, BitTorrent and magnet links.

`shaul` is the facade crate. It assembles the parser/router/backend pipeline
from [`shaul-core`](https://crates.io/crates/shaul-core),
[`shaul-http`](https://crates.io/crates/shaul-http),
[`shaul-hub`](https://crates.io/crates/shaul-hub) and optionally
[`shaul-bt`](https://crates.io/crates/shaul-bt).

## Install

```toml
[dependencies]
shaul = "1.0"
# BitTorrent support:
# shaul = { version = "1.0", features = ["bt"] }
```

MSRV is Rust 1.92.

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
            PathBuf::from("."),
            options,
        )
        .await?;
    engine.run_task(task.id).await?;
    Ok(())
}
```

Subscribe to `engine.subscribe()` for typed events. If you do not want to own a
Tokio runtime, use `default_blocking_engine`. The
[library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
covers events, the persistent queue, cancellation, telemetry and tracing, with
runnable examples in
[`crates/shaul/examples`](https://github.com/hushink/shaul/tree/main/crates/shaul/examples).

## Capabilities

- segmented HTTP downloads with work-stealing range scheduling
- resume with ETag / Last-Modified / size validation
- custom headers and bearer/basic/header authentication
- SHA-256/SHA-512/SHA-1/MD5 verification, toggleable per download
- archive extraction (zip, tar, tar.gz, tar.zst, gzip, zstd) after verification
- Hugging Face and ModelScope repositories with optional provider caches
- BitTorrent / magnet / `.torrent` via the optional `bt` feature
- persistent task queue, pause/resume/cancel, NDJSON events

## Extending

Implement `LinkParser` and/or `DownloadBackend`, then assemble a `Router` with
`ParserRegistry`. Extension is compile-time; no unstable plugin ABI is used.

## CLI

The companion [`shaul-cli`](https://crates.io/crates/shaul-cli) crate installs
the `shaul` command:

```console
$ cargo install shaul-cli
```

## Links

- [Project README](https://github.com/hushink/shaul/blob/main/README.md)
- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [Extension guide](https://github.com/hushink/shaul/blob/main/docs/extending.md)
- [API audit](https://github.com/hushink/shaul/blob/main/docs/api-audit.md)
- [Release process](https://github.com/hushink/shaul/blob/main/docs/release.md)

License: Apache-2.0.
