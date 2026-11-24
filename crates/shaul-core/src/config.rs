use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::header::{Header, RequestOptions};
use crate::task::{TaskOptions, VerifyMode};

/// Effective configuration. Precedence is CLI flags > config file >
/// environment variables > built-in defaults; see `Config::load`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
#[non_exhaustive]
pub struct Config {
    /// Download behavior and defaults.
    pub download: DownloadConfig,
    /// HTTP timeout, retry and user-agent settings.
    pub network: NetworkConfig,
    /// Hub provider cache and endpoint settings.
    pub hub: HubConfig,
    /// BitTorrent backend settings.
    pub bt: BtConfig,
    /// Directory for task sidecars; defaults to the platform data directory.
    pub state_dir: Option<PathBuf>,
    /// Default headers applied to every request; values may use `${env:NAME}`
    /// placeholders.
    pub headers: BTreeMap<String, String>,
}

/// Defaults for how files are downloaded, resumed, verified and extracted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct DownloadConfig {
    /// Default destination directory.
    pub dir: PathBuf,
    /// Default number of parallel connections per task.
    pub connections: usize,
    /// Maximum number of idle connections kept per host in the HTTP pool.
    pub per_host: usize,
    /// Maximum number of active HTTP file-transfer requests per host across
    /// all tasks.
    pub per_host_active: usize,
    /// Maximum number of active HTTP file-transfer requests across all tasks.
    pub global: usize,
    /// Global bandwidth limit in bytes per second. `None` or `0` means
    /// unlimited. Direct HTTP and Hub file transfers share this limit.
    pub rate_limit: Option<u64>,
    /// Additional bandwidth cap per destination host in bytes per second.
    /// `None` or `0` means unlimited.
    pub per_host_rate_limit: Option<u64>,
    /// Number of files downloaded concurrently within one multi-file task.
    pub file_jobs: usize,
    /// Reuse verified downloads through the content-addressed cache.
    pub content_cache: bool,
    /// Override for the content-addressed cache root.
    pub content_cache_dir: Option<PathBuf>,
    /// Refuse a task before transfer when the destination filesystem has less
    /// free space than the known remaining size.
    pub disk_space_check: bool,
    /// Size in bytes of each ranged HTTP segment.
    pub segment_size: u64,
    /// Resume interrupted downloads from checkpoints by default.
    pub resume: bool,
    /// Default checksum verification policy.
    pub verify: VerifyMode,
    /// Extract archives after download by default.
    pub extract: bool,
    /// Default extraction directory; `None` extracts under the destination.
    pub extract_dir: Option<PathBuf>,
    /// Delete archives after successful extraction by default.
    pub delete_archive: bool,
}

/// HTTP networking defaults: timeouts, retries, user agent and auth policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct NetworkConfig {
    /// Timeout in seconds for receiving response headers and for each idle
    /// body read. This is not a total transfer deadline.
    pub timeout_secs: u64,
    /// Connection timeout in seconds.
    pub connect_timeout_secs: u64,
    /// Number of retry attempts for failed HTTP transfers. Hub metadata
    /// requests are not retried; FTP and BitTorrent manage their own
    /// connection-level retries.
    pub retries: u32,
    /// `User-Agent` header sent with every request.
    pub user_agent: String,
    /// Allow authentication headers to follow cross-origin redirects.
    pub allow_cross_origin_auth: bool,
    /// Explicit HTTP/HTTPS/SOCKS proxy URL. An empty string disables ambient
    /// proxy environment variables; `None` keeps reqwest's ambient behavior.
    pub proxy: Option<String>,
    /// Comma-separated hosts that bypass the configured proxy.
    pub no_proxy: Option<String>,
}

/// How HTTP clients should resolve proxy settings.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProxySetting {
    /// Use reqwest's ambient/system proxy environment.
    Ambient,
    /// Explicitly bypass every proxy, including environment variables.
    Disabled,
    /// Use the configured proxy URL.
    Url(String),
}

/// Hugging Face and ModelScope cache and endpoint settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
#[non_exhaustive]
pub struct HubConfig {
    /// Populate a provider cache while downloading. HF uses the Python
    /// compatible `models--org--repo` layout.
    pub use_cache: bool,
    /// Override for the Hugging Face cache root.
    pub hf_cache: Option<PathBuf>,
    /// Override for the ModelScope cache root.
    pub modelscope_cache: Option<PathBuf>,
    /// Override for the Hugging Face endpoint; defaults to the public site.
    pub hf_endpoint: Option<String>,
    /// Override for the ModelScope endpoint; defaults to the public site.
    pub modelscope_endpoint: Option<String>,
}

/// BitTorrent backend settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct BtConfig {
    /// Enable DHT peer discovery.
    pub dht: bool,
    /// Use the trackers announced by the torrent.
    pub trackers: bool,
    /// Listen for incoming peer connections.
    pub listen: bool,
    /// Persist session state and fastresume data under the state directory.
    pub persistence: bool,
    /// Optional limit on connected peers.
    pub peer_limit: Option<usize>,
    /// `ip:port` endpoints used directly as initial peers.
    pub initial_peers: Vec<String>,
}

impl Default for BtConfig {
    fn default() -> Self {
        Self {
            dht: true,
            trackers: true,
            listen: true,
            persistence: true,
            peer_limit: None,
            initial_peers: Vec::new(),
        }
    }
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            connections: 8,
            per_host: 16,
            per_host_active: 16,
            global: 64,
            rate_limit: None,
            per_host_rate_limit: None,
            file_jobs: 1,
            content_cache: false,
            content_cache_dir: None,
            disk_space_check: true,
            segment_size: 8 * 1024 * 1024,
            resume: true,
            verify: VerifyMode::Auto,
            extract: false,
            extract_dir: None,
            delete_archive: false,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 60,
            connect_timeout_secs: 15,
            retries: 5,
            user_agent: format!("shaul/{}", env!("CARGO_PKG_VERSION")),
            allow_cross_origin_auth: false,
            proxy: None,
            no_proxy: None,
        }
    }
}

