use std::sync::Arc;

use async_trait::async_trait;

use crate::checksum::Checksum;
use crate::error::{Error, Result};
use crate::header::RequestOptions;
use crate::link::Link;
use crate::resource::Resource;

/// Inputs a parser may use while turning a [`Link`] into a [`Resource`].
#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    /// Request headers and authentication to use for metadata requests and to
    /// carry into the produced files.
    pub request: RequestOptions,
    /// User-supplied checksum, applied to a single-file resource.
    pub checksum: Option<Checksum>,
    /// Provider revision (branch, tag or commit) requested by the user.
    pub revision: Option<String>,
    /// Glob patterns selecting files from a multi-file resource.
    pub include: Vec<String>,
    /// Glob patterns excluding files from a multi-file resource.
    pub exclude: Vec<String>,
}

/// Turns a raw `Link` into a `Resource`. Parsers may perform network I/O to
/// discover file lists (Hub) but must not transfer file contents.
#[async_trait]
pub trait LinkParser: Send + Sync {
    /// Short, stable identifier such as `http` or `huggingface`, used in
    /// diagnostics and reported by [`Router`](crate::router::Router).
    fn name(&self) -> &'static str;
    /// Returns `true` when this parser claims `link`. The check must be cheap
    /// and free of side effects, because registration order decides which
    /// parser is asked first.
    fn supports(&self, link: &Link) -> bool;
    /// Builds the protocol-agnostic resource for a claimed link.
    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource>;
}

/// Ordered collection of [`LinkParser`]s. Lookups always follow registration
/// order.
#[derive(Clone, Default)]
pub struct ParserRegistry {
    parsers: Vec<Arc<dyn LinkParser>>,
}

impl ParserRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a parser; later registrations are consulted after earlier ones.
    pub fn register(&mut self, parser: Arc<dyn LinkParser>) {
        self.parsers.push(parser);
    }

    /// Builder-style variant of [`ParserRegistry::register`].
    pub fn with(mut self, parser: Arc<dyn LinkParser>) -> Self {
        self.register(parser);
        self
    }

    /// Iterates over registered parsers in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn LinkParser>> {
        self.parsers.iter()
    }

    /// Returns `true` when no parser has been registered.
    pub fn is_empty(&self) -> bool {
        self.parsers.is_empty()
    }

    /// Try parsers in registration order. A parser that claims a link but
    /// fails does not prevent a later parser from being tried.
    pub async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let mut last_error = None;
        let mut claimed = false;
        for parser in &self.parsers {
            if !parser.supports(link) {
                continue;
            }
            claimed = true;
            match parser.parse(link, options).await {
                Ok(resource) => return Ok(resource),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            if claimed {
                Error::InvalidLink(link.to_string())
            } else {
                Error::NoParser(link.to_string())
            }
        }))
    }
}
