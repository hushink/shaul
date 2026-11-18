# shaul-hub

[Hugging Face](https://huggingface.co) and
[ModelScope](https://modelscope.cn) support for
[shaul](https://github.com/hushink/shaul), a multi-protocol download engine.

## Supported links

- `hf://org/repo`
- `hf://datasets/org/repo`
- `ms://org/repo`
- `ms://datasets/org/repo`
- HTTPS Hugging Face `/tree/<rev>`, `/blob/<rev>/<path>` and
  `/resolve/<rev>/<path>` URLs
- HTTPS ModelScope repository URLs (optional `Revision` query parameter)

For `hf://` and `ms://` links, select a revision with `--revision <rev>`.
HTTPS URLs additionally accept `/tree/<rev>`, `/blob/<rev>/<path>`,
`/resolve/<rev>/<path>` or a `?revision=` query parameter.

Repository downloads discover the file list through the provider API, then
download each file with the segmented HTTP backend.

## Features

- Hugging Face and ModelScope tree/file-list APIs
- `X-Linked-Etag`, `X-Linked-Size` and commit metadata capture
- gated/private repository errors surfaced clearly
- optional provider cache:
  - Hugging Face: `models--org--repo/{blobs,refs,snapshots}` (Python-compatible)
  - ModelScope: shaul's own `models--org--repo` layout
- hardlink-or-copy from cache blobs into the destination
- include/exclude globs for selecting files
- provider credentials resolved at run time and never written to task sidecars

## Credentials

- Hugging Face: `HF_TOKEN` or `HUGGING_FACE_HUB_TOKEN`
- ModelScope: `MODELSCOPE_API_TOKEN` or the official cookie file
- User-supplied `--bearer-token` / `--basic` / headers take precedence

## Usage

Most users depend on the [`shaul`](https://crates.io/crates/shaul) facade.
`HubBackend`, `HuggingFaceParser`, `ModelScopeParser` and `HubClient` are
available for custom routers and tooling.

## Links

- [Project README](https://github.com/hushink/shaul/blob/main/README.md)
- [API audit](https://github.com/hushink/shaul/blob/main/docs/api-audit.md)

MSRV is Rust 1.92. License: Apache-2.0.
