# shaul-hls

[![crates.io](https://img.shields.io/crates/v/shaul-hls.svg)](https://crates.io/crates/shaul-hls)
[![docs.rs](https://docs.rs/shaul-hls/badge.svg)](https://docs.rs/shaul-hls)

**English** | [简体中文](README.zh-CN.md)

HLS backend for [shaul](https://crates.io/crates/shaul).

This crate adds an optional parser and backend for HTTP(S) `.m3u8` links. It
selects the highest-bandwidth variant from a master playlist, downloads media
segments concurrently, and concatenates them into one output file. VOD and
bounded live capture are supported. Segments are internal: the task still
produces a single file.

## Usage

Most applications should depend on the `shaul` facade with the `hls` feature:

```toml
[dependencies]
shaul = { version = "1.2", features = ["hls"] }
```

```console
$ shaul download https://example.com/video/master.m3u8 --connections 8
```

Use `--include` / `--exclude` globs to choose a variant by `NAME`,
`RESOLUTION`, `BANDWIDTH`, `CODECS` or URI:

```console
$ shaul download https://example.com/video/master.m3u8 --include '*1080*'
```

Record a live stream with a duration or segment limit:

```console
$ shaul download https://example.com/live/index.m3u8 --live-duration 10m
$ shaul download https://example.com/live/index.m3u8 --live-segments 300
```

`HlsParser` and `HlsBackend` can also be registered in a custom router. The
backend is protocol-isolated: `shaul-core` does not depend on HLS.

## Supported

- VOD media playlists with `#EXT-X-ENDLIST`
- Master and media playlists, highest-bandwidth variant by default
- TS output, and fMP4 playlists with a single `#EXT-X-MAP`
- AES-128 encryption, including key rotation and explicit or media-sequence
  IVs
- `#EXT-X-BYTERANGE` segments and init maps, including implicit offsets
- Concurrent segment downloads through the shared HTTP backend, so proxy,
  authentication, timeouts, retries, rate limits and connection budgets apply
- Segment-level resume for VOD: completed segment files are reused after a
  pause or failure
- Bounded live capture with `live_duration_secs` (`--live-duration`) or
  `live_segments` (`--live-segments`); the backend polls at the target
  duration, records new media sequence numbers, and stops at the limit or
  `#EXT-X-ENDLIST`

Not supported yet:

- SAMPLE-AES encryption
- Unbounded live capture (a duration or segment limit is required)
- Live resume (a new run starts from the current window)
- DASH (use the separate `shaul-dash` crate)
- Alternate audio renditions and multi-track selection
- ffmpeg remuxing
- Content-addressed cache for the final output
- `--mirror` (ignored for HLS tasks)

The output extension is `.mp4` when the playlist has an init map, otherwise
`.ts`. `--output` overrides it.

## Notes

- The backend re-fetches the media playlist at download time and uses its
  SHA-256 as a resume fingerprint.
- Completed segments live in `<output>.shaul.hls.parts/` and are concatenated
  through `<output>.shaul.part`; the temporary directory is removed on success.
  `shaul clean --orphans` removes the directory (and the concatenation
  `.part`) left by completed or cancelled tasks.
- Cross-origin playlist, key and segment URLs have sensitive headers stripped
  unless `allow_cross_origin_auth` is enabled.
- AES-128 keys are fetched at run time, kept in memory only and never written
  to the task sidecar. Encrypted segments are staged as `.enc` files and
  decrypted to the `.bin` files used for resume and concatenation.
- Byte-range segments use exact `Range` requests; a server that ignores
  `Range` and returns the full object falls back to discarding the prefix.
- A video-only variant will produce a video-only output when the master uses
  separate audio renditions.
- Live captures always start from a fresh temp directory. Cancelling or
  failing keeps the recorded segments; `clean --orphans` removes temp files
  from failed live captures as well as completed/cancelled tasks. The output
  is finalized only when the capture stops.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [HLS ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0013-hls-backend.md)
- [AES-128 ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0014-hls-aes128.md)
- [BYTERANGE ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0015-hls-byterange.md)
- [Live ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0017-hls-live.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul-hls)
