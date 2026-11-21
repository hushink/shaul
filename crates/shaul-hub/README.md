# shaul-hub

[![crates.io](https://img.shields.io/crates/v/shaul-hub.svg)](https://crates.io/crates/shaul-hub)
[![docs.rs](https://docs.rs/shaul-hub/badge.svg)](https://docs.rs/shaul-hub)

[Hugging Face](https://huggingface.co) and
[ModelScope](https://modelscope.cn) support for
[shaul](https://github.com/hushink/shaul), a multi-protocol download engine.

## Supported links

- `hf://org/repo`
- `hf://datasets/org/repo`
- `ms://org/repo`
- `ms://datasets/org/repo`
- Hugging Face HTTPS `/tree/<rev>`, `/blob/<rev>/<path>` and
  `/resolve/<rev>/<path>` URLs
- ModelScope HTTPS repository URLs with an optional `Revision` query parameter

For `hf://` and `ms://` links, select a revision with `--revision <rev>`.
HTTPS URLs also accept `/tree/<rev>`, `/blob/<rev>/<path>`,
`/resolve/<rev>/<path>` or `?revision=`.

## Features

- Hugging Face and ModelScope tree/file-list APIs
- `X-Linked-Etag`, `X-Linked-Size` and commit metadata capture
- Clear errors for gated or private repositories
- Optional provider cache:
  - Hugging Face: `models--org--repo/{blobs,refs,snapshots}` (Python-compatible)
  - ModelScope: shaul's `models--org--repo` layout
- Hardlink-or-copy from cache blobs into the destination
- Include/exclude globs for selecting files
- Optional Hugging Face Xet downloads through the `xet` feature, with HTTP
  fallback when token or CAS operations fail
- Provider credentials resolved at run time and never written to task sidecars

## Credentials

- Hugging Face: `HF_TOKEN` or `HUGGING_FACE_HUB_TOKEN`
- ModelScope: `MODELSCOPE_API_TOKEN` or the official cookie file
- User-supplied `--bearer-token`, `--basic` or headers take precedence

## Usage

Most users depend on the [`shaul`](https://crates.io/crates/shaul) facade.
`HubBackend`, `HuggingFaceParser`, `ModelScopeParser` and `HubClient` are
available for custom routers and tooling.

```toml
[dependencies]
shaul-hub = "1.2"

# Optional Hugging Face Xet support:
# shaul-hub = { version = "1.2", features = ["xet"] }
```

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [Library guide](https://github.com/hushink/shaul/blob/main/docs/library.md)
- [API reference](https://docs.rs/shaul-hub)
