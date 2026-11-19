# Resume and hardening

Resume is a contract between the engine, the task sidecar and each backend. It
is conservative by default: when a backend or server cannot prove that
partial bytes still belong to the same object, shaul restarts the file instead
of appending to stale data.

## Controls

- `download.resume` / `TaskOptions.resume` enable or disable resume.
  CLI: `--resume` / `--no-resume`.
- `BackendCapabilities::resume` tells the engine whether a backend can consume
  a `TaskResume` at all.
- `TaskResume` is stored in the task sidecar (`TASK_SCHEMA_VERSION = 1`).
  Each `FileResume` records the file path, part path, validator, completed
  byte ranges, size and checksum.
- `DownloadEngine` clears `DownloadRequest::resume` and sets
  `ResolvedOptions::resume = false` when the selected backend declares
  `resume = false`. It emits a warning if a saved manifest had to be
  discarded.

## Backend matrix

| Backend | Segmented | Resume | Notes |
| --- | --- | --- | --- |
| HTTP | yes | yes | Strong `ETag`/`Last-Modified`/size validators, `If-Range`, byte-range checkpoints |
| Hub | yes | yes | File transfers use the HTTP backend; cache hits skip network bytes |
| BitTorrent | no | yes | librqbit fastresume/piece state; piece hashes are the integrity check |
| FTP | no | conditional | `SIZE`/`MDTM` + `REST`; falls back to a fresh transfer if REST is rejected |
| Custom | backend-defined | backend-defined | Must honor `options.resume` and may ignore `resume` if it returns `resume = false` |

## HTTP

- A probe records `ETag`, `Last-Modified`, size and optional checksums.
- A stored manifest is reused only when its validator still matches.
- `If-Range` is sent for strong validators.
- A `206` response is range-checked against the request.
- `200` with `Content-Range` is treated as a partial response (ModelScope).
- A server that ignores `Range` triggers one single-stream fallback.
- `416` restarts the file instead of trusting the stored ranges.
- Completed ranges are clamped to the actual `.part` length before resume.
- A checksum mismatch discards the outputs and retries once from scratch.
- Mirror fallback does not reuse the primary URL's partial bytes; the mirror
  transfer starts from its own checkpoint after the compatibility check.

## FTP

- `SIZE` and `MDTM` are collected when the server provides them.
- Resume requires `options.resume`, a non-empty validator and a valid part
  file.
- `REST offset` is sent before `RETR`.
- If the server rejects REST, the backend reconnects, truncates the part,
  saves a zero-offset checkpoint and transfers from the beginning. This is an
  automatic fallback; `--no-resume` is not required.
- If `SIZE`/`MDTM` are unavailable, the file is downloaded from zero because
  shaul cannot prove that the remote object is unchanged.
- FTP has no strong ETag; a server that changes content without changing
  `MDTM`/size can defeat the validator. Supply a checksum when possible.

## Custom backends

When implementing `DownloadBackend`:

1. Return `BackendCapabilities::new(segmented, resume, multi_file)` accurately.
2. If `resume = true`, honor `DownloadRequest::resume` and
   `ResolvedOptions::resume`; checkpoint through
   `DownloadRequest::checkpoint`.
3. If `resume = false`, ignore the manifest and truncate any partial output
   before writing.
4. Treat a backend as the authority for its own format; the engine does not
   inspect backend files.

The full request type and extension guide are in [extending.md](extending.md).

## Hardening assessment

### Protected today

- Validator mismatch → restart, never append.
- Part shorter than the checkpoint → ranges are clamped to the actual file.
- Part longer than the checkpoint → resume starts at the checkpoint and
  overwrites the tail.
- Server ignores `Range` → single-stream fallback; no range state is trusted.
- Server rejects `REST` on FTP → automatic reconnect and restart from zero.
- Backend without resume → engine clears the manifest and disables resume.
- User passes `--no-resume` → the engine passes no manifest to the backend.
- Failed verification → outputs are reset once and retried from scratch.
- Final files are renamed into place only after finalizers succeed.

### Residual risks

- FTP validators are weak; content can change without `MDTM`/size changing.
  Use `--checksum` for important files.
- A server can return incorrect data with a matching validator; HTTPS and
  checksums are the remaining defenses.
- Resume data is local. If the part file is edited or damaged between runs,
  only a checksum can prove correctness.
- Disk corruption, filesystem quota changes and external cleanup can remove
  part data; shaul restarts when it cannot validate it.
- Custom backends are responsible for implementing the contract; the engine
  cannot inspect their partial files.

## Test coverage

- HTTP: segmented transfer, resume, validator invalidation, 416, 200 fallback,
  mirror fallback, multi-file concurrency and connection budgets.
- FTP: full transfer, empty file, Basic auth, REST resume and REST rejection
  fallback.
- Engine: backends that declare `resume = false` receive no manifest and
  `options.resume = false`.
- BitTorrent: local peer/piece verification and selection tests.
