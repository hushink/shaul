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

/// Feature flags a backend advertises about the transfers it supports.
///
/// The values are descriptive: callers use them for policy decisions and
/// diagnostics rather than to gate routing. `Default` means "none".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BackendCapabilities {
    /// Whether a file can be fetched as multiple byte ranges in parallel.
    pub segmented: bool,
    /// Whether an interrupted transfer can continue from a saved
    /// [`TaskResume`] instead of starting over.
    pub resume: bool,
    /// Whether a single resource may produce several output files, such as a
    /// repository snapshot.
    pub multi_file: bool,
}

impl BackendCapabilities {
    /// Creates a capability set from the individual flags.
    pub fn new(segmented: bool, resume: bool, multi_file: bool) -> Self {
        Self {
            segmented,
            resume,
            multi_file,
        }
    }
}

/// Transfers the files described by a [`Resource`].
///
/// Backends are registered on a [`Router`](crate::router::Router) and selected
/// with first-match semantics through [`supports`](DownloadBackend::supports).
/// Implementations must honor [`DownloadRequest::cancel`], emit progress on
/// [`DownloadRequest::events`], and persist resumable state through
/// [`DownloadRequest::checkpoint`].
#[async_trait]
pub trait DownloadBackend: Send + Sync {
    /// Short, stable identifier such as `http`; it is recorded on the task for
    /// diagnostics.
    fn name(&self) -> &'static str;
    /// Describes the transfer features this backend provides.
    fn capabilities(&self) -> BackendCapabilities;
    /// Returns `true` when this backend can download `resource`. The check
    /// must be cheap and free of side effects.
    fn supports(&self, resource: &Resource) -> bool;
    /// Performs the transfer. Returns `Completed` with one [`FileOutcome`] per
    /// written file, or `Paused` when progress was checkpointed and the task
    /// should stay paused.
    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome>;
}

/// Persistence hook used by backends to checkpoint partial progress without
/// knowing how tasks are stored.
#[async_trait]
pub trait ResumeCheckpoint: Send + Sync {
    /// Stores the latest resume manifest for the running task.
    async fn save(&self, resume: &TaskResume) -> Result<()>;
}

/// Everything a backend needs for one download attempt.
#[derive(Clone)]
pub struct DownloadRequest {
    /// Id of the task being executed.
    pub task_id: TaskId,
    /// Parsed resource to download.
    pub resource: Resource,
    /// Root directory that all output paths must stay under.
    pub destination: PathBuf,
    /// Fully resolved download, verification and extraction options.
    pub options: ResolvedOptions,
    /// Manifest from a previous attempt, when the task is resuming.
    pub resume: Option<TaskResume>,
    /// Cancellation signal; the backend must stop promptly once it fires.
    pub cancel: CancellationToken,
    /// Sink for progress, file and warning events.
    pub events: EventSink,
    /// Hook for persisting partial progress so a later attempt can resume.
    pub checkpoint: Arc<dyn ResumeCheckpoint>,
}

/// One file written by a backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FileOutcome {
    /// Path of the file on disk, under the task destination.
    pub path: PathBuf,
    /// Relative to the task destination, using `/`.
    pub relative_path: String,
    /// Size of the written file in bytes.
    pub size: u64,
    /// Checksum the engine should verify this file against when the task has
    /// no checksum of its own.
    pub checksum: Option<Checksum>,
    /// Set when the backend itself proved integrity, so the engine's verify
    /// finalizer can skip re-hashing.
    pub checksum_verified: bool,
}

impl FileOutcome {
    /// Builds an outcome; use this rather than a struct literal because the
    /// struct is `#[non_exhaustive]`.
    pub fn new(
        path: PathBuf,
        relative_path: String,
        size: u64,
        checksum: Option<Checksum>,
        checksum_verified: bool,
    ) -> Self {
        Self {
            path,
            relative_path,
            size,
            checksum,
            checksum_verified,
        }
    }
}

/// Result of a single backend download attempt.
#[derive(Clone, Debug)]
pub enum DownloadOutcome {
    /// All selected files were transferred.
    Completed(Vec<FileOutcome>),
    /// Progress has been checkpointed and the task should be left paused.
    Paused,
}
