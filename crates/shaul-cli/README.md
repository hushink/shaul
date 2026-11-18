# shaul-cli

Command-line interface for [shaul](https://github.com/hushink/shaul), a
unified download engine for HTTP, BitTorrent, Hugging Face and ModelScope.

The crate installs a binary named `shaul`.

## Install

```console
$ cargo install shaul-cli
```

## Usage

```console
$ shaul download https://example.com/file.tar.gz
$ shaul download hf://org/repo --include "*.safetensors"
$ shaul download magnet:?xt=urn:btih:...
$ shaul add https://example.com/a.zip https://example.com/b.zip
$ shaul run
$ shaul list
$ shaul status <id-or-suffix>
$ shaul pause <id>     # a running worker notices within ~500 ms
$ shaul resume <id>
$ shaul cancel <id>
$ shaul info <link>
$ shaul config init|show|get|set|edit|path
$ shaul completions bash
```

Useful flags include `--dir`, `--output`, `--checksum`, `--verify/--no-verify`,
`--extract/--no-extract`, `--extract-dir`, `--delete-archive`, `--header`,
`--header-file`, `--bearer-token`, `--basic`, `--revision`, `--include`,
`--exclude`, `--cache/--no-cache`, `--json`, `--force`, `--overwrite` and
`--rename`.

## Configuration

Precedence: **CLI flags > config file > environment variables > defaults**.

`shaul config path` prints the resolved config path. `--config PATH` or
`SHAUL_CONFIG` overrides it. Task state is stored in a per-user data directory
and a single `shaul run` worker owns a state directory at a time.

On Unix, task sidecars are written with mode 0600. Provider tokens read from
the environment or official credential files are never persisted. `--json`
output redacts sensitive headers and strips query strings from task views.

## Links

- [Project README](https://github.com/hushink/shaul/blob/main/README.md)
- [API audit](https://github.com/hushink/shaul/blob/main/docs/api-audit.md)

MSRV is Rust 1.92. License: Apache-2.0.
