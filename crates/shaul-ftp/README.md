# shaul-ftp

FTP backend for [shaul](https://crates.io/crates/shaul).

This crate adds an optional `ftp://` parser and a single-stream, resumable FTP
backend. It uses [`suppaftp`](https://crates.io/crates/suppaftp) and supports
`SIZE`, `MDTM` and `REST` when the server provides them. FTP does not expose
Range requests, so downloads are not segmented.

Most applications should depend on the `shaul` facade with the `ftp` feature:

```toml
[dependencies]
shaul = { version = "1.1", features = ["ftp"] }
```

The backend is protocol-isolated: `shaul-core` does not depend on FTP or
`suppaftp`. FTP transfers also participate in the opt-in content-addressed
cache when a checksum and size are known. FTPS is not part of the first
implementation.

License: Apache-2.0.
