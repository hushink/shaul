use thiserror::Error;

/// Errors produced by the core engine, store, finalizers and backends.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid link: {0}")]
    InvalidLink(String),
    #[error("no parser accepted link: {0}")]
    NoParser(String),
    #[error("no backend supports resource: {0}")]
    NoBackend(String),
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("invalid task state: {0}")]
    InvalidState(String),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Verification { expected: String, actual: String },
    #[error("no checksum available for verification")]
    ChecksumMissing,
    #[error("extraction failed: {0}")]
    Extraction(String),
    #[error("cancelled")]
    Cancelled,
    #[error("backend error: {0}")]
    Backend(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml error: {0}")]
    Toml(String),
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
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
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Error::Cancelled)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
