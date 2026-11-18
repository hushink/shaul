use std::path::PathBuf;
use std::sync::Arc;

use tokio::runtime::{Builder, Runtime};

use crate::config::Config;
use crate::engine::DownloadEngine;
use crate::error::{Error, Result};
use crate::event::DownloadEvent;
use crate::link::Link;
use crate::router::Router;
use crate::store::TaskStore;
use crate::task::{Task, TaskId, TaskOptions};

/// Synchronous facade for callers that do not want to own a Tokio runtime.
/// The async engine remains the single implementation.
pub struct BlockingEngine {
    runtime: Runtime,
    engine: Arc<DownloadEngine>,
}

impl BlockingEngine {
    /// Creates an engine backed by a dedicated multi-threaded Tokio runtime.
    pub fn new(router: Router, store: Arc<dyn TaskStore>, config: Config) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(Error::Io)?;
        let engine = Arc::new(DownloadEngine::new(Arc::new(router), store, config));
        Ok(Self { runtime, engine })
    }

    /// Returns a reference to the underlying asynchronous engine.
    pub fn engine(&self) -> &DownloadEngine {
        &self.engine
    }

    /// Subscribes to engine events, delivered over a standard library channel.
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<DownloadEvent> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut events = self.engine.subscribe();
        self.runtime.spawn(async move {
            while let Ok(event) = events.recv().await {
                if sender.send(event).is_err() {
                    break;
                }
            }
        });
        receiver
    }

    /// Adds a task and runs it to completion, blocking the current thread.
    pub fn download(&self, link: Link, destination: PathBuf, options: TaskOptions) -> Result<Task> {
        self.runtime.block_on(async {
            let task = self.engine.add(link, destination, options).await?;
            self.engine.run_task(task.id).await
        })
    }

    /// Registers a task without running it.
    pub fn add(&self, link: Link, destination: PathBuf, options: TaskOptions) -> Result<Task> {
        self.runtime
            .block_on(self.engine.add(link, destination, options))
    }

    /// Blocks until the given task finishes and returns its final state.
    pub fn run_task(&self, id: TaskId) -> Result<Task> {
        self.runtime.block_on(self.engine.run_task(id))
    }

    /// Lists all stored tasks.
    pub fn list(&self) -> Result<Vec<Task>> {
        self.runtime.block_on(self.engine.list())
    }
}
