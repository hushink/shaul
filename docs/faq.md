# FAQ

## How do I install the CLI?

```console
$ cargo install shaul-cli
```

Prebuilt archives and installers are attached to `shaul-cli-v*` GitHub
releases. The crate installs a binary named `shaul`.

## Where are tasks and partial files stored?

Each task has a JSON sidecar under the configured state directory. Partial
HTTP files use a sibling `.shaul.part` file and a resume map in the sidecar.
The default state directory is per-user; override it with `SHAUL_STATE_DIR`,
`state_dir` in `config.toml`, or `--config`.

Killing a `shaul run` worker is safe: the next worker requeues tasks left in
`Running` and resumes from the checkpointed byte ranges.

## Is resume enabled by default?

Yes. Disable it per task with `--no-resume` or in config with
`download.resume = false`. A changed `ETag`, `Last-Modified` or size restarts
the file safely instead of appending to stale data.

Services that ignore `Range` are handled with a single-stream fallback;
`200 + Content-Range` is treated as a partial response. Segment size shrinks
adaptively for small files so all configured connections can participate,
+while large files keep the configured `download.segment_size` as an upper
+bound. If the selected backend
declares that it cannot resume, the engine discards the saved manifest and
starts from zero. FTP automatically restarts from zero when the server rejects
`REST`. See [resume and hardening](resume.md).

## How does verification work?

- `--checksum sha256:<hex>` / `--checksum-file PATH` supplies an expected
  digest (SHA-256, SHA-512, SHA-1 or MD5).
- `--verify` requires a checksum and fails if none is available.
- `--no-verify` disables optional checksums; BitTorrent piece verification
  still counts as verification.
- Hugging Face `X-Linked-Etag`, HTTP `Digest`/`Content-MD5` and ModelScope
  SHA-256 values are detected when present.

## When does extraction happen?

After verification. Archives are extracted into a temporary directory and
atomically moved into place. Path traversal and symlink entries are rejected
or skipped. Built-in extraction rejects archives with more than 100,000
entries or more than 100 GiB of expanded content. Supported formats: zip, tar,
tar.gz, tar.zst, gzip and zstd.

Use `--extract-dir` for a custom destination and `--delete-archive` to remove
the archive after extraction. Extraction is off by default.

## How do Hub caches work?

Caching is opt-in (`hub.use_cache` or `--cache`). Hugging Face uses the
Python-compatible `models--org--repo/{blobs,refs,snapshots}` layout (or
`datasets--...`); ModelScope uses shaul's own layout. Snapshot and destination
files are hardlinked when possible and copied otherwise.

Provider tokens come from the environment or official credential files and are
never written into task sidecars. User-supplied `--bearer-token`/`--basic`/
headers are stored in the sidecar; prefer `${env:NAME}` placeholders.

## What is the content-addressed cache?

It is an opt-in cache for files with a known checksum and size, independent of
the Hub provider cache. Enable it with `download.content_cache = true`,
`SHAUL_CONTENT_CACHE=1`, `--content-cache`, or `TaskOptions::content_cache`.
The root defaults to the platform cache directory and can be changed with
`download.content_cache_dir` / `SHAUL_CONTENT_CACHE_DIR`.

A hit is re-hashed before use; corrupt or truncated entries are discarded. New
entries are written only after verification succeeds, so `--no-verify`
downloads are not cached. `shaul stats` reports entries/bytes and
`shaul clean --cache` removes them.

## What do `doctor`, `stats` and `clean` do?

- `doctor` checks config readability, state/download/cache directory
  writability, proxy syntax and whether provider tokens are present. It exits
  non-zero when a check fails.
- `stats` summarizes task states, known/downloaded bytes, queue depth and the
  content cache.
- `clean --finished` removes completed/cancelled/failed sidecars;
  `--completed`, `--cancelled` and `--failed` select individual states;
  `--orphans` removes `.shaul.part` files left by completed/cancelled tasks;
  `--cache` removes content-cache entries. With `--cache`, add
  `--cache-older-than 30d` and/or `--cache-max-size 50GiB` to prune instead of
  clearing everything. Add `--dry-run` to preview and `--json` for
  machine-readable output. Destination files are never deleted by `clean`.

## Can a Hub snapshot download files in parallel?

Yes. Set `download.file_jobs` (or `--file-jobs N`) to the number of files a
multi-file task may download concurrently. The default is 1, which preserves
the previous sequential behavior. Per-file checkpoints are merged into the
same sidecar, so pause/resume still works across the whole task.

## Can I supply mirrors for a direct link?

Yes. Repeat `--mirror URL` (or set `TaskOptions::mirrors`) to add alternate
full URLs for the same file. Mirrors are tried only after the primary URL
fails. A mirror must match the primary's size and, when no checksum is known,
must not expose a different strong ETag; incompatible mirrors are skipped with
a warning. Cross-origin auth stripping still applies.

## Does shaul use Hugging Face Xet?

When the `shaul` crate is built with the `xet` feature, the CLI enables it by
default. A Hub snapshot file whose tree entry contains `xetHash` is downloaded
through `hf-xet` (Apache-2.0) using the Hub `xet-read-token` endpoint. Auth,
protocol or token failures fall back to the normal HTTP resolve path, so a
missing or expired token never blocks a file that is available over HTTP.
Direct-link Xet URLs and non-Hub providers use HTTP.