/// Options after task overrides and config defaults have been resolved.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ResolvedOptions {
    /// Root directory for the task's output files.
    pub destination: PathBuf,
    /// Explicit output path, if one was requested.
    pub output: Option<PathBuf>,
    /// Effective connection count for this task.
    pub connections: usize,
    /// Effective HTTP idle-connection limit per host.
    pub per_host: usize,
    /// Maximum active HTTP file-transfer requests per host carried from
    /// configuration.
    pub per_host_active: usize,
    /// Global active HTTP file-transfer request budget carried from
    /// configuration.
    pub global: usize,
    /// Global bandwidth limit in bytes per second, if configured.
    pub rate_limit: Option<u64>,
    /// Per-host bandwidth limit in bytes per second, if configured.
    pub per_host_rate_limit: Option<u64>,
    /// Additional per-task bandwidth limit in bytes per second, if configured.
    pub task_rate_limit: Option<u64>,
    /// Number of files downloaded concurrently within this task.
    pub file_jobs: usize,
    /// Whether verified files are reused through the content-addressed cache.
    pub content_cache: bool,
    /// Root of the content-addressed cache.
    pub content_cache_dir: PathBuf,
    /// Whether to preflight the destination filesystem's free space.
    pub disk_space_check: bool,
    /// Alternate HTTP(S) URLs for the same file.
    pub mirrors: Vec<url::Url>,
    /// Byte size of each ranged HTTP segment.
    pub segment_size: u64,
    /// Whether resume is enabled for this task.
    pub resume: bool,
    /// Effective checksum verification policy.
    pub verify: VerifyMode,
    /// Expected checksum supplied by the task, if any.
    pub checksum: Option<crate::checksum::Checksum>,
    /// Whether archives should be extracted after download.
    pub extract: bool,
    /// Directory to extract archives into; `None` means the destination.
    pub extract_dir: Option<PathBuf>,
    /// Whether to delete the archive after extraction.
    pub delete_archive: bool,
    /// Whether to populate the provider cache for Hub tasks.
    pub cache: bool,
    /// Include glob filters passed from the task.
    pub include: Vec<String>,
    /// Exclude glob filters passed from the task.
    pub exclude: Vec<String>,
    /// Merged request options: config headers plus task overrides.
    pub request: RequestOptions,
    /// Whether existing destination files are replaced.
    pub overwrite: bool,
    /// Whether conflicting destinations are renamed.
    pub rename_on_conflict: bool,
    /// Effective retry count for failed transfers.
    pub retries: u32,
    /// Effective total request timeout.
    pub timeout: Duration,
    /// Effective connection timeout.
    pub connect_timeout: Duration,
    /// Effective `User-Agent` header.
    pub user_agent: String,
}

impl Config {
    /// Platform default config file path (`config.toml` in the user config
    /// directory).
    pub fn default_config_path() -> Result<PathBuf> {
        let dirs = project_dirs()?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Platform default directory for task sidecars.
    pub fn default_state_dir() -> Result<PathBuf> {
        let dirs = project_dirs()?;
        Ok(dirs.data_dir().join("tasks"))
    }

    /// Configured state directory, or the platform default.
    pub fn state_dir(&self) -> Result<PathBuf> {
        match &self.state_dir {
            Some(dir) => Ok(dir.clone()),
            None => Self::default_state_dir(),
        }
    }

    /// Resolved Hugging Face cache root: configured override or
    /// `~/.cache/huggingface/hub`.
    pub fn hf_cache_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = &self.hub.hf_cache {
            return Ok(dir.clone());
        }
        let base = BaseDirs::new()
            .ok_or_else(|| Error::Config("cannot determine home directory".to_string()))?;
        Ok(base
            .home_dir()
            .join(".cache")
            .join("huggingface")
            .join("hub"))
    }

