use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fs2::FileExt;
use serde_json::Error as JsonError;

use crate::error::{Error, Result};
use crate::task::{Task, TaskId};
use crate::util::now_ms;

/// In-place mutation applied by [`TaskStore::update`] while the task is
/// locked. Returning an error aborts the update without saving.
pub type TaskUpdate = Box<dyn for<'a> FnOnce(&'a mut Task) -> Result<()> + Send + 'static>;

/// Guard returned by [`TaskStore::try_acquire_run_lock`]. Dropping it releases
/// the cross-process lock.
pub type RunLockGuard = Box<dyn Send + Sync>;

/// Persistence interface for tasks.
///
/// Implementations must make [`TaskStore::update`] atomic with respect to
/// concurrent writers, including other processes sharing the same state
/// directory.
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Inserts or replaces the stored task.
    async fn save(&self, task: &Task) -> Result<()>;
    /// Loads a task by id, or fails with [`Error::TaskNotFound`] when absent.
    async fn load(&self, id: TaskId) -> Result<Task>;
    /// Returns all stored tasks ordered by creation time.
    async fn list(&self) -> Result<Vec<Task>>;
    /// Deletes a task. Deleting a task that does not exist is not an error.
    async fn delete(&self, id: TaskId) -> Result<()>;
    /// Applies `update` to the stored task under a lock and returns the saved
    /// result.
    async fn update(&self, id: TaskId, update: TaskUpdate) -> Result<Task>;

    /// Inserts `task` unless an active task with the same deduplication key
    /// already exists. Returns `(task, created)`; when `created` is false the
    /// returned task is the existing active task. Implementations should make
    /// the check-and-insert atomic with respect to concurrent writers.
    async fn insert_unique(&self, task: Task) -> Result<(Task, bool)> {
        for existing in self.list().await? {
            if existing.state.is_active() && existing.key() == task.key() {
                return Ok((existing, false));
            }
        }
        self.save(&task).await?;
        Ok((task, true))
    }

    /// Attempts to acquire the exclusive cross-process run lock for `id`.
    ///
    /// Returns `Ok(None)` when another live process holds the lock. The
    /// returned guard must stay alive for the duration of the run; dropping
    /// it, or process exit, releases the lock. The default implementation
    /// performs no locking, so stores that need cross-process protection
    /// should override it.
    async fn try_acquire_run_lock(&self, _id: TaskId) -> Result<Option<RunLockGuard>> {
        let guard: RunLockGuard = Box::new(());
        Ok(Some(guard))
    }
}

/// One JSON sidecar per task under a state directory. Writes are atomic
/// (temp file + rename) and serialized by a cross-process lock file.
#[derive(Clone, Debug)]
pub struct SidecarStore {
    root: PathBuf,
}

impl SidecarStore {
    /// Creates a store rooted at `root`, creating the directory if it does not
    /// exist yet.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Returns the directory holding task sidecars and the store lock file.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn task_path(&self, id: TaskId) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(".store.lock")
    }

    fn with_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.lock_path())?;
        lock.lock_exclusive()?;
        let result = f();
        let _ = FileExt::unlock(&lock);
        result
    }

    fn save_sync(&self, task: &Task) -> Result<()> {
        let path = self.task_path(task.id);
        let temp = self.root.join(format!("{}.json.tmp", task.id));
        let data = serde_json::to_vec_pretty(task)?;
        {
            let mut file = File::create(&temp)?;
            use std::io::Write;
            file.write_all(&data)?;
            file.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
            }
        }
        // `rename` replaces an existing file atomically on Unix and on Windows
        // (MoveFileEx with MOVEFILE_REPLACE_EXISTING). Deleting the destination
        // first would open a window where a crash loses the sidecar.
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.into());
        }
        Ok(())
    }

    fn load_sync(&self, id: TaskId) -> Result<Task> {
        let path = self.task_path(id);
        let data = std::fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::TaskNotFound(id.to_string())
            } else {
                Error::Io(error)
            }
        })?;
        Ok(serde_json::from_slice(&data)?)
    }

    fn list_sync(&self) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read(&path)
                .map_err(Error::Io)
                .and_then(|data| serde_json::from_slice::<Task>(&data).map_err(Error::Json))
            {
                Ok(task) => tasks.push(task),
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "skipping unreadable task sidecar");
                }
            }
        }
        tasks.sort_by_key(|task| task.created_at_ms);
        Ok(tasks)
    }

    fn update_sync(&self, id: TaskId, update: TaskUpdate) -> Result<Task> {
        self.with_lock(|| {
            let mut task = self.load_sync(id)?;
            update(&mut task)?;
            task.updated_at_ms = now_ms();
            self.save_sync(&task)?;
            Ok(task)
        })
    }

    fn delete_sync(&self, id: TaskId) -> Result<()> {
        self.with_lock(|| {
            let path = self.task_path(id);
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(Error::Io(error)),
            }
        })
    }

    fn insert_unique_sync(&self, task: Task) -> Result<(Task, bool)> {
        self.with_lock(|| {
            for existing in self.list_sync()? {
                if existing.state.is_active() && existing.key() == task.key() {
                    return Ok((existing, false));
                }
            }
            self.save_sync(&task)?;
            Ok((task, true))
        })
    }

    fn try_acquire_run_lock_sync(&self, id: TaskId) -> Result<Option<RunLockGuard>> {
        let dir = self.root.join("locks");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                let guard: RunLockGuard = Box::new(RunLock { _file: file });
                Ok(Some(guard))
            }
            Err(error) if lock_is_contended(&error) => Ok(None),
            Err(error) => Err(Error::Io(error)),
        }
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || (error.raw_os_error().is_some()
            && error.raw_os_error() == fs2::lock_contended_error().raw_os_error())
}

