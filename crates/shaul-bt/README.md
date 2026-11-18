# shaul-bt

BitTorrent support for [shaul](https://github.com/hushink/shaul), built on
[`librqbit`](https://crates.io/crates/librqbit).

## Supported inputs

- magnet links (`magnet:?xt=urn:btih:...`)
- bare 40-character v1 info hashes
- local `.torrent` files

## Features

- embedded librqbit session with DHT, trackers, listening and persistence
- file selection with include/exclude globs
- progress delivered through shaul's typed event stream
- leech-only behavior: uploads are served while downloading, then the torrent
  is paused after completion; shaul does not keep seeding

## Usage

The usual entry point is the `shaul` facade with the `bt` feature:

```toml
[dependencies]
shaul = { version = "1.0", features = ["bt"] }
```

`shaul-bt` can also be embedded directly: `TorrentParser` parses torrent
inputs, and `BtBackend` owns the librqbit session. The backend requires a
Tokio runtime.

## Links

- [Project README](https://github.com/hushink/shaul/blob/main/README.md)
- [Leech-only ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0005-bt-leech-only.md)
- [API audit](https://github.com/hushink/shaul/blob/main/docs/api-audit.md)

MSRV is Rust 1.92. License: Apache-2.0.
