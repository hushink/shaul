# shaul-core

Core domain types and download engine for [shaul](https://github.com/hushink/shaul),
a multi-protocol download engine for HTTP, BitTorrent, Hugging Face and
ModelScope.

This crate is protocol-agnostic. The pipeline is:

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
```

`DownloadEngine` drives tasks, persists resumable state, emits typed events,
and runs verification and archive-extraction finalizers.

## What is included

- `Link`, `Resource`, `Router`, `ParserRegistry`
- `LinkParser` and `DownloadBackend` extension traits
- task sidecars (`SidecarStore`), resume manifests and validators
- segmented-range helpers and SHA-256/SHA-512/SHA-1/MD5 checksums
- safe relative-path handling and archive detection/extraction
- verified, content-addressed cache (`ContentCache`)
- async and blocking engine facades
- structured `DownloadEvent` stream and low-cardinality telemetry hooks

## When to use it

Most applications should use the [`shaul`](https://crates.io/crates/shaul)
facade, which assembles the HTTP, Hub and optional BitTorrent backends.

Use `shaul-core` directly when you are building a custom parser, backend or
router, or when you want to embed only the engine and task store.

## Stability

The public API follows the 1.0 semver policy documented in
[docs/api-audit.md](https://github.com/hushink/shaul/blob/main/docs/api-audit.md).
MSRV is Rust 1.92.

## License

Apache-2.0. See [LICENSE](https://github.com/hushink/shaul/blob/main/LICENSE).