struct RunLock {
    _file: File,
}

#[async_trait]
impl TaskStore for SidecarStore {
    async fn save(&self, task: &Task) -> Result<()> {
        let store = self.clone();
        let task = task.clone();
        tokio::task::spawn_blocking(move || store.with_lock(|| store.save_sync(&task)))
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }

    async fn load(&self, id: TaskId) -> Result<Task> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.load_sync(id))
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }

    async fn list(&self) -> Result<Vec<Task>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.list_sync())
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }

    async fn delete(&self, id: TaskId) -> Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.delete_sync(id))
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }

    async fn update(&self, id: TaskId, update: TaskUpdate) -> Result<Task> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.update_sync(id, update))
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }

    async fn insert_unique(&self, task: Task) -> Result<(Task, bool)> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.insert_unique_sync(task))
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }

    async fn try_acquire_run_lock(&self, id: TaskId) -> Result<Option<RunLockGuard>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.try_acquire_run_lock_sync(id))
            .await
            .map_err(|error| Error::Other(format!("store task failed: {error}")))?
    }
}

#[allow(dead_code)]
fn _assert_json_error(_: JsonError) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Link;
    use crate::task::{Task, TaskOptions, TaskState};

    #[tokio::test]
    async fn saves_loads_and_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = SidecarStore::new(dir.path()).unwrap();
        let task = Task::new(
            Link::new("https://example.com/file").unwrap(),
            dir.path().to_path_buf(),
            TaskOptions::default(),
        );
        store.save(&task).await.unwrap();
        let loaded = store.load(task.id).await.unwrap();
        assert_eq!(loaded.link.as_str(), "https://example.com/file");

        let updated = store
            .update(
                task.id,
                Box::new(|task| {
                    task.state = TaskState::Paused;
                    Ok(())
                }),
            )
            .await
            .unwrap();
        assert_eq!(updated.state, TaskState::Paused);
        assert_eq!(store.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn overwriting_a_sidecar_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = SidecarStore::new(dir.path()).unwrap();
        let mut task = Task::new(
            Link::new("https://example.com/file").unwrap(),
            dir.path().to_path_buf(),
            TaskOptions::default(),
        );
        store.save(&task).await.unwrap();
        task.state = TaskState::Paused;
        store.save(&task).await.unwrap();

        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
        assert_eq!(store.load(task.id).await.unwrap().state, TaskState::Paused);
    }

    #[tokio::test]
    async fn unique_insert_returns_an_active_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let store = SidecarStore::new(dir.path()).unwrap();
        let first = Task::new(
            Link::new("https://example.com/file").unwrap(),
            dir.path().to_path_buf(),
            TaskOptions::default(),
        );
        store.save(&first).await.unwrap();

        let second = Task::new(
            Link::new("https://example.com/file").unwrap(),
            dir.path().to_path_buf(),
            TaskOptions::default(),
        );
        let (returned, created) = store.insert_unique(second.clone()).await.unwrap();
        assert!(!created);
        assert_eq!(returned.id, first.id);
        assert_eq!(store.list().await.unwrap().len(), 1);

        let mut completed = first;
        completed.state = TaskState::Completed;
        store.save(&completed).await.unwrap();
        let (returned, created) = store.insert_unique(second.clone()).await.unwrap();
        assert!(created);
        assert_eq!(returned.id, second.id);
        assert_eq!(store.list().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn listing_skips_corrupt_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let store = SidecarStore::new(dir.path()).unwrap();
        let task = Task::new(
            Link::new("https://example.com/file").unwrap(),
            dir.path().to_path_buf(),
            TaskOptions::default(),
        );
        store.save(&task).await.unwrap();
        std::fs::write(dir.path().join("corrupt.json"), b"{not valid json").unwrap();

        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task.id);
    }
}
