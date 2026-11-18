use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::checksum::Checksum;
use crate::config::ResolvedOptions;
use crate::error::Result;
use crate::event::EventSink;
use crate::resource::Resource;
use crate::task::{TaskId, TaskResume};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub segmented: bool,
    pub resume: bool,
    pub multi_file: bool,
}

#[async_trait]
pub trait DownloadBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities;
    fn supports(&self, resource: &Resource) -> bool;
    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome>;
}

/// Persistence hook used by backends to checkpoint partial progress without
/// knowing how tasks are stored.
#[async_trait]
pub trait ResumeCheckpoint: Send + Sync {
    async fn save(&self, resume: &TaskResume) -> Result<()>;
}

#[derive(Clone)]
pub struct DownloadRequest {
    pub task_id: TaskId,
    pub resource: Resource,
    pub destination: PathBuf,
    pub options: ResolvedOptions,
    pub resume: Option<TaskResume>,
    pub cancel: CancellationToken,
    pub events: EventSink,
    pub checkpoint: Arc<dyn ResumeCheckpoint>,
}

#[derive(Clone, Debug)]
pub struct FileOutcome {
    pub path: PathBuf,
    /// Relative to the task destination, using `/`.
    pub relative_path: String,
    pub size: u64,
    pub checksum: Option<Checksum>,
    pub checksum_verified: bool,
}

#[derive(Clone, Debug)]
pub enum DownloadOutcome {
    Completed(Vec<FileOutcome>),
    /// Progress has been checkpointed and the task should be left paused.
    Paused,
}
