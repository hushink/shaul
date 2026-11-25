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

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;

#[cfg(feature = "bt")]
pub use shaul_bt::{BtBackend, TorrentParser};
pub use shaul_core::*;
#[cfg(feature = "dash")]
pub use shaul_dash::{DashBackend, DashClient, DashParser};
#[cfg(feature = "ftp")]
pub use shaul_ftp::{FtpBackend, FtpParser};
#[cfg(feature = "hls")]
pub use shaul_hls::{HlsBackend, HlsClient, HlsParser};
pub use shaul_http::{DirectLinkParser, HttpBackend};
pub use shaul_hub::{
    HubBackend, HubClient, HuggingFaceParser, ModelScopeParser, hf_token_available,
    modelscope_credentials_available,
};

/// Build the default router. BitTorrent (when the `bt` feature is enabled) is
/// registered first, then Hub parsers/backends, then FTP, HLS and DASH (when
/// enabled), then the direct HTTP fallback.
pub fn default_router(config: &Config) -> Result<Router> {
    let client = Arc::new(HubClient::new(config)?);
    #[cfg(feature = "hls")]
    let hls_client = Arc::new(HlsClient::new(config)?);
    #[cfg(feature = "dash")]
    let dash_client = Arc::new(DashClient::new(config)?);
    let mut parsers = ParserRegistry::new();
    #[cfg(feature = "bt")]
    parsers.register(Arc::new(TorrentParser));
    parsers.register(Arc::new(HuggingFaceParser::new(client.clone())));
    parsers.register(Arc::new(ModelScopeParser::new(client.clone())));
    #[cfg(feature = "ftp")]
    parsers.register(Arc::new(FtpParser));
    #[cfg(feature = "hls")]
    parsers.register(Arc::new(HlsParser::new(hls_client.clone())));
    #[cfg(feature = "dash")]
    parsers.register(Arc::new(DashParser::new(dash_client.clone())));
    parsers.register(shaul_core::parser_arc(DirectLinkParser));

    #[cfg(feature = "bt")]
    let bt_backend: Option<Arc<dyn DownloadBackend>> = Some(Arc::new(BtBackend::new(config)?));
    #[cfg(not(feature = "bt"))]
    let bt_backend: Option<Arc<dyn DownloadBackend>> = None;
    let http = HttpBackend::new(config)?;
    #[cfg(feature = "ftp")]
    let ftp_backend: Option<Arc<dyn DownloadBackend>> =
        Some(Arc::new(FtpBackend::new()) as Arc<dyn DownloadBackend>);
    #[cfg(not(feature = "ftp"))]
    let ftp_backend: Option<Arc<dyn DownloadBackend>> = None;
    #[cfg(feature = "hls")]
    let hls_backend: Option<Arc<dyn DownloadBackend>> =
        Some(Arc::new(HlsBackend::new(hls_client, http.clone())) as Arc<dyn DownloadBackend>);
    #[cfg(not(feature = "hls"))]
    let hls_backend: Option<Arc<dyn DownloadBackend>> = None;
    #[cfg(feature = "dash")]
    let dash_backend: Option<Arc<dyn DownloadBackend>> =
        Some(Arc::new(DashBackend::new(dash_client, http.clone())) as Arc<dyn DownloadBackend>);
    #[cfg(not(feature = "dash"))]
    let dash_backend: Option<Arc<dyn DownloadBackend>> = None;
    let backends: Vec<Arc<dyn DownloadBackend>> = bt_backend
        .into_iter()
        .chain(ftp_backend)
        .chain(hls_backend)
        .chain(dash_backend)
        .chain([
            Arc::new(HubBackend::with_http_and_xet(client, http.clone()))
                as Arc<dyn DownloadBackend>,
            shaul_core::backend_arc(http),
        ])
        .collect();
    Ok(Router::new(parsers, backends))
}

fn default_parts(config: &Config) -> Result<(Router, Arc<dyn TaskStore>)> {
    let router = default_router(config)?;
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(config.state_dir()?)?);
    Ok((router, store))
}

/// Build the default engine (enabled protocol backends + sidecar store).
pub fn default_engine(config: Config) -> Result<(DownloadEngine, Arc<dyn TaskStore>)> {
    let (router, store) = default_parts(&config)?;
    let engine = DownloadEngine::new(Arc::new(router), store.clone(), config);
    Ok((engine, store))
}

/// Build the blocking engine facade over the same default backends and
/// sidecar store. Use this when the caller does not own a Tokio runtime.
pub fn default_blocking_engine(config: Config) -> Result<BlockingEngine> {
    let (router, store) = default_parts(&config)?;
    BlockingEngine::new(router, store, config)
}
