use std::sync::Arc;

use async_trait::async_trait;

use crate::checksum::Checksum;
use crate::error::{Error, Result};
use crate::header::RequestOptions;
use crate::link::Link;
use crate::resource::Resource;

#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    pub request: RequestOptions,
    pub checksum: Option<Checksum>,
    pub revision: Option<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

/// Turns a raw `Link` into a `Resource`. Parsers may perform network I/O to
/// discover file lists (Hub) but must not transfer file contents.
#[async_trait]
pub trait LinkParser: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, link: &Link) -> bool;
    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource>;
}

#[derive(Clone, Default)]
pub struct ParserRegistry {
    parsers: Vec<Arc<dyn LinkParser>>,
}

impl ParserRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, parser: Arc<dyn LinkParser>) {
        self.parsers.push(parser);
    }

    pub fn with(mut self, parser: Arc<dyn LinkParser>) -> Self {
        self.register(parser);
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn LinkParser>> {
        self.parsers.iter()
    }

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
