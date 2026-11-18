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

pub type TaskId = Uuid;

pub const TASK_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Task {
    pub schema_version: u32,
    pub id: TaskId,
    pub link: Link,
    #[serde(default)]
    pub resource: Option<Resource>,
    #[serde(default)]
    pub backend: Option<String>,
    pub destination: PathBuf,
    pub options: TaskOptions,
    pub state: TaskState,
    #[serde(default)]
    pub files: Vec<FileItem>,
    #[serde(default)]
    pub resume: Option<TaskResume>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl Task {
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

    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }

    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.link.canonical(),
            self.destination.to_string_lossy()
        )
    }

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

    pub fn total_bytes(&self) -> Option<u64> {
        self.files
            .iter()
            .try_fold(0u64, |sum, file| file.size.map(|size| sum + size))
    }

    pub fn downloaded_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.downloaded).sum()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskState {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Paused)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskOptions {
    #[serde(default)]
    pub output: Option<PathBuf>,
    #[serde(default)]
    pub connections: Option<usize>,
    #[serde(default)]
    pub resume: Option<bool>,
    #[serde(default)]
    pub verify: Option<VerifyMode>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub extract: Option<bool>,
    #[serde(default)]
    pub extract_dir: Option<PathBuf>,
    #[serde(default)]
    pub delete_archive: Option<bool>,
    /// Use the provider cache for Hub tasks (HF-compatible layout for HF).
    #[serde(default)]
    pub cache: Option<bool>,
    #[serde(default)]
    pub request: RequestOptions,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub rename_on_conflict: bool,
    /// Add-time only; persisted for diagnostics.
    #[serde(default)]
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum VerifyMode {
    #[default]
    Auto,
    Require,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileItem {
    pub path: String,
    #[serde(default)]
    pub url: Option<Url>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub downloaded: u64,
    pub state: FileState,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default = "default_selected")]
    pub selected: bool,
}

fn default_selected() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FileState {
    Pending,
    Downloading,
    Completed,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TaskResume {
    pub version: u32,
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
    pub fn file(&self, url: &str) -> Option<&FileResume> {
        self.files.get(url)
    }

    pub fn upsert(&mut self, url: String, file: FileResume) {
        self.files.insert(url, file);
    }

    pub fn downloaded(&self) -> u64 {
        self.files
            .values()
            .map(|file| completed_bytes(&file.completed))
            .sum()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileResume {
    pub url: String,
    /// Relative to the task destination, using `/` separators.
    pub final_path: String,
    /// Relative to the task destination, using `/` separators.
    pub part_path: String,
    #[serde(default)]
    pub validator: Validator,
    #[serde(default)]
    pub total_size: Option<u64>,
    pub segment_size: u64,
    #[serde(default)]
    pub completed: Vec<ByteRange>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub updated_at_ms: u64,
}

impl FileResume {
    pub fn completed_bytes(&self) -> u64 {
        completed_bytes(&self.completed)
    }

    pub fn is_complete(&self) -> bool {
        match self.total_size {
            Some(total) => self.completed_bytes() >= total,
            None => false,
        }
    }

    pub fn normalize(&mut self) {
        self.completed = merge_ranges(std::mem::take(&mut self.completed));
        self.updated_at_ms = now_ms();
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Validator {
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl Validator {
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

pub fn destination_file(destination: &Path, relative: &str) -> PathBuf {
    destination.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Partial-file path used by segmented backends.
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut value = final_path.as_os_str().to_os_string();
    value.push(".shaul.part");
    PathBuf::from(value)
}
