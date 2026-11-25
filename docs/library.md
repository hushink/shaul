# Using shaul as a library

`shaul` embeds the same engine the CLI uses. You get segmented HTTP
transfers, resume, verification/extraction finalizers, Hugging Face and
ModelScope snapshots, HLS playlists, static DASH manifests, and (with the `bt`
feature) BitTorrent, without going through the command line.

The examples below are complete programs under
[`crates/shaul/examples`](../crates/shaul/examples):

| Example | Shows |
| --- | --- |
| [`library_basic.rs`](../crates/shaul/examples/library_basic.rs) | Build the default engine, add and run one task |
| [`library_events.rs`](../crates/shaul/examples/library_events.rs) | Consume the typed `EventKind` stream |
| [`library_blocking.rs`](../crates/shaul/examples/library_blocking.rs) | Use the synchronous facade |
| [`library_queue.rs`](../crates/shaul/examples/library_queue.rs) | Persist tasks, pause/resume, run a queue |
| [`custom_protocol.rs`](../crates/shaul/examples/custom_protocol.rs) | Custom parser + backend + telemetry |
| [`custom_extractor.rs`](../crates/shaul/examples/custom_extractor.rs) | Custom archive format via `ExtractorRegistry` |
| [`custom_parser.rs`](../crates/shaul/examples/custom_parser.rs) | Offline custom parser |

## Dependency

```toml
[dependencies]
shaul = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

# Optional: BitTorrent, FTP, HLS, DASH and Hugging Face Xet support.
# shaul = { version = "1.2", features = ["bt", "dash", "ftp", "hls", "xet"] }
```

The async API needs a Tokio runtime. The blocking facade
(`default_blocking_engine`) creates and owns one for you.

## Default assembly

`default_engine` wires the built-in parsers and backends in priority order and
opens the sidecar task store:

- BitTorrent (only with the `bt` feature)
- Hugging Face and ModelScope (Xet downloads when the `xet` feature is on and
  the tree API reports a `xetHash`)
- FTP (only with the `ftp` feature; single-stream, no FTPS yet)
- HLS (only with the `hls` feature; VOD playlists, one concatenated output)
  and bounded live HLS capture when `TaskOptions::live_duration_secs` or
  `live_segments` is set (not resumable)
- DASH (only with the `dash` feature; static single-period manifests and
  bounded dynamic captures with SegmentTemplate + SegmentTimeline or a
  constant-duration template, one concatenated output)
- direct links as the fallback

```rust
use shaul::{Config, default_engine};

async fn build_engine() -> shaul::Result<()> {
    let (engine, store) = default_engine(Config::default())?;
    // `store` is the same Arc the engine uses; drop it or use it directly.
    let _ = (engine, store);
    Ok(())
}
```

`Config::default()` writes task sidecars under the platform data directory.
`Config::load` reads the same sources as the CLI:

```rust
use shaul::{Config, default_engine};

async fn build_engine() -> shaul::Result<()> {
    // defaults -> environment -> config file (or $SHAUL_CONFIG / explicit path)
    let (config, path) = Config::load(None)?;
    println!("config: {}", path.display());
    let (engine, _store) = default_engine(config)?;
    let _ = engine;
    Ok(())
}
```

The library does not parse command-line flags. If your application has flags,
apply them to the `Config` after `load` so your precedence stays
**flags > config file > environment > defaults**. A custom state directory is
available as `config.state_dir = Some(path)` before building the engine.

`Config::default()` is intentionally inert: it does not read the user's config
file or environment. Library callers must fill the fields they need. The CLI
uses `Config::load` and then applies its flags, which is the main capability
difference between the two entry points.

### Network controls

Proxy and bandwidth settings are part of the same `Config`, and every option
is available to library callers:

