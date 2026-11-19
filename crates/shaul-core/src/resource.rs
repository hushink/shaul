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
    /// Protocol family and its identifying details.
    pub kind: ResourceKind,
    /// Display name chosen by the parser, such as the URL filename or the
    /// repository name.
    pub name: String,
    /// Files that make up the resource. Protocols that only discover files
    /// while downloading, such as BitTorrent, leave this empty.
    pub files: Vec<ResourceFile>,
    /// Request overrides carried over from the CLI and configuration.
    #[serde(default)]
    pub request: RequestOptions,
    /// Checksum supplied when the task was created, preserved in the
    /// sidecar and shown in CLI output.
    #[serde(default)]
    pub checksum: Option<Checksum>,
    /// Revision requested for the task, such as a Hub branch or tag.
    #[serde(default)]
    pub revision: Option<String>,
    /// Protocol-private key/value data persisted in the sidecar. New
    /// protocol fields belong here rather than in new struct fields.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
/// Protocol family a [`Resource`] is fetched with.
pub enum ResourceKind {
    /// A direct HTTP(S) download.
    Http,
    /// A repository snapshot from a model hub.
    Hub {
        /// Hub service hosting the repository.
        provider: HubProvider,
        /// Repository identifier such as `org/name`.
        repo: String,
        /// Whether the repository is a model or a dataset.
        repo_type: HubRepoType,
        /// Branch, tag, or commit to resolve.
        revision: String,
    },
    /// A torrent, given as a magnet link or a `.torrent` file.
    BitTorrent {
        /// Where the torrent metadata comes from.
        source: BitTorrentSource,
        /// Display name taken from the torrent metadata, when available.
        name: Option<String>,
    },
    /// A third-party protocol selected by a custom parser and consumed by a
    /// custom [`DownloadBackend`](crate::backend::DownloadBackend).
    ///
    /// `protocol` is matched by the backend's `supports` method; protocol
    /// data belongs in [`Resource::metadata`] and
    /// [`ResourceFile::metadata`].
    Custom {
        /// Parser-defined protocol identifier, for example `demo`.
        protocol: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
/// Where the metadata for a BitTorrent resource comes from.
pub enum BitTorrentSource {
    /// A magnet link, or a bare v1 info hash.
    Magnet,
    /// A local `.torrent` file.
    TorrentFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
/// Hub service hosting a repository snapshot.
pub enum HubProvider {
    /// Hugging Face, including compatible mirrors.
    HuggingFace,
    /// ModelScope.
    ModelScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
/// Kind of repository hosted on a hub.
pub enum HubRepoType {
    /// A model repository.
    Model,
    /// A dataset repository.
    Dataset,
}

/// One file belonging to a [`Resource`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceFile {
    /// Relative output path. Always validated and safe to join under the
    /// task destination.
    pub path: String,
    /// Resolved URL the backend fetches.
    pub url: Url,
    /// Expected size in bytes, when the protocol or a previous probe
    /// reported it.
    #[serde(default)]
    pub size: Option<u64>,
    /// Expected checksum for this file, when known.
    #[serde(default)]
    pub checksum: Option<Checksum>,
    /// Additional headers applied only to this file's requests.
    #[serde(default)]
    pub headers: Vec<Header>,
    /// Protocol-private key/value data preserved in the sidecar.
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
    /// Build a single-file [`ResourceKind::Http`] resource for `url`.
    ///
    /// The display name and output path are derived from the URL's last path
    /// segment and validated with [`sanitize_relative_path`], so an unsafe
    /// filename fails the parse instead of being used.
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

    /// Sum of the known file sizes, or `None` if any file size is unknown.
    /// An empty file list sums to `Some(0)`.
    pub fn total_size(&self) -> Option<u64> {
        self.files
            .iter()
            .try_fold(0u64, |sum, file| file.size.map(|size| sum + size))
    }
}

impl ResourceFile {
    /// Validate `path` as a destination-relative path and replace the file's
    /// output path. Absolute paths and `..` traversal return an error.
    pub fn with_path(mut self, path: impl AsRef<str>) -> Result<Self> {
        self.path = sanitize_relative_path(path.as_ref())?;
        Ok(self)
    }
}
