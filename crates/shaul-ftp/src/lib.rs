//! FTP support for shaul.
//!
//! The [`FtpParser`] turns `ftp://` links into a single-file
//! [`ResourceKind::Custom`](shaul_core::ResourceKind::Custom) resource, and
//! [`FtpBackend`] transfers it with `suppaftp`. FTP has no Range requests, so
//! the backend is single-stream; it uses `SIZE`, `MDTM` and `REST` for
//! conservative resume when the server supports them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Single-stream FTP download backend.
pub mod backend;
/// Parser for `ftp://` links.
pub mod parser;

pub use backend::FtpBackend;
pub use parser::{FTP_PROTOCOL, FtpParser};

use shaul_core::{ParserRegistry, Router, backend_arc, parser_arc};

/// Build a router containing only the FTP parser and backend.
pub fn default_router() -> Router {
    let parsers = ParserRegistry::new().with(parser_arc(FtpParser));
    Router::new(parsers, vec![backend_arc(FtpBackend::new())])
}
