use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::checksum::Checksum;
use crate::header::RequestOptions;
use crate::link::Link;
use crate::range::{ByteRange, completed_bytes, merge_ranges};
use crate::resource::Resource;
use crate::util::now_ms;

/// Unique identifier for a persisted task, also used as its sidecar file name.
pub type TaskId = Uuid;

/// Version of the serialized task sidecar schema.
pub const TASK_SCHEMA_VERSION: u32 = 1;

/// A persisted download task: the requested link, its resolved resource,
/// destination and current progress.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Task {
    /// Sidecar schema version; see [`TASK_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Unique task id, also used as the sidecar file name.
    pub id: TaskId,
    /// Original link supplied by the user.
    pub link: Link,
    /// Parsed, backend-agnostic description of what to download.
    #[serde(default)]
    pub resource: Option<Resource>,
    /// Name of the backend selected to execute the task.
    #[serde(default)]
    pub backend: Option<String>,
    /// Root directory that relative output paths are resolved against.
    pub destination: PathBuf,
    /// Per-task options supplied when the task was added.
    pub options: TaskOptions,
    /// Current lifecycle state of the task.
    pub state: TaskState,
    /// Per-file progress entries generated from the resource.
    #[serde(default)]
    pub files: Vec<FileItem>,
    /// Per-file resume checkpoints written while downloading.
    #[serde(default)]
    pub resume: Option<TaskResume>,
    /// Error message from the most recent failure, if any.
    #[serde(default)]
    pub error: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: u64,
    /// Timestamp of the last modification in Unix milliseconds.
    pub updated_at_ms: u64,
}

impl Task {
    /// Create a pending task with a fresh id and current timestamps. The file
    /// list stays empty until [`Task::set_resource`] is called.
    pub fn new(link: Link, destination: PathBuf, options: TaskOptions) -> Self {
        let now = now_ms();
        Self {
            schema_version: TASK_SCHEMA_VERSION,
            id: Uuid::now_v7(),
            link,
            resource: None,
            backend: None,
            destination,
            options,
            state: TaskState::Pending,
            files: Vec::new(),
            resume: None,
            error: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    /// Refresh `updated_at_ms` to the current time.
    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }

    /// De-duplication key built from the canonical link and the destination
    /// path.
    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.link.canonical(),
            self.destination.to_string_lossy()
        )
    }

    /// Attach the resolved resource and rebuild the per-file progress entries
    /// from it.
    pub fn set_resource(&mut self, resource: Resource) {
        self.files = resource
            .files
            .iter()
            .map(|file| FileItem {
                path: file.path.clone(),
                url: Some(file.url.clone()),
                size: file.size,
                downloaded: 0,
                state: FileState::Pending,
                checksum: file.checksum.clone(),
                selected: true,
            })
            .collect();
        self.resource = Some(resource);
    }

    /// Pull completed byte counts and final paths back from the resume
    /// manifest so `status`/`list` show meaningful progress.
    pub fn refresh_from_resume(&mut self) {
        let Some(resume) = &self.resume else {
            return;
        };
        for file in &mut self.files {
            let Some(url) = &file.url else {
                continue;
            };
            if let Some(entry) = resume.files.get(url.as_str()) {
                file.downloaded = completed_bytes(&entry.completed);
                if let Some(size) = entry.total_size {
                    file.size = Some(size);
                }
                file.path = entry.final_path.clone();
            }
        }
    }

    /// Total size of all files in bytes, or `None` if any file has an unknown
    /// size.
    pub fn total_bytes(&self) -> Option<u64> {
        self.files
            .iter()
            .try_fold(0u64, |sum, file| file.size.map(|size| sum + size))
    }

    /// Sum of the bytes downloaded so far across all files.
    pub fn downloaded_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.downloaded).sum()
    }
}

/// Lifecycle state of a task.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskState {
    /// Created but not started yet.
    Pending,
    /// Currently downloading.
    Running,
    /// Interrupted by the user; may be resumed.
    Paused,
    /// Finished successfully.
    Completed,
    /// Stopped because of an error.
    Failed,
    /// Abandoned by the user; it cannot be resumed.
    Cancelled,
}

