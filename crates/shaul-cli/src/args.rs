use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
use shaul::{
    Auth, Checksum, Error, Header, ParseOptions, RequestOptions, Result, TaskOptions, VerifyMode,
};

use crate::support::{parse_basic_auth, parse_checksum, parse_headers, read_header_file};

#[derive(Parser, Debug)]
#[command(
    name = "shaul",
    version,
    about = "A unified, resumable download engine for HTTP, BitTorrent and model hubs",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Path to config.toml (overrides SHAUL_CONFIG and the platform default)
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Emit NDJSON events / JSON output
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress progress output
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,

    /// Increase log verbosity
    #[arg(long, short = 'v', global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// HTTP/HTTPS/SOCKS proxy URL
    #[arg(long, global = true, value_name = "URL")]
    pub proxy: Option<String>,

    /// Comma-separated hosts that bypass the proxy
    #[arg(long, global = true, value_name = "HOSTS")]
    pub no_proxy: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Download links immediately
    Download(DownloadArgs),
    /// Add links to the persistent queue
    Add(DownloadArgs),
    /// Run the queue worker
    Run(RunArgs),
    /// List known tasks
    List,
    /// Show a task
    Status(IdArgs),
    /// Pause a task
    Pause(IdArgs),
    /// Requeue a paused or failed task
    Resume(IdArgs),
    /// Cancel a task
    Cancel(IdArgs),
    /// Resolve a link without downloading
    Info(InfoArgs),
    /// Manage configuration
    Config(ConfigArgs),
    /// Generate shell completions
    Completions { shell: clap_complete::Shell },
}

#[derive(Args, Debug, Clone, Default)]
pub struct RequestArgs {
    /// Extra HTTP header, repeatable ("Name: value")
    #[arg(long = "header", short = 'H', value_name = "NAME: VALUE")]
    pub header: Vec<String>,

    /// Read headers from a file (one "Name: value" per line)
    #[arg(long, value_name = "PATH")]
    pub header_file: Option<PathBuf>,

    /// Bearer token for the request
    #[arg(long, value_name = "TOKEN")]
    pub bearer_token: Option<String>,

    /// Basic auth credentials ("user:password")
    #[arg(long, value_name = "USER:PASSWORD")]
    pub basic: Option<String>,

    /// Allow Authorization/Cookie headers to follow cross-origin redirects
    #[arg(long)]
    pub allow_cross_origin_auth: bool,
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// One or more links to download
    #[arg(value_name = "LINK", required_unless_present = "input_file")]
    pub links: Vec<String>,

    /// Read links from a file (one per line, `#` comments ignored)
    #[arg(long, value_name = "PATH")]
    pub input_file: Option<PathBuf>,

    /// Download directory (defaults to the configured download.dir)
    #[arg(long, short = 'd', value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Output file or directory (single-link only)
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Connections per file
    #[arg(long, short = 'n', value_name = "N")]
    pub connections: Option<usize>,

    /// Additional bandwidth cap for this task (for example 10MiB, 500K)
    #[arg(long, value_name = "RATE")]
    pub limit: Option<String>,

    /// Enable resume (default)
    #[arg(long, conflicts_with = "no_resume")]
    pub resume: bool,

    /// Disable resume for this task
    #[arg(long)]
    pub no_resume: bool,

    /// Require checksum verification (fails if no checksum is known)
    #[arg(long, conflicts_with = "no_verify")]
    pub verify: bool,

    /// Disable checksum verification
    #[arg(long)]
    pub no_verify: bool,

    /// Expected checksum (`sha256:<hex>`, `sha512:<hex>`, `sha1:<hex>`, `md5:<hex>`)
    #[arg(long, conflicts_with = "checksum_file", value_name = "ALG:HEX")]
    pub checksum: Option<String>,

    /// Read an expected checksum from a file (first non-comment line)
    #[arg(long, value_name = "PATH")]
    pub checksum_file: Option<PathBuf>,

    /// Extract archives after a successful download
    #[arg(long, conflicts_with = "no_extract")]
    pub extract: bool,

    /// Disable archive extraction for this task
    #[arg(long)]
    pub no_extract: bool,

    /// Extraction root (defaults to the task destination)
    #[arg(long, value_name = "DIR")]
    pub extract_dir: Option<PathBuf>,

    /// Delete the archive after successful extraction
    #[arg(long)]
    pub delete_archive: bool,

    /// Populate the provider cache for Hub tasks
    #[arg(long, conflicts_with = "no_cache")]
    pub cache: bool,

    /// Disable the provider cache for Hub tasks
    #[arg(long)]
    pub no_cache: bool,

    /// Overwrite existing destination files
    #[arg(long, conflicts_with = "rename")]
    pub overwrite: bool,

