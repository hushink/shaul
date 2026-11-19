use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use percent_encoding::percent_decode_str;
use shaul_core::{
    Auth, BackendCapabilities, ByteRange, ContentCache, DownloadBackend, DownloadOutcome,
    DownloadRequest, Error, EventKind, EventSink, FileOutcome, FileResume, Resource, ResourceFile,
    ResourceKind, Result, TaskResume, Validator, VerifyMode, metrics, part_path,
};
use suppaftp::FtpError;
use suppaftp::tokio::AsyncFtpStream;
use suppaftp::types::FileType;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio_util::sync::CancellationToken;

use crate::parser::FTP_PROTOCOL;

const READ_BUFFER_SIZE: usize = 64 * 1024;

/// Single-stream, resumable FTP backend.
pub struct FtpBackend;

impl FtpBackend {
    /// Creates an FTP backend.
    pub fn new() -> Self {
        Self
    }
}

impl Default for FtpBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DownloadBackend for FtpBackend {
    fn name(&self) -> &'static str {
        "ftp"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(false, true, false)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(
            &resource.kind,
            ResourceKind::Custom { protocol } if protocol == FTP_PROTOCOL
        )
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        if request.resource.files.len() != 1 {
            return Err(Error::Backend(
                "FTP resources must contain exactly one file".to_string(),
            ));
        }
        let file = request
            .resource
            .files
            .first()
            .ok_or_else(|| Error::Backend("FTP resource has no file".to_string()))?
            .clone();
        let url = file.url.clone();
        if !url.scheme().eq_ignore_ascii_case("ftp") {
            return Err(Error::Backend(
                "FTP backend received a non-ftp URL".to_string(),
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| Error::Backend("FTP URL has no host".to_string()))?
            .to_string();
        let port = url.port().unwrap_or(21);
        let remote_path = percent_decode_str(url.path())
            .decode_utf8()
            .map_err(|_| Error::Backend("FTP path is not valid UTF-8".to_string()))?
            .into_owned();
        if remote_path.is_empty() || remote_path.ends_with('/') {
            return Err(Error::Backend(
                "FTP backend needs a file path, not a directory".to_string(),
            ));
        }

        let checksum = request
            .options
            .checksum
            .clone()
            .or_else(|| file.checksum.clone());
        if request.options.verify == VerifyMode::Require && checksum.is_none() {
            return Err(Error::ChecksumMissing);
        }

        let (final_path, relative) = resolve_final_path(&request, &file)?;
        let final_path = apply_conflict_policy(final_path, &request.options).await?;
        let part = part_path(&final_path);
        let key = url.as_str().to_string();
        let mut task_resume = request.resume.clone().unwrap_or_default();
        let stored = task_resume.files.get(&key).cloned();

        let (mut stream, size, last_modified) =
            connect_and_probe(&request, &host, port, &remote_path).await?;
        let total = size;
        if request.options.disk_space_check
            && let Some(total) = total
        {
            shaul_core::ensure_free_space(&request.destination, total)?;
        }
        if request.options.content_cache
            && let (Some(checksum), Some(size)) = (checksum.as_ref(), total)
            && let Ok(cache) = ContentCache::new(&request.options.content_cache_dir)
        {
            match cache.lookup_verified(checksum, size).await {
                Ok(Some(cached)) => match cache.materialize(&cached, &final_path, size).await {
                    Ok(()) => {
                        let relative = relative_path(&request.destination, &final_path);
                        request
                            .events
                            .telemetry()
                            .counter(metrics::CACHE_HITS, 1, &[]);
                        request
                            .events
                            .telemetry()
                            .counter(metrics::CACHE_BYTES, size, &[]);
                        request.events.emit(EventKind::FileStarted {
                            task_id: request.task_id,
                            file: relative.clone(),
                            total_bytes: Some(size),
                        });
                        request.events.emit(EventKind::Progress {
                            task_id: request.task_id,
                            file: relative.clone(),
                            downloaded_bytes: size,
                            total_bytes: Some(size),
                        });
                        return Ok(DownloadOutcome::Completed(vec![FileOutcome::new(
                            final_path,
                            relative,
                            size,
                            Some(checksum.clone()),
                            true,
                        )]));
                    }
                    Err(error) => warning(
                        &request,
                        format!("could not materialize cached file: {error}"),
                    ),
                },
                Ok(None) => request
                    .events
                    .telemetry()
                    .counter(metrics::CACHE_MISSES, 1, &[]),
                Err(error) => warning(&request, format!("could not read content cache: {error}")),
            }
        }
        let validator = Validator {
            etag: None,
            last_modified,
            size: total,
        };
        let resume_valid = request.options.resume
            && stored
                .as_ref()
                .map(|stored| !stored.validator.is_empty() && stored.validator.matches(&validator))
                .unwrap_or(false);

        ensure_parent(&final_path).await?;
        let existing_len = tokio::fs::metadata(&part)
            .await
            .ok()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let mut offset = if resume_valid {
            let completed = stored
                .as_ref()
                .map(FileResume::completed_bytes)
                .unwrap_or_default();
            completed.min(existing_len).min(total.unwrap_or(u64::MAX))
        } else {
            0
        };
        if let Some(total) = total
            && offset > total
        {
            offset = total;
        }

        if offset == 0 {
            remove_if_exists(&part).await?;
            tokio::fs::write(&part, []).await?;
        } else {
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&part)
                .await?;
            file.set_len(offset).await?;
        }
        let mut part_file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&part)
            .await?;
        part_file.seek(SeekFrom::Start(offset)).await?;

