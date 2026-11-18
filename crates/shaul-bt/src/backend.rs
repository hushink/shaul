use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use librqbit::file_info::FileInfo;
use librqbit::{
    AddTorrent, AddTorrentOptions, DhtSessionConfig, ListenerOptions, Session, SessionOptions,
    SessionPersistenceConfig, TorrentStatsState,
};
use shaul_core::{
    BackendCapabilities, BitTorrentSource, Config, DownloadBackend, DownloadOutcome,
    DownloadRequest, Error, EventKind, FileOutcome, ResourceKind, Result,
};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct BtBackend {
    session: Arc<OnceCell<Arc<Session>>>,
    output_root: PathBuf,
    state_dir: PathBuf,
    options: BtOptions,
}

#[derive(Clone)]
struct BtOptions {
    dht: bool,
    trackers: bool,
    listen: bool,
    persistence: bool,
    peer_limit: Option<usize>,
    initial_peers: Vec<SocketAddr>,
    client_name: String,
}

impl BtBackend {
    pub fn new(config: &Config) -> Result<Self> {
        let state_dir = config.state_dir()?.join("bt");
        std::fs::create_dir_all(&state_dir)?;
        let output_root = absolute_path(&config.download.dir);
        std::fs::create_dir_all(&output_root)?;
        let initial_peers = config
            .bt
            .initial_peers
            .iter()
            .map(|value| {
                value.parse::<SocketAddr>().map_err(|error| {
                    Error::Config(format!("invalid BT initial peer {value}: {error}"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            session: Arc::new(OnceCell::new()),
            output_root,
            state_dir,
            options: BtOptions {
                dht: config.bt.dht,
                trackers: config.bt.trackers,
                listen: config.bt.listen,
                persistence: config.bt.persistence,
                peer_limit: config.bt.peer_limit,
                initial_peers,
                client_name: format!("shaul/{}", env!("CARGO_PKG_VERSION")),
            },
        })
    }

    async fn session(&self) -> Result<Arc<Session>> {
        let session =
            self.session
                .get_or_try_init(|| async {
                    let options = SessionOptions {
                        dht: self.options.dht.then(DhtSessionConfig::default),
                        disable_trackers: !self.options.trackers,
                        fastresume: self.options.persistence,
                        persistence: self.options.persistence.then(|| {
                            SessionPersistenceConfig::Json {
                                folder: Some(self.state_dir.clone()),
                            }
                        }),
                        listen: self.options.listen.then(ListenerOptions::default),
                        peer_limit: self.options.peer_limit,
                        client_name_and_version: Some(self.options.client_name.clone()),
                        ..SessionOptions::default()
                    };
                    Session::new_with_opts(self.output_root.clone(), options)
                        .await
                        .map_err(|error| {
                            Error::Backend(format!("failed to start BitTorrent session: {error:#}"))
                        })
                })
                .await?;
        Ok(session.clone())
    }

    async fn download_inner(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let source = match &request.resource.kind {
            ResourceKind::BitTorrent { source, .. } => *source,
            _ => {
                return Err(Error::NoBackend(
                    "BtBackend received a non-BitTorrent resource".to_string(),
                ));
            }
        };
        let add = match source {
            BitTorrentSource::Magnet => {
                let magnet =
                    request.resource.metadata.get("bt.magnet").ok_or_else(|| {
                        Error::InvalidLink("magnet resource is missing".to_string())
                    })?;
                AddTorrent::from_url(magnet.clone())
            }
            BitTorrentSource::TorrentFile => {
                let path = request
                    .resource
                    .metadata
                    .get("bt.torrent_path")
                    .ok_or_else(|| Error::InvalidLink("torrent path is missing".to_string()))?;
                let bytes = tokio::fs::read(path).await.map_err(|error| {
                    Error::Backend(format!("cannot read torrent file {path}: {error}"))
                })?;
                AddTorrent::from_bytes(bytes)
            }
        };

        let session = self.session().await?;
        let destination = absolute_path(&request.destination);
        tokio::fs::create_dir_all(&destination).await?;
        let add_options = AddTorrentOptions {
            overwrite: true,
            output_folder: Some(destination.to_string_lossy().to_string()),
            initial_peers: (!self.options.initial_peers.is_empty())
                .then(|| self.options.initial_peers.clone()),
            disable_trackers: !self.options.trackers,
            peer_limit: self.options.peer_limit,
            ..AddTorrentOptions::default()
        };
        let response = session
            .add_torrent(add, Some(add_options))
            .await
            .map_err(|error| Error::Backend(format!("failed to add torrent: {error:#}")))?;
        let handle = response
            .into_handle()
            .ok_or_else(|| Error::Backend("torrent was listed but not started".to_string()))?;

        // A previously paused/resumed task may come back as an AlreadyManaged
        // handle in the Paused state. Completed torrents stay paused so they
        // never start seeding.
        let initial_stats = handle.stats();
        if matches!(initial_stats.state, TorrentStatsState::Paused) && !initial_stats.finished {
            session
                .unpause(&handle)
                .await
                .map_err(|error| Error::Backend(format!("failed to resume torrent: {error:#}")))?;
        }

        let include = build_globset(&request.options.include)?;
        let exclude = build_globset(&request.options.exclude)?;
        let mut file_infos: Vec<FileInfo> = Vec::new();
        let mut selected: Option<HashSet<usize>> = None;
        let mut selection_applied = false;
        let mut announced = HashSet::new();
        let mut last_progress: HashMap<usize, u64> = HashMap::new();

        loop {
            if request.cancel.is_cancelled() {
                let _ = session.pause(&handle).await;
                return Err(Error::Cancelled);
            }
            let stats = handle.stats();
            if let Some(error) = stats.error.clone() {
                let _ = session.pause(&handle).await;
                return Err(Error::Backend(error));
            }
            if matches!(stats.state, TorrentStatsState::Error) {
                let _ = session.pause(&handle).await;
                return Err(Error::Backend(
                    "BitTorrent task entered an error state".to_string(),
                ));
            }

            if let Ok(infos) = handle.with_metadata(|metadata| metadata.file_infos.clone()) {
                if file_infos.is_empty() {
                    file_infos = infos;
                    let indices = select_indices(&file_infos, include.as_ref(), exclude.as_ref());
                    if indices.is_empty() {
                        let _ = session.pause(&handle).await;
                        return Err(Error::Backend(
                            "no torrent files matched the include/exclude filters".to_string(),
                        ));
                    }
                    selection_applied = indices.len() == file_infos.len();
                    selected = Some(indices);
                }

                if let Some(indices) = &selected {
                    // librqbit only accepts selection updates once the torrent
                    // is live or paused; metadata can appear while initializing.
                    if !selection_applied
                        && matches!(
                            stats.state,
                            TorrentStatsState::Live | TorrentStatsState::Paused
                        )
                    {
                        session
                            .update_only_files(&handle, indices)
                            .await
                            .map_err(|error| {
                                Error::Backend(format!(
                                    "failed to apply torrent file selection: {error:#}"
                                ))
                            })?;
                        selection_applied = true;
                    }
                    for &index in indices {
                        let info = &file_infos[index];
                        let relative = outcome_relative(
                            &destination,
                            handle.output_folder(),
                            &info.relative_filename,
                        );
                        if announced.insert(index) {
                            request.events.emit(EventKind::FileStarted {
                                task_id: request.task_id,
                                file: relative.clone(),
                                total_bytes: Some(info.len),
                            });
                        }
                        let downloaded = stats.file_progress.get(index).copied().unwrap_or(0);
                        if last_progress.get(&index).copied() != Some(downloaded) {
                            last_progress.insert(index, downloaded);
                            request.events.emit(EventKind::Progress {
                                task_id: request.task_id,
                                file: relative,
                                downloaded_bytes: downloaded,
                                total_bytes: Some(info.len),
                            });
                        }
                    }
                }
            }

            if stats.finished {
                break;
            }

            tokio::select! {
                _ = request.cancel.cancelled() => {
                    let _ = session.pause(&handle).await;
                    return Err(Error::Cancelled);
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        // Leech-only: pause immediately after completion so the process does
        // not keep seeding. BitTorrent piece hashes already verified the data.
        let _ = session.pause(&handle).await;

        if file_infos.is_empty() {
            return Err(Error::Backend(
                "BitTorrent task finished without file metadata".to_string(),
            ));
        }
        let indices = selected.unwrap_or_else(|| (0..file_infos.len()).collect());
        let mut outcomes = Vec::with_capacity(indices.len());
        for index in indices {
            let info = &file_infos[index];
            let path = handle.output_folder().join(&info.relative_filename);
            let relative_path = outcome_relative(
                &destination,
                handle.output_folder(),
                &info.relative_filename,
            );
            outcomes.push(FileOutcome {
                path,
                relative_path,
                size: info.len,
                checksum: None,
                checksum_verified: true,
            });
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}

#[async_trait]
impl DownloadBackend for BtBackend {
    fn name(&self) -> &'static str {
        "bittorrent"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            segmented: false,
            resume: true,
            multi_file: true,
        }
    }

    fn supports(&self, resource: &shaul_core::Resource) -> bool {
        matches!(&resource.kind, ResourceKind::BitTorrent { .. })
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        self.download_inner(request).await
    }
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|error| Error::Config(format!("invalid glob {pattern}: {error}")))?,
        );
    }
    builder
        .build()
        .map(Some)
        .map_err(|error| Error::Config(format!("invalid glob set: {error}")))
}

fn select_indices(
    files: &[FileInfo],
    include: Option<&GlobSet>,
    exclude: Option<&GlobSet>,
) -> HashSet<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, file)| {
            let path = file.relative_filename.to_string_lossy().replace('\\', "/");
            include.map(|set| set.is_match(&path)).unwrap_or(true)
                && !exclude.map(|set| set.is_match(&path)).unwrap_or(false)
        })
        .map(|(index, _)| index)
        .collect()
}

fn outcome_relative(destination: &Path, output_folder: &Path, relative: &Path) -> String {
    let full = output_folder.join(relative);
    full.strip_prefix(destination)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| relative.to_string_lossy().replace('\\', "/"))
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}
