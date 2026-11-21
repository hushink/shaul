# shaul-ftp

[![crates.io](https://img.shields.io/crates/v/shaul-ftp.svg)](https://crates.io/crates/shaul-ftp)
[![docs.rs](https://docs.rs/shaul-ftp/badge.svg)](https://docs.rs/shaul-ftp)

FTP backend for [shaul](https://crates.io/crates/shaul).

This crate adds an optional `ftp://` parser and a single-stream, resumable FTP
backend. It uses [`suppaftp`](https://crates.io/crates/suppaftp) and supports
`SIZE`, `MDTM` and `REST` when the server provides them. FTP does not expose
Range requests, so downloads are not segmented.

## Usage

Most applications should depend on the `shaul` facade with the `ftp` feature:

```toml
[dependencies]
shaul = { version = "1.2", features = ["ftp"] }
```

```console
$ shaul download ftp://user:password@example.com/pub/file.bin --dir ./downloads
```

`FtpParser` and `FtpBackend` can also be registered in a custom router. The
backend is protocol-isolated: `shaul-core` does not depend on FTP or
`suppaftp`.

## Notes

- Authentication uses Basic credentials from URL userinfo or `--basic`.
- Resume uses `REST` plus `SIZE`/`MDTM`; if the server rejects `REST`, shaul
  reconnects and restarts from zero.
- FTP transfers participate in the opt-in content-addressed cache when a
  checksum and size are known.
- FTPS is not enabled.

## Documentation

- [Project README](https://github.com/hushink/shaul#readme)
- [FAQ](https://github.com/hushink/shaul/blob/main/docs/faq.md)
- [API reference](https://docs.rs/shaul-ftp)
