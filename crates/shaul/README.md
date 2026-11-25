# shaul

[![crates.io](https://img.shields.io/crates/v/shaul.svg)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](https://github.com/hushink/shaul/blob/main/Cargo.toml)

**English** | [简体中文](README.zh-CN.md)

A unified, resumable download engine for direct HTTP links, HLS, Hugging Face,
ModelScope, BitTorrent, FTP and optional Hugging Face Xet.

`shaul` is the facade crate. It assembles the parser → router → backend
pipeline from [`shaul-core`](https://crates.io/crates/shaul-core),
[`shaul-http`](https://crates.io/crates/shaul-http),
[`shaul-hub`](https://crates.io/crates/shaul-hub) and, when enabled,
[`shaul-bt`](https://crates.io/crates/shaul-bt),
[`shaul-ftp`](https://crates.io/crates/shaul-ftp) and
[`shaul-hls`](https://crates.io/crates/shaul-hls).

## Install

```toml
[dependencies]
shaul = "1.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

# Optional protocol support:
# shaul = { version = "1.2", features = ["bt", "ftp", "hls", "xet"] }
```

| Feature | Adds |
| --- | --- |
| `bt` | BitTorrent, magnet links and `.torrent` files |
| `ftp` | Single-stream resumable `ftp://` downloads |
| `hls` | VOD HLS (`.m3u8`) playlists, concatenated into one file |
| `xet` | Hugging Face Xet downloads with HTTP fallback |

HLS phase 1 supports TS and single-init-map fMP4 playlists, segment-level
resume and concurrent segment downloads. AES-128, `#EXT-X-BYTERANGE`, live
playlists and DASH are not supported yet.

## Quick start

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

    let task = engine.run_task(task.id).await?;
    println!("{:?}: {}", task.state, task.downloaded_bytes());
    Ok(())
}
```

`Config::default()` is inert: it reads no config file and no environment
variables. Use `Config::load` when you want the CLI's layering
(files, `SHAUL_*`, provider environment variables).

## Configuration

```rust,no_run
use shaul::Config;

fn build() -> shaul::Result<()> {
    // defaults -> environment -> config file (or $SHAUL_CONFIG / an explicit path)
    let (config, path) = Config::load(None)?;
    println!("config: {}", path.display());

    // Apply application flags here so your precedence stays:
    // flags > config file > environment > defaults.
    let (engine, _store) = shaul::default_engine(config)?;
    let _ = engine;
    Ok(())
}
```

Useful switches:

```rust,no_run
use shaul::{Config, TaskOptions};

fn configure() -> shaul::Result<(Config, TaskOptions)> {
    let mut config = Config::default();
    config.network.proxy = Some("socks5://127.0.0.1:1080".to_string());
    config.network.no_proxy = Some("localhost,127.0.0.1".to_string());
    // An empty proxy string disables ambient HTTP(S)_PROXY settings:
    // config.network.proxy = Some(String::new());

    config.download.rate_limit = Some(8 * 1024 * 1024); // global bytes/sec
    config.download.per_host_rate_limit = Some(2 * 1024 * 1024);
    config.download.file_jobs = 4; // files inside one multi-file task
    config.download.global = 32; // active HTTP transfers across tasks
    config.download.per_host_active = 8; // active HTTP transfers per host
    config.download.content_cache = true;

    let options = TaskOptions {
        rate_limit: Some(1024 * 1024), // additional per-task cap
        file_jobs: Some(4),
        ..TaskOptions::default()
    };
    Ok((config, options))
}
```

## Observe progress and lifecycle events

`engine.subscribe()` returns a `tokio::sync::broadcast::Receiver<DownloadEvent>`.
Delivery is best effort: a slow subscriber may miss events, but the task state
in the store remains authoritative.

```rust,no_run
use std::path::PathBuf;
use std::time::Duration;

use shaul::{Config, EventKind, Link, TaskOptions, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;
    let mut events = engine.subscribe();

    let listener = tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    eprintln!("event stream lagged by {missed} events");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            match event.kind {
                EventKind::TaskStarted { task_id, .. } => {
                    println!("[{task_id}] started");
                }
                EventKind::FileStarted {
                    file,
                    total_bytes,
                    ..
                } => {
                    println!("{file}: starting ({total_bytes:?} bytes)");
                }
                EventKind::Progress {
                    file,
                    downloaded_bytes,
                    total_bytes,
                    ..
                } => {
                    println!("{file}: {downloaded_bytes}/{total_bytes:?}");
                }
                EventKind::Retry {
                    file,
                    attempt,
                    error,
                    ..
                } => {
                    eprintln!("{file}: retry #{attempt}: {error}");
                }
                EventKind::VerificationCompleted { ok, actual, .. } => {
                    println!("verified={ok} digest={actual}");
                }
                EventKind::ExtractionCompleted {
                    target,
                    files_extracted,
                    ..
                } => {
                    println!("extracted {files_extracted} file(s) to {}", target.display());
                }
                EventKind::TaskCompleted { bytes, files, .. } => {
                    println!("completed: {files} file(s), {bytes} bytes");
                    break;
                }
                EventKind::TaskFailed { error, .. } => {
                    eprintln!("failed: {error}");
                    break;
                }
                EventKind::TaskPaused { .. } | EventKind::TaskCancelled { .. } => break,
                _ => {}
            }
        }
    });

    let task = engine
        .add(
            Link::new("https://example.com/file.bin")?,
            PathBuf::from("./downloads"),
            TaskOptions {
                file_jobs: Some(4),
                ..TaskOptions::default()
            },
        )
        .await?;

    // Cancel after 10 seconds as a demonstration.
    let cancel = shaul::CancellationToken::new();
    let stopper = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            cancel.cancel();
        })
    };
    let _task = engine.run_task_with_cancel(task.id, Some(cancel)).await?;
    stopper.abort();
    let _ = listener.await;
    Ok(())
}
```

The same event stream drives the CLI's progress bars and `--json` NDJSON output.
For library callers that only need counters, see [Telemetry](#telemetry).

## Blocking facade

Use `default_blocking_engine` when your program does not own a Tokio runtime.
It creates one internally and exposes synchronous methods.

```rust,no_run
use std::path::PathBuf;

use shaul::{Config, DownloadEvent, EventKind, Link, TaskOptions};

fn main() -> shaul::Result<()> {
    let engine = shaul::default_blocking_engine(Config::default())?;

    // std::sync::mpsc::Receiver<DownloadEvent>
    let events = engine.subscribe();
    std::thread::spawn(move || {
        for DownloadEvent { kind, .. } in events {
            if let EventKind::Progress {
                file,
                downloaded_bytes,
                total_bytes,
                ..
            } = kind
            {
                println!("{file}: {downloaded_bytes}/{total_bytes:?}");
            }
        }
    });

    let task = engine.download(
        Link::new("https://example.com/file.bin")?,
        PathBuf::from("./downloads"),
        TaskOptions::default(),
    )?;
    println!("{:?}", task.state);
    Ok(())
}
```

## Persistent queue, pause/resume and cancellation

Tasks are persisted as JSON sidecars. The store deduplicates active tasks by
canonical link + destination, and every running task already holding a
cross-process lock cannot be started a second time. `requeue_stale` recovers
`Running` tasks whose OS lock was released by a crashed process.

```rust,no_run
use std::path::PathBuf;
use std::time::Duration;

use shaul::{CancellationToken, Config, Link, TaskOptions, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;

    // After a crash, requeue tasks left in Running whose runner lock is gone.
    engine.requeue_stale().await?;

    let destination = PathBuf::from("./downloads");
    let task = engine
        .add(
            Link::new("https://example.com/a.bin")?,
            destination.clone(),
            TaskOptions {
                priority: Some(10),
                ..TaskOptions::default()
            },
        )
        .await?;

    let paused = engine.pause(task.id).await?;
    println!("paused: {:?}", paused.state);
    let resumed = engine.resume(task.id).await?;
    println!("resumed: {:?}", resumed.state);

    // Run with an external cancellation token.
    let cancel = CancellationToken::new();
    let stop = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            cancel.cancel();
        })
    };
    let task = engine.run_task_with_cancel(task.id, Some(cancel)).await?;
    stop.abort();
    println!("final: {:?}", task.state);

    // Process pending tasks, at most 4 concurrently.
    let completed = engine.run_pending(4, CancellationToken::new()).await?;
    println!("completed {} task(s)", completed.len());

    for task in engine.list().await? {
        println!("{} {:?} {}", task.id, task.state, task.link);
    }
    Ok(())
}
```

Cancelling a segmented transfer pauses it (state `Paused`) and keeps the
checkpointed `.part` file; run it again to resume. A task locked by a live
runner returns `invalid task state: already running`.

## Verification, extraction and the content cache

```rust,no_run
use std::path::PathBuf;