```rust
use shaul::{Config, TaskOptions};

fn configure() -> shaul::Result<(Config, TaskOptions)> {
    let mut config = Config::default();
    config.network.proxy = Some("socks5://127.0.0.1:1080".to_string());
    config.network.no_proxy = Some("localhost,127.0.0.1".to_string());
    config.download.rate_limit = Some(8 * 1024 * 1024); // global bytes/sec
    config.download.per_host_rate_limit = Some(2 * 1024 * 1024); // per host
    config.download.file_jobs = 4; // files within one multi-file task
    config.download.global = 32; // active HTTP file transfers across tasks
    config.download.per_host_active = 8; // active HTTP file transfers per host
    config.download.content_cache = true; // reuse verified files by checksum+size
    config.download.content_cache_dir = Some("/var/cache/shaul/content".into());
    config.download.disk_space_check = true; // fail before a known-too-large transfer

    let options = TaskOptions {
        rate_limit: Some(1024 * 1024), // additional per-task cap
        file_jobs: Some(4),
        // Alternate full URLs for the same file, tried after the primary fails.
        mirrors: vec!["https://mirror.example.com/file.bin".parse().unwrap()],
        ..TaskOptions::default()
    };
    Ok((config, options))
}
```

The global, per-host and per-task caps are combined: the effective rate is the
lowest of the configured limits. `None` or `Some(0)` means no cap at that
layer. `file_jobs` controls
how many files in one task transfer concurrently. `mirrors` contains alternate
full URLs for the same file and is used only after the primary fails.
`global` and `per_host_active` cap active HTTP file-transfer requests. The
proxy covers HTTP and Hub metadata/file requests; BitTorrent DHT/UDP traffic is
not proxied and BitTorrent rate limiting is not part of this slice.
`disk_space_check` preflights the destination filesystem when every file size
is known and fails the task before transferring if free space is insufficient.

### Content-addressed cache

`download.content_cache` is opt-in and only applies when both a checksum and a
size are known. Set `download.content_cache_dir` to move the cache away from
the platform cache directory. The same switch is available per task as
`TaskOptions::content_cache`.

```rust
use shaul::{Config, ContentCache};

fn inspect_cache(config: &Config) -> shaul::Result<()> {
    let cache = ContentCache::new(config.content_cache_dir()?)?;
    println!("{}", cache.root().display());
    Ok(())
}
```

Entries are keyed by `<algorithm>/<digest>`. A hit is re-hashed before use; a
corrupt or truncated entry is removed and the transfer falls back to the
network. Files are inserted only after the verify finalizer succeeds, so
`--no-verify` downloads are never cached. `ContentCache::stats`,
`ContentCache::prune` and `ContentCache::clear` are available for maintenance
tooling; the CLI exposes them through `shaul stats` and `shaul clean --cache`.
`ContentCache::clear`, `prune` and `prune_temp` refuse a root whose top level
does not look like a content cache, so a misconfigured path cannot wipe an
unrelated directory.

## One task at a time

`add` resolves the link through the router (Hub links can make metadata
requests here) and persists a `Pending` task. `run_task` drives it to a
terminal state:

```rust
use std::path::PathBuf;

use shaul::{Config, Link, TaskOptions, default_engine};

async fn download_one() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;
    let options = TaskOptions {
        connections: Some(8),
        verify: Some(shaul::VerifyMode::Auto),
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
    println!("{:?}", task.state);
    Ok(())
}
```

`run_task` returns `Ok` for `Completed`, and also for `Paused`/`Cancelled` if
that is where the task ended. A transfer failure returns `Err` **and** persists
the task as `TaskState::Failed` with `task.error` set, so `engine.status(id)`
still tells you what happened.

`add` is idempotent for an active task with the same canonical link and
destination: it returns the existing task unless `TaskOptions::force` is set.
Use `TaskOptions::output` for a single-file direct link, and
`overwrite`/`rename_on_conflict` for destination collisions.

