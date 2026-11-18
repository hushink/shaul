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
    pub timestamp_ms: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl DownloadEvent {
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
pub enum EventKind {
    TaskAdded {
        task_id: TaskId,
        link: String,
    },
    TaskStarted {
        task_id: TaskId,
        link: String,
    },
    TaskStateChanged {
        task_id: TaskId,
        state: TaskState,
    },
    FileStarted {
        task_id: TaskId,
        file: String,
        total_bytes: Option<u64>,
    },
    Progress {
        task_id: TaskId,
        file: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    FileCompleted {
        task_id: TaskId,
        file: String,
        path: PathBuf,
        size: u64,
        checksum_verified: bool,
    },
    VerificationStarted {
        task_id: TaskId,
        file: String,
        algorithm: String,
        expected: String,
    },
    VerificationCompleted {
        task_id: TaskId,
        file: String,
        ok: bool,
        actual: String,
    },
    ExtractionStarted {
        task_id: TaskId,
        file: String,
        target: PathBuf,
    },
    ExtractionCompleted {
        task_id: TaskId,
        file: String,
        target: PathBuf,
        files_extracted: u64,
    },
    Warning {
        task_id: Option<TaskId>,
        message: String,
    },
    TaskCompleted {
        task_id: TaskId,
        files: usize,
        bytes: u64,
    },
    TaskFailed {
        task_id: TaskId,
        error: String,
    },
    TaskPaused {
        task_id: TaskId,
    },
    TaskCancelled {
        task_id: TaskId,
    },
}

#[derive(Clone)]
pub struct EventSink {
    sender: broadcast::Sender<DownloadEvent>,
}

impl EventSink {
    pub fn channel(capacity: usize) -> (Self, broadcast::Receiver<DownloadEvent>) {
        let (sender, receiver) = broadcast::channel(capacity.max(16));
        (Self { sender }, receiver)
    }

    pub fn emit(&self, kind: EventKind) {
        let _ = self.sender.send(DownloadEvent::new(kind));
    }

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
