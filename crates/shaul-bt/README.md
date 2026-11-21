# shaul-bt

[![crates.io](https://img.shields.io/crates/v/shaul-bt.svg)](https://crates.io/crates/shaul-bt)
[![docs.rs](https://docs.rs/shaul-bt/badge.svg)](https://docs.rs/shaul-bt)

BitTorrent support for [shaul](https://github.com/hushink/shaul), built on
[`librqbit`](https://crates.io/crates/librqbit).

## Supported inputs

- Magnet links (`magnet:?xt=urn:btih:...`)
- Bare 40-character BitTorrent v1 info hashes
- Local `.torrent` files

## Features

- Embedded librqbit session with DHT, trackers, listening and persistence
- File selection with include/exclude globs
- Progress delivered through shaul's typed event stream
- Leech-only behavior: uploads are served while downloading, then the torrent
  is paused after completion; shaul does not keep seeding

## Usage

The usual entry point is the `shaul` facade with the `bt` feature:

```toml
[dependencies]
shaul = { version = "1.2", features = ["bt"] }
```

`shaul-bt` can also be embedded directly: `TorrentParser` parses torrent
inputs, and `BtBackend` owns the librqbit session. The backend requires a Tokio
runtime.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [Leech-only ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0005-bt-leech-only.md)
- [API reference](https://docs.rs/shaul-bt)
