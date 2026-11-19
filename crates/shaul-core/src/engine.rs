use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::telemetry::{NoopTelemetry, Telemetry, metrics};

/// Coordinates the task lifecycle: resolving links, adding and running tasks,
/// pausing, resuming, cancelling and requeueing tasks left behind by a crashed
/// process.
pub struct DownloadEngine {
    router: Arc<Router>,
    store: Arc<dyn TaskStore>,
    config: Config,
    events: EventSink,
    running: Arc<Mutex<HashMap<TaskId, CancellationToken>>>,
    finalizers: FinalizerPipeline,
    telemetry: Arc<dyn Telemetry>,
}

impl DownloadEngine {
    /// Creates an engine over the given router, task store and configuration.
    /// Subscribe with [`DownloadEngine::subscribe`] before adding tasks to
    /// observe their events.
    pub fn new(router: Arc<Router>, store: Arc<dyn TaskStore>, config: Config) -> Self {
        let (events, _) = EventSink::channel(4096);
        Self {
            router,
            store,
            config,
            events,
            running: Arc::new(Mutex::new(HashMap::new())),
            finalizers: FinalizerPipeline::default(),
            telemetry: Arc::new(NoopTelemetry),
        }
    }

    /// Replaces the no-op telemetry sink. Builder form; use
    /// [`DownloadEngine::set_telemetry`] to change it after construction.
    pub fn with_telemetry(mut self, telemetry: Arc<dyn Telemetry>) -> Self {
        self.telemetry = telemetry.clone();
        self.events.set_telemetry(telemetry);
        self
    }

    /// Replaces the telemetry sink used for subsequent tasks.
    pub fn set_telemetry(&mut self, telemetry: Arc<dyn Telemetry>) {
        self.telemetry = telemetry.clone();
        self.events.set_telemetry(telemetry);
    }

    /// Returns the installed telemetry sink.
    pub fn telemetry(&self) -> &Arc<dyn Telemetry> {
        &self.telemetry
    }

    /// Replaces the post-download finalizer pipeline. The default verifies
    /// checksums and extracts archives.
    pub fn with_finalizers(mut self, finalizers: FinalizerPipeline) -> Self {
        self.finalizers = finalizers;
        self
    }

    /// Replaces the finalizer pipeline used for subsequent tasks.
    pub fn set_finalizers(&mut self, finalizers: FinalizerPipeline) {
        self.finalizers = finalizers;
    }

