//! Low-cardinality metrics hooks for external monitoring systems.
//!
//! Logging and tracing use the [`tracing`] crate; this module is for numeric
//! metrics that should not be carried on the per-task event stream. Install an
//! implementation with [`crate::DownloadEngine::with_telemetry`].
//!
//! Implementations are called from the task executor and must not block. Use a
//! channel or batch updates when exporting to a slow backend.

/// Receives counters, gauges and histograms emitted by the engine.
///
/// Every method has a no-op default so implementations only override the
/// metric kinds they care about. Label values are low-cardinality by design
/// (for example `backend = "http"`); task ids are not used as labels.
pub trait Telemetry: Send + Sync {
    /// Adds `value` to a monotonically increasing counter.
    fn counter(&self, _name: &str, _value: u64, _labels: &[(&str, &str)]) {}
    /// Records the current value of a gauge.
    fn gauge(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}
    /// Records an observation in a histogram (seconds, bytes, and so on).
    fn histogram(&self, _name: &str, _value: f64, _labels: &[(&str, &str)]) {}
}

/// A telemetry implementation that discards every metric.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopTelemetry;

impl Telemetry for NoopTelemetry {}

/// Stable metric names emitted by the engine.
///
/// Labels: task lifecycle metrics use `backend`; duration also uses `state`
/// (`completed`, `failed`, `paused` or `cancelled`). `shaul.active_tasks` has
/// no labels.
pub mod metrics {
    /// A task was persisted. Labels: `backend`.
    pub const TASKS_ADDED: &str = "shaul.tasks.added";
    /// A task began running. Labels: `backend`.
    pub const TASKS_STARTED: &str = "shaul.tasks.started";
    /// A task completed successfully. Labels: `backend`.
    pub const TASKS_COMPLETED: &str = "shaul.tasks.completed";
    /// A task failed. Labels: `backend`.
    pub const TASKS_FAILED: &str = "shaul.tasks.failed";
    /// A task was paused. Labels: `backend`.
    pub const TASKS_PAUSED: &str = "shaul.tasks.paused";
    /// A task was cancelled. Labels: `backend`.
    pub const TASKS_CANCELLED: &str = "shaul.tasks.cancelled";
    /// Wall-clock duration of one run attempt. Labels: `backend`, `state`.
    pub const TASK_DURATION_SECONDS: &str = "shaul.task.duration_seconds";
    /// Bytes of a successfully completed task. Labels: `backend`.
    pub const BYTES_DOWNLOADED: &str = "shaul.bytes.downloaded";
    /// Number of tasks currently owned by this engine. No labels.
    pub const ACTIVE_TASKS: &str = "shaul.active_tasks";
}