        let mut file_resume = FileResume {
            url: key.clone(),
            final_path: manifest_path(&request.destination, &final_path),
            part_path: manifest_path(&request.destination, &part),
            validator: validator.clone(),
            total_size: total,
            segment_size: request.options.segment_size,
            completed: if offset > 0 {
                vec![ByteRange::new(0, offset)?]
            } else {
                Vec::new()
            },
            checksum: checksum.clone(),
            metadata: Default::default(),
            updated_at_ms: 0,
        };
        file_resume.normalize();
        task_resume.files.insert(key.clone(), file_resume.clone());
        request.checkpoint.save(&task_resume).await?;

        let progress = Arc::new(AtomicU64::new(offset));
        request.events.emit(EventKind::FileStarted {
            task_id: request.task_id,
            file: relative.clone(),
            total_bytes: total,
        });
        let progress_stop = CancellationToken::new();
        let _progress_guard = ProgressGuard(progress_stop.clone());
        let reporter = spawn_progress_reporter(
            request.events.clone(),
            request.task_id,
            relative.clone(),
            total,
            progress.clone(),
            progress_stop.clone(),
        );

        let resume_offset = usize::try_from(offset)
            .map_err(|_| Error::Backend("FTP resume offset is too large".to_string()))?;
        stream
            .resume_transfer(resume_offset)
            .await
            .map_err(ftp_error)?;
        let mut resume_rejected = false;
        let mut transfer = loop {
            let result =
                tokio::time::timeout(request.options.timeout, stream.retr_as_stream(&remote_path))
                    .await;
            match result {
                Ok(Ok(transfer)) => break transfer,
                Ok(Err(error)) if offset > 0 && !resume_rejected => {
                    tracing::warn!(
                        %error,
                        "FTP server rejected the resume offset; restarting from zero"
                    );
                    warning(
                        &request,
                        format!(
                            "FTP server rejected resume at offset {offset}; restarting from zero"
                        ),
                    );
                    resume_rejected = true;
                    offset = 0;
                    part_file.set_len(0).await?;
                    part_file.seek(SeekFrom::Start(0)).await?;
                    progress.store(0, Ordering::Relaxed);
                    file_resume.completed.clear();
                    file_resume.normalize();
                    task_resume.files.insert(key.clone(), file_resume.clone());
                    request.checkpoint.save(&task_resume).await?;
                    match connect_and_probe(&request, &host, port, &remote_path).await {
                        Ok((new_stream, _, _)) => stream = new_stream,
                        Err(error) => {
                            progress_stop.cancel();
                            let _ = reporter.await;
                            return Err(error);
                        }
                    }
                }
                Ok(Err(error)) => {
                    progress_stop.cancel();
                    let _ = reporter.await;
                    return Err(ftp_error(error));
                }
                Err(_) => {
                    progress_stop.cancel();
                    let _ = reporter.await;
                    return Err(Error::Backend("FTP RETR timed out".to_string()));
                }
            }
        };

