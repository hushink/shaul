//! Direct-link HTTP backend for shaul.

pub mod backend;
pub mod filename;
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
