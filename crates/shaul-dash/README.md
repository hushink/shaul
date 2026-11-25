# shaul-dash

[![crates.io](https://img.shields.io/crates/v/shaul-dash.svg)](https://crates.io/crates/shaul-dash)
[![docs.rs](https://docs.rs/shaul-dash/badge.svg)](https://docs.rs/shaul-dash)

**English** | [简体中文](README.zh-CN.md)

DASH backend for [shaul](https://crates.io/crates/shaul).

This crate adds an optional parser and backend for HTTP(S) `.mpd` links. It
selects the highest-bandwidth video representation, downloads its
initialization and media segments, and concatenates them into one output file.
Static VOD manifests and bounded dynamic (live) captures are supported.
Segments are internal: the task still produces a single file.

## Usage

Most applications should depend on the `shaul` facade with the `dash` feature:

```toml
[dependencies]
shaul = { version = "1.2", features = ["dash"] }
```

```console
$ shaul download https://example.com/video/manifest.mpd --connections 8
```

Use `--include` / `--exclude` globs to choose a representation by `id`,
`mimeType`, `codecs`, `lang`, resolution or `bandwidth`:

```console
$ shaul download https://example.com/video/manifest.mpd --include '*1080*'
```

Record a dynamic live stream with a duration or segment limit:

```console
$ shaul download https://example.com/live/manifest.mpd --live-duration 10m
$ shaul download https://example.com/live/manifest.mpd --live-segments 300
```

`DashParser` and `DashBackend` can also be registered in a custom router. The
backend is protocol-isolated: `shaul-core` does not depend on DASH.

## Supported

- Static (`type="static"`) and dynamic (`type="dynamic"`) manifests with exactly
  one Period
- Highest-bandwidth video representation by default; audio-only manifests fall
  back to the best audio representation
- `SegmentTemplate` with `$Number$`, `$Time$`, `$RepresentationID$`,
  `$Bandwidth$`, `%0Nd` formatting, constant `duration`, `endNumber` or
  `SegmentTimeline` (non-negative repeat counts)
- Dynamic manifests require `SegmentTemplate` + `SegmentTimeline`; the backend
  polls at `minimumUpdatePeriod`, deduplicates by presentation time, and stops
  at a duration/segment limit or when the MPD becomes static
- `SegmentList` with optional `Initialization` and `mediaRange` byte ranges
- `SegmentBase`/BaseURL single-object representations (the whole object is
  downloaded; `sidx` is not parsed)
- BaseURL inheritance across MPD, Period, AdaptationSet and Representation
- Concurrent segment downloads through the shared HTTP backend, so proxy,
  authentication, timeouts, retries, rate limits and connection budgets apply
- Segment-level VOD resume: completed segment files are reused after a pause or
  failure (dynamic captures are not resumable)

Not supported yet:

- Dynamic manifests without a `SegmentTimeline`, or with multiple Periods
- DRM (`ContentProtection`, CENC) and `xlink:href`
- Subtitles, thumbnails, alternate audio muxing and multi-track selection
- ffmpeg remuxing
- `sidx` parsing and segment-internal byte-level resume
- Content-addressed cache for the final output
- `--mirror` (ignored for DASH tasks)

The output extension is `.webm` for WebM representations, otherwise `.mp4`.
`--output` overrides it.

## Notes

- The backend re-fetches the manifest at download time and uses its SHA-256,
  the selected representation id and the segment count as the resume
  fingerprint.
- Completed segments live in `<output>.shaul.dash.parts/` and are concatenated
  through `<output>.shaul.part`; the temporary directory is removed on success.
  `shaul clean --orphans` removes the directory (and the concatenation
  `.part`) left by completed or cancelled tasks.
- Cross-origin manifest and segment URLs have sensitive headers stripped unless
  `allow_cross_origin_auth` is enabled.
- A video-only representation will produce a video-only output when the
  manifest uses separate audio AdaptationSets.
- Dynamic captures always start from a fresh temp directory and the current
  timeline window. Cancelling or failing keeps the recorded segments;
  `clean --orphans` removes temp files from failed dynamic captures as well as
  completed/cancelled tasks. The output is finalized only when the capture
  stops.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [DASH ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0016-dash-backend.md)
- [Live ADR](https://github.com/hushink/shaul/blob/main/docs/adr/0018-dash-live.md)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul-dash)