    /// Resolved ModelScope cache root: configured override or the platform
    /// cache directory.
    pub fn modelscope_cache_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = &self.hub.modelscope_cache {
            return Ok(dir.clone());
        }
        Ok(project_dirs()?.cache_dir().join("modelscope"))
    }

    /// Resolved content-addressed cache root: configured override or the
    /// platform cache directory under `shaul/content`.
    pub fn content_cache_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = &self.download.content_cache_dir
            && !dir.as_os_str().is_empty()
        {
            return Ok(dir.clone());
        }
        Ok(project_dirs()?.cache_dir().join("content"))
    }

    /// Resolve the proxy mode from `network.proxy`. An empty configured value
    /// means "direct connection"; an unset value keeps reqwest's ambient
    /// environment behavior.
    pub fn proxy_setting(&self) -> ProxySetting {
        match self.network.proxy.as_deref() {
            None => ProxySetting::Ambient,
            Some(value) if value.trim().is_empty() => ProxySetting::Disabled,
            Some(value) => ProxySetting::Url(value.to_string()),
        }
    }

    /// Configured Hugging Face endpoint or the public default.
    pub fn hf_endpoint(&self) -> String {
        self.hub
            .hf_endpoint
            .clone()
            .unwrap_or_else(|| "https://huggingface.co".to_string())
    }

    /// Configured ModelScope endpoint or the public default.
    pub fn modelscope_endpoint(&self) -> String {
        self.hub
            .modelscope_endpoint
            .clone()
            .unwrap_or_else(|| "https://modelscope.cn".to_string())
    }

    /// Resolve the config path from an explicit argument, `SHAUL_CONFIG`, or
    /// the platform default.
    pub fn resolve_path(explicit: Option<&Path>) -> Result<PathBuf> {
        if let Some(path) = explicit {
            return Ok(path.to_path_buf());
        }
        if let Ok(path) = std::env::var("SHAUL_CONFIG")
            && !path.trim().is_empty()
        {
            return Ok(PathBuf::from(path));
        }
        Self::default_config_path()
    }

    /// Load defaults, then environment variables, then the config file. CLI
    /// flags are applied by the caller afterwards.
    pub fn load(explicit: Option<&Path>) -> Result<(Self, PathBuf)> {
        let path = Self::resolve_path(explicit)?;
        let mut config = Self::default();
        config.apply_env()?;
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            let partial: PartialConfig = toml::from_str(&text)?;
            partial.apply(&mut config)?;
        }
        Ok((config, path))
    }

    /// Load only defaults + file. Used by `config set` so environment values
    /// are not baked into the file.
    pub fn load_file_only(path: &Path) -> Result<Self> {
        let mut config = Self::default();
        if path.exists() {
            let text = std::fs::read_to_string(path)?;
            let partial: PartialConfig = toml::from_str(&text)?;
            partial.apply(&mut config)?;
        }
        Ok(config)
    }

    /// Serialize the config as TOML, creating parent directories as needed.
    ///
    /// The file is written through a sibling temporary file and renamed into
    /// place. On Unix it is created with mode 0600 because it can contain
    /// literal header values and proxy credentials.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        Self::save_text(path, &text)
    }

    /// Atomically write TOML text to `path`, creating parent directories as
    /// needed. On Unix the file is created with mode 0600.
    pub fn save_text(path: &Path, text: &str) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let file_name = path
            .file_name()
            .ok_or_else(|| Error::Config(format!("invalid config path: {}", path.display())))?;
        let mut temp_name = std::ffi::OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(".tmp-{}", uuid::Uuid::new_v4()));
        let temp = path.with_file_name(temp_name);

        let write = || -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            use std::io::Write;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        };
        if let Err(error) = write() {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.into());
        }
        Ok(())
    }

    /// Apply `SHAUL_*` and provider environment variables on top of the
    /// current values.
    pub fn apply_env(&mut self) -> Result<()> {
        if let Some(value) = env("SHAUL_DIR") {
            self.download.dir = PathBuf::from(value);
        }
        if let Some(value) = env_parse::<usize>("SHAUL_CONNECTIONS")? {
            self.download.connections = value;
        }
        if let Some(value) = env_parse::<usize>("SHAUL_PER_HOST")? {
            self.download.per_host = value;
        }
        if let Some(value) = env_parse::<usize>("SHAUL_PER_HOST_ACTIVE")? {
            self.download.per_host_active = value.max(1);
        }
        if let Some(value) = env_parse::<usize>("SHAUL_GLOBAL")? {
            self.download.global = value;
        }
        if let Some(value) = env("SHAUL_RATE_LIMIT") {
            self.download.rate_limit = Some(parse_bytes(&value)?).filter(|value| *value > 0);
        }
        if let Some(value) = env("SHAUL_PER_HOST_RATE_LIMIT") {
            self.download.per_host_rate_limit =
                Some(parse_bytes(&value)?).filter(|value| *value > 0);
        }
        if let Some(value) = env_parse::<usize>("SHAUL_FILE_JOBS")? {
            self.download.file_jobs = value.max(1);
        }
        if let Some(value) = env_parse::<bool>("SHAUL_CONTENT_CACHE")? {
            self.download.content_cache = value;
        }
        if let Some(value) = env("SHAUL_CONTENT_CACHE_DIR") {
            self.download.content_cache_dir = Some(PathBuf::from(value));
        }
        if let Some(value) = env_parse::<bool>("SHAUL_DISK_SPACE_CHECK")? {
            self.download.disk_space_check = value;
        }
        if let Some(value) = env("SHAUL_SEGMENT_SIZE") {
            self.download.segment_size = parse_bytes(&value)?;
        }
        if let Some(value) = env_parse::<bool>("SHAUL_RESUME")? {
            self.download.resume = value;
        }
        if let Some(value) = env("SHAUL_VERIFY") {
            self.download.verify = parse_verify_mode(&value)?;
        }
        if let Some(value) = env_parse::<bool>("SHAUL_EXTRACT")? {
            self.download.extract = value;
        }
        if let Some(value) = env("SHAUL_EXTRACT_DIR") {
            self.download.extract_dir = Some(PathBuf::from(value));
        }
        if let Some(value) = env_parse::<bool>("SHAUL_DELETE_ARCHIVE")? {
            self.download.delete_archive = value;
        }
        if let Some(value) = env("SHAUL_STATE_DIR") {
            self.state_dir = Some(PathBuf::from(value));
        }
        if let Some(value) = env_parse::<u64>("SHAUL_TIMEOUT_SECS")? {
            self.network.timeout_secs = value;
        }
        if let Some(value) = env_parse::<u64>("SHAUL_CONNECT_TIMEOUT_SECS")? {
            self.network.connect_timeout_secs = value;
        }
        if let Some(value) = env_parse::<u32>("SHAUL_RETRIES")? {
            self.network.retries = value;
        }
        if let Some(value) = env("SHAUL_USER_AGENT") {
            self.network.user_agent = value;
        }
        if let Some(value) = env_parse::<bool>("SHAUL_ALLOW_CROSS_ORIGIN_AUTH")? {
            self.network.allow_cross_origin_auth = value;
        }
        if let Some(value) = env("SHAUL_PROXY").or_else(|| {
            env("HTTPS_PROXY")
                .or_else(|| env("https_proxy"))
                .or_else(|| env("HTTP_PROXY"))
                .or_else(|| env("http_proxy"))
                .or_else(|| env("ALL_PROXY"))
                .or_else(|| env("all_proxy"))
        }) {
            self.network.proxy = Some(value);
        }
        if let Some(value) = env("SHAUL_NO_PROXY")
            .or_else(|| env("NO_PROXY"))
            .or_else(|| env("no_proxy"))
        {
            self.network.no_proxy = Some(value);
        }

        if let Some(value) = env("HF_HOME") {
            self.hub.hf_cache = Some(PathBuf::from(value).join("hub"));
        }
        if let Some(value) = env("HF_HUB_CACHE") {
            self.hub.hf_cache = Some(PathBuf::from(value));
        }
        if let Some(value) = env("SHAUL_HF_CACHE") {
            self.hub.hf_cache = Some(PathBuf::from(value));
        }
        if let Some(value) = env("HF_ENDPOINT") {
            self.hub.hf_endpoint = Some(value);
        }
        if let Some(value) = env("SHAUL_HF_ENDPOINT") {
            self.hub.hf_endpoint = Some(value);
        }
        if let Some(value) = env("MODELSCOPE_CACHE") {
            self.hub.modelscope_cache = Some(PathBuf::from(value));
        }
        if let Some(value) = env("SHAUL_MODELSCOPE_CACHE") {
            self.hub.modelscope_cache = Some(PathBuf::from(value));
        }
        if let Some(value) = env("MODELSCOPE_ENDPOINT").or_else(|| env("MODELSCOPE_DOMAIN")) {
            self.hub.modelscope_endpoint = Some(value);
        }
        if let Some(value) = env("SHAUL_MODELSCOPE_ENDPOINT") {
            self.hub.modelscope_endpoint = Some(value);
        }
        if let Some(value) = env_parse::<bool>("SHAUL_HUB_USE_CACHE")? {
            self.hub.use_cache = value;
        }

        if let Some(value) = env_parse::<bool>("SHAUL_BT_DHT")? {
            self.bt.dht = value;
        }
        if let Some(value) = env_parse::<bool>("SHAUL_BT_TRACKERS")? {
            self.bt.trackers = value;
        }
        if let Some(value) = env_parse::<bool>("SHAUL_BT_LISTEN")? {
            self.bt.listen = value;
        }
        if let Some(value) = env_parse::<bool>("SHAUL_BT_PERSISTENCE")? {
            self.bt.persistence = value;
        }
        if let Some(value) = env_parse::<usize>("SHAUL_BT_PEER_LIMIT")? {
            self.bt.peer_limit = Some(value);
        }
        if let Some(value) = env("SHAUL_BT_INITIAL_PEERS") {
            self.bt.initial_peers = value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect();
        }
        Ok(())
    }

    /// Parse the configured default headers into validated [`Header`] values.
    pub fn header_defaults(&self) -> Result<Vec<Header>> {
        self.headers
            .iter()
            .map(|(name, value)| Header::new(name, value))
            .collect()
    }

    /// Combine config defaults with per-task options into fully resolved
    /// settings.
    pub fn resolve(&self, destination: &Path, options: &TaskOptions) -> Result<ResolvedOptions> {
        let mut request = options.request.merged(&self.header_defaults()?);
        request.allow_cross_origin_auth |= self.network.allow_cross_origin_auth;
        Ok(ResolvedOptions {
            destination: destination.to_path_buf(),
            output: options.output.clone(),
            connections: options
                .connections
                .unwrap_or(self.download.connections)
                .max(1),
            per_host: self.download.per_host.max(1),
            per_host_active: self.download.per_host_active.max(1),
            global: self.download.global.max(1),
            rate_limit: self.download.rate_limit.filter(|value| *value > 0),
            per_host_rate_limit: self.download.per_host_rate_limit.filter(|value| *value > 0),
            task_rate_limit: options.rate_limit.filter(|value| *value > 0),
            file_jobs: options.file_jobs.unwrap_or(self.download.file_jobs).max(1),
            content_cache: options.content_cache.unwrap_or(self.download.content_cache),
            content_cache_dir: self.content_cache_dir()?,
            disk_space_check: self.download.disk_space_check,
            mirrors: options.mirrors.clone(),
            segment_size: self.download.segment_size.max(64 * 1024),
            resume: options.resume.unwrap_or(self.download.resume),
            verify: options.verify.unwrap_or(self.download.verify),
            checksum: options.checksum.clone(),
            extract: options.extract.unwrap_or(self.download.extract),
            extract_dir: options
                .extract_dir
                .clone()
                .or_else(|| self.download.extract_dir.clone()),
            delete_archive: options
                .delete_archive
                .unwrap_or(self.download.delete_archive),
            cache: options.cache.unwrap_or(self.hub.use_cache),
            include: options.include.clone(),
            exclude: options.exclude.clone(),
            request,
            overwrite: options.overwrite,
            rename_on_conflict: options.rename_on_conflict,
            retries: self.network.retries,
            timeout: Duration::from_secs(self.network.timeout_secs.max(1)),
            connect_timeout: Duration::from_secs(self.network.connect_timeout_secs.max(1)),
            user_agent: self.network.user_agent.clone(),
        })
    }

    /// Read a config value by its dotted key, as used by `config get`.
    pub fn get_value(&self, key: &str) -> Result<String> {
        let value = match key {
            "download.dir" => self.download.dir.display().to_string(),
            "download.connections" => self.download.connections.to_string(),
            "download.per_host" => self.download.per_host.to_string(),
            "download.per_host_active" => self.download.per_host_active.to_string(),
            "download.global" => self.download.global.to_string(),
            "download.rate_limit" => self
                .download
                .rate_limit
                .map(|value| value.to_string())
                .unwrap_or_default(),
            "download.per_host_rate_limit" => self
                .download
                .per_host_rate_limit
                .map(|value| value.to_string())
                .unwrap_or_default(),
            "download.file_jobs" => self.download.file_jobs.to_string(),
            "download.content_cache" => self.download.content_cache.to_string(),
            "download.content_cache_dir" => self.content_cache_dir()?.display().to_string(),
            "download.disk_space_check" => self.download.disk_space_check.to_string(),
            "download.segment_size" => self.download.segment_size.to_string(),
            "download.resume" => self.download.resume.to_string(),
            "download.verify" => verify_mode_str(self.download.verify).to_string(),
            "download.extract" => self.download.extract.to_string(),
            "download.extract_dir" => self
                .download
                .extract_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            "download.delete_archive" => self.download.delete_archive.to_string(),
            "network.timeout_secs" => self.network.timeout_secs.to_string(),
            "network.connect_timeout_secs" => self.network.connect_timeout_secs.to_string(),
            "network.retries" => self.network.retries.to_string(),
            "network.user_agent" => self.network.user_agent.clone(),
            "network.allow_cross_origin_auth" => self.network.allow_cross_origin_auth.to_string(),
            "network.proxy" => self.network.proxy.clone().unwrap_or_default(),
            "network.no_proxy" => self.network.no_proxy.clone().unwrap_or_default(),
            "hub.use_cache" => self.hub.use_cache.to_string(),
            "hub.hf_cache" => self.hf_cache_dir()?.display().to_string(),
            "hub.modelscope_cache" => self.modelscope_cache_dir()?.display().to_string(),
            "hub.hf_endpoint" => self.hf_endpoint(),
            "hub.modelscope_endpoint" => self.modelscope_endpoint(),
            "bt.dht" => self.bt.dht.to_string(),
            "bt.trackers" => self.bt.trackers.to_string(),
            "bt.listen" => self.bt.listen.to_string(),
            "bt.persistence" => self.bt.persistence.to_string(),
            "bt.peer_limit" => self
                .bt
                .peer_limit
                .map(|value| value.to_string())
                .unwrap_or_default(),
            "bt.initial_peers" => self.bt.initial_peers.join(","),
            "state_dir" => self
                .state_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            _ if key.starts_with("headers.") => {
                let name = &key["headers.".len()..];
                self.headers.get(name).cloned().unwrap_or_default()
            }
            _ => return Err(Error::Config(format!("unknown config key: {key}"))),
        };
        Ok(value)
    }

    /// Set a config value by its dotted key, as used by `config set`.
    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "download.dir" => self.download.dir = PathBuf::from(value),
            "download.connections" => self.download.connections = parse_value(value)?,
            "download.per_host" => self.download.per_host = parse_value(value)?,
            "download.per_host_active" => {
                self.download.per_host_active = parse_value::<usize>(value)?.max(1)
            }
            "download.global" => self.download.global = parse_value(value)?,
            "download.rate_limit" => {
                self.download.rate_limit = if value.trim().is_empty() {
                    None
                } else {
                    Some(parse_bytes(value)?).filter(|value| *value > 0)
                }
            }
            "download.per_host_rate_limit" => {
                self.download.per_host_rate_limit = if value.trim().is_empty() {
                    None
                } else {
                    Some(parse_bytes(value)?).filter(|value| *value > 0)
                }
            }
            "download.file_jobs" => self.download.file_jobs = parse_value::<usize>(value)?.max(1),
            "download.content_cache" => self.download.content_cache = parse_value(value)?,
            "download.content_cache_dir" => {
                self.download.content_cache_dir = if value.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                }
            }
            "download.disk_space_check" => self.download.disk_space_check = parse_value(value)?,
            "download.segment_size" => self.download.segment_size = parse_bytes(value)?,
            "download.resume" => self.download.resume = parse_value(value)?,
            "download.verify" => self.download.verify = parse_verify_mode(value)?,
            "download.extract" => self.download.extract = parse_value(value)?,
            "download.extract_dir" => {
                self.download.extract_dir = if value.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                }
            }
            "download.delete_archive" => self.download.delete_archive = parse_value(value)?,
            "network.timeout_secs" => self.network.timeout_secs = parse_value(value)?,
            "network.connect_timeout_secs" => {
                self.network.connect_timeout_secs = parse_value(value)?
            }
            "network.retries" => self.network.retries = parse_value(value)?,
            "network.user_agent" => self.network.user_agent = value.to_string(),
            "network.allow_cross_origin_auth" => {
                self.network.allow_cross_origin_auth = parse_value(value)?
            }
            "network.proxy" => self.network.proxy = Some(value.to_string()),
            "network.no_proxy" => {
                self.network.no_proxy = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            }
            "hub.use_cache" => self.hub.use_cache = parse_value(value)?,
            "hub.hf_cache" => {
                self.hub.hf_cache = if value.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                }
            }
            "hub.modelscope_cache" => {
                self.hub.modelscope_cache = if value.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                }
            }
            "hub.hf_endpoint" => {
                self.hub.hf_endpoint = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            }
            "hub.modelscope_endpoint" => {
                self.hub.modelscope_endpoint = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            }
            "bt.dht" => self.bt.dht = parse_value(value)?,
            "bt.trackers" => self.bt.trackers = parse_value(value)?,
            "bt.listen" => self.bt.listen = parse_value(value)?,
            "bt.persistence" => self.bt.persistence = parse_value(value)?,
            "bt.peer_limit" => {
                self.bt.peer_limit = if value.trim().is_empty() {
                    None
                } else {
                    Some(parse_value(value)?)
                }
            }
            "bt.initial_peers" => {
                self.bt.initial_peers = value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "state_dir" => {
                self.state_dir = if value.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                }
            }
            _ if key.starts_with("headers.") => {
                let name = key["headers.".len()..].to_string();
                Header::new(&name, value)?;
                if value.trim().is_empty() {
                    self.headers.remove(&name);
                } else {
                    self.headers.insert(name, value.to_string());
                }
            }
            _ => return Err(Error::Config(format!("unknown config key: {key}"))),
        }
        Ok(())
    }
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "shaul")
        .ok_or_else(|| Error::Config("cannot determine platform config directory".to_string()))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_parse<T: FromStr>(name: &str) -> Result<Option<T>> {
    match env(name) {
        Some(value) => value
            .parse::<T>()
            .map(Some)
            .map_err(|_| Error::Config(format!("invalid value for {name}: {value}"))),
        None => Ok(None),
    }
}

