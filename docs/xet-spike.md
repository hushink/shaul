# Xet backend spike

- Date: 2026-11-19
- Decision: **implement behind an opt-in `xet` feature in `shaul-hub`; keep
  HTTP resolve fallback** (no separate `shaul-xet` crate, so 1.2.0 does not
  need another new-crate bootstrap)

## Findings

- Hugging Face's Rust client is published as [`hf-xet`](https://crates.io/crates/hf-xet)
  on crates.io. The workspace `xet-core` and the published crate are
  **Apache-2.0**, so the license is compatible with shaul's dependency policy.
- The crate exposes a high-level session API and a streaming download path
  (`XetSessionBuilder`, upload/download commit/stream types). It is also used
  by `huggingface_hub` for large-file transfers.
- The Rust API has historically been described as not yet a stable public API;
  upstream issue
  [huggingface/xet-core#628](https://github.com/huggingface/xet-core/issues/628)
  tracks stabilization. A future minor upgrade could therefore be disruptive
  even though the crate version is semver-tagged.
- Integration requires more than a URL swap: parse `X-Xet-Hash`,
  `X-Xet-Cas-Url`, `X-Xet-Token-Expiration` and `X-Xet-Refresh-Route`, refresh
  short-lived tokens, perform CAS reconstruction, and fall back to the regular
  Hub resolve path when any step fails.
- The dependency is substantial (tens of thousands of lines plus its own
  runtime/network stack). Adding it to the default build or to
  `shaul-core` would violate the "heavy capability stays optional" rule.
- A meaningful e2e test requires a local CAS fixture or a mocked Xet service;
  we do not want the default test suite to depend on Hugging Face's external
  CAS. This needs a dedicated fixture design.

## Implemented design

Xet is an opt-in `xet` feature on `shaul-hub`, forwarded by `shaul` and
enabled by the CLI. It is not a separate crate: this keeps the dependency
graph optional without requiring another first-publish bootstrap.

- The HF tree parser records `xetHash` as `hub.xet_hash`.
- The Hub backend asks the `xet-read-token` endpoint for a short-lived token,
  then downloads through `hf-xet` using the existing provider auth headers.
- Any token, protocol or CAS error removes the partial output and falls back
  to the normal HTTP resolve path.
- `shaul-core` and the default `shaul` feature set do not depend on `hf-xet`.
- `hf-xet` and `xet-*` are pinned to 1.6.0: 1.7+ pulls `sysinfo` 0.39, which
  requires Rust 1.95 and would break the 1.92 MSRV. The 1.6 line still pulls
  `humantime`, covered by the documented RUSTSEC-2026-0542 ignore.

## Remaining verification

There is no external-network Xet test in the default suite. Unit tests cover
metadata/header construction and the existing Hub e2e tests exercise the HTTP
fallback. A local CAS fixture is still desirable before claiming full Xet
coverage; it should cover success, token refresh, hash mismatch and fallback
without external network.

## Acceptance criteria

1. `xet` is an optional feature; `shaul`'s default features do not change.
2. HTTP fallback is preserved on token, CAS or protocol errors.
3. No Xet-specific types leak into `shaul-core`.
4. Local fixture tests cover success, token refresh, hash mismatch and
   fallback; external network is never required.
5. The dependency and its transitive licenses pass `cargo-deny`.
