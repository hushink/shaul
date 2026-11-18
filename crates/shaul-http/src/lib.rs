//! Direct-link HTTP backend for shaul.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Segmented direct-link download backend.
pub mod backend;
mod filename;
/// Parser for direct HTTP/HTTPS links.
pub mod parser;

pub use backend::HttpBackend;
pub use parser::DirectLinkParser;

use shaul_core::{Config, ParserRegistry, Result, Router, backend_arc, parser_arc};

/// Build a router containing only the default direct-link HTTP stack.
pub fn default_router(config: &Config) -> Result<Router> {
    let parser = ParserRegistry::new().with(parser_arc(DirectLinkParser));
    let backend = backend_arc(HttpBackend::new(config)?);
    Ok(Router::new(parser, vec![backend]))
}