        let mut buffer = vec![0u8; READ_BUFFER_SIZE];
        let mut since_checkpoint = 0u64;
        loop {
            let read = tokio::select! {
                _ = request.cancel.cancelled() => {
                    checkpoint(&request, &mut task_resume, &mut file_resume, offset).await?;
                    part_file.sync_all().await?;
                    progress_stop.cancel();
                    let _ = reporter.await;
                    return Err(Error::Cancelled);
                }
                result = tokio::time::timeout(
                    request.options.timeout,
                    transfer.read(&mut buffer),
                ) => match result {
                    Ok(result) => result.map_err(|error| Error::Backend(format!("FTP read: {error}")))?,
                    Err(_) => {
                        checkpoint(&request, &mut task_resume, &mut file_resume, offset).await?;
                        part_file.sync_all().await?;
                        progress_stop.cancel();
                        let _ = reporter.await;
                        return Err(Error::Backend("FTP read timed out".to_string()));
                    }
                },
            };
            if read == 0 {
                break;
            }
            part_file.write_all(&buffer[..read]).await?;
            offset += read as u64;
            progress.fetch_add(read as u64, Ordering::Relaxed);
            since_checkpoint += read as u64;
            if since_checkpoint >= request.options.segment_size {
                checkpoint(&request, &mut task_resume, &mut file_resume, offset).await?;
                since_checkpoint = 0;
            }
        }
        part_file.flush().await?;
        part_file.sync_all().await?;
        match tokio::time::timeout(request.options.timeout, transfer.finish()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                progress_stop.cancel();
                let _ = reporter.await;
                return Err(ftp_error(error));
            }
            Err(_) => {
                progress_stop.cancel();
                let _ = reporter.await;
                return Err(Error::Backend(
                    "FTP transfer completion timed out".to_string(),
                ));
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), stream.quit()).await;
        progress_stop.cancel();
        let _ = reporter.await;

        if let Some(total) = total
            && offset != total
        {
            return Err(Error::Backend(format!(
                "FTP transfer ended at {offset} of {total} bytes"
            )));
        }

        ensure_parent(&final_path).await?;
        remove_if_exists(&final_path).await?;
        tokio::fs::rename(&part, &final_path).await?;
        let actual_size = tokio::fs::metadata(&final_path).await?.len();

        file_resume.completed = if actual_size > 0 {
            vec![ByteRange::new(0, actual_size)?]
        } else {
            Vec::new()
        };
        file_resume.total_size = Some(actual_size);
        file_resume.normalize();
        task_resume.files.insert(key, file_resume);
        request.checkpoint.save(&task_resume).await?;

        request.events.emit(EventKind::Progress {
            task_id: request.task_id,
            file: relative.clone(),
            downloaded_bytes: actual_size,
            total_bytes: Some(actual_size),
        });
        Ok(DownloadOutcome::Completed(vec![FileOutcome::new(
            final_path,
            relative,
            actual_size,
            checksum,
            false,
        )]))
    }
}

