use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fs2::FileExt;
use serde_json::Error as JsonError;

use crate::error::{Error, Result};
use crate::task::{Task, TaskId};
use crate::util::now_ms;

pub type TaskUpdate = Box<dyn for<'a> FnOnce(&'a mut Task) -> Result<()> + Send + 'static>;

#[async_trait]
pub trait TaskStore: Send + Sync {
    async fn save(&self, task: &Task) -> Result<()>;
    async fn load(&self, id: TaskId) -> Result<Task>;
    async fn list(&self) -> Result<Vec<Task>>;
    async fn delete(&self, id: TaskId) -> Result<()>;
    async fn update(&self, id: TaskId, update: TaskUpdate) -> Result<Task>;
}

/// One JSON sidecar per task under a state directory. Writes are atomic
/// (temp file + rename) and serialized by a cross-process lock file.
#[derive(Clone, Debug)]
pub struct SidecarStore {
    root: PathBuf,
}

impl SidecarStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

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
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        std::fs::rename(&temp, &path)?;
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
