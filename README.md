# shaul

[![CI](https://github.com/hushink/shaul/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/hushink/shaul/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/shaul.svg)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue)](https://github.com/hushink/shaul/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](https://github.com/hushink/shaul/blob/main/LICENSE)

**English** | [简体中文](README.zh-CN.md)

A resumable, pluggable download engine for Rust — one API for HTTP(S), HLS,
DASH, Hugging Face, ModelScope, BitTorrent, FTP and optional Xet.

shaul combines segmented transfers, safe resume, checksum verification,
archive extraction, a persistent queue and an optional content-addressed cache
behind a small set of replaceable components. Use it as a library or through
the `shaul` command.

> [!NOTE]
> The CLI enables BitTorrent, FTP, HLS, DASH and Xet. Library users start with
> the default feature set and opt in with `features = ["bt"]`, `["ftp"]`,
> `["hls"]`, `["dash"]` and/or `["xet"]`.

## Why shaul

- **One download model for many protocols.** A `LinkParser` turns input into a
  protocol-agnostic `Resource`; a `Router` selects the first backend that can
  handle it; a `DownloadEngine` manages tasks and persistence. HLS and DASH
  segments stay inside the backend, so the task still produces one output file.
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
shaul = { version = "1.2", features = ["bt", "dash", "ftp", "hls", "xet"] }
```

`shaul` has an empty default feature set; every protocol feature is opt-in.

## Quick start

### Download from the CLI

```console
# Direct HTTP link with resume and verification
$ shaul download https://example.com/model.bin \
      --checksum sha256:<hex> --connections 8

# POST a request body to an API that returns a file
$ shaul download https://example.com/api/export \
      --method POST --data '{"id":42}' \
      -H 'Content-Type: application/json'

# HLS: highest-bandwidth variant, segments concatenated into one file
$ shaul download https://example.com/video/master.m3u8 --connections 8
$ shaul download https://example.com/video/master.m3u8 --include '*1080*'

# HLS live: record until a duration or segment limit (ENDLIST also stops it)
$ shaul download https://example.com/live/index.m3u8 --live-duration 10m
$ shaul download https://example.com/live/index.m3u8 --live-segments 300

# DASH: highest-bandwidth video representation from a static VOD manifest
$ shaul download https://example.com/video/manifest.mpd --connections 8
$ shaul download https://example.com/video/manifest.mpd --include '*1080*'

# DASH live: bounded dynamic-manifest capture
$ shaul download https://example.com/live/manifest.mpd --live-duration 10m
$ shaul download https://example.com/live/manifest.mpd --live-segments 300

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
$ shaul list --status pending
$ shaul status <id-or-suffix>
$ shaul pause <id> <id>     # one or more ids
$ shaul pause --all         # pending, running and paused tasks
$ shaul resume --all        # paused and failed tasks
$ shaul cancel --all        # pending, running and paused tasks
$ shaul resume <id>         # a running worker notices within ~500 ms

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
| HLS | `https://example.com/video/master.m3u8` |
| DASH | `https://example.com/video/manifest.mpd` |
| Hugging Face | `hf://org/repo`, `hf://datasets/org/repo`, repository URLs |
| ModelScope | `ms://org/repo`, `ms://datasets/org/repo`, repository URLs |
| BitTorrent | magnet links, 40-character v1 info hashes, local `.torrent` files |
| FTP | `ftp://user:password@example.com/path/file.bin` |
| Custom protocols | `ResourceKind::Custom { protocol }` plus a custom parser/backend |

Hugging Face and ModelScope links accept `--revision`, `--include` and
`--exclude`; `--revision` overrides a revision embedded in the URL. Repository
HTTPS URLs also accept `/tree/<rev>`, `/blob/<rev>/<path>` and
`/resolve/<rev>/<path>` path styles. HLS and DASH links use
`--include`/`--exclude` to select a variant or representation.

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
- Non-GET HTTP requests (`POST`, `PUT`, `PATCH`, ...) with an optional body,
  including a `--data-file` variant. These are sent as a single request and
  do not segment, resume or use mirrors.
- Multi-file tasks download files concurrently with `file_jobs`.

### HLS

- HTTP(S) `.m3u8` master and media playlists. The highest-bandwidth variant is
  selected by default; repeat `--include`/`--exclude` with globs matching
  `NAME`, `RESOLUTION`, `BANDWIDTH`, `CODECS` or the URI to choose another.
- Segments download concurrently through the shared HTTP stack, so proxy,
  authentication, timeouts, retries, rate limits and connection budgets apply.
- AES-128 encrypted segments, including key rotation and explicit or
  media-sequence IVs. Keys are fetched at run time, kept in memory and never
  written to the task sidecar.
- `#EXT-X-BYTERANGE` segments and init maps, including implicit offsets.
- Segment-level resume: a playlist SHA-256 guards the completed segment files
  in `<output>.shaul.hls.parts/`.
- Live media playlists can be recorded with `--live-duration` or
  `--live-segments`. The backend polls at `#EXT-X-TARGETDURATION`, records only
  new media sequence numbers, and stops at the limit or `#EXT-X-ENDLIST`.
  Live captures are not resumable.
- TS output and fMP4 playlists with a single `#EXT-X-MAP`. SAMPLE-AES is not
  supported yet.

### DASH

- HTTP(S) `.mpd` manifests: static lists with one or more Periods, or dynamic
  lists with a single Period. The highest-bandwidth video Representation is
  selected by default; repeat `--include`/`--exclude` with globs matching `id`,
  `mimeType`, `codecs`, `lang`, resolution or `bandwidth` to choose another.
- `SegmentTemplate` (`$Number$`, `$Time$`, `$RepresentationID$`,
  `$Bandwidth$`, constant duration, `endNumber` or `SegmentTimeline`),
  `SegmentList`, and `SegmentBase`/BaseURL single-object representations.
- BaseURL inheritance across MPD, Period, AdaptationSet and Representation;
  byte ranges reuse the exact `Range` HTTP path.
- Initialization and media segments download concurrently through the shared
  HTTP stack and are concatenated into one `.mp4` or `.webm` file.
  VOD resume is guarded by an MPD SHA-256, the representation id and the
  segment count.
- Static multi-Period manifests concatenate compatible representations: all
  Periods must share MIME/codecs and either have no initialization segment or
  the same initialization segment. The common init is written once. Different
  init segments require remuxing and fail explicitly.
- Dynamic (live) manifests can be recorded with `--live-duration` or
  `--live-segments` when they use `SegmentTemplate` + `SegmentTimeline`, or a
  constant-duration `SegmentTemplate` with `availabilityStartTime`. The backend
  polls at `minimumUpdatePeriod`, deduplicates by presentation time, and stops
  at the limit, when the MPD becomes static, or when the Period ends.
  Constant-duration captures start at the live edge and use `publishTime` (or
  the local clock) to compute availability. Dynamic captures are not resumable.
- DRM (`ContentProtection`), dynamic multi-Period manifests, multi-Period
  manifests with different init segments/MIME/codecs, `availabilityTimeOffset`
  and low-latency chunked delivery, subtitles, alternate audio muxing and
  remuxing are not supported yet.

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
- Batch queue control: `pause`/`resume`/`cancel` accept multiple ids or
  `--all`, and `list --status` filters by state.
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
- A non-GET method, or any request with a body, is a single-shot transfer: the
  probe response is streamed directly, so the request is sent exactly once.
  Resume and mirror fallback are disabled, and `POST`/`PATCH` are not retried
  automatically. `301`/`302` switch `POST` to `GET`, `303` switches any
  non-`HEAD` method to `GET`, and `307`/`308` preserve the method and body.
  A cross-origin `307`/`308` that would forward a body requires
  `allow_cross_origin_auth`. Bodies support `${env:NAME}` placeholders and are
  redacted from `--json` output.
- Resume is enabled by default. A backend advertises support through
  `BackendCapabilities`; when it cannot resume, the engine discards the saved
  manifest and starts from zero. FTP restarts from zero when the server
  rejects `REST`.
- HLS and DASH tasks produce one output file. Segments are written under
  `<output>.shaul.hls.parts/` or `<output>.shaul.dash.parts/`, concatenated
  through `<output>.shaul.part`, and the temporary directory is removed on
  success. Resume is segment-level; a changed playlist or manifest fingerprint
  restarts. `--mirror` and the final-output content cache do not apply.
  Cross-origin playlist/key/segment URLs strip sensitive headers unless
  `allow_cross_origin_auth` is set. Live HLS and dynamic DASH captures always
  start fresh and are not resumable; cancelling or failing leaves the recorded
  segments for inspection, and `clean --orphans` also removes temp files from
  failed live captures. HLS AES-128 keys are
  fetched at run time and are never persisted; encrypted segments are staged
  as `.enc` and decrypted to `.bin` files used for resume and concatenation.
  Byte-range segments use exact `Range` requests, with a full-object fallback
  for servers that ignore `Range`.
- `--verify` requires a checksum. `--no-verify` disables optional checksum
  verification. BitTorrent piece verification is internal and counts as
  verification.
- Extraction is off by default. The archive is verified first, extracted into
  a temporary directory and renamed into place. `--rename` suffixes an
  existing extraction target; `--no-delete-archive` keeps the archive even when
  the config deletes archives.
- `--output` is for single-file direct HTTP/FTP/HLS/DASH links. Use `--dir`
  for Hub repositories and BitTorrent; those backends reject `--output`.
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
