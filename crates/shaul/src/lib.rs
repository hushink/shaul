//! Default assembly of the shaul download engine.
//!
//! ```no_run
//! use shaul::{Config, default_engine};
//!
//! # async fn example() -> shaul::Result<()> {
//! let config = Config::default();
//! let (engine, _store) = default_engine(config)?;
//! let link = shaul::Link::new("https://example.com/model.bin")?;
//! let task = engine
//!     .add(link, std::path::PathBuf::from("."), shaul::TaskOptions::default())
//!     .await?;
//! engine.run_task(task.id).await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

#[cfg(feature = "bt")]
pub use shaul_bt::{BtBackend, TorrentParser};
pub use shaul_core::*;
pub use shaul_http::{DirectLinkParser, HttpBackend};
pub use shaul_hub::{HubBackend, HubClient, HuggingFaceParser, ModelScopeParser};

/// Build the default router. BitTorrent (when the `bt` feature is enabled) is
/// registered first, then Hub parsers/backends, then the direct HTTP fallback.
pub fn default_router(config: &Config) -> Result<Router> {
    let client = Arc::new(HubClient::new(config)?);
    let mut parsers = ParserRegistry::new();
    #[cfg(feature = "bt")]
    parsers.register(Arc::new(TorrentParser));
    parsers.register(Arc::new(HuggingFaceParser::new(client.clone())));
    parsers.register(Arc::new(ModelScopeParser::new(client.clone())));
    parsers.register(shaul_core::parser_arc(DirectLinkParser));

    #[cfg(feature = "bt")]
    let bt_backend: Option<Arc<dyn DownloadBackend>> = Some(Arc::new(BtBackend::new(config)?));
    #[cfg(not(feature = "bt"))]
    let bt_backend: Option<Arc<dyn DownloadBackend>> = None;
    let backends: Vec<Arc<dyn DownloadBackend>> = bt_backend
        .into_iter()
        .chain([
            Arc::new(HubBackend::new(client, config)?) as Arc<dyn DownloadBackend>,
            shaul_core::backend_arc(HttpBackend::new(config)?),
        ])
        .collect();
    Ok(Router::new(parsers, backends))
}

/// Build the default engine (BT + Hub + direct-link backends + sidecar store).
pub fn default_engine(config: Config) -> Result<(DownloadEngine, Arc<dyn TaskStore>)> {
    let router = default_router(&config)?;
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(config.state_dir()?)?);
    let engine = DownloadEngine::new(Arc::new(router), store.clone(), config);
    Ok((engine, store))
}
