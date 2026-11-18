use std::sync::Arc;

use crate::backend::DownloadBackend;
use crate::error::{Error, Result};
use crate::link::Link;
use crate::parser::{LinkParser, ParseOptions, ParserRegistry};
use crate::resource::Resource;

#[derive(Clone)]
pub struct RoutedResource {
    pub resource: Resource,
    pub backend: Arc<dyn DownloadBackend>,
    pub parser: &'static str,
}

/// Resolves links through an ordered parser list and selects the first
/// backend that supports the resulting resource.
pub struct Router {
    parsers: ParserRegistry,
    backends: Vec<Arc<dyn DownloadBackend>>,
}

impl Router {
    pub fn new(parsers: ParserRegistry, backends: Vec<Arc<dyn DownloadBackend>>) -> Self {
        Self { parsers, backends }
    }

    pub fn parsers(&self) -> &ParserRegistry {
        &self.parsers
    }

    pub fn backends(&self) -> &[Arc<dyn DownloadBackend>] {
        &self.backends
    }

    /// The first parser that claims a link is authoritative. Falling through
    /// to a later parser after a Hub metadata error would silently download
    /// an HTML page as if it were a file, so parse errors are propagated.
    pub async fn resolve(&self, link: &Link, options: &ParseOptions) -> Result<RoutedResource> {
        for parser in self.parsers.iter() {
            if !parser.supports(link) {
                continue;
            }
            let resource = parser.parse(link, options).await?;
            let backend = self.backend_for(&resource)?;
            return Ok(RoutedResource {
                resource,
                backend,
                parser: parser.name(),
            });
        }
        Err(Error::NoParser(link.to_string()))
    }

    pub fn backend_for(&self, resource: &Resource) -> Result<Arc<dyn DownloadBackend>> {
        self.backends
            .iter()
            .find(|backend| backend.supports(resource))
            .cloned()
            .ok_or_else(|| Error::NoBackend(format!("{:?}", resource.kind)))
    }
}

pub fn parser_arc(parser: impl LinkParser + 'static) -> Arc<dyn LinkParser> {
    Arc::new(parser)
}

pub fn backend_arc(backend: impl DownloadBackend + 'static) -> Arc<dyn DownloadBackend> {
    Arc::new(backend)
}
