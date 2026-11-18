use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::checksum::Checksum;
use crate::error::Result;
use crate::header::{Header, RequestOptions};
use crate::path::{filename_from_url, sanitize_relative_path};

/// A parsed, backend-agnostic description of what the user wants to download.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub kind: ResourceKind,
    pub name: String,
    pub files: Vec<ResourceFile>,
    #[serde(default)]
    pub request: RequestOptions,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResourceKind {
    Http,
    Hub {
        provider: HubProvider,
        repo: String,
        repo_type: HubRepoType,
        revision: String,
    },
    BitTorrent {
        source: BitTorrentSource,
        name: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitTorrentSource {
    Magnet,
    TorrentFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubProvider {
    HuggingFace,
    ModelScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubRepoType {
    Model,
    Dataset,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceFile {
    /// Relative output path. Always validated and safe to join under the
    /// task destination.
    pub path: String,
    pub url: Url,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Direct links prefer the server's `Content-Disposition` filename; Hub
    /// snapshots use the repository-relative path.
    #[serde(default = "default_true")]
    pub use_response_filename: bool,
}

fn default_true() -> bool {
    true
}

impl Resource {
    pub fn http_file(
        url: Url,
        request: RequestOptions,
        checksum: Option<Checksum>,
    ) -> Result<Self> {
        let name = filename_from_url(&url);
        let path = sanitize_relative_path(&name)?;
        Ok(Self {
            kind: ResourceKind::Http,
            name,
            files: vec![ResourceFile {
                path,
                url,
                size: None,
                checksum: None,
                headers: Vec::new(),
                metadata: BTreeMap::new(),
                use_response_filename: true,
            }],
            request,
            checksum,
            revision: None,
            metadata: BTreeMap::new(),
        })
    }

    pub fn total_size(&self) -> Option<u64> {
        self.files
            .iter()
            .try_fold(0u64, |sum, file| file.size.map(|size| sum + size))
    }
}

impl ResourceFile {
    pub fn with_path(mut self, path: impl AsRef<str>) -> Result<Self> {
        self.path = sanitize_relative_path(path.as_ref())?;
        Ok(self)
    }
}
