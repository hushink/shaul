# shaul-core

[![crates.io](https://img.shields.io/crates/v/shaul-core.svg)](https://crates.io/crates/shaul-core)
[![docs.rs](https://docs.rs/shaul-core/badge.svg)](https://docs.rs/shaul-core)

**English** | [简体中文](README.zh-CN.md)

Core domain types and download engine for
[shaul](https://github.com/hushink/shaul), a multi-protocol download engine for
HTTP, BitTorrent, Hugging Face, ModelScope and FTP.

`shaul-core` is protocol-agnostic. It defines the common pipeline:

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
```

`DownloadEngine` drives tasks, persists resumable state, emits typed events
and runs verification and archive-extraction finalizers.

## Included

- `Link`, `Resource`, `Router` and `ParserRegistry`
- `LinkParser` and `DownloadBackend` extension traits
- Task sidecars (`SidecarStore`), atomic writes and cross-process run locks
- Resume manifests and validators with stale-task recovery
- Segmented-range arithmetic and SHA-256/SHA-512/SHA-1/MD5 checksums
- Safe relative-path handling and archive detection/extraction
- Failed-verification quarantine (`<name>.shaul.failed`)
- Strict configuration parsing that rejects unknown keys and empty paths
- Verified content-addressed cache (`ContentCache`) with layout guards
- Async and blocking engine facades
- Structured `DownloadEvent` stream and low-cardinality telemetry hooks

## When to use it

Most applications should use the [`shaul`](https://crates.io/crates/shaul)
facade, which assembles HTTP, Hub and optional BitTorrent/FTP/Xet backends.

Use `shaul-core` directly when you are building a custom parser, backend or
router, or when you want to embed only the engine and task store.

```toml
[dependencies]
shaul-core = "1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Example

```rust,no_run
use shaul_core::{Config, SidecarStore, TaskStore};

#[tokio::main]
async fn main() -> shaul_core::Result<()> {
    let config = Config::default();
    let store = SidecarStore::new(config.state_dir()?)?;
    let tasks = store.list().await?;
    println!("{} task(s)", tasks.len());
    Ok(())
}
```

Build a `Router` from your parsers and backends, then pass it to
`DownloadEngine::new` together with a `TaskStore` and `Config`.

## Documentation

- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [Extension guide](https://github.com/hushink/shaul/blob/main/docs/extending.md)
- [Resume and hardening](https://github.com/hushink/shaul/blob/main/docs/resume.md)
- [API reference](https://docs.rs/shaul-core)
