# Xet backend spike

- Date: 2026-11-19
- Decision: **defer implementation; keep HTTP resolve fallback**

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

## Decision

Do not add `hf-xet` in the 1.2.0 release. Keep the existing HTTP resolve
fallback, and treat Xet as a separate `shaul-xet` crate behind an opt-in
`xet` feature after the upstream API stabilizes or after a follow-up spike
builds a local CAS fixture.

Acceptance criteria for a future implementation:

1. `shaul-xet` is an optional crate; `shaul`'s default features do not change.
2. HTTP fallback is preserved on token, CAS or protocol errors.
3. No Xet-specific types leak into `shaul-core`.
4. Local fixture tests cover success, token refresh, hash mismatch and
   fallback; external network is never required.
5. The dependency and its transitive licenses pass `cargo-deny`.
