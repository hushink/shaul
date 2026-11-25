use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use shaul_core::{
    BackendCapabilities, ByteRange, DownloadBackend, DownloadOutcome, DownloadRequest, Error,
    EventKind, EventSink, FileOutcome, FileResume, RequestOptions, ResolvedOptions, Resource,
    ResourceFile, ResourceKind, Result, ResumeCheckpoint, TaskResume, VerifyMode,
    ensure_free_space, part_path,
};
use shaul_http::HttpBackend;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use url::Url;

use crate::client::{HlsClient, same_origin};
use crate::parser::{HLS_PROTOCOL, validate_media};
use crate::playlist::{Playlist, fingerprint};

const STATE_KEY: &str = "hls.state";
const TEMP_DIR_KEY: &str = "hls.temp_dir";
const ORIGIN_KEY: &str = "hls.origin";
const SEGMENT_DIR_SUFFIX: &str = ".shaul.hls.parts";
const INIT_NAME: &str = "init.bin";

/// HLS backend. Segments are downloaded concurrently through a shared
/// [`HttpBackend`] and concatenated into one output file.
#[derive(Clone)]
pub struct HlsBackend {
    client: Arc<HlsClient>,
    http: HttpBackend,
}

impl HlsBackend {
    /// Creates an HLS backend that uses `client` for playlists and `http` for
    /// segment transfers.
    pub fn new(client: Arc<HlsClient>, http: HttpBackend) -> Self {
        Self { client, http }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ResumeState {
    fingerprint: String,
    segment_count: usize,
}

struct CheckpointWriter {
    task_resume: TaskResume,
    file_resume: FileResume,
    key: String,
}

impl CheckpointWriter {
    async fn save_progress(&mut self, request: &DownloadRequest, progress: u64) -> Result<()> {
        // HLS resume is driven by the segment files; `completed` is kept in
        // sync only so `status`/`list` show a useful byte count.
        self.file_resume.completed = if progress > 0 {
            vec![ByteRange::new(0, progress)?]
        } else {
            Vec::new()
        };
        self.file_resume.normalize();
        self.task_resume
            .files
            .insert(self.key.clone(), self.file_resume.clone());
        request.checkpoint.save(&self.task_resume).await
    }
}

#[async_trait]
impl DownloadBackend for HlsBackend {
    fn name(&self) -> &'static str {
        HLS_PROTOCOL
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, true, false)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(
            &resource.kind,
            ResourceKind::Custom { protocol } if protocol == HLS_PROTOCOL
        )
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        if request.resource.files.len() != 1 {
            return Err(Error::Backend(
                "HLS resources must contain exactly one file".to_string(),
            ));
        }
        let file = request.resource.files[0].clone();
        if !request.options.mirrors.is_empty() {
            request.events.emit(EventKind::Warning {
                task_id: Some(request.task_id),
                message: "mirror fallback is not supported for HLS tasks".to_string(),
            });
        }
        let key = file.url.as_str().to_string();
        let checksum = request
            .options
            .checksum
            .clone()
            .or_else(|| file.checksum.clone());
        if request.options.verify == VerifyMode::Require && checksum.is_none() {
            return Err(Error::ChecksumMissing);
        }

        let trust_origin = request
            .resource
            .metadata
            .get(ORIGIN_KEY)
            .and_then(|value| Url::parse(value).ok())
            .unwrap_or_else(|| origin_url(&file.url));
        let playlist_request =
            request_for_origin(&request.options.request, &trust_origin, &file.url);
        let (text, media_url) = self
            .client
            .fetch_text(&file.url, &playlist_request, &request.cancel)
            .await?;
        let playlist = Playlist::parse(&media_url, &text)?;
        let Playlist::Media(media) = playlist else {
            return Err(Error::Backend(
                "HLS resource does not point to a media playlist".to_string(),
            ));
        };
        validate_media(&media)?;

        let extension = if media.init.is_some() { "mp4" } else { "ts" };
        let (final_path, _) = resolve_final_path(&request, &file, extension)?;
        let final_path = apply_conflict_policy(final_path, &request.options).await?;
        let relative = relative_path(&request.destination, &final_path);
        let part = part_path(&final_path);
        let temp_relative = format!("{relative}{SEGMENT_DIR_SUFFIX}");
        let temp_dir = request.destination.join(&temp_relative);

        let segment_count = media.segments.len();
        let current_state = ResumeState {
            fingerprint: fingerprint(&text),
            segment_count,
        };
        let stored = request
            .resume
            .as_ref()
            .and_then(|resume| resume.files.get(&key).cloned());
        let expected_final = path_for_manifest(&request.destination, &final_path);
        let can_resume = stored
            .as_ref()
            .is_some_and(|stored| stored.final_path == expected_final)
            && stored_state(stored.as_ref()).is_some_and(|state| {
                state.fingerprint == current_state.fingerprint
                    && state.segment_count == current_state.segment_count
            });
        if let Some(stored) = &stored {
            // The output path (and therefore the temp directory name) can
            // change if --output changed or the playlist switched between TS
            // and fMP4. Remove the old segment directory instead of leaking it.
            let old_temp = request
                .destination
                .join(format!("{}{SEGMENT_DIR_SUFFIX}", stored.final_path));
            if old_temp != temp_dir {
                remove_dir_all_if_exists(&old_temp).await?;
            }
        }
        if !can_resume {
            remove_dir_all_if_exists(&temp_dir).await?;
        }
        create_dir_all(&temp_dir).await?;

        let completed = if can_resume {
            scan_completed(&temp_dir, segment_count).await
        } else {
            Vec::new()
        };
        let init_path = media.init.as_ref().map(|_| temp_dir.join(INIT_NAME));
        let init_done = match &init_path {
            Some(path) => tokio::fs::try_exists(path).await.unwrap_or(false),
            None => true,
        };

        request.events.emit(EventKind::FileStarted {
            task_id: request.task_id,
            file: relative.clone(),
            total_bytes: None,
        });
        let progress = Arc::new(AtomicU64::new(
            resumed_bytes(&temp_dir, &completed, init_path.as_deref()).await?,
        ));
        emit_progress(&request, &relative, &progress);

        let file_resume = FileResume {
            url: key.clone(),
            final_path: expected_final,
            part_path: path_for_manifest(&request.destination, &part),
            validator: Default::default(),
            total_size: None,
            segment_size: request.options.segment_size,
            completed: Vec::new(),
            checksum: checksum.clone(),
            metadata: BTreeMap::from([
                (
                    STATE_KEY.to_string(),
                    serde_json::to_string(&current_state).map_err(|error| {
                        Error::Backend(format!("could not serialize HLS state: {error}"))
                    })?,
                ),
                (TEMP_DIR_KEY.to_string(), temp_relative),
            ]),
            updated_at_ms: 0,
        };
        let writer = Arc::new(Mutex::new(CheckpointWriter {
            task_resume: request.resume.clone().unwrap_or_default(),
            file_resume,
            key: key.clone(),
        }));
        writer
            .lock()
            .await
            .save_progress(&request, progress.load(Ordering::Relaxed))
            .await?;

        let segment_events =
            EventSink::default().with_telemetry(request.events.telemetry().clone());
        if let Some(init_url) = &media.init
            && !init_done
        {
            let options = options_for_origin(&request.options, &trust_origin, init_url);
            let outcome = download_one(
                &self.http,
                &request,
                &options,
                init_url,
                &temp_dir,
                INIT_NAME,
                &segment_events,
            )
            .await?;
            let current = progress.fetch_add(outcome.size, Ordering::Relaxed) + outcome.size;
            emit_progress(&request, &relative, &progress);
            writer.lock().await.save_progress(&request, current).await?;
        }

        let pending: Vec<usize> = (0..segment_count)
            .filter(|index| !completed.contains(index))
            .collect();
        if !pending.is_empty() {
            let concurrency = request.options.connections.max(1).min(pending.len());
            let http = &self.http;
            let request_ref = &request;
            let temp_dir_ref = temp_dir.clone();
            let segment_events = segment_events.clone();
            let progress = progress.clone();
            let relative = relative.clone();
            let writer = writer.clone();
            stream::iter(pending)
                .map(|index| {
                    let url = media.segments[index].uri.clone();
                    let options = options_for_origin(&request_ref.options, &trust_origin, &url);
                    let segment_events = segment_events.clone();
                    let progress = progress.clone();
                    let relative = relative.clone();
                    let temp_dir = temp_dir_ref.clone();
                    let writer = writer.clone();
                    async move {
                        let outcome = download_one(
                            http,
                            request_ref,
                            &options,
                            &url,
                            &temp_dir,
                            &segment_name(index),
                            &segment_events,
                        )
                        .await?;
                        let current =
                            progress.fetch_add(outcome.size, Ordering::Relaxed) + outcome.size;
                        request_ref.events.emit(EventKind::Progress {
                            task_id: request_ref.task_id,
                            file: relative,
                            downloaded_bytes: current,
                            total_bytes: None,
                        });
                        writer
                            .lock()
                            .await
                            .save_progress(request_ref, current)
                            .await?;
                        Ok::<usize, Error>(index)
                    }
                })
                .buffer_unordered(concurrency)
                .try_collect::<Vec<_>>()
                .await?;
        }

        let total = total_bytes(&temp_dir, segment_count, init_path.as_deref()).await?;
        if request.options.disk_space_check {
            ensure_free_space(&request.destination, total)?;
        }
        let written = concat(&final_path, &temp_dir, init_path.as_deref(), segment_count).await?;

        {
            let mut writer = writer.lock().await;
            writer.file_resume.total_size = Some(written);
            writer.file_resume.completed = if written > 0 {
                vec![ByteRange::new(0, written)?]
            } else {
                Vec::new()
            };
            writer.file_resume.normalize();
            let key = writer.key.clone();
            let entry = writer.file_resume.clone();
            writer.task_resume.files.insert(key, entry);
            request.checkpoint.save(&writer.task_resume).await?;
        }
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        request.events.emit(EventKind::Progress {
            task_id: request.task_id,
            file: relative.clone(),
            downloaded_bytes: written,
            total_bytes: Some(written),
        });
        Ok(DownloadOutcome::Completed(vec![FileOutcome::new(
            final_path, relative, written, checksum, false,
        )]))
    }
}