Other useful `TaskOptions` fields include `resume`, `checksum`, `extract_dir`,
`delete_archive`, `cache`, `content_cache`, `revision`, `include`/`exclude`,
`file_jobs`, `mirrors`, `priority`, and `request` (headers and auth). See
[`TaskOptions`](https://docs.rs/shaul/latest/shaul/struct.TaskOptions.html) for
the full list.

## Resume and backend capabilities

Resume is enabled by default and can be disabled per task with
`TaskOptions { resume: Some(false), .. }`. The engine passes a saved
`TaskResume` only when the selected backend declares
`BackendCapabilities::resume = true` and the task did not disable resume. A
backend that cannot resume receives no manifest and `options.resume = false`;
the engine emits a warning when an existing manifest has to be discarded.

HTTP and Hub use sidecar byte ranges plus `ETag`/`Last-Modified`/size
validators. FTP uses `SIZE`/`MDTM` plus `REST`, and automatically restarts from
zero when the server rejects REST. BitTorrent relies on piece verification and
librqbit fastresume data. See [the resume and hardening guide](resume.md) for
the full contract, residual risks and the custom-backend checklist.

## Events

Every progress update, verification step, state transition and retry is a
typed `DownloadEvent`. The CLI consumes the same stream for progress bars and
NDJSON.

```rust
use shaul::{Config, EventKind, Link, TaskOptions, default_engine};

async fn download_with_progress() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;
    let mut events = engine.subscribe();

    let listener = tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event.kind {
                EventKind::Progress { file, downloaded_bytes, total_bytes, .. } => {
                    println!("{file}: {downloaded_bytes}/{total_bytes:?}");
                }
                EventKind::TaskCompleted { bytes, .. } => {
                    println!("done: {bytes} bytes");
                    break;
                }
                EventKind::TaskFailed { error, .. } => {
                    eprintln!("failed: {error}");
                    break;
                }
                EventKind::Retry { attempt, error, .. } => {
                    eprintln!("retry {attempt}: {error}");
                }
                _ => {}
            }
        }
    });

    let task = engine
        .add(
            Link::new("https://example.com/data.bin")?,
            std::path::PathBuf::from("."),
            TaskOptions::default(),
        )
        .await?;
    engine.run_task(task.id).await?;
    let _ = listener.await;
    Ok(())
}
```

`EventKind` is `#[non_exhaustive]`: keep a wildcard arm. Delivery is best
effort — a receiver that falls behind gets `Lagged` and should continue; it
should not assume it saw every progress tick. Subscribe before `add` if you
need `TaskAdded` and `TaskStarted`.

## Queue and lifecycle

The task store is persistent, so a library can implement the same
`add`/`run` worker split as the CLI:

```rust
use std::path::PathBuf;

use shaul::{CancellationToken, Config, Link, TaskOptions, default_engine};

async fn run_queue() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;
    engine.requeue_stale().await?;

    let destination = PathBuf::from(".");
    let a = engine
        .add(Link::new("https://example.com/a.bin")?, destination.clone(), TaskOptions::default())
        .await?;
    let b = engine
        .add(
            Link::new("https://example.com/b.bin")?,
            destination,
            TaskOptions {
                priority: Some(10),
                ..TaskOptions::default()
            },
        )
        .await?;

    engine.pause(b.id).await?;
    let cancel = CancellationToken::new();
    let done = engine.run_pending(2, cancel).await?;
    println!("completed {} task(s)", done.len());

    engine.resume(b.id).await?;
    engine.run_pending(1, CancellationToken::new()).await?;
    println!("{:?} {:?}", engine.status(a.id).await?.state, engine.status(b.id).await?.state);
    Ok(())
}
```

- `run_pending(jobs, cancel)` runs only tasks in `Pending` state, at most
  `jobs` concurrently (clamped to at least one), all sharing `cancel`. It
  selects higher `TaskOptions::priority` first, then older tasks. Tasks that
  fail are omitted from the returned `Vec<Task>`; inspect the store or the
  event stream for their errors.
- `pause`, `resume` and `cancel` update the sidecar. A running `run_task`
  observes a pause/cancel written by another process within about 500 ms and
  stops. `resume` moves any non-completed task back to `Pending` and clears
  `error`; `cancel` on an already completed task is an error.
- Call `requeue_stale()` only when your application is the single worker that
  owns the state directory. It moves tasks left in `Running` by a previous
  process back to `Pending`; running it while another worker is alive would
  disturb that worker.
- `run_task_with_cancel(id, Some(token))` lets your application stop an
  attempt (for example on `Ctrl-C`). External cancellation stops the transfer
  and leaves the task `Paused` so it can resume. To abandon a task instead,
  call `engine.cancel(id)`.

## Blocking API

For synchronous applications, `default_blocking_engine` builds the same
backends and store around a dedicated Tokio runtime:

```rust
use shaul::{Config, Link, TaskOptions, default_blocking_engine};

fn main() -> shaul::Result<()> {
    let engine = default_blocking_engine(Config::default())?;
    let task = engine.download(
        Link::new("https://example.com/data.bin")?,
        std::path::PathBuf::from("."),
        TaskOptions::default(),
    )?;
    println!("{:?}", task.state);
    Ok(())
}
```

`BlockingEngine::subscribe()` returns a `std::sync::mpsc::Receiver`. `add`,
`run_task`, `list`, `status`, `pause`, `resume`, `cancel`, `run_pending` and
`requeue_stale` are available as blocking calls, and `engine()` exposes the
underlying async engine. For a custom router, use
`BlockingEngine::new(router, store, config)`.

## Custom assembly and observability

To replace a component, build the `Router` yourself and pass it to
`DownloadEngine::new`. The [extension guide](extending.md) covers custom
`LinkParser`/`DownloadBackend` implementations, `ResourceKind::Custom`,
`TaskStore`, the finalizer pipeline, and custom archive formats through
`ArchiveExtractor`/`ExtractorRegistry` (including user-provided 7z/rar/xz).

The default assembly can also be adjusted after construction:

```rust
use std::sync::Arc;

use shaul::{Config, Telemetry, default_engine};

struct Metrics;

impl Telemetry for Metrics {
    fn counter(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
        // forward to Prometheus / OpenTelemetry / StatsD
        let _ = (name, value, labels);
    }
}

async fn build_engine() -> shaul::Result<()> {
    let (engine, _store) = default_engine(Config::default())?;
    let engine = engine.with_telemetry(Arc::new(Metrics));
    let _ = engine;
    Ok(())
}
```

`Telemetry` methods default to no-ops, so implement only what you need. The
metric names and labels are listed in [the extension guide](extending.md) and
in `shaul::metrics`; `shaul.retries` counts retry events, which are also
available as `EventKind::Retry`. Cache hits/misses, active HTTP connections and
queue depth are emitted as `shaul.cache.*`, `shaul.http.active_connections`
and `shaul.queue.depth`. `with_finalizers` replaces verification/extraction
with your own pipeline.

Logging uses the `tracing` facade; shaul never installs a subscriber, so the
embedding application decides where spans go:

```rust,ignore
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();
```

Span names such as `shaul.engine.run_task`, `shaul.http.file` and
`shaul.hub.download` are stable enough to filter on. Use `Telemetry` for
numeric aggregation and `subscribe()` for per-task progress; the three
mechanisms are independent.

## Error handling and testing

`shaul::Error` distinguishes configuration, state-machine, backend and
verification failures; `Error::is_cancelled` recognizes cancellation. The
engine's own tests run against local HTTP/Hub/BitTorrent fixtures, so
`cargo test --workspace` needs no external network. For your own integration
tests, build a custom `Router` with test parsers/backends — see
[`docs/extending.md`](extending.md) and `crates/shaul/tests/e2e.rs`.
