use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The raw input supplied by a user: a URL, magnet link, torrent path or Hub
/// repository reference.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Link(String);

impl Link {
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw = raw.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(Error::InvalidLink("link is empty".to_string()));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Stable key used for de-duplicating active tasks.
    pub fn canonical(&self) -> String {
        self.0.trim().to_string()
    }
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for Link {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self> {
        Link::new(value)
    }
}

impl TryFrom<String> for Link {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Link::new(value)
    }
}
