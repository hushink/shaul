# shaul-cli

[![crates.io](https://img.shields.io/crates/v/shaul-cli.svg)](https://crates.io/crates/shaul-cli)

Command-line interface for [shaul](https://github.com/hushink/shaul), a
unified download engine for HTTP, Hugging Face, ModelScope, BitTorrent, FTP
and optional Xet.

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

# Hugging Face and ModelScope
$ shaul download hf://org/repo --revision main --include '*.safetensors' --cache
$ shaul download https://modelscope.cn/models/org/repo

# BitTorrent and FTP
$ shaul download 'magnet:?xt=urn:btih:<info-hash>'
$ shaul download ./example.torrent --include '*.mkv' --dir ./media
$ shaul download ftp://user:password@example.com/pub/file.bin

# Proxy and bandwidth limits
$ shaul download https://example.com/file.bin \
      --proxy socks5://127.0.0.1:1080 \
      --limit 10MiB --per-host-limit 5MiB

# Persistent queue
$ shaul add --input-file links.txt
$ shaul run --limit 50MiB
$ shaul list
$ shaul status <id-or-suffix>
$ shaul pause <id>     # a running worker notices within ~500 ms
$ shaul resume <id>
$ shaul cancel <id>

# Diagnostics and maintenance
$ shaul info <link>
$ shaul doctor
$ shaul stats
$ shaul clean --finished --cache
$ shaul config init|show|get|set|edit|path
$ shaul completions bash
```

Common flags: `--dir`, `--output`, `--checksum`, `--verify/--no-verify`,
`--extract/--no-extract`, `--extract-dir`, `--delete-archive`, `--header`,
`--header-file`, `--bearer-token`, `--basic`, `--revision`, `--include`,
`--exclude`, `--cache/--no-cache`, `--content-cache/--no-content-cache`,
`--file-jobs`, `--mirror`, `--priority`, `--global-connections`,
`--per-host-active`, `--per-host-limit`, `--json`, `--force`, `--overwrite`
and `--rename`.

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

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
