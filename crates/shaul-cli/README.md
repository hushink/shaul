# shaul-cli

[![crates.io](https://img.shields.io/crates/v/shaul-cli.svg)](https://crates.io/crates/shaul-cli)

**English** | [简体中文](README.zh-CN.md)

Command-line interface for [shaul](https://github.com/hushink/shaul), a
unified download engine for HTTP, HLS, DASH, Hugging Face, ModelScope,
BitTorrent, FTP and optional Xet.

The crate installs a binary named `shaul`.

## Install

```console
$ cargo install shaul-cli
```

Prebuilt archives and installers for Linux (x86_64/aarch64), macOS
(x86_64/aarch64) and Windows (x86_64) are attached to the
[`shaul-cli` releases](https://github.com/hushink/shaul/releases).

## Usage

```console
# Direct HTTP
$ shaul download https://example.com/file.tar.gz
$ shaul download https://example.com/file.bin --connections 8 \
      --checksum sha256:<hex>
$ shaul download https://example.com/api/export --method POST \
      --data '{"id":42}' -H 'Content-Type: application/json'

# Hugging Face and ModelScope
$ shaul download hf://org/repo --revision main --include '*.safetensors' --cache
$ shaul download https://modelscope.cn/models/org/repo

# BitTorrent and FTP
$ shaul download 'magnet:?xt=urn:btih:<info-hash>'
$ shaul download ./example.torrent --include '*.mkv' --dir ./media
$ shaul download ftp://user:password@example.com/pub/file.bin

# HLS: highest-bandwidth variant, concatenated into one file
$ shaul download https://example.com/video/master.m3u8 --connections 8
$ shaul download https://example.com/video/master.m3u8 --include '*1080*'

# DASH: highest-bandwidth video representation from a static VOD manifest
$ shaul download https://example.com/video/manifest.mpd --connections 8
$ shaul download https://example.com/video/manifest.mpd --include '*1080*'

# Proxy and bandwidth limits
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

# Diagnostics and maintenance
$ shaul info <link>
$ shaul doctor
$ shaul stats
$ shaul clean --finished --cache
$ shaul config init|show|get|set|edit|path
$ shaul completions bash
```

Common flags: `--dir`, `--output`, `--checksum`, `--verify/--no-verify`,
`--extract/--no-extract`, `--extract-dir`, `--delete-archive`,
`--no-delete-archive`, `--header`,
`--header-file`, `--bearer-token`, `--basic`, `--method`, `--data`,
`--data-file`, `--revision`, `--include`,
`--exclude`, `--cache/--no-cache`, `--content-cache/--no-content-cache`,
`--file-jobs`, `--mirror`, `--priority`, `--global-connections`,
`--per-host-active`, `--per-host-limit`, `--json`, `--force`, `--overwrite`
and `--rename`.

`pause` / `resume` / `cancel` accept several ids, or `--all`. `--all` defaults
to pending/running/paused tasks for pause and cancel, and to paused/failed
tasks for resume; `--status` overrides the default.
`list --status` accepts repeated state names.

`--include` / `--exclude` also choose an HLS variant by `NAME`, `RESOLUTION`,
`BANDWIDTH`, `CODECS` or URI, and a DASH Representation by `id`, `mimeType`,
`codecs`, `lang`, resolution or `bandwidth`. HLS supports VOD TS and
single-init-map fMP4 playlists, AES-128 encrypted segments,
`#EXT-X-BYTERANGE` and segment-level resume. DASH supports static
single-period manifests with SegmentTemplate, SegmentList or SegmentBase
addressing and segment-level resume. SAMPLE-AES, live playlists, DRM and
multi-period DASH are not supported yet.

`--method`, `--data` and `--data-file` apply to direct HTTP(S) links only.
`--data` implies `POST` when `--method` is not given. These requests are
single-shot: they do not segment, resume, use mirrors or retry `POST`/`PATCH`
automatically. The body supports `${env:NAME}` placeholders and is redacted
from `--json` output. `--data-file` is read when the command runs and its
contents are persisted in the task sidecar like an inline `--data` body.

`shaul run --jobs N` processes tasks concurrently. `shaul download --limit`
and `shaul add --limit` cap one task; `shaul run --limit` caps the worker
globally.

## Configuration

Precedence is **CLI flags > config file > environment variables > defaults**.

`shaul config path` prints the resolved config path. `--config PATH` or
`SHAUL_CONFIG` overrides it. Task state is stored in a per-user data directory;
a single `shaul run` worker owns a state directory at a time.

On Unix, task sidecars are written with mode 0600. Provider tokens read from
the environment or official credential files are never persisted. `--json`
output redacts sensitive headers and strips query strings from task views.

Configuration files reject unknown keys, so a typo fails loudly instead of
being ignored. `shaul config set` edits the TOML document in place and keeps
existing comments and formatting. `shaul doctor` also checks the official
Hugging Face and ModelScope credential files. Verification failures are renamed
to `<name>.shaul.failed`, and `clean --orphans` ignores unsafe paths recorded in
a sidecar. It removes leftover `.shaul.part` files plus HLS
`.shaul.hls.parts/` and DASH `.shaul.dash.parts/` directories for completed or
cancelled tasks, does not follow symlinks while sizing them, and never deletes
destination files.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
