//! HLS support for shaul.
//!
//! [`HlsParser`] turns HTTP(S) `.m3u8` links into a single
//! [`ResourceKind::Custom`](shaul_core::ResourceKind::Custom) resource, and
//! [`HlsBackend`] downloads and concatenates its segments into one output
//! file. Segments are an internal detail: the task still produces one
//! [`shaul_core::FileOutcome`].
//!
//! Supported: VOD TS playlists and fMP4 playlists with a single
//! `#EXT-X-MAP`, AES-128 encryption (including key rotation), and
//! `#EXT-X-BYTERANGE` segments and init maps, plus segment-level resume.
//! Live playlists, SAMPLE-AES and DASH are not supported.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// HLS download backend.
pub mod backend;
/// HTTP client used for playlists.
pub mod client;
/// Parser for HTTP(S) `.m3u8` links.
pub mod parser;
/// Master and media playlist parsing.
pub mod playlist;

pub use backend::HlsBackend;
pub use client::HlsClient;
pub use parser::{HLS_PROTOCOL, HlsParser};

use std::sync::Arc;

use shaul_core::{Config, ParserRegistry, Result, Router, backend_arc, parser_arc};

/// Build a router containing only the HLS parser and backend.
pub fn default_router(config: &Config) -> Result<Router> {
    let client = Arc::new(HlsClient::new(config)?);
    let http = shaul_http::HttpBackend::new(config)?;
    let parsers = ParserRegistry::new().with(parser_arc(HlsParser::new(client.clone())));
    Ok(Router::new(
        parsers,
        vec![backend_arc(HlsBackend::new(client, http))],
    ))
}