struct NoopCheckpoint;

#[async_trait]
impl ResumeCheckpoint for NoopCheckpoint {
    async fn save(&self, _resume: &TaskResume) -> Result<()> {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_one(
    http: &HttpBackend,
    request: &DownloadRequest,
    options: &ResolvedOptions,
    url: &Url,
    destination: &Path,
    file_name: &str,
    events: &EventSink,
) -> Result<FileOutcome> {
    let file = ResourceFile {
        path: file_name.to_string(),
        url: url.clone(),
        size: None,
        checksum: None,
        headers: Vec::new(),
        metadata: BTreeMap::new(),
        use_response_filename: false,
    };
    let resource = Resource {
        kind: ResourceKind::Http,
        name: file_name.to_string(),
        files: vec![file.clone()],
        request: options.request.clone(),
        checksum: None,
        revision: None,
        metadata: BTreeMap::new(),
    };
    let mut segment_options = options.clone();
    segment_options.output = None;
    segment_options.connections = 1;
    segment_options.file_jobs = 1;
    segment_options.checksum = None;
    segment_options.verify = VerifyMode::Disabled;
    segment_options.extract = false;
    segment_options.content_cache = false;
    segment_options.mirrors.clear();
    segment_options.resume = false;
    segment_options.overwrite = true;
    segment_options.rename_on_conflict = false;
    segment_options.request.method = None;
    segment_options.request.body = None;
    let mut resume = TaskResume::default();
    let segment_request = DownloadRequest {
        task_id: request.task_id,
        resource,
        destination: destination.to_path_buf(),
        options: segment_options,
        resume: None,
        cancel: request.cancel.clone(),
        events: events.clone(),
        checkpoint: Arc::new(NoopCheckpoint),
    };
    http.download_file(&segment_request, &file, &mut resume)
        .await
}

fn request_for_origin(request: &RequestOptions, trust: &Url, target: &Url) -> RequestOptions {
    let mut request = request.clone();
    if !same_origin(trust, target) && !request.allow_cross_origin_auth {
        request.headers.retain(|header| !header.is_sensitive());
        request.auth = None;
    }
    request
}

fn options_for_origin(options: &ResolvedOptions, trust: &Url, target: &Url) -> ResolvedOptions {
    let mut options = options.clone();
    options.request = request_for_origin(&options.request, trust, target);
    options
}

fn origin_url(url: &Url) -> Url {
    Url::parse(&url.origin().ascii_serialization()).unwrap_or_else(|_| url.clone())
}

fn stored_state(stored: Option<&FileResume>) -> Option<ResumeState> {
    stored
        .and_then(|stored| stored.metadata.get(STATE_KEY))
        .and_then(|value| serde_json::from_str(value).ok())
}

async fn scan_completed(temp_dir: &Path, count: usize) -> Vec<usize> {
    let mut completed = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(temp_dir).await else {
        return completed;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry
            .file_type()
            .await
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(index) = name
            .strip_prefix("segment-")
            .and_then(|name| name.strip_suffix(".bin"))
            .and_then(|name| name.parse::<usize>().ok())
        else {
            continue;
        };
        if index < count {
            completed.push(index);
        }
    }
    completed.sort_unstable();
    completed.dedup();
    completed
}

async fn resumed_bytes(temp_dir: &Path, completed: &[usize], init: Option<&Path>) -> Result<u64> {
    let mut total = 0;
    if let Some(init) = init
        && let Ok(metadata) = tokio::fs::metadata(init).await
    {
        total += metadata.len();
    }
    for index in completed {
        if let Ok(metadata) = tokio::fs::metadata(segment_path(temp_dir, *index)).await {
            total += metadata.len();
        }
    }
    Ok(total)
}

async fn total_bytes(temp_dir: &Path, count: usize, init: Option<&Path>) -> Result<u64> {
    let mut total = 0;
    if let Some(init) = init {
        total += tokio::fs::metadata(init).await?.len();
    }
    for index in 0..count {
        total += tokio::fs::metadata(segment_path(temp_dir, index))
            .await?
            .len();
    }
    Ok(total)
}

async fn concat(
    final_path: &Path,
    temp_dir: &Path,
    init: Option<&Path>,
    count: usize,
) -> Result<u64> {
    let part = part_path(final_path);
    ensure_parent(&part).await?;
    remove_if_exists(&part).await?;
    let mut output = tokio::fs::File::create(&part).await?;
    let mut written = 0u64;
    if let Some(init) = init {
        written += copy_file(init, &mut output).await?;
    }
    for index in 0..count {
        written += copy_file(&segment_path(temp_dir, index), &mut output).await?;
    }
    output.flush().await?;
    output.sync_all().await?;
    drop(output);
    remove_if_exists(final_path).await?;
    tokio::fs::rename(&part, final_path).await?;
    Ok(written)
}

async fn copy_file(path: &Path, output: &mut tokio::fs::File) -> Result<u64> {
    let mut input = tokio::fs::File::open(path).await?;
    Ok(tokio::io::copy(&mut input, output).await?)
}

fn segment_name(index: usize) -> String {
    format!("segment-{index:05}.bin")
}

fn segment_path(temp_dir: &Path, index: usize) -> PathBuf {
    temp_dir.join(segment_name(index))
}

fn emit_progress(request: &DownloadRequest, relative: &str, progress: &AtomicU64) {
    request.events.emit(EventKind::Progress {
        task_id: request.task_id,
        file: relative.to_string(),
        downloaded_bytes: progress.load(Ordering::Relaxed),
        total_bytes: None,
    });
}

fn default_output_path(file: &ResourceFile, extension: &str) -> PathBuf {
    let path = Path::new(&file.path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("stream");
    path.with_file_name(format!("{stem}.{extension}"))
}

fn resolve_final_path(
    request: &DownloadRequest,
    file: &ResourceFile,
    extension: &str,
) -> Result<(PathBuf, String)> {
    let relative = default_output_path(file, extension);
    let final_path = if let Some(output) = &request.options.output {
        if output.is_dir() {
            output.join(&relative)
        } else {
            output.clone()
        }
    } else {
        request.destination.join(&relative)
    };
    Ok((
        final_path.clone(),
        relative_path(&request.destination, &final_path),
    ))
}

async fn apply_conflict_policy(path: PathBuf, options: &ResolvedOptions) -> Result<PathBuf> {
    if !tokio::fs::try_exists(&path).await? {
        return Ok(path);
    }
    if options.overwrite {
        remove_if_exists(&path).await?;
        return Ok(path);
    }
    if options.rename_on_conflict {
        return unique_path(&path).await;
    }
    Err(Error::Backend(format!(
        "destination already exists: {} (use --force or --rename)",
        path.display()
    )))
}

async fn unique_path(path: &Path) -> Result<PathBuf> {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..10_000 {
        let candidate = path.with_file_name(format!("{stem}-{index}{extension}"));
        if !tokio::fs::try_exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    Err(Error::Backend(
        "could not find a free destination name".to_string(),
    ))
}

fn relative_path(destination: &Path, path: &Path) -> String {
    path.strip_prefix(destination)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn path_for_manifest(destination: &Path, path: &Path) -> String {
    relative_path(destination, path)
}

async fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

async fn remove_dir_all_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

async fn create_dir_all(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path).await?;
    Ok(())
}
