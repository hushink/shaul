# shaul

[![CI](https://github.com/hushink/shaul/actions/workflows/ci.yml/badge.svg?branch=main&style=flat-square)](https://github.com/hushink/shaul/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/shaul.svg?style=flat-square)](https://crates.io/crates/shaul)
[![docs.rs](https://docs.rs/shaul/badge.svg?style=flat-square)](https://docs.rs/shaul)
[![MSRV](https://img.shields.io/badge/MSRV-1.92-blue?style=flat-square)](https://github.com/hushink/shaul/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)

A resumable, pluggable download engine for Rust — library and CLI.

[简体中文](README.zh-CN.md)

> **Status: 1.0.3** — HTTP, Hugging Face, ModelScope and BitTorrent are
> implemented and published to [crates.io](https://crates.io/crates/shaul).
> See the [API audit](docs/api-audit.md) for the 1.x stability policy.

## Why shaul

Most Rust download crates are thin, single-stream HTTP helpers. shaul is built
around a small domain model that keeps protocol details behind replaceable
backends:

- **Segmented HTTP downloads** with a work-stealing range queue and a
  per-file connection budget.
- **Correct resume**: versioned sidecar checkpoints,
  `ETag`/`Last-Modified`/size validators, `If-Range`, and a safe fallback when a
  server ignores `Range` (200 instead of 206, including ModelScope's
  `200 + Content-Range` behavior).
- **Checksums**: SHA-256/SHA-512/SHA-1/MD5; Hugging Face LFS
  `X-Linked-Etag`, HTTP `Digest`/`Content-MD5`, ModelScope `Sha256` and HF tree
  `lfs.oid` are detected automatically.
- **Finalizer pipeline**: merge (only for fragment-producing backends) →
  verify → extract → atomic rename.
- **Archive extraction**: zip, tar, tar.gz, tar.zst, gz and zst with
  path-traversal protection and atomic replacement. Extraction is pluggable
  through `ArchiveExtractor`, so 7z/rar/xz can be supplied by third parties.
- **Per-task headers and auth**: repeatable headers, Bearer/Basic auth,
  cross-origin redirect stripping, and `${env:NAME}` placeholders.
- **Hub-aware**: Hugging Face and ModelScope repository snapshots, revisions,
  include/exclude globs, provider tokens from environment/credential files,
  and an optional Hugging Face-compatible cache.
- **BitTorrent**: magnet links, bare v1 info hashes and local `.torrent` files;
  DHT, trackers, peer wire, per-file selection and pause/resume through
  librqbit. Leech-only: the torrent is paused after completion, so shaul does
  not keep seeding.
- **Persistent queue**: `shaul add`, a `shaul run` worker, and
  pause/resume/cancel across processes.
- **Network controls**: explicit HTTP/HTTPS/SOCKS5 proxy with `NO_PROXY`, plus
  global and per-task bandwidth caps.
- **Observability**: one typed event stream feeds both human progress bars and
  NDJSON output; `tracing` spans and a pluggable `Telemetry` hook cover logs,
  metrics and external monitoring.

## Architecture

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
                                   |
                            DownloadEngine
                                   |
                     Task -> FileItem -> Segment / Piece
                                   |
                    Finalizer: merge -> verify -> extract -> rename
```

Workspace crates:

- `shaul-core` — domain types, engine, router, task store, resume, finalizers.
- `shaul-http` — direct-link HTTP parser and segmented transfer backend.
- `shaul-hub` — Hugging Face/ModelScope parsers, REST clients and backend.
- `shaul-bt` — BitTorrent parser and leech-only backend (optional).
- `shaul-cli` — the `shaul` binary.
- `shaul` — default assembly; `bt` is an opt-in feature for library users and
  enabled by the CLI.

`LinkParser` may perform network I/O to discover metadata (Hub links); a local
`.torrent` path is resolved immediately. `Resource` is protocol-agnostic;
`Router` selects the first backend that supports the resource. The first parser
that claims a link is authoritative, so a Hub metadata error never falls
through to downloading an HTML page. See [GLOSSARY.md](GLOSSARY.md) and
[docs/adr](docs/adr).

## Quick start

### CLI

```console
$ cargo install shaul-cli
# Prebuilt archives and installers: shaul-cli GitHub releases
# (available from the next CLI release onward)

# Direct link
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

# Proxy and bandwidth limit
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 --limit 10MiB

# Persistent queue and batch input
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list
$ shaul status <id-or-suffix>
$ shaul pause <id>      # a running worker notices within ~500 ms
$ shaul resume <id>
```

`hf://org/repo`, `hf://datasets/org/repo`, `ms://org/repo` and
`ms://datasets/org/repo` are also accepted. `/tree/<rev>`, `/blob/<rev>/<path>`
and `/resolve/<rev>/<path>` URL forms are recognized. A 40-character hex
string is accepted as a bare BitTorrent v1 info hash.

Useful flags: `--dir`, `--output` (single-file direct links), `--checksum`,
`--verify/--no-verify`, `--extract/--no-extract`, `--extract-dir`,
`--delete-archive`, `--header`, `--header-file`, `--bearer-token`, `--basic`,
`--proxy`, `--no-proxy`, `--limit`, `--input-file`, `--revision`,
`--include`, `--exclude`, `--cache/--no-cache`, `--json`, `--force`,
`--overwrite`, `--rename`. `--limit` on `download`/`add` caps that task;
`shaul run --limit` caps the worker globally.

### Library

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
            PathBuf::from("."),
            options,
        )
        .await?;
    engine.run_task(task.id).await?;
    Ok(())
}
```

`default_engine` registers the Hub and direct HTTP parsers/backends; enable the
`bt` feature on the `shaul` dependency to add BitTorrent. Subscribe to
`engine.subscribe()` for typed events. If you do not want to own a Tokio
runtime, use `default_blocking_engine`.

The [library guide](docs/library.md) covers configuration, events, the
persistent queue, cancellation, the blocking facade, telemetry and tracing,
with runnable examples:

- [`library_basic`](crates/shaul/examples/library_basic.rs) — add and run a task
- [`library_events`](crates/shaul/examples/library_events.rs) — typed events
- [`library_blocking`](crates/shaul/examples/library_blocking.rs) — sync API
- [`library_queue`](crates/shaul/examples/library_queue.rs) — pause/resume and batch runs

To add a new protocol, implement `LinkParser` and/or `DownloadBackend`, then
assemble a `Router` with `ParserRegistry` and the backends in priority order.
See the [extension guide](docs/extending.md),
[`custom_parser`](crates/shaul/examples/custom_parser.rs),
[`custom_protocol`](crates/shaul/examples/custom_protocol.rs) and
[`custom_extractor`](crates/shaul/examples/custom_extractor.rs). Extension is
compile-time: no unstable dynamic-plugin ABI is involved.

## Configuration

Precedence: **CLI flags > config file > environment variables > defaults**.

`shaul config path` prints the resolved path. `--config PATH` or `SHAUL_CONFIG`
overrides it. `shaul config init|show|get|set|edit` manage the file.

```toml
[download]
dir = "."
connections = 8
per_host = 16
global = 64             # reserved; not enforced yet
segment_size = 8388608
rate_limit = 0         # bytes/sec shared by all tasks; 0 = unlimited
resume = true
verify = "auto"        # auto | require | disabled
extract = false
extract_dir = ""
delete_archive = false

[network]
timeout_secs = 60
connect_timeout_secs = 15
retries = 5
user_agent = "shaul/1.0.3"
allow_cross_origin_auth = false
# proxy = "http://127.0.0.1:8080"   # http://, https:// or socks5://
# no_proxy = "localhost,127.0.0.1"

[hub]
use_cache = false      # enable HF-compatible / ModelScope caches
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

Network environment variables include `SHAUL_RATE_LIMIT`, `SHAUL_PROXY`,
`SHAUL_NO_PROXY`, and the conventional `HTTP_PROXY` / `HTTPS_PROXY` /
`ALL_PROXY` / `NO_PROXY` names. Explicit settings win over environment values.

Hub-related environment variables include `HF_TOKEN`,
`HUGGING_FACE_HUB_TOKEN`, `HF_HOME`, `HF_HUB_CACHE`, `HF_ENDPOINT`,
`MODELSCOPE_API_TOKEN`, `MODELSCOPE_CACHE`, `MODELSCOPE_ENDPOINT`,
`SHAUL_HF_CACHE`, `SHAUL_MODELSCOPE_CACHE`, `SHAUL_HF_ENDPOINT`,
`SHAUL_MODELSCOPE_ENDPOINT` and `SHAUL_HUB_USE_CACHE`.

BitTorrent environment variables include `SHAUL_BT_DHT`, `SHAUL_BT_TRACKERS`,
`SHAUL_BT_LISTEN`, `SHAUL_BT_PERSISTENCE`, `SHAUL_BT_PEER_LIMIT` and
`SHAUL_BT_INITIAL_PEERS` (comma-separated `ip:port`).

On Unix, task sidecars are written with mode 0600. Provider tokens read from
the environment or official credential files are resolved at run time and are
never persisted. User-supplied `--bearer-token`/`--basic`/headers are stored in
the sidecar; prefer `${env:NAME}` placeholders. `--json` output redacts
sensitive headers and strips HTTP query strings from task views. `config show`
and `config get network.proxy` redact proxy credentials.

## Semantics worth knowing

- Segmented HTTP downloads write into one preallocated `.part` file at byte
  offsets; there is no per-segment file merge. A merge step exists for
  backends that produce independent fragments.
- Resume is on by default. A changed validator restarts the file safely.
  A server that ignores `Range` triggers a single-stream fallback; `416` is
  handled explicitly. `200 + Content-Range` is treated as a partial response.
- `--verify` requires a checksum. `--no-verify` disables optional checksums.
  BitTorrent piece verification is internal and counts as verification.
- Extraction is off by default. The archive is verified first, extracted into
  a temporary directory, then renamed into place. Path traversal and symlink
  entries are rejected or skipped.
- Hub snapshots land at `<destination>/<repo-name>/<repo-relative-path>`.
  `--output` is for single-file direct links; use `--dir` for Hub repositories
  and BitTorrent. `--checksum` is accepted only for a single-file Hub or direct
  link; it is rejected for BitTorrent.
- Hub caches are opt-in (`hub.use_cache` / `--cache`). The HF cache uses
  `models--org--repo/{blobs,refs,snapshots}` (or `datasets--...`); snapshot and
  destination files are hardlinked or copied from blobs. ModelScope uses
  shaul's own `models--org--repo` layout under its cache root.
- BitTorrent is leech-only: it responds to uploads while downloading, then
  pauses the torrent after completion and does not keep seeding. Piece state
  and fastresume data are owned by librqbit under `<state_dir>/bt`. The task
  sidecar is updated on completion/pause; live BT progress is delivered through
  the event stream.
- `shaul pause/resume/cancel` update the sidecar; a running `shaul run` worker
  notices the change within about 500 ms. A worker holds a lock file and is
  single-instance per state directory.

## Roadmap

- **0.3** — done: BitTorrent backend via `librqbit`, leech-only.
- **1.0** — done: 1.0 API freeze, per-crate READMEs, cross-platform CI and
  release-plz publishing.
- **1.1** — next: proxy, bandwidth limiting, replaceable archive extractors,
  full `BlockingEngine` lifecycle and `--input-file`.
- **1.2** — throughput and multi-source: parallel files, enforced connection
  budgets, mirror fallback and task priority.
- **1.3** — Hub differentiation: optional Xet backend (after a spike), CAS
  cache and cache maintenance.
- **1.4** — operations: retry/connection/cache metrics, OpenTelemetry
  adapters, `doctor`/`clean`/`stats`.

The detailed plan, extension boundaries and release gates live in
[docs/roadmap.md](docs/roadmap.md).

Not planned for 1.x: daemon/RPC, seeding, aria2 control-file compatibility,
built-in 7z/rar/xz extraction (third parties can implement `ArchiveExtractor`),
FTP/SFTP/WebDAV/S3 signing, dynamic plugin ABI and WASM.

## Testing

```console
$ cargo test --workspace
```

The suite covers segmented downloads, resume, 200 fallback, redirects, custom
headers, checksum verification, tar.gz/zip extraction, HF/MS repository
downloads, include filters, auth failures, cache population/cache hits, and a
local BitTorrent peer (torrent creation, seeder session and leech download).
No external network is required.

## API stability

The 1.0 stability audit — semver rules, MSRV policy, constructible types and
known risks — lives in [docs/api-audit.md](docs/api-audit.md). All crates use
`#![forbid(unsafe_code)]`. Cross-platform CI covers Linux, Windows and macOS,
plus rustfmt, clippy, rustdoc, feature combinations and MSRV 1.92.

## Documentation

- [Library guide](docs/library.md) — embedding, events, queue, blocking API
  and observability
- [Roadmap](docs/roadmap.md) — 1.x capabilities, extension boundaries and
  release gates
- [Extension guide](docs/extending.md) — custom parsers/backends, routing and
  testing
- [FAQ](docs/faq.md) — resume, verification, extraction, Hub caches and the
  queue
- [API audit](docs/api-audit.md) — 1.x semver policy and known risks
- [Release process](docs/release.md) — trusted publishing and CLI binaries
- [Benchmarks](docs/benchmarks.md) — local HTTP throughput and resume baseline
- [Architecture decisions](docs/adr) and [glossary](GLOSSARY.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
