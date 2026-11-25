use std::sync::Arc;

use crate::backend::DownloadBackend;
use crate::error::{Error, Result};
use crate::link::Link;
use crate::parser::{LinkParser, ParseOptions, ParserRegistry};
use crate::resource::Resource;

/// A parsed resource together with the parser and backend selected for it.
#[derive(Clone)]
pub struct RoutedResource {
    /// The protocol-agnostic resource produced by the parser.
    pub resource: Resource,
    /// The first registered backend whose `supports` accepted the resource.
    pub backend: Arc<dyn DownloadBackend>,
    /// Name of the parser that produced the resource.
    pub parser: &'static str,
}

/// Resolves links through an ordered parser list and selects the first
/// backend that supports the resulting resource.
pub struct Router {
    parsers: ParserRegistry,
    backends: Vec<Arc<dyn DownloadBackend>>,
}

impl Router {
    /// Creates a router from parser and backend lists. Both lists are ordered:
    /// the first parser that claims a link and the first backend that supports
    /// the resulting resource win.
    pub fn new(parsers: ParserRegistry, backends: Vec<Arc<dyn DownloadBackend>>) -> Self {
        Self { parsers, backends }
    }

    /// Returns the parser registry used by [`Router::resolve`].
    pub fn parsers(&self) -> &ParserRegistry {
        &self.parsers
    }

    /// Returns the backends in priority order.
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
            if (options.request.method.is_some() || options.request.body.is_some())
                && backend.name() != "http"
            {
                return Err(Error::Config(format!(
                    "HTTP method/body require a direct HTTP(S) link (selected backend: {})",
                    backend.name()
                )));
            }
            return Ok(RoutedResource {
                resource,
                backend,
                parser: parser.name(),
            });
        }
        Err(Error::NoParser(link.to_string()))
    }

    /// Returns the first registered backend that supports `resource`, or
    /// [`Error::NoBackend`] when none does.
    pub fn backend_for(&self, resource: &Resource) -> Result<Arc<dyn DownloadBackend>> {
        self.backends
            .iter()
            .find(|backend| backend.supports(resource))
            .cloned()
            .ok_or_else(|| Error::NoBackend(format!("{:?}", resource.kind)))
    }
}

/// Erases a concrete parser into the trait object accepted by
/// [`ParserRegistry::register`](crate::parser::ParserRegistry::register).
pub fn parser_arc(parser: impl LinkParser + 'static) -> Arc<dyn LinkParser> {
    Arc::new(parser)
}

/// Erases a concrete backend into the trait object accepted by
/// [`Router::new`].
pub fn backend_arc(backend: impl DownloadBackend + 'static) -> Arc<dyn DownloadBackend> {
    Arc::new(backend)
}
