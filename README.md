# shaul

[![CI](https://github.com/hushink/shaul/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/hushink/shaul/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/shaul.svg)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](https://github.com/hushink/shaul/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](https://github.com/hushink/shaul/blob/main/LICENSE)

**English** | [简体中文](README.zh-CN.md)

A resumable, pluggable download engine for Rust — one API for HTTP(S),
Hugging Face, ModelScope, BitTorrent, FTP and optional Xet.

shaul combines segmented transfers, safe resume, checksum verification,
archive extraction, a persistent queue and an optional content-addressed cache
behind a small set of replaceable components. Use it as a library or through
the `shaul` command.

> [!NOTE]
> The CLI enables BitTorrent, FTP and Xet. Library users start with the default
> feature set and opt in with `features = ["bt"]`, `["ftp"]` and/or `["xet"]`.

## Why shaul

- **One download model for many protocols.** A `LinkParser` turns input into a
  protocol-agnostic `Resource`; a `Router` selects the first backend that can
  handle it; a `DownloadEngine` manages tasks and persistence.
- **Resume that checks its assumptions.** Sidecar checkpoints record byte
  ranges, checksums and validators. A changed `ETag`, `Last-Modified` or size
  restarts the file safely; servers that ignore `Range` fall back to one stream.
- **Built for large model repositories.** Hugging Face and ModelScope
  snapshots support revisions, include/exclude globs, provider credentials and
  optional provider caches. Multi-file tasks can download files concurrently.
- **Production controls.** Global, per-host and per-task bandwidth limits,
  active connection budgets, proxy support, compatible mirror fallback and
  priority-based queue scheduling.
- **Extensible by design.** Parsers, backends, task stores, finalizers and
  archive extractors are traits. No unstable plugin ABI is involved.

## How it works

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
                                   |
                            DownloadEngine
                                   |
                     Task -> FileItem -> Segment / Piece
                                   |
                    Finalizer: merge -> verify -> extract -> rename
```

Parsers understand input formats; resources describe files without protocol
details; routers choose a backend; the engine owns task state, retries,
checkpointing and finalization.

## Install

### CLI

```console
$ cargo install shaul-cli
```

Prebuilt archives and installers are attached to the
[`shaul-cli` releases](https://github.com/hushink/shaul/releases).

### Library

```toml
[dependencies]
shaul = "1.2"
```

Optional protocol support:

```toml
[dependencies]
shaul = { version = "1.2", features = ["bt", "ftp", "xet"] }
```

`shaul` has an empty default feature set. `bt`, `ftp` and `xet` are opt-in.

## Quick start

### Download from the CLI

```console
# Direct HTTP link with resume and verification
$ shaul download https://example.com/model.bin \
      --checksum sha256:<hex> --connections 8

# Hugging Face repository snapshot
$ export HF_TOKEN=hf_...
$ shaul download https://huggingface.co/org/repo \
      --revision main --include '*.safetensors' --cache

# ModelScope repository snapshot
$ export MODELSCOPE_API_TOKEN=...
$ shaul download https://modelscope.cn/models/org/repo

# BitTorrent
$ shaul download 'magnet:?xt=urn:btih:<info-hash>&dn=example'
$ shaul download ./example.torrent --include '*.mkv' --dir ./media

# FTP
$ shaul download ftp://user:password@example.com/pub/file.bin --dir ./downloads

# Proxy, bandwidth and host limits
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 \
      --limit 10MiB --per-host-limit 5MiB

# Persistent queue
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list
$ shaul status <id-or-suffix>
$ shaul pause <id>
$ shaul resume <id>

# Health, statistics and cleanup
$ shaul doctor
$ shaul stats
$ shaul clean --finished --cache
```

### Embed the library

```rust
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

`default_engine` registers the Hub and direct HTTP parsers/backends. Enable
`bt`, `ftp` and/or `xet` to add those backends. Use `engine.subscribe()` for
typed progress events, or `default_blocking_engine` when you do not own a Tokio
runtime.

## Supported inputs

| Input | Examples |
| --- | --- |
| Direct HTTP(S) | `https://example.com/file.bin` |
| Hugging Face | `hf://org/repo`, `hf://datasets/org/repo`, repository URLs |
| ModelScope | `ms://org/repo`, `ms://datasets/org/repo`, repository URLs |
| BitTorrent | magnet links, 40-character v1 info hashes, local `.torrent` files |
| FTP | `ftp://user:password@example.com/path/file.bin` |
| Custom protocols | `ResourceKind::Custom { protocol }` plus a custom parser/backend |

Hugging Face and ModelScope links accept `--revision`, `--include` and
`--exclude`; `--revision` overrides a revision embedded in the URL. Repository
HTTPS URLs also accept `/tree/<rev>`, `/blob/<rev>/<path>` and
`/resolve/<rev>/<path>` path styles.

## Features

### Transfers

- Segmented HTTP downloads with a work-stealing range queue and adaptive
  segment sizing.
- Safe resume with `ETag`, `Last-Modified` and size validators; explicit `416`
  handling and a single-stream fallback when a server ignores `Range`.
- Compatible mirror fallback: mirrors must match the primary file's size and
  validator (or checksum) before they are used.
- HTTP/HTTPS/SOCKS5 proxy with `NO_PROXY`; global, per-host and per-task
  bandwidth caps; active request budgets across tasks and hosts.
- Multi-file tasks download files concurrently with `file_jobs`.

### Integrity and post-processing

- Checksum verification with SHA-256, SHA-512, SHA-1 and MD5; automatic
  detection of provider checksums and common HTTP digest headers.
- Verification runs before extraction; corrupt archives are not unpacked.
- Archive extraction for zip, tar, tar.gz, tar.zst, gzip and zstd with
  path-traversal protection, atomic replacement and expansion limits.
- Custom archive formats can be provided through `ArchiveExtractor`.

### Hubs and optional Xet

- Hugging Face and ModelScope snapshot downloads with provider tokens from
  environment variables or official credential files.
- Optional HF-compatible or ModelScope cache with hardlink-or-copy placement.
- Optional Xet downloads for Hugging Face files that advertise `xetHash`.
  Token or protocol failures fall back to regular HTTP.

### Queue and automation

- Persistent sidecar task store with pause, resume and cancel across
  processes.
- Priority scheduling and jittered retries with typed retry events.
- NDJSON output, typed events, `tracing` spans and pluggable telemetry.
- `doctor`, `stats` and `clean` commands for routine maintenance.

### Content cache

Enable `download.content_cache` (or `--content-cache`) to reuse files whose
checksum and size are known. A cache hit is verified again before use; entries
are written only after verification succeeds. The cache can be pruned by age
or total size with `shaul clean --cache`; destructive maintenance refuses a
directory whose top level does not look like a content cache.

## Configuration

Precedence is:

```text
CLI flags > config file > environment variables > defaults
```

`shaul config path` prints the resolved config path. `--config PATH` or
`SHAUL_CONFIG` overrides it. `shaul config init|show|get|set|edit` manage the
file. Unknown keys are rejected, and the file is written atomically; on Unix
it is created with mode 0600. `config set` edits the file in place and
preserves comments and key order.

```toml
[download]
dir = "."
connections = 8
per_host = 16
per_host_active = 16
global = 64
segment_size = 8388608
rate_limit = 0
per_host_rate_limit = 0
file_jobs = 1
content_cache = false
# content_cache_dir = "/var/cache/shaul/content"
disk_space_check = true
resume = true
verify = "auto"        # auto | require | disabled
extract = false
# extract_dir = "/path/to/extract"
delete_archive = false

[network]
timeout_secs = 60
connect_timeout_secs = 15
retries = 5            # HTTP transfers
user_agent = "shaul/1.2.1"
allow_cross_origin_auth = false
# proxy = "http://127.0.0.1:8080"
# no_proxy = "localhost,127.0.0.1"

[hub]
use_cache = false
# hf_cache = "/path/to/huggingface/hub"
# modelscope_cache = "/path/to/modelscope"
# hf_endpoint = "https://hf-mirror.com"
# modelscope_endpoint = "https://modelscope.cn"

[bt]
dht = true
trackers = true
listen = true
persistence = true
# peer_limit = 50
# initial_peers = ["1.2.3.4:6881"]

# state_dir = "/path/to/tasks"

[headers]
# X-Test = "value"
# Authorization = "Bearer ${env:MY_TOKEN}"
```

Set `network.proxy = ""` (or pass `--proxy ""`) to disable environment
proxies and connect directly.

Common environment variables include `SHAUL_CONFIG`, `SHAUL_DIR`,
`SHAUL_STATE_DIR`, `SHAUL_CONNECTIONS`, `SHAUL_RATE_LIMIT`,
`SHAUL_PER_HOST_RATE_LIMIT`, `SHAUL_FILE_JOBS`, `SHAUL_CONTENT_CACHE`,
`SHAUL_CONTENT_CACHE_DIR`, `SHAUL_DISK_SPACE_CHECK`, `SHAUL_PROXY`,
`SHAUL_NO_PROXY`, `HF_TOKEN` and `MODELSCOPE_API_TOKEN`. See the CLI's
`config` commands for the full set.

Provider tokens are resolved at run time and are not written to task
sidecars. On Unix, task sidecars are created with mode 0600. `--json` output
redacts sensitive headers and removes query strings from HTTP task views.

## Semantics worth knowing

- Segmented HTTP downloads write into one preallocated `.part` file at byte
  offsets; there is no per-segment file merge.
- Resume is enabled by default. A backend advertises support through
  `BackendCapabilities`; when it cannot resume, the engine discards the saved
  manifest and starts from zero. FTP restarts from zero when the server
  rejects `REST`.
- `--verify` requires a checksum. `--no-verify` disables optional checksum
  verification. BitTorrent piece verification is internal and counts as
  verification.
- Extraction is off by default. The archive is verified first, extracted into
  a temporary directory and renamed into place. `--rename` suffixes an
  existing extraction target; `--no-delete-archive` keeps the archive even when
  the config deletes archives.
- `--output` is for single-file direct HTTP/FTP links. Use `--dir` for Hub
  repositories and BitTorrent; those backends reject `--output`.
- BitTorrent is leech-only: uploads are served while downloading, then the
  torrent is paused after completion. Existing destination files are refused
  unless `--force` is passed; librqbit's persisted state normally handles
  resume.

## Library documentation

The [library guide](docs/library.md) covers configuration, events, the
persistent queue, cancellation, the blocking API, telemetry and tracing. It
links to runnable examples:

- [`library_basic`](crates/shaul/examples/library_basic.rs) — add and run a task
- [`library_events`](crates/shaul/examples/library_events.rs) — typed events
- [`library_blocking`](crates/shaul/examples/library_blocking.rs) — sync facade
- [`library_queue`](crates/shaul/examples/library_queue.rs) — pause/resume and batch runs

To add a protocol, implement `LinkParser` and/or `DownloadBackend`, then
assemble a `Router` with `ParserRegistry`. See the
[extension guide](docs/extending.md) and the
[`custom_parser`](crates/shaul/examples/custom_parser.rs),
[`custom_protocol`](crates/shaul/examples/custom_protocol.rs) and
[`custom_extractor`](crates/shaul/examples/custom_extractor.rs) examples.

## Documentation

- [Library guide](docs/library.md)
- [Extension guide](docs/extending.md)
- [Resume and hardening](docs/resume.md)
- [FAQ](docs/faq.md)
- [API reference](https://docs.rs/shaul)
