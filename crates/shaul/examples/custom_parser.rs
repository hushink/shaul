//! Custom `LinkParser` example.
//!
//! `demo://<file>` is rewritten to a direct HTTPS URL and then handled by the
//! built-in HTTP backend. Run with:
//!
//! ```console
//! cargo run -p shaul --example custom_parser -- demo://model.bin
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use shaul::{
    Config, DirectLinkParser, DownloadEngine, Error, HttpBackend, Link, LinkParser, ParseOptions,
    ParserRegistry, Resource, Result, Router, SidecarStore, TaskOptions, TaskStore, backend_arc,
    parser_arc,
};
use url::Url;

struct DemoParser;

#[async_trait]
impl LinkParser for DemoParser {
    fn name(&self) -> &'static str {
        "demo"
    }

    fn supports(&self, link: &Link) -> bool {
        link.as_str().starts_with("demo://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let name = link
            .as_str()
            .trim_start_matches("demo://")
            .trim_matches('/');
        if name.is_empty() || name.contains("..") {
            return Err(Error::InvalidLink(link.to_string()));
        }
        let url = Url::parse(&format!("https://example.com/{name}"))
            .map_err(|_| Error::InvalidLink(link.to_string()))?;
        Resource::http_file(url, options.request.clone(), options.checksum.clone())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = Config::default();
    config.state_dir = Some(std::env::temp_dir().join("shaul-custom-parser-example"));
    let mut parsers = ParserRegistry::new().with(parser_arc(DemoParser));
    parsers.register(parser_arc(DirectLinkParser));
    let router = Router::new(parsers, vec![backend_arc(HttpBackend::new(&config)?)]);
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(config.state_dir()?)?);
    let engine = DownloadEngine::new(Arc::new(router), store, config);

    let link = Link::new(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "demo://model.bin".to_string()),
    )?;
    let task = engine
        .add(link, std::env::current_dir()?, TaskOptions::default())
        .await?;
    println!("parsed: {}", task.link);
    println!("backend: {}", task.backend.as_deref().unwrap_or("-"));
    println!("files:   {}", task.files.len());
    println!("state:   {:?}", task.state);
    Ok(())
}
