use thiserror::Error;

/// Errors produced by the core engine, store, finalizers and backends.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The supplied link is malformed or empty.
    #[error("invalid link: {0}")]
    InvalidLink(String),
    /// No registered parser accepted the link.
    #[error("no parser accepted link: {0}")]
    NoParser(String),
    /// No registered backend supports the resolved resource.
    #[error("no backend supports resource: {0}")]
    NoBackend(String),
    /// No task with the given identifier exists in the store.
    #[error("task not found: {0}")]
    TaskNotFound(String),
    /// The task is not in a state that allows the requested operation.
    #[error("invalid task state: {0}")]
    InvalidState(String),
    /// A downloaded file did not match the expected checksum.
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Verification {
        /// Expected digest formatted as `<algorithm>:<hex>`.
        expected: String,
        /// Digest computed from the downloaded file.
        actual: String,
    },
    /// Verification was required but no checksum is available.
    #[error("no checksum available for verification")]
    ChecksumMissing,
    /// Archive extraction failed.
    #[error("extraction failed: {0}")]
    Extraction(String),
    /// The task was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The backend reported an error.
    #[error("backend error: {0}")]
    Backend(String),
    /// The configuration is invalid or inconsistent.
    #[error("config error: {0}")]
    Config(String),
    /// An I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization or deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// TOML serialization or deserialization failed.
    #[error("toml error: {0}")]
    Toml(String),
    /// A URL could not be parsed.
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
    /// An error that does not fit any other category.
    #[error("{0}")]
    Other(String),
}

impl From<toml::de::Error> for Error {
    fn from(value: toml::de::Error) -> Self {
        Error::Toml(value.to_string())
    }
}

impl From<toml::ser::Error> for Error {
    fn from(value: toml::ser::Error) -> Self {
        Error::Toml(value.to_string())
    }
}

impl Error {
    /// Returns `true` if this error represents a cancellation.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Error::Cancelled)
    }
}

/// Result type used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