The Xet path currently has unit coverage for metadata/header construction and
fallback, but no external-network test; Xet CAS downloads are verified through
`hf-xet` and the normal checksum finalizer.

## Does shaul support FTP?

Yes: plain `ftp://` links are available through the optional `ftp` feature (the
CLI enables it). FTP has no HTTP Range semantics, so downloads are
single-stream; resume uses `REST` plus `SIZE`/`MDTM` when the server supports
them. FTP authentication uses Basic credentials (`--basic` or URL userinfo).
FTPS, FTP proxies and recursive directory downloads are not enabled yet.

## Does BitTorrent seed after completion?

No. shaul is leech-only: it responds to uploads while downloading, then pauses
the torrent after completion. Piece state and fastresume data are owned by
librqbit under `<state_dir>/bt`.

## How do `add`, `run`, `pause` and `resume` interact?

`shaul add` persists a task and exits. `shaul run` is a long-running worker
that processes pending tasks; it holds a state-directory lock so only one
worker runs per state directory. `pause`/`resume`/`cancel` edit the sidecar,
and a running worker notices within about 500 ms.

## How does queue priority work?

`shaul add --priority N` stores a per-task priority; larger values are selected
first by `shaul run`. Tasks with the same priority run in creation order. The
default priority is 0.

## What is the configuration precedence?

**CLI flags > config file > environment variables > defaults.**

`shaul config path` prints the resolved file. `--config PATH` or
`SHAUL_CONFIG` overrides the location. Common environment variables include
`SHAUL_DIR`, `SHAUL_CONNECTIONS`, `SHAUL_RATE_LIMIT`, `SHAUL_FILE_JOBS`,
`SHAUL_CONTENT_CACHE`, `SHAUL_CONTENT_CACHE_DIR`, `SHAUL_DISK_SPACE_CHECK`,
`SHAUL_PER_HOST_ACTIVE`, `SHAUL_PER_HOST_RATE_LIMIT`, `SHAUL_GLOBAL`,
`SHAUL_PROXY`, `SHAUL_NO_PROXY`, `SHAUL_STATE_DIR`, `SHAUL_VERIFY`, `HF_TOKEN`
and `MODELSCOPE_API_TOKEN`; see the README for the full list.

## How do I enable logs and metrics?

Add `tracing-subscriber` in your application and install a subscriber; the
library only emits `tracing` spans/events. The CLI already initializes one:

```console
$ RUST_LOG=shaul_core=debug,shaul_http=debug shaul download https://example.com/file.bin
```

For numeric metrics, implement `shaul::Telemetry` and attach it with
`engine.with_telemetry(...)`. Stable names include `shaul.tasks.*`,
`shaul.bytes.downloaded`, `shaul.task.duration_seconds`,
`shaul.active_tasks`, `shaul.retries`, `shaul.http.active_connections`,
`shaul.queue.depth` and `shaul.cache.*`. Task events (`engine.subscribe()`)
carry per-task detail and avoid the high-cardinality labels you would not want
in a metrics backend.

See [docs/extending.md](extending.md) for complete parser, backend, finalizer
and telemetry examples.

## Does shaul support proxies, bandwidth or connection limits?

Yes in the 1.1 line:

- `network.proxy` / `--proxy` accepts `http://`, `https://` and `socks5://`.
- `network.no_proxy` / `--no-proxy` is a comma-separated bypass list.
- `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` are read by
  `Config::load`; explicit config and CLI values win.
- `download.rate_limit` sets a global bytes-per-second cap.
- `download.per_host_rate_limit` / `--per-host-limit` adds a cap shared by all
  transfers to one host.
- `TaskOptions::rate_limit` adds a per-task cap. `shaul download --limit` and
  `shaul add --limit` set that field (and `add` persists it); `shaul run
  --limit` sets the worker's global cap.
- `download.global` caps active HTTP file-transfer requests across tasks, while
  `download.per_host_active` caps them per host. File-level concurrency uses
  `download.file_jobs`; the connection budget still applies across all files.
  Hub metadata requests and FTP transfers are not counted.

The proxy covers HTTP/Hub metadata and file transfers. FTP and BitTorrent
traffic are not proxied, and BitTorrent rate limiting is not part of this
slice. The library does not read environment variables unless the caller uses
`Config::load`; `Config::default()` requires the caller to fill these fields.

## What do the JSON outputs contain?

`--json` emits NDJSON for progress events and pretty JSON for `add`, `list`,
`status`, `info`, `config show`, `doctor`, `stats` and `clean`. Sensitive
headers are replaced with `<redacted>`, HTTP query strings are stripped from
task views, and provider tokens are never included.

## Why did a download fail with 416 or an HTML file?

- `416` usually means the partial file no longer matches the remote object;
  shaul restarts the file safely.
- An HTML file where a model file was expected usually means a Hub URL was
  parsed as a direct link, or the repository is gated/private. Check
  `shaul info <link>` and the provider token.
- A `checksum mismatch` means the bytes differ from the expected digest; the
  file is not finalized.
- `insufficient disk space` means the preflight found less free space than the
  known remaining size. Free space, disable `download.disk_space_check`, or
  download to another filesystem.

## Which platforms are supported?

The library and CLI target Rust 1.92. CI covers Linux, Windows and macOS.
Prebuilt CLI binaries cover Linux x86_64/aarch64, macOS Intel/Apple Silicon
and Windows x86_64.