    /// Rename the destination on conflict (file-1.ext, file-2.ext, ...)
    #[arg(long)]
    pub rename: bool,

    /// Bypass active-task de-duplication (and overwrite files)
    #[arg(long)]
    pub force: bool,

    /// Revision for hub links
    #[arg(long)]
    pub revision: Option<String>,

    /// Include glob for Hub or BitTorrent file selection (repeatable)
    #[arg(long)]
    pub include: Vec<String>,

    /// Exclude glob for Hub or BitTorrent file selection (repeatable)
    #[arg(long)]
    pub exclude: Vec<String>,

    #[command(flatten)]
    pub request: RequestArgs,
}

#[derive(Args, Debug)]
pub struct InfoArgs {
    /// Link to resolve
    #[arg(value_name = "LINK")]
    pub link: String,

    /// Expected checksum
    #[arg(long, value_name = "ALG:HEX")]
    pub checksum: Option<String>,

    /// Revision for hub links
    #[arg(long)]
    pub revision: Option<String>,

    /// Include glob for hub links (repeatable)
    #[arg(long)]
    pub include: Vec<String>,

    /// Exclude glob for hub links (repeatable)
    #[arg(long)]
    pub exclude: Vec<String>,

    #[command(flatten)]
    pub request: RequestArgs,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Number of tasks processed concurrently
    #[arg(long, short = 'j', default_value_t = 1, value_name = "N")]
    pub jobs: usize,

    /// Global bandwidth limit for the worker (for example 10MiB, 500K)
    #[arg(long, value_name = "RATE")]
    pub limit: Option<String>,

    /// Exit when the queue is empty instead of waiting for new tasks
    #[arg(long)]
    pub once: bool,
}

#[derive(Args, Debug)]
pub struct IdArgs {
    /// Task id (full UUID or unique prefix)
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCmd,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the resolved config file path
    Path,
    /// Print the effective configuration as TOML
    Show,
    /// Print a single effective config value
    Get { key: String },
    /// Set a value in the config file
    Set { key: String, value: String },
    /// Write a default config file
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Open the config file in $VISUAL / $EDITOR
    Edit,
}

impl RequestArgs {
    pub fn to_settings(&self) -> Result<RequestOptions> {
        let mut headers = parse_headers(&self.header)?;
        if let Some(path) = &self.header_file {
            headers.extend(read_header_file(path)?);
        }
        let auth = match (&self.bearer_token, &self.basic) {
            (Some(_), Some(_)) => {
                return Err(Error::Config(
                    "use either --bearer-token or --basic, not both".to_string(),
                ));
            }
            (Some(token), None) => Some(Auth::bearer(token.clone())),
            (None, Some(value)) => Some(parse_basic_auth(value)?),
            (None, None) => None,
        };
        Ok(RequestOptions {
            headers,
            auth,
            allow_cross_origin_auth: self.allow_cross_origin_auth,
        })
    }
}

impl DownloadArgs {
    pub fn to_task_options(&self) -> Result<TaskOptions> {
        let resume = if self.no_resume {
            Some(false)
        } else if self.resume {
            Some(true)
        } else {
            None
        };
        let verify = if self.no_verify {
            Some(VerifyMode::Disabled)
        } else if self.verify {
            Some(VerifyMode::Require)
        } else {
            None
        };
        let extract = if self.no_extract {
            Some(false)
        } else if self.extract {
            Some(true)
        } else {
            None
        };
        let checksum = parse_checksum(&self.checksum, &self.checksum_file)?;
        let cache = if self.no_cache {
            Some(false)
        } else if self.cache {
            Some(true)
        } else {
            None
        };
        let rate_limit = self
            .limit
            .as_deref()
            .map(shaul::config::parse_bytes)
            .transpose()?;
        Ok(TaskOptions {
            output: self.output.clone(),
            connections: self.connections,
            rate_limit,
            resume,
            verify,
            checksum,
            extract,
            extract_dir: self.extract_dir.clone(),
            delete_archive: self.delete_archive.then_some(true),
            cache,
            request: self.request.to_settings()?,
            revision: self.revision.clone(),
            include: self.include.clone(),
            exclude: self.exclude.clone(),
            overwrite: self.overwrite || self.force,
            rename_on_conflict: self.rename,
            force: self.force,
        })
    }
}

impl InfoArgs {
    pub fn to_parse_options(&self) -> Result<ParseOptions> {
        Ok(ParseOptions {
            request: self.request.to_settings()?,
            checksum: parse_checksum(&self.checksum, &None)?,
            revision: self.revision.clone(),
            include: self.include.clone(),
            exclude: self.exclude.clone(),
        })
    }
}

#[allow(dead_code)]
fn _assert_header(_: Header, _: Checksum) {}