use shaul::{Checksum, Config, Link, TaskOptions, VerifyMode, default_engine};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;

    let task = engine
        .add(
            Link::new("https://example.com/model.tar.gz")?,
            PathBuf::from("./downloads"),
            TaskOptions {
                checksum: Some(Checksum::parse("sha256:<hex>")?),
                verify: Some(VerifyMode::Require),
                extract: Some(true),
                extract_dir: Some(PathBuf::from("./extracted")),
                delete_archive: Some(true),
                content_cache: Some(true),
                ..TaskOptions::default()
            },
        )
        .await?;

    engine.run_task(task.id).await?;
    Ok(())
}
```

- `VerifyMode::Auto` verifies when a checksum is known; `Require` fails without
  one; `Disabled` skips optional verification.
- Verification runs before extraction. A terminal checksum mismatch quarantines
  the output as `<name>.shaul.failed`.
- `--rename`/`TaskOptions::rename_on_conflict` suffixes an existing extraction
  target (`name-1`, ...) instead of failing.
- The content cache only stores files with a known checksum and size; hits are
  re-hashed before use.

```rust,no_run
use shaul::{Config, ContentCache};

#[tokio::main]
async fn main() -> shaul::Result<()> {
    let config = Config::default();
    let cache = ContentCache::new(config.content_cache_dir()?)?;
    let stats = cache.stats().await?;
    println!("{} entries, {} bytes", stats.entries, stats.bytes);

    // Destructive maintenance refuses directories that do not look like a cache.
    let report = cache.prune(None, Some(50 * 1024 * 1024 * 1024)).await?;
    println!("removed {} entries", report.entries_removed);
    Ok(())
}
```

## Build a custom parser and backend

A parser turns a `Link` into a protocol-agnostic `Resource`; a backend consumes
that resource. Register both in a `Router` and pass it to `DownloadEngine`.
`async-trait` is required to implement the async traits.

```rust,no_run
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use shaul::{
    BackendCapabilities, Config, DownloadBackend, DownloadEngine, DownloadOutcome,
    DownloadRequest, Error, EventKind, FileOutcome, Link, LinkParser, ParseOptions,
    ParserRegistry, Resource, ResourceFile, ResourceKind, Result, Router, SidecarStore,
    TaskStore, backend_arc, parser_arc,
};
use url::Url;

