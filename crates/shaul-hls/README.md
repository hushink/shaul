# shaul-hls

[![crates.io](https://img.shields.io/crates/v/shaul-hls.svg)](https://crates.io/crates/shaul-hls)
[![docs.rs](https://docs.rs/shaul-hls/badge.svg)](https://docs.rs/shaul-hls)

**English** | [简体中文](README.zh-CN.md)

HLS backend for [shaul](https://crates.io/crates/shaul).

This crate adds an optional parser and backend for HTTP(S) `.m3u8` links. It
selects the highest-bandwidth variant from a master playlist, downloads media
segments concurrently, and concatenates them into one output file. Segments
are internal: the task still produces a single file.

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

`HlsParser` and `HlsBackend` can also be registered in a custom router. The
backend is protocol-isolated: `shaul-core` does not depend on HLS.

## Phase 1 scope

- VOD media playlists with `#EXT-X-ENDLIST`
- Master and media playlists, highest-bandwidth variant by default
- TS output, and fMP4 playlists with a single `#EXT-X-MAP`
- Concurrent segment downloads through the shared HTTP backend, so proxy,
  authentication, timeouts, retries, rate limits and connection budgets apply
- Segment-level resume: completed segment files are reused after a pause or
  failure

Not supported yet:

- AES-128 or SAMPLE-AES encryption
- `#EXT-X-BYTERANGE` segments
- Live playlists
- DASH (`mpd`)
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
- Cross-origin playlist and segment URLs have sensitive headers stripped
  unless `allow_cross_origin_auth` is enabled.
- A video-only variant will produce a video-only output when the master uses
  separate audio renditions.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [HLS ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0013-hls-backend.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul-hls)