impl TaskState {
    /// Whether the task occupies an active de-duplication slot: `Pending`,
    /// `Running` or `Paused`.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Paused)
    }
}

/// Per-task overrides captured when a task is added. Unset fields fall back to
/// the corresponding [`Config`](crate::config::Config) defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskOptions {
    /// Output file or directory override for a single-file direct link.
    #[serde(default)]
    pub output: Option<PathBuf>,
    /// Number of parallel connections used for this task.
    #[serde(default)]
    pub connections: Option<usize>,
    /// Additional bandwidth cap for this task in bytes per second. `None` or
    /// `Some(0)` means no task-specific cap; the global limit still applies.
    #[serde(default)]
    pub rate_limit: Option<u64>,
    /// Number of files downloaded concurrently within this task.
    #[serde(default)]
    pub file_jobs: Option<usize>,
    /// Whether to resume from existing partial data.
    #[serde(default)]
    pub resume: Option<bool>,
    /// Checksum verification policy for this task.
    #[serde(default)]
    pub verify: Option<VerifyMode>,
    /// Expected checksum for the downloaded content.
    #[serde(default)]
    pub checksum: Option<Checksum>,
    /// Whether to extract archives after downloading.
    #[serde(default)]
    pub extract: Option<bool>,
    /// Directory to extract archives into.
    #[serde(default)]
    pub extract_dir: Option<PathBuf>,
    /// Whether to delete the archive after successful extraction.
    #[serde(default)]
    pub delete_archive: Option<bool>,
    /// Use the provider cache for Hub tasks (HF-compatible layout for HF).
    #[serde(default)]
    pub cache: Option<bool>,
    /// Per-task headers, authentication and cross-origin policy.
    #[serde(default)]
    pub request: RequestOptions,
    /// Hub revision (branch, tag or commit) to download.
    #[serde(default)]
    pub revision: Option<String>,
    /// Glob patterns selecting which Hub or BitTorrent files to download.
    #[serde(default)]
    pub include: Vec<String>,
    /// Glob patterns excluding Hub or BitTorrent files from the download.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Overwrite existing destination files.
    #[serde(default)]
    pub overwrite: bool,
    /// Rename conflicting destinations (`name-1.ext`, `name-2.ext`, ...)
    /// instead of failing.
    #[serde(default)]
    pub rename_on_conflict: bool,
    /// Add-time only; persisted for diagnostics.
    #[serde(default)]
    pub force: bool,
}

/// Policy for checking downloaded content against an expected checksum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum VerifyMode {
    /// Verify when a checksum is known, otherwise skip. This is the default.
    #[default]
    Auto,
    /// Fail when no checksum is available.
    Require,
    /// Never verify.
    Disabled,
}

/// Progress and state for one file belonging to a task.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileItem {
    /// Relative output path under the task destination, using `/` separators.
    pub path: String,
    /// Source URL, when the file was resolved from a link.
    #[serde(default)]
    pub url: Option<Url>,
    /// Expected size in bytes, once known.
    #[serde(default)]
    pub size: Option<u64>,
    /// Bytes downloaded so far.
    #[serde(default)]
    pub downloaded: u64,
    /// Current per-file state.
    pub state: FileState,
    /// Expected checksum, when known.
    #[serde(default)]
    pub checksum: Option<Checksum>,
    /// Whether this file is selected for download.
    #[serde(default = "default_selected")]
    pub selected: bool,
}

fn default_selected() -> bool {
    true
}

/// Lifecycle state of a single file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FileState {
    /// Not started yet.
    Pending,
    /// Transfer is in progress.
    Downloading,
    /// Downloaded and finalized.
    Completed,
    /// Excluded from the download or intentionally left untouched.
    Skipped,
    /// Stopped because of an error.
    Failed,
}