struct MemoryParser;

#[async_trait]
impl LinkParser for MemoryParser {
    fn name(&self) -> &'static str {
        "memory-demo"
    }

    fn supports(&self, link: &Link) -> bool {
        link.as_str().starts_with("mem://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let name = link.as_str().trim_start_matches("mem://").trim_matches('/');
        if name.is_empty() || name.contains("..") {
            return Err(Error::InvalidLink(link.to_string()));
        }
        Ok(Resource {
            kind: ResourceKind::Custom {
                protocol: "mem".to_string(),
            },
            name: name.to_string(),
            files: vec![ResourceFile {
                path: name.to_string(),
                url: Url::parse(&format!("mem://{name}"))
                    .map_err(|_| Error::InvalidLink(link.to_string()))?,
                size: Some(5),
                checksum: None,
                headers: Vec::new(),
                metadata: BTreeMap::new(),
                use_response_filename: false,
            }],
            request: options.request.clone(),
            checksum: None,
            revision: None,
            metadata: BTreeMap::new(),
        })
    }
}

struct MemoryBackend;

#[async_trait]
impl DownloadBackend for MemoryBackend {
    fn name(&self) -> &'static str {
        "memory-demo"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, false, true)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(&resource.kind, ResourceKind::Custom { protocol } if protocol == "mem")
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let payload = b"hello";
        let mut outcomes = Vec::new();
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let path = request.destination.join(&file.path);
            tokio::fs::write(&path, payload).await?;
            request.events.emit(EventKind::Progress {
                task_id: request.task_id,
                file: file.path.clone(),
                downloaded_bytes: payload.len() as u64,
                total_bytes: Some(payload.len() as u64),
            });
            outcomes.push(FileOutcome::new(
                path,
                file.path.clone(),
                payload.len() as u64,
                None,
                false,
            ));
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = Config::default();
    config.state_dir = Some("./state".into());

    let parsers = ParserRegistry::new().with(parser_arc(MemoryParser));
    let router = Router::new(parsers, vec![backend_arc(MemoryBackend)]);
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(config.state_dir()?)?);
    let engine = DownloadEngine::new(Arc::new(router), store, config);

    let task = engine
        .add(Link::new("mem://hello.txt")?, "./downloads".into(), Default::default())
        .await?;
    engine.run_task(task.id).await?;
    Ok(())
}
```

Backends must honor `DownloadRequest::cancel`, emit progress through
`DownloadRequest::events`, and persist resumable progress through
`DownloadRequest::checkpoint`. See `crates/shaul/examples/custom_protocol.rs`
and `custom_parser.rs` for complete runnable versions.

## Custom finalizers and archive extractors

`FinalizerPipeline` runs after a transfer completes. The default pipeline is
`VerifyFinalizer` followed by `ExtractFinalizer`.

```rust,no_run
use std::sync::Arc;

