# Extending shaul

shaul is built around four replaceable pieces:

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
```

- `LinkParser` turns raw user input into a protocol-agnostic `Resource`.
- `Resource` lists files, URLs, checksums and provider metadata.
- `Router` picks the first parser that claims a link, then the first backend
  that supports the resource.
- `DownloadBackend` transfers the files and returns `FileOutcome`s.

`DownloadEngine`, the task sidecar store and the finalizer pipeline
(`verify -> extract -> rename`) are independent of these implementations.

## A custom LinkParser

The complete example is in
[`crates/shaul/examples/custom_parser.rs`](../crates/shaul/examples/custom_parser.rs).
The important parts:

```rust,ignore
struct DemoParser;

#[async_trait]
impl LinkParser for DemoParser {
    fn name(&self) -> &'static str { "demo" }

    fn supports(&self, link: &Link) -> bool {
        link.as_str().starts_with("demo://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let name = link.as_str().trim_start_matches("demo://");
        let url = Url::parse(&format!("https://example.com/{name}"))?;
        Resource::http_file(url, options.request.clone(), options.checksum.clone())
    }
}
```

Register the parser before the fallback direct-link parser:

```rust,ignore
let mut parsers = ParserRegistry::new().with(parser_arc(DemoParser));
parsers.register(parser_arc(DirectLinkParser));
let router = Router::new(parsers, vec![backend_arc(HttpBackend::new(&config)?)]);
```

Run it with:

```console
$ cargo run -p shaul --example custom_parser -- demo://model.bin
```

The parser example only resolves and persists the task, so it works offline.
The [custom_protocol example](../crates/shaul/examples/custom_protocol.rs)
runs a complete parser + backend + telemetry pipeline.

Guidelines:

- `supports()` must be cheap and side-effect free.
- `parse()` may perform metadata requests (Hub APIs), but should not download
  file contents.
- Populate `ResourceFile::path` with a safe relative path. Parsers for external
  names should call `sanitize_relative_path`.
- Put protocol-private data in `metadata` maps instead of changing public
  structs.
- The first parser that claims a link is authoritative; return an error rather
  than falling through to an HTML download.

## A custom DownloadBackend

```rust,ignore
struct MyBackend;

#[async_trait]
impl DownloadBackend for MyBackend {
    fn name(&self) -> &'static str { "my-backend" }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, false, false)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(
            &resource.kind,
            ResourceKind::Custom { protocol } if protocol == "my-protocol"
        )
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let mut outcomes = Vec::new();
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let path = request.destination.join(&file.path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // ... transfer the file, emitting EventKind::Progress ...
            let size = tokio::fs::metadata(&path).await?.len();
            outcomes.push(FileOutcome::new(
                path,
                file.path.clone(),
                size,
                file.checksum.clone(),
                false,
            ));
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}
```

A backend must:

- honor `request.cancel` and persist resumable state through
  `request.checkpoint.save(&TaskResume)`;
- emit `EventKind::Progress`/`FileStarted`/`FileCompleted` on `request.events`;
- return paths under `request.destination` and relative paths with `/`;
- use `FileOutcome::new(...)` (the struct is `#[non_exhaustive]`);
- leave verification and extraction to the engine's finalizers unless the
  protocol already proves integrity (BitTorrent pieces set `checksum_verified`).

`BackendCapabilities` is descriptive: advertise `segmented`, `resume` and
`multi_file` accurately so callers can make policy decisions. When `resume` is
false, the engine passes `resume: None` and forces `options.resume = false`, and
it warns if a saved manifest had to be discarded. The backend must still
truncate any partial output before writing. If `resume` is true, honor both the
request manifest and `options.resume`; see [resume.md](resume.md).

Third-party protocols should use `ResourceKind::Custom { protocol }` and put
their data in `Resource::metadata` / `ResourceFile::metadata`. The complete
parser + backend + telemetry example is
[`crates/shaul/examples/custom_protocol.rs`](../crates/shaul/examples/custom_protocol.rs).

## Routing order

`Router` uses first-match semantics:

1. Parsers run in registration order; the first `supports() == true` is
   authoritative.
2. Backends are checked in registration order for the parsed `Resource`.

For a custom protocol, register its parser before `DirectLinkParser`. For a
backend that should win over the built-in HTTP backend, register it first.

## Finalizers

The default pipeline verifies checksums and extracts archives. Replace it with
`with_finalizers` or `set_finalizers`; finalizers run in order and stop at the
first error:

```rust,ignore
struct AuditFinalizer;

#[async_trait]
impl Finalizer for AuditFinalizer {
    fn name(&self) -> &'static str { "audit" }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        tracing::info!(task_id = %context.task_id, file = %context.file.relative_path);
        Ok(FinalizeResult::default())
    }
}

let pipeline = FinalizerPipeline::new(vec![
    Arc::new(VerifyFinalizer),
    Arc::new(ExtractFinalizer),
    Arc::new(AuditFinalizer),
]);
let engine = engine.with_finalizers(pipeline);
```

`FinalizeContext` gives you the task id, destination, completed `FileOutcome`,
resolved options and the event sink. Verification and extraction emit their own
events; custom steps should emit `EventKind::Warning`/`FileCompleted` as needed.

## Custom archive extractors

Extraction is a replaceable component. `ArchiveExtractor` handles one format;
`ExtractorRegistry` tries extractors in order and the first `supports()` match
owns the file. The default registry ends with the built-in zip/tar/gzip/zstd
extractor, so custom formats can be prepended:

```rust,ignore
struct SevenZipExtractor;

impl ArchiveExtractor for SevenZipExtractor {
    fn name(&self) -> &'static str { "7z" }

    fn supports(&self, path: &Path) -> Result<bool> {
        Ok(path.extension().and_then(|extension| extension.to_str()) == Some("7z"))
    }

    fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
        // Extract into a temporary directory under request.extract_root,
        // validate every entry with sanitize_relative_path, then rename the
        // target into place.
        // ...
    }
}

let registry = ExtractorRegistry::default().with(Arc::new(SevenZipExtractor));
let pipeline = FinalizerPipeline::with_extractor_registry(Arc::new(registry));
let engine = engine.with_finalizers(pipeline);
```

Rules:

- `supports()` must be cheap and side-effect free. The first claiming extractor
  is authoritative; an extraction error is not retried with the next extractor.
- Write only under `ExtractorRequest::extract_root`. The registry canonicalizes
  the returned `ExtractReport::target` and rejects paths outside that root.
- Honor `ExtractorRequest::rename_on_conflict` by suffixing the target
  (`name-1`, `name-2`, ...) when it already exists instead of replacing it or
  failing; the built-in extractor does this for `--rename`.
- Reject absolute paths, `..`, symlinks and special entries in your format;
  `sanitize_relative_path` and `safe_join` are available for this.
- Extract into a temporary directory and rename into place; do not leave a
  half-written target on failure.
- Apply your own entry-count, expanded-size and compression-ratio limits. The
  built-in extractor caps archives at 100,000 entries and 100 GiB expanded per
  archive; custom formats are responsible for their own bomb protection.
- The trait is synchronous and runs inside `spawn_blocking`. Do not block the
  async runtime directly.

7z, rar and xz are intentionally not built in. Implement them with a
third-party crate or external tool and choose their license and security
posture yourself. See
[`custom_extractor.rs`](../crates/shaul/examples/custom_extractor.rs) for a
complete registry example.

To keep the default verify step and add another finalizer, build the pipeline
explicitly:

```rust,ignore
let pipeline = FinalizerPipeline::new(vec![
    Arc::new(VerifyFinalizer),
    Arc::new(ExtractorFinalizer::new(Arc::new(registry))),
    Arc::new(AuditFinalizer),
]);
```

## Telemetry

`Telemetry` is a low-cardinality metrics hook. Every method defaults to a no-op,
so an implementation can override only counters, gauges or histograms:

```rust,ignore
struct Metrics;

impl Telemetry for Metrics {
    fn counter(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
        // forward to Prometheus/OpenTelemetry/StatsD
    }
    fn histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {}
}

let engine = engine.with_telemetry(Arc::new(Metrics));
```

Stable metric names live in `shaul::metrics`:

| Metric | Kind | Labels |
| --- | --- | --- |
| `shaul.tasks.added` | counter | `backend` |
| `shaul.tasks.started` | counter | `backend` |
| `shaul.tasks.completed` | counter | `backend` |
| `shaul.tasks.failed` | counter | `backend` |
| `shaul.tasks.paused` | counter | `backend` |
| `shaul.tasks.cancelled` | counter | `backend` |
| `shaul.task.duration_seconds` | histogram | `backend`, `state` |
| `shaul.bytes.downloaded` | counter | `backend` |
| `shaul.active_tasks` | gauge | none |
| `shaul.retries` | counter | none |
| `shaul.http.active_connections` | gauge | none |
| `shaul.queue.depth` | gauge | none |
| `shaul.cache.hits` | counter | none |
| `shaul.cache.misses` | counter | none |
| `shaul.cache.bytes` | counter | none |
| `shaul.cache.inserts` | counter | none |

Callbacks run on the task executor; do not block. Use a channel or batch when
exporting to a slow backend. Task ids are deliberately not metric labels to
avoid unbounded cardinality; use `EventSink` for per-task detail. Backends can
reach the attached telemetry through `request.events.telemetry()` without
storing a second handle.

### Prometheus and OpenTelemetry

Keep vendor SDKs in the application, not in shaul-core. An adapter implements
`Telemetry` and forwards the low-cardinality names above to its own registry:

- Prometheus: use a `prometheus` registry and map `counter` to `Counter`,
  `gauge` to `Gauge`, and `histogram` to `Histogram`; register names once and
  keep labels to the documented `backend`/`state` values.
- OpenTelemetry: use the metrics SDK and a periodic reader/exporter; map the
  same names to `u64` counters, `f64` gauges and histograms. Do not attach task
  ids, URLs or file paths as attributes.

Both approaches keep the core dependency graph unchanged and let the embedding
application choose its exporter and license posture.

## Logging and tracing

The engine and backends use the [`tracing`](https://docs.rs/tracing) facade.
Applications choose a subscriber:

```rust,ignore
use tracing_subscriber::EnvFilter;

tracing_subscriber::fmt()
    .with_env_filter(EnvFilter::from_default_env())
    .init();
```

```console
$ RUST_LOG=shaul_core=debug,shaul_http=debug,shaul_hub=debug,shaul_bt=debug \
    shaul download https://example.com/file.bin
```

Spans include `shaul.engine.add`, `shaul.engine.run_task`,
`shaul.engine.attempt`, `shaul.engine.finalize`, `shaul.http.download`,
`shaul.http.file`, `shaul.hub.download` and `shaul.bittorrent.download`, with
fields such as `task_id`, `backend`, `file` and `state`. The CLI initializes a
subscriber; library users install their own. Both systems can be used together:
tracing for logs/spans and `Telemetry` for numeric aggregation.

For per-task progress, subscribe with `DownloadEngine::subscribe()`; the same
typed events drive the CLI progress bars and NDJSON output. Backend retry
attempts are reported as `EventKind::Retry` and counted in the `shaul.retries`
metric, so event consumers and metrics backends can observe the same retry
activity. A custom store can
be supplied to `DownloadEngine::new` by implementing `TaskStore`; `update` must
be atomic with respect to other processes.

## Tests

Keep extensions testable without external network access:

- use a local `TcpListener`/mock API for parser metadata;
- test resume by truncating the partial file and re-running;
- assert that tasks leave `Running` after a crash and are requeued by
  `DownloadEngine::requeue_stale`;
- run `cargo test --workspace --all-targets` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

See [GLOSSARY.md](../GLOSSARY.md), [docs/adr](adr), and the
[API audit](api-audit.md) for the public contract.
