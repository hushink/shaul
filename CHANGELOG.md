# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