use async_trait::async_trait;
use shaul::{
    ArchiveExtractor, ExtractReport, ExtractorRegistry, ExtractorRequest, FinalizeContext,
    FinalizeResult, Finalizer, FinalizerPipeline, Result, VerifyFinalizer,
};

struct AuditFinalizer;

#[async_trait]
impl Finalizer for AuditFinalizer {
    fn name(&self) -> &'static str {
        "audit"
    }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        println!(
            "verified={} file={}",
            context.file.checksum_verified, context.file.relative_path
        );
        Ok(FinalizeResult::default())
    }
}

struct DemoExtractor;

impl ArchiveExtractor for DemoExtractor {
    fn name(&self) -> &'static str {
        "demo"
    }

    fn supports(&self, path: &std::path::Path) -> Result<bool> {
        Ok(path.extension().and_then(|ext| ext.to_str()) == Some("demo"))
    }

    fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
        let target = request.extract_root.join("demo");
        std::fs::create_dir_all(&target)?;
        std::fs::copy(request.path, target.join("payload.txt"))?;
        Ok(ExtractReport {
            target,
            files_extracted: 1,
            warnings: Vec::new(),
        })
    }
}

fn install(engine: &mut shaul::DownloadEngine) {
    let registry = Arc::new(ExtractorRegistry::default().with(Arc::new(DemoExtractor)));
    engine.set_finalizers(
        FinalizerPipeline::new(vec![
            Arc::new(VerifyFinalizer),
            Arc::new(AuditFinalizer),
            Arc::new(shaul::ExtractorFinalizer::new(registry)),
        ]),
    );
}
```

Custom extractors must keep every write under `ExtractorRequest::extract_root`,
reject unsafe entry paths and honor `ExtractorRequest::rename_on_conflict`.
The registry canonicalizes the returned target and rejects escapes.

## Telemetry

`Telemetry` receives low-cardinality counters, gauges and histograms. Every
method has a no-op default; implementations must not block the executor.

```rust,no_run
use std::sync::Arc;

use shaul::{Telemetry, metrics};

struct Metrics;

impl Telemetry for Metrics {
    fn counter(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
        eprintln!("counter {name}={value} {labels:?}");
    }

    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        eprintln!("gauge {name}={value} {labels:?}");
    }

    fn histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        eprintln!("histogram {name}={value} {labels:?}");
    }
}

fn attach(mut engine: shaul::DownloadEngine) -> shaul::DownloadEngine {
    engine.set_telemetry(Arc::new(Metrics));
    // Stable names include metrics::TASKS_COMPLETED, metrics::BYTES_DOWNLOADED,
    // metrics::ACTIVE_CONNECTIONS, metrics::CACHE_HITS and metrics::QUEUE_DEPTH.
    engine
}
```

## Capabilities

- Segmented HTTP downloads with a work-stealing range queue and adaptive
  segment sizing
- Safe resume with `ETag`, `Last-Modified` and size validation
- Custom headers and bearer, basic or header authentication
- HTTP/HTTPS/SOCKS5 proxy with `NO_PROXY`; an empty proxy value disables ambient
  proxy variables
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

## Examples

| Example | Shows |
| --- | --- |
| [`library_basic`](examples/library_basic.rs) | Build the default engine, add and run one task |
| [`library_events`](examples/library_events.rs) | Consume the typed `EventKind` stream |
| [`library_blocking`](examples/library_blocking.rs) | Use the synchronous facade |
| [`library_queue`](examples/library_queue.rs) | Persist tasks, pause/resume and run a queue |
| [`custom_parser`](examples/custom_parser.rs) | Offline custom parser |
| [`custom_protocol`](examples/custom_protocol.rs) | Custom parser + backend + telemetry |
| [`custom_extractor`](examples/custom_extractor.rs) | Custom archive format |

```console
$ cargo run -p shaul --example library_events -- https://example.com/file.bin .
$ cargo run -p shaul --example custom_protocol
```

## Extending

Implement `LinkParser` and/or `DownloadBackend`, then assemble a `Router` with
`ParserRegistry`. Archive formats are extensible through `ArchiveExtractor` and
`ExtractorRegistry`; post-processing steps implement `Finalizer`. Extension is
compile-time; no unstable plugin ABI is used.

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
