use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::backend::{
    DownloadBackend, DownloadOutcome, DownloadRequest, FileOutcome, ResumeCheckpoint,
};
use crate::config::{Config, ResolvedOptions};
use crate::error::{Error, Result};
use crate::event::{EventKind, EventSink};
use crate::finalizer::{FinalizeContext, FinalizerPipeline};
use crate::link::Link;
use crate::parser::ParseOptions;
use crate::resource::Resource;
use crate::router::{RoutedResource, Router};
use crate::store::TaskStore;
use crate::task::{
    FileItem, FileState, Task, TaskId, TaskOptions, TaskResume, TaskState, part_path,
};

pub struct DownloadEngine {
    router: Arc<Router>,
    store: Arc<dyn TaskStore>,
    config: Config,
    events: EventSink,
    running: Arc<Mutex<HashMap<TaskId, CancellationToken>>>,
    finalizers: FinalizerPipeline,
}

impl DownloadEngine {
    pub fn new(router: Arc<Router>, store: Arc<dyn TaskStore>, config: Config) -> Self {
        let (events, _) = EventSink::channel(4096);
        Self {
            router,
            store,
            config,
            events,
            running: Arc::new(Mutex::new(HashMap::new())),
            finalizers: FinalizerPipeline::default(),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::event::DownloadEvent> {
        self.events.subscribe()
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn events(&self) -> &EventSink {
        &self.events
    }

    pub fn store(&self) -> &Arc<dyn TaskStore> {
        &self.store
    }

    pub async fn resolve(&self, link: &Link, options: &ParseOptions) -> Result<RoutedResource> {
        self.router.resolve(link, options).await
    }

    pub async fn add(
        &self,
        link: Link,
        destination: PathBuf,
        options: TaskOptions,
    ) -> Result<Task> {
        if !options.force {
            let key = format!("{}|{}", link.canonical(), destination.display());
            for task in self.store.list().await? {
                if task.state.is_active() && task.key() == key {
                    return Ok(task);
                }
            }
        }

        let parse_options = ParseOptions {
            request: options.request.clone(),
            checksum: options.checksum.clone(),
            revision: options.revision.clone(),
            include: options.include.clone(),
            exclude: options.exclude.clone(),
        };
        let routed = self.router.resolve(&link, &parse_options).await?;
        let mut task = Task::new(link, destination, options);
        task.backend = Some(routed.backend.name().to_string());
        task.set_resource(routed.resource);
        self.store.save(&task).await?;
        self.events.emit(EventKind::TaskAdded {
            task_id: task.id,
            link: task.link.to_string(),
        });
        Ok(task)
    }

    pub async fn run_task(&self, id: TaskId) -> Result<Task> {
        self.run_task_with_cancel(id, None).await
    }

    pub async fn run_task_with_cancel(
        &self,
        id: TaskId,
        external_cancel: Option<CancellationToken>,
    ) -> Result<Task> {
        let task = self.store.load(id).await?;
        match task.state {
            TaskState::Completed => return Ok(task),
            TaskState::Cancelled => {
                return Err(Error::InvalidState(format!("task {id} was cancelled")));
            }
            _ => {}
        }

        let resource = task
            .resource
            .clone()
            .ok_or_else(|| Error::InvalidState(format!("task {id} has no parsed resource")))?;
        let backend = self.router.backend_for(&resource)?;
        let options = self.config.resolve(&task.destination, &task.options)?;
        let cancel = external_cancel.unwrap_or_default();

        self.running.lock().await.insert(id, cancel.clone());
        self.store
            .update(
                id,
                Box::new(|task| {
                    task.state = TaskState::Running;
                    task.error = None;
                    Ok(())
                }),
            )
            .await?;
        self.events.emit(EventKind::TaskStarted {
            task_id: id,
            link: task.link.to_string(),
        });

        let checkpoint = Arc::new(StoreCheckpoint {
            store: self.store.clone(),
            task_id: id,
        });
        let request = DownloadRequest {
            task_id: id,
            resource,
            destination: task.destination.clone(),
            options: options.clone(),
            resume: task.resume.clone(),
            cancel: cancel.clone(),
            events: self.events.clone(),
            checkpoint,
        };

        // A pause/cancel issued by another process only changes the sidecar;
        // this watcher turns that state change into cancellation here.
        let watcher = {
            let store = self.store.clone();
            let token = cancel.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {
                            if let Ok(task) = store.load(id).await
                                && matches!(
                                    task.state,
                                    TaskState::Paused | TaskState::Cancelled
                                ) {
                                    token.cancel();
                                    break;
                                }
                        }
                    }
                }
            })
        };