/// Per-file resume checkpoints persisted alongside a task.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TaskResume {
    /// Schema version of the resume manifest.
    pub version: u32,
    /// Checkpoints keyed by source URL.
    #[serde(default)]
    pub files: BTreeMap<String, FileResume>,
}

impl Default for TaskResume {
    fn default() -> Self {
        Self {
            version: 1,
            files: BTreeMap::new(),
        }
    }
}

impl TaskResume {
    /// Checkpoint for `url`, if one exists.
    pub fn file(&self, url: &str) -> Option<&FileResume> {
        self.files.get(url)
    }

    /// Insert or replace the checkpoint for `url`.
    pub fn upsert(&mut self, url: String, file: FileResume) {
        self.files.insert(url, file);
    }

    /// Total completed bytes across all checkpointed files.
    pub fn downloaded(&self) -> u64 {
        self.files
            .values()
            .map(|file| completed_bytes(&file.completed))
            .sum()
    }
}

/// Resume checkpoint for one partially downloaded file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileResume {
    /// Source URL that identifies this checkpoint.
    pub url: String,
    /// Relative to the task destination, using `/` separators.
    pub final_path: String,
    /// Relative to the task destination, using `/` separators.
    pub part_path: String,
    /// Validators captured when the partial data was written.
    #[serde(default)]
    pub validator: Validator,
    /// Total size of the file in bytes, when known.
    #[serde(default)]
    pub total_size: Option<u64>,
    /// Segment size used by the segmented download.
    pub segment_size: u64,
    /// Byte ranges already written to the partial file.
    #[serde(default)]
    pub completed: Vec<ByteRange>,
    /// Expected checksum for the file, when known.
    #[serde(default)]
    pub checksum: Option<Checksum>,
    /// Backend-specific metadata gathered while probing the file.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Timestamp of the last checkpoint update in Unix milliseconds.
    pub updated_at_ms: u64,
}

impl FileResume {
    /// Total bytes covered by the completed ranges.
    pub fn completed_bytes(&self) -> u64 {
        completed_bytes(&self.completed)
    }

    /// Whether every byte of a file with a known size has been completed.
    pub fn is_complete(&self) -> bool {
        match self.total_size {
            Some(total) => self.completed_bytes() >= total,
            None => false,
        }
    }

    /// Merge overlapping or adjacent ranges and refresh `updated_at_ms`.
    pub fn normalize(&mut self) {
        self.completed = merge_ranges(std::mem::take(&mut self.completed));
        self.updated_at_ms = now_ms();
    }
}

/// HTTP validators captured from a remote file and used to decide whether a
/// stored partial download can still be resumed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Validator {
    /// `ETag` from the last response.
    #[serde(default)]
    pub etag: Option<String>,
    /// `Last-Modified` from the last response.
    #[serde(default)]
    pub last_modified: Option<String>,
    /// Size of the remote file in bytes.
    #[serde(default)]
    pub size: Option<u64>,
}

impl Validator {
    /// Whether no validator signal has been recorded.
    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none() && self.size.is_none()
    }

    /// A stored validator is compatible with a fresh probe when the strongest
    /// available signal still matches. Weak ETags are not used for range
    /// validation, so they fall back to Last-Modified + size.
    pub fn matches(&self, fresh: &Validator) -> bool {
        if let Some(etag) = &self.etag
            && !etag.starts_with("W/")
        {
            return fresh.etag.as_deref() == Some(etag.as_str());
        }
        if let Some(last_modified) = &self.last_modified {
            return fresh.last_modified.as_deref() == Some(last_modified.as_str())
                && (self.size.is_none() || self.size == fresh.size);
        }
        if let Some(size) = self.size {
            return fresh.size == Some(size);
        }
        false
    }
}

/// Join a manifest-style relative path (using `/` separators) under
/// `destination`.
pub fn destination_file(destination: &Path, relative: &str) -> PathBuf {
    destination.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Partial-file path used by segmented backends.
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut value = final_path.as_os_str().to_os_string();
    value.push(".shaul.part");
    PathBuf::from(value)
}
