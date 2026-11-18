use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::checksum::Checksum;
use crate::task::{TaskId, TaskState};
use crate::util::now_ms;

/// A structured progress event. The CLI consumes the same stream for both
/// human-readable progress bars and `--json` output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadEvent {
    /// Creation time in milliseconds since the UNIX epoch.
    pub timestamp_ms: u64,
    /// What happened; flattened into the serialized object under the `event`
    /// tag.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl DownloadEvent {
    /// Create an event of `kind` stamped with the current time.
    pub fn new(kind: EventKind) -> Self {
        Self {
            timestamp_ms: now_ms(),
            kind,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
/// Everything that can happen to a task between creation and completion.
///
/// Serialized with an `event` tag in `snake_case`; consumers should tolerate
/// unknown variants because the enum is non-exhaustive.
pub enum EventKind {
    /// A task was registered and persisted, before any transfer starts.
    TaskAdded {
        /// Task that was added.
        task_id: TaskId,
        /// Task's link, rendered as a string.
        link: String,
    },
    /// A task began running in a worker.
    TaskStarted {
        /// Task that started.
        task_id: TaskId,
        /// Task's link, rendered as a string.
        link: String,
    },
    /// A task was explicitly moved to another state, such as being requeued.
    TaskStateChanged {
        /// Task whose state changed.
        task_id: TaskId,
        /// New state of the task.
        state: TaskState,
    },
    /// A file started transferring, or was found already complete.
    FileStarted {
        /// Task the file belongs to.
        task_id: TaskId,
        /// Destination-relative path of the file, using `/` separators.
        file: String,
        /// Expected size in bytes, when known.
        total_bytes: Option<u64>,
    },
    /// Byte-level progress for one file.
    Progress {
        /// Task the file belongs to.
        task_id: TaskId,
        /// Destination-relative path of the file, using `/` separators.
        file: String,
        /// Bytes transferred so far, including resumed data.
        downloaded_bytes: u64,
        /// Expected size in bytes, when known.
        total_bytes: Option<u64>,
    },
    /// A file finished downloading and passed the engine's finalizers.
    FileCompleted {
        /// Task the file belongs to.
        task_id: TaskId,
        /// Destination-relative path of the file, using `/` separators.
        file: String,
        /// Final location of the file on disk.
        path: PathBuf,
        /// Final size in bytes.
        size: u64,
        /// Whether the content was verified, including BitTorrent piece
        /// verification.
        checksum_verified: bool,
    },
    /// Checksum verification began for one file.
    VerificationStarted {
        /// Task the file belongs to.
        task_id: TaskId,
        /// Destination-relative path of the file, using `/` separators.
        file: String,
        /// Checksum algorithm name, such as `sha256`.
        algorithm: String,
        /// Expected digest as lowercase hex.
        expected: String,
    },
    /// Checksum verification finished for one file.
    VerificationCompleted {
        /// Task the file belongs to.
        task_id: TaskId,
        /// Destination-relative path of the file, using `/` separators.
        file: String,
        /// Whether the computed digest matched the expected one.
        ok: bool,
        /// Computed digest as lowercase hex.
        actual: String,
    },
    /// Extraction of an archive began.
    ExtractionStarted {
        /// Task the archive belongs to.
        task_id: TaskId,
        /// Destination-relative path of the archive, using `/` separators.
        file: String,
        /// Directory the archive is extracted into.
        target: PathBuf,
    },
    /// Extraction of an archive finished.
    ExtractionCompleted {
        /// Task the archive belongs to.
        task_id: TaskId,
        /// Destination-relative path of the archive, using `/` separators.
        file: String,
        /// Directory the archive was extracted into.
        target: PathBuf,
        /// Number of files written by the extraction.
        files_extracted: u64,
    },
    /// A non-fatal problem that did not abort the download.
    Warning {
        /// Task the warning relates to, when it is tied to one.
        task_id: Option<TaskId>,
        /// Human-readable warning text.
        message: String,
    },
    /// The task completed successfully after all finalizers ran.
    TaskCompleted {
        /// Task that completed.
        task_id: TaskId,
        /// Number of files in the task.
        files: usize,
        /// Total size of those files in bytes.
        bytes: u64,
    },
    /// The task failed and will not continue on its own.
    TaskFailed {
        /// Task that failed.
        task_id: TaskId,
        /// Final error message.
        error: String,
    },
    /// The task was paused and can be resumed later.
    TaskPaused {
        /// Task that was paused.
        task_id: TaskId,
    },
    /// The task was cancelled and will not be resumed.
    TaskCancelled {
        /// Task that was cancelled.
        task_id: TaskId,
    },
}

/// Cloneable handle used to broadcast [`DownloadEvent`]s to subscribers.
///
/// Clones share the same broadcast channel. Delivery is best effort: emitting
/// with no subscribers is a no-op, and a receiver that falls behind may miss
/// some events.
#[derive(Clone)]
pub struct EventSink {
    sender: broadcast::Sender<DownloadEvent>,
}

impl EventSink {
    /// Create a sink and its first receiver. The broadcast buffer holds at
    /// least 16 events even when `capacity` is smaller.
    pub fn channel(capacity: usize) -> (Self, broadcast::Receiver<DownloadEvent>) {
        let (sender, receiver) = broadcast::channel(capacity.max(16));
        (Self { sender }, receiver)
    }

    /// Broadcast a new event stamped with the current time.
    pub fn emit(&self, kind: EventKind) {
        let _ = self.sender.send(DownloadEvent::new(kind));
    }

    /// Open another receiver; only events emitted after this call are seen.
    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventSink {
    fn default() -> Self {
        Self::channel(1024).0
    }
}

#[allow(dead_code)]
fn _assert_checksum_serializable(_: &Checksum) {}
