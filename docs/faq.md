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
`200 + Content-Range` is treated as a partial response.

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
or skipped. Supported formats: zip, tar, tar.gz, tar.zst, gzip and zstd.

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

## Does BitTorrent seed after completion?

No. shaul is leech-only: it responds to uploads while downloading, then pauses
the torrent after completion. Piece state and fastresume data are owned by
librqbit under `<state_dir>/bt`.

## How do `add`, `run`, `pause` and `resume` interact?

`shaul add` persists a task and exits. `shaul run` is a long-running worker
that processes pending tasks; it holds a state-directory lock so only one
worker runs per state directory. `pause`/`resume`/`cancel` edit the sidecar,
and a running worker notices within about 500 ms.

## What is the configuration precedence?

**CLI flags > config file > environment variables > defaults.**

`shaul config path` prints the resolved file. `--config PATH` or
`SHAUL_CONFIG` overrides the location. Common environment variables include
`SHAUL_DIR`, `SHAUL_CONNECTIONS`, `SHAUL_RATE_LIMIT`, `SHAUL_PROXY`,
`SHAUL_NO_PROXY`, `SHAUL_STATE_DIR`, `SHAUL_VERIFY`, `HF_TOKEN` and
`MODELSCOPE_API_TOKEN`; see the README for the full list.

## How do I enable logs and metrics?

Add `tracing-subscriber` in your application and install a subscriber; the
library only emits `tracing` spans/events. The CLI already initializes one:

```console
$ RUST_LOG=shaul_core=debug,shaul_http=debug shaul download https://example.com/file.bin
```

For numeric metrics, implement `shaul::Telemetry` and attach it with
`engine.with_telemetry(...)`. Stable names include `shaul.tasks.*`,
`shaul.bytes.downloaded`, `shaul.task.duration_seconds` and
`shaul.active_tasks`. Task events (`engine.subscribe()`) carry per-task detail
and avoid the high-cardinality labels you would not want in a metrics backend.

See [docs/extending.md](extending.md) for complete parser, backend, finalizer
and telemetry examples.

## Does shaul support proxies or bandwidth limits?

Yes in the 1.1 line:

- `network.proxy` / `--proxy` accepts `http://`, `https://` and `socks5://`.
- `network.no_proxy` / `--no-proxy` is a comma-separated bypass list.
- `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` are read by
  `Config::load`; explicit config and CLI values win.
- `download.rate_limit` sets a global bytes-per-second cap.
- `TaskOptions::rate_limit` adds a per-task cap. `shaul download --limit` and
  `shaul add --limit` set that field (and `add` persists it); `shaul run
  --limit` sets the worker's global cap.

The proxy covers HTTP/Hub metadata and file transfers. BitTorrent DHT/UDP
traffic is not proxied. BitTorrent rate limiting is not part of this slice.
The library does not read environment variables unless the caller uses
`Config::load`; `Config::default()` requires the caller to fill these fields.

## What do the JSON outputs contain?

`--json` emits NDJSON for progress events and pretty JSON for `add`, `list`,
`status`, `info` and `config show`. Sensitive headers are replaced with
`<redacted>`, HTTP query strings are stripped from task views, and provider
tokens are never included.

## Why did a download fail with 416 or an HTML file?

- `416` usually means the partial file no longer matches the remote object;
  shaul restarts the file safely.
- An HTML file where a model file was expected usually means a Hub URL was
  parsed as a direct link, or the repository is gated/private. Check
  `shaul info <link>` and the provider token.
- A `checksum mismatch` means the bytes differ from the expected digest; the
  file is not finalized.

## Which platforms are supported?

The library and CLI target Rust 1.92. CI covers Linux, Windows and macOS.
Prebuilt CLI binaries cover Linux x86_64/aarch64, macOS Intel/Apple Silicon
and Windows x86_64.