        let result = self.run_attempts(&request, backend, &options).await;
        watcher.abort();
        self.running.lock().await.remove(&id);
        result
    }

    async fn run_attempts(
        &self,
        request: &DownloadRequest,
        backend: Arc<dyn DownloadBackend>,
        options: &ResolvedOptions,
    ) -> Result<Task> {
        let mut verification_retries = 0u32;
        loop {
            let outcome = backend.download(request.clone()).await;
            match outcome {
                Ok(DownloadOutcome::Completed(files)) => {
                    match self
                        .finalize_files(request.task_id, &request.destination, options, &files)
                        .await
                    {
                        Ok(()) => {
                            let total_bytes: u64 = files.iter().map(|file| file.size).sum();
                            let file_count = files.len();
                            let items = file_items(&files);
                            let task = self
                                .store
                                .update(
                                    request.task_id,
                                    Box::new(move |task| {
                                        task.state = TaskState::Completed;
                                        task.error = None;
                                        task.resume = None;
                                        task.files = items;
                                        Ok(())
                                    }),
                                )
                                .await?;
                            self.events.emit(EventKind::TaskCompleted {
                                task_id: request.task_id,
                                files: file_count,
                                bytes: total_bytes,
                            });
                            return Ok(task);
                        }
                        Err(Error::Verification { .. }) if verification_retries < 1 => {
                            verification_retries += 1;
                            self.events.emit(EventKind::Warning {
                                task_id: Some(request.task_id),
                                message: "checksum mismatch; retrying once from scratch"
                                    .to_string(),
                            });
                            self.reset_outputs(request.task_id, &files).await?;
                            continue;
                        }
                        Err(error) => {
                            let message = error.to_string();
                            self.fail(request.task_id, error).await?;
                            return Err(Error::Backend(message));
                        }
                    }
                }
                Ok(DownloadOutcome::Paused) => {
                    return self.mark_paused(request.task_id).await;
                }
                Err(error) if error.is_cancelled() => {
                    return self.mark_paused(request.task_id).await;
                }
                Err(error) => {
                    let message = error.to_string();
                    self.fail(request.task_id, error).await?;
                    return Err(Error::Backend(message));
                }
            }
        }
    }

    async fn finalize_files(
        &self,
        task_id: TaskId,
        destination: &std::path::Path,
        options: &ResolvedOptions,
        files: &[FileOutcome],
    ) -> Result<()> {
        for file in files {
            let context = FinalizeContext {
                task_id,
                destination,
                file,
                options,
                events: &self.events,
            };
            let result = self.finalizers.run(&context).await?;
            self.events.emit(EventKind::FileCompleted {
                task_id,
                file: file.relative_path.clone(),
                path: file.path.clone(),
                size: file.size,
                checksum_verified: result.checksum_verified,
            });
        }
        Ok(())
    }

    async fn reset_outputs(&self, id: TaskId, files: &[FileOutcome]) -> Result<()> {
        for file in files {
            let _ = tokio::fs::remove_file(&file.path).await;
            let _ = tokio::fs::remove_file(part_path(&file.path)).await;
        }
        self.store
            .update(
                id,
                Box::new(|task| {
                    task.resume = None;
                    for file in &mut task.files {
                        file.downloaded = 0;
                        file.state = FileState::Pending;
                    }
                    Ok(())
                }),
            )
            .await?;
        Ok(())
    }

    async fn fail(&self, id: TaskId, error: Error) -> Result<()> {
        let message = error.to_string();
        let stored = message.clone();
        self.store
            .update(
                id,
                Box::new(move |task| {
                    if task.state == TaskState::Running {
                        task.state = TaskState::Failed;
                        task.error = Some(stored.clone());
                    }
                    Ok(())
                }),
            )
            .await?;
        self.events.emit(EventKind::TaskFailed {
            task_id: id,
            error: message,
        });
        Ok(())
    }

    async fn mark_paused(&self, id: TaskId) -> Result<Task> {
        let task = self.store.load(id).await?;
        match task.state {
            TaskState::Cancelled => {
                self.events.emit(EventKind::TaskCancelled { task_id: id });
                Ok(task)
            }
            TaskState::Paused => {
                self.events.emit(EventKind::TaskPaused { task_id: id });
                Ok(task)
            }
            _ => {
                let task = self
                    .store
                    .update(
                        id,
                        Box::new(|task| {
                            task.state = TaskState::Paused;
                            Ok(())
                        }),
                    )
                    .await?;
                self.events.emit(EventKind::TaskPaused { task_id: id });
                Ok(task)
            }
        }
    }

    pub async fn run_pending(&self, jobs: usize, cancel: CancellationToken) -> Result<Vec<Task>> {
        let tasks = self.store.list().await?;
        let ids: Vec<TaskId> = tasks
            .into_iter()
            .filter(|task| task.state == TaskState::Pending)
            .map(|task| task.id)
            .collect();
        let jobs = jobs.max(1);
        let results = futures_util::stream::iter(ids.into_iter().map(|id| {
            let cancel = cancel.clone();
            async move { self.run_task_with_cancel(id, Some(cancel)).await }
        }))
        .buffer_unordered(jobs)
        .collect::<Vec<_>>()
        .await;

        Ok(results.into_iter().filter_map(Result::ok).collect())
    }

    /// A previous process may have left tasks in `running`. The CLI calls
    /// this only while holding the single-worker lock.
    pub async fn requeue_stale(&self) -> Result<Vec<TaskId>> {
        let mut requeued = Vec::new();
        for task in self.store.list().await? {
            if task.state == TaskState::Running {
                self.store
                    .update(
                        task.id,
                        Box::new(|task| {
                            task.state = TaskState::Pending;
                            Ok(())
                        }),
                    )
                    .await?;
                requeued.push(task.id);
                self.events.emit(EventKind::Warning {
                    task_id: Some(task.id),
                    message: "requeued a task left running by a previous process".to_string(),
                });
            }
        }
        Ok(requeued)
    }

    pub async fn pause(&self, id: TaskId) -> Result<Task> {
        let task = self
            .store
            .update(
                id,
                Box::new(|task| {
                    if task.state == TaskState::Completed {
                        return Err(Error::InvalidState("task already completed".to_string()));
                    }
                    task.state = TaskState::Paused;
                    Ok(())
                }),
            )
            .await?;
        if let Some(token) = self.running.lock().await.get(&id) {
            token.cancel();
        }
        self.events.emit(EventKind::TaskPaused { task_id: id });
        Ok(task)
    }

    pub async fn cancel(&self, id: TaskId) -> Result<Task> {
        let task = self
            .store
            .update(
                id,
                Box::new(|task| {
                    task.state = TaskState::Cancelled;
                    Ok(())
                }),
            )
            .await?;
        if let Some(token) = self.running.lock().await.get(&id) {
            token.cancel();
        }
        self.events.emit(EventKind::TaskCancelled { task_id: id });
        Ok(task)
    }

    pub async fn resume(&self, id: TaskId) -> Result<Task> {
        let task = self
            .store
            .update(
                id,
                Box::new(|task| {
                    if task.state == TaskState::Completed {
                        return Err(Error::InvalidState("task already completed".to_string()));
                    }
                    task.state = TaskState::Pending;
                    task.error = None;
                    Ok(())
                }),
            )
            .await?;
        self.events.emit(EventKind::TaskStateChanged {
            task_id: id,
            state: TaskState::Pending,
        });
        Ok(task)
    }

    pub async fn list(&self) -> Result<Vec<Task>> {
        self.store.list().await
    }

    pub async fn status(&self, id: TaskId) -> Result<Task> {
        self.store.load(id).await
    }
}

struct StoreCheckpoint {
    store: Arc<dyn TaskStore>,
    task_id: TaskId,
}

#[async_trait]
impl ResumeCheckpoint for StoreCheckpoint {
    async fn save(&self, resume: &TaskResume) -> Result<()> {
        let resume = resume.clone();
        self.store
            .update(
                self.task_id,
                Box::new(move |task| {
                    task.resume = Some(resume.clone());
                    task.refresh_from_resume();
                    Ok(())
                }),
            )
            .await?;
        Ok(())
    }
}

fn file_items(files: &[FileOutcome]) -> Vec<FileItem> {
    files
        .iter()
        .map(|file| FileItem {
            path: file.relative_path.clone(),
            url: None,
            size: Some(file.size),
            downloaded: file.size,
            state: FileState::Completed,
            checksum: file.checksum.clone(),
            selected: true,
        })
        .collect()
}

#[allow(dead_code)]
fn _assert_resource(_: &Resource) {}