async fn connect_and_probe(
    request: &DownloadRequest,
    host: &str,
    port: u16,
    remote_path: &str,
) -> Result<(AsyncFtpStream, Option<u64>, Option<String>)> {
    let (username, password) = ftp_auth(request)?;
    let connect = AsyncFtpStream::connect((host, port));
    let mut stream = tokio::time::timeout(request.options.connect_timeout, connect)
        .await
        .map_err(|_| Error::Backend("FTP connection timed out".to_string()))?
        .map_err(ftp_error)?;
    tokio::time::timeout(request.options.timeout, async {
        stream.login(username, password).await?;
        stream.transfer_type(FileType::Binary).await?;
        Ok::<(), FtpError>(())
    })
    .await
    .map_err(|_| Error::Backend("FTP login timed out".to_string()))?
    .map_err(ftp_error)?;

    let size = tokio::time::timeout(request.options.timeout, stream.size(remote_path))
        .await
        .ok()
        .and_then(|result| result.ok())
        .map(|size| size as u64);
    let last_modified = tokio::time::timeout(request.options.timeout, stream.mdtm(remote_path))
        .await
        .ok()
        .and_then(|result| result.ok())
        .map(|time| time.to_string());
    Ok((stream, size, last_modified))
}

fn ftp_auth(request: &DownloadRequest) -> Result<(String, String)> {
    let auth = request
        .resource
        .request
        .auth
        .as_ref()
        .or(request.options.request.auth.as_ref());
    match auth {
        None => Ok(("anonymous".to_string(), "anonymous@".to_string())),
        Some(Auth::Basic { username, password }) => Ok((username.clone(), password.clone())),
        Some(Auth::Bearer { .. }) => Err(Error::Config(
            "FTP does not support bearer authentication; use --basic".to_string(),
        )),
        Some(Auth::Header(_)) => Err(Error::Config(
            "FTP does not support header authentication; use --basic".to_string(),
        )),
    }
}

async fn checkpoint(
    request: &DownloadRequest,
    task_resume: &mut TaskResume,
    file_resume: &mut FileResume,
    offset: u64,
) -> Result<()> {
    file_resume.completed = if offset > 0 {
        vec![ByteRange::new(0, offset)?]
    } else {
        Vec::new()
    };
    file_resume.normalize();
    task_resume
        .files
        .insert(file_resume.url.clone(), file_resume.clone());
    request.checkpoint.save(task_resume).await
}

fn resolve_final_path(request: &DownloadRequest, file: &ResourceFile) -> Result<(PathBuf, String)> {
    if let Some(output) = &request.options.output {
        if output.is_dir() {
            let path = output.join(&file.path);
            return Ok((path.clone(), relative_path(&request.destination, &path)));
        }
        return Ok((output.clone(), relative_path(&request.destination, output)));
    }
    let path = request.destination.join(&file.path);
    Ok((path.clone(), relative_path(&request.destination, &path)))
}

async fn apply_conflict_policy(
    path: PathBuf,
    options: &shaul_core::ResolvedOptions,
) -> Result<PathBuf> {
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
    match path.strip_prefix(destination) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string()),
    }
}

fn manifest_path(destination: &Path, path: &Path) -> String {
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

struct ProgressGuard(CancellationToken);

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn spawn_progress_reporter(
    events: EventSink,
    task_id: shaul_core::TaskId,
    file: String,
    total: Option<u64>,
    downloaded: Arc<AtomicU64>,
    stop: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    events.emit(EventKind::Progress {
                        task_id,
                        file: file.clone(),
                        downloaded_bytes: downloaded.load(Ordering::Relaxed),
                        total_bytes: total,
                    });
                }
                _ = stop.cancelled() => {
                    events.emit(EventKind::Progress {
                        task_id,
                        file,
                        downloaded_bytes: downloaded.load(Ordering::Relaxed),
                        total_bytes: total,
                    });
                    break;
                }
            }
        }
    })
}

fn warning(request: &DownloadRequest, message: impl Into<String>) {
    request.events.emit(EventKind::Warning {
        task_id: Some(request.task_id),
        message: message.into(),
    });
}

fn ftp_error(error: FtpError) -> Error {
    Error::Backend(format!("FTP: {error}"))
}