fn parse_value<T: FromStr>(value: &str) -> Result<T> {
    value
        .parse::<T>()
        .map_err(|_| Error::Config(format!("invalid value: {value}")))
}

fn non_empty_path(value: PathBuf) -> Option<PathBuf> {
    (!value.as_os_str().is_empty()).then_some(value)
}

fn non_empty_string(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

/// Parse a verify mode name or one of its boolean-like aliases.
pub fn parse_verify_mode(value: &str) -> Result<VerifyMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(VerifyMode::Auto),
        "require" | "on" | "true" | "yes" | "1" => Ok(VerifyMode::Require),
        "disabled" | "off" | "false" | "no" | "0" | "none" => Ok(VerifyMode::Disabled),
        _ => Err(Error::Config(format!("invalid verify mode: {value}"))),
    }
}

/// Canonical string form of a verify mode.
pub fn verify_mode_str(mode: VerifyMode) -> &'static str {
    match mode {
        VerifyMode::Auto => "auto",
        VerifyMode::Require => "require",
        VerifyMode::Disabled => "disabled",
    }
}

/// Parse byte sizes such as `8MiB`, `256K`, `1G` or a plain byte count.
pub fn parse_bytes(value: &str) -> Result<u64> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = lower.strip_suffix("kib") {
        (number, 1024)
    } else if let Some(number) = lower.strip_suffix("mib") {
        (number, 1024 * 1024)
    } else if let Some(number) = lower.strip_suffix("gib") {
        (number, 1024 * 1024 * 1024)
    } else if let Some(number) = lower.strip_suffix("kb") {
        (number, 1000)
    } else if let Some(number) = lower.strip_suffix("mb") {
        (number, 1000 * 1000)
    } else if let Some(number) = lower.strip_suffix("gb") {
        (number, 1000 * 1000 * 1000)
    } else if let Some(number) = lower.strip_suffix('k') {
        (number, 1024)
    } else if let Some(number) = lower.strip_suffix('m') {
        (number, 1024 * 1024)
    } else if let Some(number) = lower.strip_suffix('g') {
        (number, 1024 * 1024 * 1024)
    } else {
        (lower.as_str(), 1)
    };
    let number = number.trim();
    let parsed = number
        .parse::<f64>()
        .map_err(|_| Error::Config(format!("invalid byte size: {value}")))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(Error::Config(format!("invalid byte size: {value}")));
    }
    Ok((parsed * multiplier as f64) as u64)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialConfig {
    #[serde(default)]
    download: Option<PartialDownload>,
    #[serde(default)]
    network: Option<PartialNetwork>,
    #[serde(default)]
    hub: Option<PartialHub>,
    #[serde(default)]
    bt: Option<PartialBt>,
    #[serde(default)]
    state_dir: Option<PathBuf>,
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialDownload {
    #[serde(default)]
    dir: Option<PathBuf>,
    #[serde(default)]
    connections: Option<usize>,
    #[serde(default)]
    per_host: Option<usize>,
    #[serde(default)]
    per_host_active: Option<usize>,
    #[serde(default)]
    global: Option<usize>,
    #[serde(default)]
    rate_limit: Option<u64>,
    #[serde(default)]
    per_host_rate_limit: Option<u64>,
    #[serde(default)]
    file_jobs: Option<usize>,
    #[serde(default)]
    content_cache: Option<bool>,
    #[serde(default)]
    content_cache_dir: Option<PathBuf>,
    #[serde(default)]
    disk_space_check: Option<bool>,
    #[serde(default)]
    segment_size: Option<u64>,
    #[serde(default)]
    resume: Option<bool>,
    #[serde(default)]
    verify: Option<VerifyMode>,
    #[serde(default)]
    extract: Option<bool>,
    #[serde(default)]
    extract_dir: Option<PathBuf>,
    #[serde(default)]
    delete_archive: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialNetwork {
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    connect_timeout_secs: Option<u64>,
    #[serde(default)]
    retries: Option<u32>,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    allow_cross_origin_auth: Option<bool>,
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default)]
    no_proxy: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialHub {
    #[serde(default)]
    use_cache: Option<bool>,
    #[serde(default)]
    hf_cache: Option<PathBuf>,
    #[serde(default)]
    modelscope_cache: Option<PathBuf>,
    #[serde(default)]
    hf_endpoint: Option<String>,
    #[serde(default)]
    modelscope_endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialBt {
    #[serde(default)]
    dht: Option<bool>,
    #[serde(default)]
    trackers: Option<bool>,
    #[serde(default)]
    listen: Option<bool>,
    #[serde(default)]
    persistence: Option<bool>,
    #[serde(default)]
    peer_limit: Option<usize>,
    #[serde(default)]
    initial_peers: Option<Vec<String>>,
}

impl PartialConfig {
    fn apply(self, config: &mut Config) -> Result<()> {
        if let Some(download) = self.download {
            if let Some(value) = download.dir {
                config.download.dir = value;
            }
            if let Some(value) = download.connections {
                config.download.connections = value;
            }
            if let Some(value) = download.per_host {
                config.download.per_host = value;
            }
            if let Some(value) = download.per_host_active {
                config.download.per_host_active = value.max(1);
            }
            if let Some(value) = download.global {
                config.download.global = value;
            }
            if let Some(value) = download.rate_limit {
                config.download.rate_limit = (value > 0).then_some(value);
            }
            if let Some(value) = download.per_host_rate_limit {
                config.download.per_host_rate_limit = (value > 0).then_some(value);
            }
            if let Some(value) = download.file_jobs {
                config.download.file_jobs = value.max(1);
            }
            if let Some(value) = download.content_cache {
                config.download.content_cache = value;
            }
            if let Some(value) = download.content_cache_dir {
                config.download.content_cache_dir = non_empty_path(value);
            }
            if let Some(value) = download.disk_space_check {
                config.download.disk_space_check = value;
            }
            if let Some(value) = download.segment_size {
                config.download.segment_size = value;
            }
            if let Some(value) = download.resume {
                config.download.resume = value;
            }
            if let Some(value) = download.verify {
                config.download.verify = value;
            }
            if let Some(value) = download.extract {
                config.download.extract = value;
            }
            if let Some(value) = download.extract_dir {
                config.download.extract_dir = non_empty_path(value);
            }
            if let Some(value) = download.delete_archive {
                config.download.delete_archive = value;
            }
        }
        if let Some(network) = self.network {
            if let Some(value) = network.timeout_secs {
                config.network.timeout_secs = value;
            }
            if let Some(value) = network.connect_timeout_secs {
                config.network.connect_timeout_secs = value;
            }
            if let Some(value) = network.retries {
                config.network.retries = value;
            }
            if let Some(value) = network.user_agent {
                config.network.user_agent = value;
            }
            if let Some(value) = network.allow_cross_origin_auth {
                config.network.allow_cross_origin_auth = value;
            }
            if let Some(value) = network.proxy {
                config.network.proxy = Some(value);
            }
            if let Some(value) = network.no_proxy {
                config.network.no_proxy = (!value.trim().is_empty()).then_some(value);
            }
        }
        if let Some(hub) = self.hub {
            if let Some(value) = hub.use_cache {
                config.hub.use_cache = value;
            }
            if let Some(value) = hub.hf_cache {
                config.hub.hf_cache = non_empty_path(value);
            }
            if let Some(value) = hub.modelscope_cache {
                config.hub.modelscope_cache = non_empty_path(value);
            }
            if let Some(value) = hub.hf_endpoint {
                config.hub.hf_endpoint = non_empty_string(value);
            }
            if let Some(value) = hub.modelscope_endpoint {
                config.hub.modelscope_endpoint = non_empty_string(value);
            }
        }
        if let Some(bt) = self.bt {
            if let Some(value) = bt.dht {
                config.bt.dht = value;
            }
            if let Some(value) = bt.trackers {
                config.bt.trackers = value;
            }
            if let Some(value) = bt.listen {
                config.bt.listen = value;
            }
            if let Some(value) = bt.persistence {
                config.bt.persistence = value;
            }
            if let Some(value) = bt.peer_limit {
                config.bt.peer_limit = Some(value);
            }
            if let Some(value) = bt.initial_peers {
                config.bt.initial_peers = value
                    .into_iter()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect();
            }
        }
        if let Some(value) = self.state_dir {
            config.state_dir = non_empty_path(value);
        }
        if let Some(headers) = self.headers {
            for (name, value) in headers {
                Header::new(&name, &value)?;
                config.headers.insert(name, value);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_overrides_environment_layer() {
        let mut config = Config::default();
        config.download.connections = 3; // pretend env set this
        let partial: PartialConfig = toml::from_str("[download]\nconnections = 11\n").unwrap();
        partial.apply(&mut config).unwrap();
        assert_eq!(config.download.connections, 11);
    }

    #[test]
    fn parses_byte_sizes() {
        assert_eq!(parse_bytes("8MiB").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_bytes("1.5G").unwrap(), 1_610_612_736);
    }

    #[test]
    fn gets_and_sets_values() {
        let mut config = Config::default();
        config.set_value("download.connections", "12").unwrap();
        assert_eq!(config.get_value("download.connections").unwrap(), "12");
        config.set_value("download.rate_limit", "2MiB").unwrap();
        assert_eq!(
            config.get_value("download.rate_limit").unwrap(),
            (2 * 1024 * 1024).to_string()
        );
        config
            .set_value("download.per_host_rate_limit", "1MiB")
            .unwrap();
        assert_eq!(
            config.get_value("download.per_host_rate_limit").unwrap(),
            (1024 * 1024).to_string()
        );
        config.set_value("download.file_jobs", "3").unwrap();
        assert_eq!(config.get_value("download.file_jobs").unwrap(), "3");
        config.set_value("download.content_cache", "true").unwrap();
        assert_eq!(config.get_value("download.content_cache").unwrap(), "true");
        config
            .set_value("download.content_cache_dir", "/tmp/shaul-content")
            .unwrap();
        assert_eq!(
            config.get_value("download.content_cache_dir").unwrap(),
            Path::new("/tmp/shaul-content").display().to_string()
        );
        config.set_value("download.per_host_active", "7").unwrap();
        assert_eq!(config.get_value("download.per_host_active").unwrap(), "7");
        config
            .set_value("network.proxy", "http://127.0.0.1:8080")
            .unwrap();
        assert_eq!(
            config.get_value("network.proxy").unwrap(),
            "http://127.0.0.1:8080"
        );
        config
            .set_value("network.no_proxy", "localhost,127.0.0.1")
            .unwrap();
        assert_eq!(
            config.get_value("network.no_proxy").unwrap(),
            "localhost,127.0.0.1"
        );
        config.set_value("headers.X-Test", "yes").unwrap();
        assert_eq!(config.get_value("headers.X-Test").unwrap(), "yes");
    }

    #[test]
    fn rejects_unknown_config_keys() {
        for text in [
            "[download]\nconnection = 8\n",
            "[dowload]\nconnections = 8\n",
            "headers = { Authorization = \"x\" }\nextra = 1\n",
            "[bt]\npeer_limits = 5\n",
        ] {
            assert!(
                toml::from_str::<PartialConfig>(text).is_err(),
                "unknown key was accepted: {text}"
            );
        }
    }

    #[test]
    fn empty_optional_values_are_unset() {
        let partial: PartialConfig = toml::from_str(
            r#"
state_dir = ""
[download]
extract_dir = ""
content_cache_dir = ""
[hub]
hf_cache = ""
modelscope_cache = ""
hf_endpoint = ""
modelscope_endpoint = ""
[bt]
initial_peers = ["", " 1.2.3.4:6881 "]
"#,
        )
        .unwrap();
        let mut config = Config::default();
        partial.apply(&mut config).unwrap();
        assert!(config.state_dir.is_none());
        assert!(config.download.extract_dir.is_none());
        assert!(config.download.content_cache_dir.is_none());
        assert!(config.hub.hf_cache.is_none());
        assert!(config.hub.modelscope_cache.is_none());
        assert!(config.hub.hf_endpoint.is_none());
        assert!(config.hub.modelscope_endpoint.is_none());
        assert_eq!(config.bt.initial_peers, vec!["1.2.3.4:6881".to_string()]);
    }

    #[test]
    fn task_auth_overrides_config_authorization_header() {
        let mut config = Config::default();
        config
            .headers
            .insert("Authorization".to_string(), "Bearer config".to_string());
        let options = TaskOptions {
            request: RequestOptions {
                auth: Some(crate::header::Auth::bearer("cli")),
                ..RequestOptions::default()
            },
            ..TaskOptions::default()
        };
        let resolved = config.resolve(Path::new("."), &options).unwrap();
        assert_eq!(
            resolved.request.auth,
            Some(crate::header::Auth::bearer("cli"))
        );
        assert!(
            resolved
                .request
                .headers
                .iter()
                .all(|header| !header.name.eq_ignore_ascii_case("authorization"))
        );
    }

    #[test]
    fn proxy_setting_distinguishes_ambient_and_disabled() {
        let mut config = Config::default();
        assert_eq!(config.proxy_setting(), ProxySetting::Ambient);

        config.network.proxy = Some("http://127.0.0.1:8080".to_string());
        assert_eq!(
            config.proxy_setting(),
            ProxySetting::Url("http://127.0.0.1:8080".to_string())
        );

        let partial: PartialConfig = toml::from_str("[network]\nproxy = \"\"\n").unwrap();
        config.network.proxy = Some("http://env:8080".to_string());
        partial.apply(&mut config).unwrap();
        assert_eq!(config.network.proxy, Some(String::new()));
        assert_eq!(config.proxy_setting(), ProxySetting::Disabled);

        config.set_value("network.proxy", "").unwrap();
        assert_eq!(config.proxy_setting(), ProxySetting::Disabled);
    }

    #[test]
    fn save_replaces_the_file_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        config.download.connections = 3;
        config.save(&path).unwrap();
        config.download.connections = 7;
        config.save(&path).unwrap();

        let loaded = Config::load_file_only(&path).unwrap();
        assert_eq!(loaded.download.connections, 7);
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn resolves_proxy_and_rate_limits() {
        let mut config = Config::default();
        config.download.rate_limit = Some(1024);
        config.download.per_host_rate_limit = Some(512);
        config.network.proxy = Some("http://127.0.0.1:8080".to_string());
        config.network.no_proxy = Some("localhost".to_string());
        config.download.file_jobs = 4;
        config.download.content_cache = true;
        let options = TaskOptions {
            rate_limit: Some(2048),
            file_jobs: Some(2),
            content_cache: Some(false),
            ..TaskOptions::default()
        };
        let resolved = config.resolve(Path::new("."), &options).unwrap();
        assert_eq!(resolved.rate_limit, Some(1024));
        assert_eq!(resolved.per_host_rate_limit, Some(512));
        assert_eq!(resolved.task_rate_limit, Some(2048));
        assert_eq!(resolved.file_jobs, 2);
        assert_eq!(resolved.per_host_active, 16);
        assert!(!resolved.content_cache);
        assert!(resolved.content_cache_dir.ends_with("content"));

        let partial: PartialConfig = toml::from_str(
            "[download]\nrate_limit = 0\n[network]\nproxy = \"\"\nno_proxy = \"\"\n",
        )
        .unwrap();
        partial.apply(&mut config).unwrap();
        assert_eq!(config.download.rate_limit, None);
        assert_eq!(config.network.proxy, Some(String::new()));
        assert_eq!(config.proxy_setting(), ProxySetting::Disabled);
        assert_eq!(config.network.no_proxy, None);
    }
}
