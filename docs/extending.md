# Extending shaul

shaul is built around four replaceable pieces:

```text
Link -> LinkParser -> Resource -> Router -> DownloadBackend
```

- `LinkParser` turns raw user input into a protocol-agnostic `Resource`.
- `Resource` lists files, URLs, checksums and provider metadata.
- `Router` picks the first parser that claims a link, then the first backend
  that supports the resource.
- `DownloadBackend` transfers the files and returns `FileOutcome`s.

`DownloadEngine`, the task sidecar store and the finalizer pipeline
(`verify -> extract -> rename`) are independent of these implementations.

## A custom LinkParser

The complete example is in
[`crates/shaul/examples/custom_parser.rs`](../crates/shaul/examples/custom_parser.rs).
The important parts:

```rust,ignore
struct DemoParser;

#[async_trait]
impl LinkParser for DemoParser {
    fn name(&self) -> &'static str { "demo" }

    fn supports(&self, link: &Link) -> bool {
        link.as_str().starts_with("demo://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let name = link.as_str().trim_start_matches("demo://");
        let url = Url::parse(&format!("https://example.com/{name}"))?;
        Resource::http_file(url, options.request.clone(), options.checksum.clone())
    }
}
```

Register the parser before the fallback direct-link parser:

```rust,ignore
let mut parsers = ParserRegistry::new().with(parser_arc(DemoParser));
parsers.register(parser_arc(DirectLinkParser));
let router = Router::new(parsers, vec![backend_arc(HttpBackend::new(&config)?)]);
```

Run it with:

```console
$ cargo run -p shaul --example custom_parser -- demo://model.bin
```

Guidelines:

- `supports()` must be cheap and side-effect free.
- `parse()` may perform metadata requests (Hub APIs), but should not download
  file contents.
- Populate `ResourceFile::path` with a safe relative path. Parsers for external
  names should call `sanitize_relative_path`.
- Put protocol-private data in `metadata` maps instead of changing public
  structs.
- The first parser that claims a link is authoritative; return an error rather
  than falling through to an HTML download.

## A custom DownloadBackend

```rust,ignore
struct MyBackend;

#[async_trait]
impl DownloadBackend for MyBackend {
    fn name(&self) -> &'static str { "my-backend" }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, false, false)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(resource.kind, ResourceKind::Http)
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let mut outcomes = Vec::new();
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let path = request.destination.join(&file.path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // ... transfer the file, emitting EventKind::Progress ...
            let size = tokio::fs::metadata(&path).await?.len();
            outcomes.push(FileOutcome::new(
                path,
                file.path.clone(),
                size,
                file.checksum.clone(),
                false,
            ));
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}
```

A backend must:

- honor `request.cancel` and persist resumable state through
  `request.checkpoint.save(&TaskResume)`;
- emit `EventKind::Progress`/`FileStarted`/`FileCompleted` on `request.events`;
- return paths under `request.destination` and relative paths with `/`;
- use `FileOutcome::new(...)` (the struct is `#[non_exhaustive]`);
- leave verification and extraction to the engine's finalizers unless the
  protocol already proves integrity (BitTorrent pieces set `checksum_verified`).

`BackendCapabilities` is descriptive: advertise `segmented`, `resume` and
`multi_file` accurately so callers can make policy decisions.

## Routing order

`Router` uses first-match semantics:

1. Parsers run in registration order; the first `supports() == true` is
   authoritative.
2. Backends are checked in registration order for the parsed `Resource`.

For a custom protocol, register its parser before `DirectLinkParser`. For a
backend that should win over the built-in HTTP backend, register it first.

## Tests

Keep extensions testable without external network access:

- use a local `TcpListener`/mock API for parser metadata;
- test resume by truncating the partial file and re-running;
- assert that tasks leave `Running` after a crash and are requeued by
  `DownloadEngine::requeue_stale`;
- run `cargo test --workspace --all-targets` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

See [GLOSSARY.md](../GLOSSARY.md), [docs/adr](adr), and the
[API audit](api-audit.md) for the public contract.
