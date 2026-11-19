//! Complete custom-protocol example: parser, router, backend, telemetry.
//!
//! `mem://hello.txt` is parsed into `ResourceKind::Custom { protocol: "mem" }`
//! and served by a backend that writes a fixed payload. This is the supported
//! extension path for third-party protocols without changing shaul's core.
//!
//! ```console
//! cargo run -p shaul --example custom_protocol
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use shaul::{
    BackendCapabilities, Config, DownloadBackend, DownloadEngine, DownloadOutcome, DownloadRequest,
    Error, EventKind, FileOutcome, Link, LinkParser, ParseOptions, ParserRegistry, Resource,
    ResourceFile, ResourceKind, Result, Router, SidecarStore, TaskOptions, TaskStore, Telemetry,
    backend_arc, parser_arc,
};
use url::Url;

const PAYLOAD: &[u8] = b"hello from the custom memory backend\n";

struct MemoryParser;

#[async_trait]
impl LinkParser for MemoryParser {
    fn name(&self) -> &'static str {
        "memory-demo"
    }

    fn supports(&self, link: &Link) -> bool {
        link.as_str().starts_with("mem://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let name = link.as_str().trim_start_matches("mem://").trim_matches('/');
        if name.is_empty() || name.contains("..") {
            return Err(Error::InvalidLink(link.to_string()));
        }
        Ok(Resource {
            kind: ResourceKind::Custom {
                protocol: "mem".to_string(),
            },
            name: name.to_string(),
            files: vec![ResourceFile {
                path: name.to_string(),
                url: Url::parse(&format!("mem://{name}"))
                    .map_err(|_| Error::InvalidLink(link.to_string()))?,
                size: Some(PAYLOAD.len() as u64),
                checksum: None,
                headers: Vec::new(),
                metadata: BTreeMap::new(),
                use_response_filename: false,
            }],
            request: options.request.clone(),
            checksum: None,
            revision: None,
            metadata: BTreeMap::new(),
        })
    }
}

struct MemoryBackend;

#[async_trait]
impl DownloadBackend for MemoryBackend {
    fn name(&self) -> &'static str {
        "memory-demo"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, false, true)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(
            &resource.kind,
            ResourceKind::Custom { protocol } if protocol == "mem"
        )
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let mut outcomes = Vec::new();
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let path = request.destination.join(&file.path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, PAYLOAD).await?;
            request.events.emit(EventKind::FileStarted {
                task_id: request.task_id,
                file: file.path.clone(),
                total_bytes: Some(PAYLOAD.len() as u64),
            });
            request.events.emit(EventKind::Progress {
                task_id: request.task_id,
                file: file.path.clone(),
                downloaded_bytes: PAYLOAD.len() as u64,
                total_bytes: Some(PAYLOAD.len() as u64),
            });
            outcomes.push(FileOutcome::new(
                path,
                file.path.clone(),
                PAYLOAD.len() as u64,
                None,
                false,
            ));
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}

struct PrintTelemetry;

impl Telemetry for PrintTelemetry {
    fn counter(&self, name: &str, value: u64, labels: &[(&str, &str)]) {
        eprintln!("counter {name}={value} {labels:?}");
    }

    fn gauge(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        eprintln!("gauge {name}={value} {labels:?}");
    }

    fn histogram(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        eprintln!("histogram {name}={value:.6} {labels:?}");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = std::env::temp_dir().join(format!("shaul-custom-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let mut config = Config::default();
    config.state_dir = Some(root.join("state"));

    let parsers = ParserRegistry::new().with(parser_arc(MemoryParser));
    let router = Router::new(parsers, vec![backend_arc(MemoryBackend)]);
    let store: Arc<dyn TaskStore> = Arc::new(SidecarStore::new(config.state_dir()?)?);
    let engine = DownloadEngine::new(Arc::new(router), store, config)
        .with_telemetry(Arc::new(PrintTelemetry));

    let task = engine
        .add(
            Link::new("mem://hello.txt")?,
            root.clone(),
            TaskOptions::default(),
        )
        .await?;
    let task = engine.run_task(task.id).await?;
    println!("state: {:?}", task.state);
    println!("file:  {}", root.join("hello.txt").display());
    Ok(())
}
