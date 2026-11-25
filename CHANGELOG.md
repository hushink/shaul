# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(http,cli)* support non-GET direct downloads with an optional request body
  (`--method`, `--data`, `--data-file`), including single-shot semantics,
  RFC-style redirect handling and `--json` body redaction
- *(hls)* add the optional `shaul-hls` crate and `hls` feature: VOD `.m3u8`
  master/media playlists, highest-bandwidth variant selection, concurrent
  segment downloads, TS/fMP4 concatenation and segment-level resume
- *(hls)* add AES-128 decryption with key rotation, explicit or media-sequence
  IVs, and encrypted-segment staging
- *(http,hls)* add exact byte-range downloads (`HttpBackend::download_range`)
  and support `#EXT-X-BYTERANGE` segments and init maps, including implicit
  offsets and a full-object fallback
- *(dash)* add the optional `shaul-dash` crate and `dash` feature: static
  single-period VOD `.mpd` manifests, highest-bandwidth representation
  selection, SegmentTemplate/SegmentList/SegmentBase addressing, byte ranges,
  concurrent segment downloads and segment-level resume
- *(cli)* batch `pause`/`resume`/`cancel` with multiple ids or `--all`, plus
  `list --status` state filters
- *(cli)* `clean --orphans` removes HLS/DASH segment temp directories for
  completed/cancelled tasks, reporting their recursive size and skipping
  symlinked or unsafe paths

## [1.2.2](https://github.com/hushink/shaul/compare/shaul-v1.2.1...shaul-v1.2.2) - 2026-11-24

### Fixed

- *(http,hub,bt)* harden transfers, provider paths and overwrite behavior
- *(core)* validate configuration and harden task execution
- *(cli)* stop rendering a duplicate progress bar
- *(cli)* redact secrets and make config edits safe

### Other

- add bilingual READMEs and document hardening

## [1.2.1](https://github.com/hushink/shaul/compare/shaul-v1.2.0...shaul-v1.2.1) - 2026-11-21

### Fixed

- *(cli)* show live download progress

### Other

- rewrite READMEs for users
- update release status to 1.2.0

## [1.2.0](https://github.com/hushink/shaul/compare/shaul-v1.1.0...shaul-v1.2.0) - 2026-11-19

### Added

- add Xet downloads and per-host rate limiting
- add content cache and maintenance commands
- harden downloads and cache maintenance

### Fixed

- keep Xet on the 1.92 MSRV

## [1.1.0](https://github.com/hushink/shaul/compare/shaul-v1.0.3...shaul-v1.1.0) - 2026-11-19

### Added

- add queue priority and observable retries
- fall back to compatible HTTP mirrors
- enforce global and per-host HTTP request budgets
- download multi-file tasks with bounded concurrency
- add an optional FTP backend
- add proxy, bandwidth limits and blocking lifecycle parity
- make archive extraction replaceable
- expose custom protocols, telemetry and finalizer injection

### Fixed

- fall back gracefully when resume is unsupported
- preserve 1.x semver compatibility

### Other

- add library embedding guide and examples
- document custom protocols, telemetry and tracing
- add local HTTP throughput and resume baseline
- keep finalizer context unwind-safe

## [1.0.3](https://github.com/hushink/shaul/compare/shaul-v1.0.2...shaul-v1.0.3) - 2026-11-18

### Fixed

- reject cancelling completed tasks

### Other

- document all public APIs and enforce missing_docs
- add extension guide, FAQ and a custom parser example
- cover corrupt sidecars and worker crash recovery

## [1.0.2](https://github.com/hushink/shaul/compare/shaul-v1.0.1...shaul-v1.0.2) - 2026-11-18

### Other

- *(cli)* cover queue, config precedence and JSON redaction

## [1.0.1](https://github.com/hushink/shaul/compare/shaul-v1.0.0...shaul-v1.0.1) - 2026-11-18

### Other

- update Cargo.toml dependencies
- update Cargo.lock dependencies

## [1.0.0](https://github.com/hushink/shaul/releases/tag/shaul-v1.0.0) - 2026-11-18

### Added

- initial shaul 0.3.0 download engine

### Other

- add per-crate README files
- prepare 1.0 release automation with release-plz
- audit 1.0 API and add cross-platform CI