    /// Subscribes to the engine's event stream. Only events emitted after the
    /// call are received.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::event::DownloadEvent> {
        self.events.subscribe()
    }

    /// Returns the configuration used to resolve task options.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the event sink backends use to report progress.
    pub fn events(&self) -> &EventSink {
        &self.events
    }

    /// Returns the task store backing the engine.
    pub fn store(&self) -> &Arc<dyn TaskStore> {
        &self.store
    }

    /// Parses `link` and selects a backend without creating a task.
    pub async fn resolve(&self, link: &Link, options: &ParseOptions) -> Result<RoutedResource> {
        self.router.resolve(link, options).await
    }

    /// Resolves `link` and persists a new pending task.
    ///
    /// Unless [`TaskOptions::force`] is set, an active (pending, running or
    /// paused) task with the same canonical link and destination is returned
    /// instead of creating a duplicate.
    #[tracing::instrument(
        name = "shaul.engine.add",
        level = "debug",
        skip(self, link, destination, options)
    )]
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
        let backend = task.backend.as_deref().unwrap_or("unknown");
        self.telemetry
            .counter(metrics::TASKS_ADDED, 1, &[("backend", backend)]);
        tracing::debug!(task_id = %task.id, backend, "task added");
        Ok(task)
    }

    /// Runs a task to a terminal state without an external cancellation token.
    pub async fn run_task(&self, id: TaskId) -> Result<Task> {
        self.run_task_with_cancel(id, None).await
    }

    /// Runs a task, optionally linked to an external cancellation token.
    ///
    /// A task that is already completed is returned unchanged, while running a
    /// cancelled task is an error. A pause or cancel requested through the
    /// store (for example by another process) is observed while the transfer
    /// runs and stops it.
    #[tracing::instrument(
        name = "shaul.engine.run_task",
        level = "debug",
        skip(self, external_cancel),
        fields(task_id = %id)
    )]
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

        let backend_name = task
            .backend
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let metric_labels = [("backend", backend_name.as_str())];
        let started = Instant::now();
        let resource = task
            .resource
            .clone()
            .ok_or_else(|| Error::InvalidState(format!("task {id} has no parsed resource")))?;
        let backend = self.router.backend_for(&resource)?;
        let capabilities = backend.capabilities();
        let mut options = self.config.resolve(&task.destination, &task.options)?;
        if !capabilities.resume {
            options.resume = false;
        }
        let resume = if capabilities.resume && options.resume {
            task.resume.clone()
        } else {
            None
        };
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
        self.telemetry
            .counter(metrics::TASKS_STARTED, 1, &metric_labels);
        let active = self.running.lock().await.len() as f64;
        self.telemetry.gauge(metrics::ACTIVE_TASKS, active, &[]);
        tracing::debug!(task_id = %id, backend = %backend_name, "task started");
        if !capabilities.resume && task.resume.is_some() {
            self.events.emit(EventKind::Warning {
                task_id: Some(id),
                message: format!(
                    "backend {backend_name} does not support resume; starting this task from scratch"
                ),
            });
        }

        let checkpoint = Arc::new(StoreCheckpoint {
            store: self.store.clone(),
            task_id: id,
        });
        let request = DownloadRequest {
            task_id: id,
            resource,
            destination: task.destination.clone(),
            options: options.clone(),
            resume,
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

        let state = match &result {
            Ok(task) => match task.state {
                TaskState::Completed => "completed",
                TaskState::Paused => "paused",
                TaskState::Cancelled => "cancelled",
                _ => "unknown",
            },
            Err(_) => "failed",
        };
        let state_labels = [("backend", backend_name.as_str()), ("state", state)];
        self.telemetry.histogram(
            metrics::TASK_DURATION_SECONDS,
            started.elapsed().as_secs_f64(),
            &state_labels,
        );
        match state {
            "completed" => {
                self.telemetry
                    .counter(metrics::TASKS_COMPLETED, 1, &metric_labels);
                if let Ok(task) = &result {
                    self.telemetry.counter(
                        metrics::BYTES_DOWNLOADED,
                        task.downloaded_bytes(),
                        &metric_labels,
                    );
                }
            }
            "paused" => self
                .telemetry
                .counter(metrics::TASKS_PAUSED, 1, &metric_labels),
            "cancelled" => self
                .telemetry
                .counter(metrics::TASKS_CANCELLED, 1, &metric_labels),
            _ => self
                .telemetry
                .counter(metrics::TASKS_FAILED, 1, &metric_labels),
        }
        let active = self.running.lock().await.len() as f64;
        self.telemetry.gauge(metrics::ACTIVE_TASKS, active, &[]);
        tracing::debug!(task_id = %id, backend = %backend_name, state, "task finished");
        result
    }

    #[tracing::instrument(
        name = "shaul.engine.attempt",
        level = "debug",
        skip(self, request, backend, options),
        fields(task_id = %request.task_id)
    )]
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
                            tracing::warn!(
                                task_id = %request.task_id,
                                "checksum mismatch; retrying once from scratch"
                            );
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

    #[tracing::instrument(
        name = "shaul.engine.finalize",
        level = "debug",
        skip(self, destination, options, files),
        fields(task_id = %task_id, files = files.len())
    )]
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

    #[tracing::instrument(
        name = "shaul.engine.fail",
        level = "debug",
        skip(self, error),
        fields(task_id = %id)
    )]
    async fn fail(&self, id: TaskId, error: Error) -> Result<()> {
        let message = error.to_string();
        tracing::warn!(task_id = %id, error = %message, "task failed");
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

    /// Runs every task currently in `Pending` state, at most `jobs` at a time
    /// (`jobs` is clamped to at least one). Higher-priority tasks are selected
    /// first, then older tasks. All tasks share `cancel`; tasks whose run fails
    /// are omitted from the returned list.
    pub async fn run_pending(&self, jobs: usize, cancel: CancellationToken) -> Result<Vec<Task>> {
        let tasks = self.store.list().await?;
        let pending = tasks
            .iter()
            .filter(|task| task.state == TaskState::Pending)
            .count() as f64;
        self.telemetry.gauge(metrics::QUEUE_DEPTH, pending, &[]);
        let ids = pending_task_ids(tasks);
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

    /// Marks a task paused and cancels its in-flight transfer, if any. It can
    /// later be restarted with [`DownloadEngine::resume`] or `run_task`;
    /// pausing a completed task is an error.
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

    /// Marks a task cancelled and cancels its in-flight transfer, if any. A
    /// cancelled task is not restarted by [`DownloadEngine::run_task`]; use
    /// [`DownloadEngine::resume`] to move it back to pending. Cancelling an
    /// already completed task is an error.
    pub async fn cancel(&self, id: TaskId) -> Result<Task> {
        let task = self
            .store
            .update(
                id,
                Box::new(|task| {
                    if task.state == TaskState::Completed {
                        return Err(Error::InvalidState("task already completed".to_string()));
                    }
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

    /// Moves a task back to `Pending` and clears its error so the next run
    /// retries it, reusing any saved resume manifest. Resuming an already
    /// completed task is an error.
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

    /// Returns all tasks known to the store, oldest first.
    pub async fn list(&self) -> Result<Vec<Task>> {
        self.store.list().await
    }

    /// Loads a single task by id.
    pub async fn status(&self, id: TaskId) -> Result<Task> {
        self.store.load(id).await
    }
}

fn pending_task_ids(mut tasks: Vec<Task>) -> Vec<TaskId> {
    tasks.retain(|task| task.state == TaskState::Pending);
    tasks.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.created_at_ms.cmp(&right.created_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    });
    tasks.into_iter().map(|task| task.id).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Link;

    fn task(link: &str, priority: Option<i32>, created_at_ms: u64) -> Task {
        let mut task = Task::new(
            Link::new(link).unwrap(),
            PathBuf::from("."),
            TaskOptions {
                priority,
                ..TaskOptions::default()
            },
        );
        task.created_at_ms = created_at_ms;
        task
    }

    #[test]
    fn orders_pending_tasks_by_priority_then_age() {
        let low = task("low", Some(-1), 1);
        let high_old = task("high-old", Some(10), 2);
        let high_new = task("high-new", Some(10), 3);
        let normal = task("normal", None, 4);
        let mut paused = task("paused", Some(100), 0);
        paused.state = TaskState::Paused;

        let ordered = pending_task_ids(vec![
            normal.clone(),
            high_new.clone(),
            paused,
            low.clone(),
            high_old.clone(),
        ]);
        assert_eq!(ordered, vec![high_old.id, high_new.id, normal.id, low.id]);
    }
}
