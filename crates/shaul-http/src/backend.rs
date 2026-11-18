use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use reqwest::header::{
    ACCEPT_ENCODING, AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, LOCATION,
};
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use shaul_core::{
    BackendCapabilities, Checksum, ChecksumAlgorithm, Config, DownloadBackend, DownloadOutcome,
    DownloadRequest, Error, EventKind, EventSink, FileOutcome, FileResume, Header, Resource,
    ResourceFile, ResourceKind, Result, TaskId, TaskResume, Validator, VerifyMode, completed_bytes,
    merge_ranges, missing_ranges, part_path, sanitize_file_name, split_range, util::now_ms,
};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::filename::parse_content_disposition;

const MAX_REDIRECTS: usize = 10;

const CAPTURE_HEADERS: &[&str] = &[
    "content-disposition",
    "etag",
    "last-modified",
    "content-length",
    "accept-ranges",
    "content-range",
    "digest",
    "content-md5",
    "x-linked-etag",
    "x-linked-size",
    "x-repo-commit",
    "x-xet-hash",
    "x-xet-token-expiration",
    "x-xet-refresh-route",
    "x-error-code",
];

const RESERVED_HEADERS: &[&str] = &[
    "range",
    "if-range",
    "host",
    "content-length",
    "accept-encoding",
    "connection",
    "transfer-encoding",
];

#[derive(Clone)]
pub struct HttpBackend {
    client: Client,
}

impl HttpBackend {
    pub fn new(config: &Config) -> Result<Self> {
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(
                config.network.connect_timeout_secs.max(1),
            ))
            .pool_max_idle_per_host(config.download.per_host.max(1))
            .user_agent(config.network.user_agent.clone())
            .build()
            .map_err(|error| Error::Backend(format!("failed to build HTTP client: {error}")))?;
        Ok(Self { client })
    }

    /// Download one resource file, sharing the caller's task resume. This is
    /// public so protocol-specific backends (Hub) can reuse the segmented,
    /// resumable HTTP transfer without duplicating it.
    pub async fn download_file(
        &self,
        request: &DownloadRequest,
        file: &ResourceFile,
        task_resume: &mut TaskResume,
    ) -> Result<FileOutcome> {
        let key = file.url.as_str().to_string();

        if let Some(stored) = task_resume.files.get(&key).cloned()
            && stored.is_complete()
        {
            let final_path = manifest_path(&request.destination, &stored.final_path);
            let part = manifest_path(&request.destination, &stored.part_path);
            if tokio::fs::try_exists(&final_path).await? {
                let size = tokio::fs::metadata(&final_path).await?.len();
                let relative = relative_path(&request.destination, &final_path);
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
                return Ok(FileOutcome {
                    path: final_path,
                    relative_path: relative,
                    size,
                    checksum: stored.checksum.or(request.options.checksum.clone()),
                    checksum_verified: false,
                });
            }
            if tokio::fs::try_exists(&part).await? {
                ensure_parent(&final_path).await?;
                remove_if_exists(&final_path).await?;
                tokio::fs::rename(&part, &final_path).await?;
                let size = tokio::fs::metadata(&final_path).await?.len();
                let relative = relative_path(&request.destination, &final_path);
                request.events.emit(EventKind::FileStarted {
                    task_id: request.task_id,
                    file: relative.clone(),
                    total_bytes: Some(size),
                });
                return Ok(FileOutcome {
                    path: final_path,
                    relative_path: relative,
                    size,
                    checksum: stored.checksum.or(request.options.checksum.clone()),
                    checksum_verified: false,
                });
            }
        }

        let probe = self.probe(request, file).await?;
        let total = probe.total_size;
        let validator = probe.validator();
        let checksum = request
            .options
            .checksum
            .clone()
            .or_else(|| probe.checksum.clone())
            .or_else(|| file.checksum.clone());
        if request.options.verify == VerifyMode::Require && checksum.is_none() {
            return Err(Error::ChecksumMissing);
        }

        let default_name = file
            .path
            .rsplit('/')
            .next()
            .map(sanitize_file_name)
            .unwrap_or_else(|| "download".to_string());
        let (mut final_path, relative) =
            resolve_final_path(request, file, probe.filename.as_deref(), &default_name)?;

        let stored = task_resume.files.get(&key).cloned();
        let final_for_manifest = path_for_manifest(&request.destination, &final_path);
        let stored_matches_path = stored
            .as_ref()
            .map(|stored| stored.final_path == final_for_manifest)
            .unwrap_or(false);
        let resume_valid = request.options.resume
            && stored
                .as_ref()
                .map(|stored| {
                    stored_matches_path
                        && !stored.validator.is_empty()
                        && stored.validator.matches(&validator)
                        && match (stored.total_size, total) {
                            (Some(old), Some(new)) => old == new,
                            _ => true,
                        }
                })
                .unwrap_or(false);

        if stored.is_some() && !resume_valid {
            request.events.emit(EventKind::Warning {
                task_id: Some(request.task_id),
                message: "resume state is no longer valid; restarting this file".to_string(),
            });
        }

        final_path = apply_conflict_policy(final_path, &request.options).await?;
        let part_name = part_path(&final_path);
        let part_for_manifest = path_for_manifest(&request.destination, &part_name);

        let mut file_resume = FileResume {
            url: key.clone(),
            final_path: path_for_manifest(&request.destination, &final_path),
            part_path: part_for_manifest.clone(),
            validator: validator.clone(),
            total_size: total,
            segment_size: request.options.segment_size,
            completed: if resume_valid {
                stored
                    .as_ref()
                    .map(|stored| stored.completed.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            checksum: checksum.clone(),
            metadata: {
                let mut metadata = probe.headers.clone();
                metadata.insert("final-url".to_string(), redacted_url(&probe.final_url));
                metadata
            },
            updated_at_ms: now_ms(),
        };

        // Reconcile existing partial data with the checkpoint.
        let existing_part_len = tokio::fs::metadata(&part_name)
            .await
            .ok()
            .map(|metadata| metadata.len());
        if !resume_valid {
            file_resume.completed.clear();
        } else if let Some(len) = existing_part_len {
            let limit = total.map(|total| total.min(len)).unwrap_or(len);
            file_resume.completed = shaul_core::range::clamp_to_len(&file_resume.completed, limit);
        } else {
            file_resume.completed.clear();
        }
        file_resume.normalize();

        ensure_parent(&final_path).await?;
        if let Some(total) = total {
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&part_name)
                .await?;
            if !resume_valid {
                file.set_len(0).await?;
            }
            file.set_len(total).await?;
            file.sync_all().await?;
        } else if !resume_valid {
            tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part_name)
                .await?;
        } else if !tokio::fs::try_exists(&part_name).await? {
            tokio::fs::File::create(&part_name).await?;
            file_resume.completed.clear();
        }

        task_resume.files.insert(key.clone(), file_resume.clone());
        request.checkpoint.save(task_resume).await?;

        let initial = completed_bytes(&file_resume.completed);
        let progress = Arc::new(AtomicU64::new(initial));
        request.events.emit(EventKind::FileStarted {
            task_id: request.task_id,
            file: relative.clone(),
            total_bytes: total,
        });

        let progress_stop = CancellationToken::new();
        let reporter = spawn_progress_reporter(
            request.events.clone(),
            request.task_id,
            relative.clone(),
            total,
            progress.clone(),
            progress_stop.clone(),
        );

        let transfer_result = if let Some(0) = total {
            // Empty file: the pre-allocated part is already the final content.
            task_resume.files.insert(key.clone(), file_resume.clone());
            request.checkpoint.save(task_resume).await?;
            Ok(file_resume.clone())
        } else if probe.range_supported
            && total.map(|total| total > 0).unwrap_or(false)
            && request.options.connections > 1
        {
            self.transfer_segmented(
                request,
                file,
                &part_name,
                &key,
                file_resume.clone(),
                task_resume.clone(),
                progress.clone(),
                total,
            )
            .await
        } else {
            self.transfer_single(
                request,
                file,
                &part_name,
                &key,
                file_resume.clone(),
                task_resume.clone(),
                progress.clone(),
                probe.range_supported,
            )
            .await
        };

        progress_stop.cancel();
        let _ = reporter.await;

        let mut final_resume = match transfer_result {
            Ok(resume) => resume,
            Err(error) => return Err(error),
        };

        if let Some(total) = final_resume.total_size
            && total > 0
        {
            final_resume.completed = vec![shaul_core::ByteRange::new(0, total)?];
        }
        final_resume.normalize();
        request.events.emit(EventKind::Progress {
            task_id: request.task_id,
            file: relative.clone(),
            downloaded_bytes: final_resume.completed_bytes(),
            total_bytes: final_resume.total_size,
        });
        task_resume.files.insert(key, final_resume.clone());
        request.checkpoint.save(task_resume).await?;

        ensure_parent(&final_path).await?;
        remove_if_exists(&final_path).await?;
        if tokio::fs::try_exists(&part_name).await? {
            tokio::fs::rename(&part_name, &final_path).await?;
        } else {
            tokio::fs::write(&final_path, []).await?;
        }
        let size = tokio::fs::metadata(&final_path).await?.len();
        Ok(FileOutcome {
            path: final_path,
            relative_path: relative,
            size,
            checksum,
            checksum_verified: false,
        })
    }

    async fn probe(&self, request: &DownloadRequest, file: &ResourceFile) -> Result<Probe> {
        let mut attempt = 0u32;
        loop {
            let headers = build_headers(&request.options, &file.headers, Some("bytes=0-0"), None)?;
            let sent = self
                .send_follow(&file.url, &headers, &request.options, &request.cancel)
                .await;
            match sent {
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    if attempt >= request.options.retries {
                        return Err(error);
                    }
                    sleep_or_cancel(backoff(attempt), &request.cancel).await?;
                    attempt += 1;
                }
                Ok((response, captured, final_url)) => {
                    let status = response.status();
                    let retry_after = retry_after(response.headers());
                    if status.is_server_error()
                        || status == StatusCode::REQUEST_TIMEOUT
                        || status == StatusCode::TOO_MANY_REQUESTS
                    {
                        drop(response);
                        if attempt >= request.options.retries {
                            return Err(http_error(status, &captured));
                        }
                        sleep_or_cancel(
                            retry_after.unwrap_or_else(|| backoff(attempt)),
                            &request.cancel,
                        )
                        .await?;
                        attempt += 1;
                        continue;
                    }
                    if matches!(
                        status,
                        StatusCode::UNAUTHORIZED
                            | StatusCode::FORBIDDEN
                            | StatusCode::NOT_FOUND
                            | StatusCode::GONE
                    ) {
                        return Err(http_error(status, &captured));
                    }

                    let content_range = captured
                        .get("content-range")
                        .and_then(|value| parse_content_range(value));
                    let linked_size = captured
                        .get("x-linked-size")
                        .and_then(|value| parse_u64(value));
                    let content_length = captured
                        .get("content-length")
                        .and_then(|value| parse_u64(value));
                    let (range_supported, total_size) = match status {
                        StatusCode::PARTIAL_CONTENT => (
                            true,
                            content_range
                                .and_then(|(_, _, total)| total)
                                .or(linked_size),
                        ),
                        StatusCode::OK => match content_range {
                            // Some servers answer a ranged request with 200 +
                            // Content-Range (ModelScope does this). The body is
                            // still a partial range, so use the total from
                            // Content-Range rather than the range length in
                            // Content-Length.
                            Some((_, _, total)) => (true, total.or(linked_size)),
                            None => (false, linked_size.or(content_length)),
                        },
                        StatusCode::RANGE_NOT_SATISFIABLE => {
                            (true, content_range.and_then(|(_, _, total)| total))
                        }
                        _ => {
                            return Err(Error::Backend(format!(
                                "unexpected HTTP status during probe: {status}"
                            )));
                        }
                    };

                    let checksum = captured
                        .get("x-linked-etag")
                        .and_then(|value| Checksum::from_etag(value))
                        .or_else(|| parse_digest(&captured));
                    let etag = captured
                        .get("etag")
                        .cloned()
                        .or_else(|| captured.get("x-linked-etag").cloned());
                    let last_modified = captured.get("last-modified").cloned();
                    let filename = captured
                        .get("content-disposition")
                        .and_then(|value| parse_content_disposition(value));
                    let mut headers = captured;
                    if let Some(total) = total_size {
                        headers.insert("total-size".to_string(), total.to_string());
                    }
                    return Ok(Probe {
                        final_url,
                        total_size,
                        range_supported,
                        filename,
                        etag,
                        last_modified,
                        checksum,
                        headers,
                    });
                }
            }
        }
    }

    async fn send_follow(
        &self,
        url: &Url,
        headers: &HeaderMap,
        options: &shaul_core::ResolvedOptions,
        cancel: &CancellationToken,
    ) -> Result<(Response, BTreeMap<String, String>, Url)> {
        let mut current = url.clone();
        let mut headers = headers.clone();
        let mut captured = BTreeMap::new();

        for _ in 0..MAX_REDIRECTS {
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                response = self.client.get(current.clone()).headers(headers.clone()).send() => {
                    response.map_err(|error| Error::Backend(format!("request failed: {error}")))?
                }
            };
            capture_headers(&response, &mut captured);
            let status = response.status();
            if matches!(
                status,
                StatusCode::MOVED_PERMANENTLY
                    | StatusCode::FOUND
                    | StatusCode::SEE_OTHER
                    | StatusCode::TEMPORARY_REDIRECT
                    | StatusCode::PERMANENT_REDIRECT
            ) {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        Error::Backend("redirect response missing Location".to_string())
                    })?;
                let next = current
                    .join(location)
                    .map_err(|error| Error::Backend(format!("invalid redirect URL: {error}")))?;
                if !same_origin(&current, &next) && !options.request.allow_cross_origin_auth {
                    for name in options.request.sensitive_header_names() {
                        headers.remove(name.as_str());
                    }
                    headers.remove(AUTHORIZATION);
                    headers.remove(COOKIE);
                }
                current = next;
                continue;
            }
            return Ok((response, captured, current));
        }
        Err(Error::Backend("too many redirects".to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn transfer_segmented(
        &self,
        request: &DownloadRequest,
        file: &ResourceFile,
        part_name: &Path,
        key: &str,
        file_resume: FileResume,
        task_resume: TaskResume,
        progress: Arc<AtomicU64>,
        total: Option<u64>,
    ) -> Result<FileResume> {
        let total =
            total.ok_or_else(|| Error::Backend("segmented transfer without size".to_string()))?;
        let missing = missing_ranges(total, &file_resume.completed);
        let mut chunks = Vec::new();
        for range in missing {
            chunks.extend(split_range(range, request.options.segment_size));
        }
        if chunks.is_empty() {
            return Ok(file_resume);
        }

        let mut state = task_resume.clone();
        state.files.insert(key.to_string(), file_resume.clone());
        let resume_state = Arc::new(Mutex::new(state));
        let checkpoint_lock = Arc::new(Mutex::new(()));
        let queue = Arc::new(std::sync::Mutex::new(VecDeque::from(chunks)));
        let range_broken = Arc::new(AtomicBool::new(false));
        let child_cancel = request.cancel.child_token();
        let if_range = strong_if_range(&file_resume.validator);
        let workers = request
            .options
            .connections
            .min(queue.lock().map(|queue| queue.len()).unwrap_or(1))
            .max(1);

        let mut set = tokio::task::JoinSet::new();
        for _ in 0..workers {
            let backend = self.clone();
            let request = request.clone();
            let file = file.clone();
            let part_name = part_name.to_path_buf();
            let key = key.to_string();
            let queue = queue.clone();
            let resume_state = resume_state.clone();
            let checkpoint_lock = checkpoint_lock.clone();
            let range_broken = range_broken.clone();
            let progress = progress.clone();
            let cancel = child_cancel.clone();
            let if_range = if_range.clone();
            set.spawn(async move {
                backend
                    .segment_worker(
                        &request,
                        &file,
                        &part_name,
                        &key,
                        queue,
                        resume_state,
                        checkpoint_lock,
                        range_broken,
                        progress,
                        cancel,
                        total,
                        if_range,
                    )
                    .await
            });
        }

        let mut first_error = None;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(FetchError::Cancelled)) => {}
                Ok(Err(FetchError::RangeBroken)) => {}
                Ok(Err(FetchError::Fatal(error))) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    child_cancel.cancel();
                }
                Ok(Err(FetchError::Retryable(message))) => {
                    if first_error.is_none() {
                        first_error = Some(Error::Backend(message));
                    }
                    child_cancel.cancel();
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(Error::Other(format!("segment worker failed: {error}")));
                    }
                    child_cancel.cancel();
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }

        if range_broken.load(Ordering::SeqCst) {
            request.events.emit(EventKind::Warning {
                task_id: Some(request.task_id),
                message: "server ignored a ranged request; falling back to a single stream"
                    .to_string(),
            });
            let file_handle = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(part_name)
                .await?;
            file_handle.set_len(0).await?;
            file_handle.sync_all().await?;
            progress.store(0, Ordering::Relaxed);
            let mut reset = file_resume;
            reset.completed.clear();
            reset.normalize();
            return self
                .transfer_single(
                    request,
                    file,
                    part_name,
                    key,
                    reset,
                    task_resume,
                    progress,
                    false,
                )
                .await;
        }

        let final_state = resume_state.lock().await.clone();
        let mut final_resume = final_state
            .files
            .get(key)
            .cloned()
            .ok_or_else(|| Error::Backend("missing resume state after transfer".to_string()))?;
        final_resume.completed = vec![shaul_core::ByteRange::new(0, total)?];
        final_resume.normalize();
        let mut snapshot = final_state;
        snapshot.files.insert(key.to_string(), final_resume.clone());
        request.checkpoint.save(&snapshot).await?;
        Ok(final_resume)
    }

    #[allow(clippy::too_many_arguments)]
    async fn segment_worker(
        &self,
        request: &DownloadRequest,
        file: &ResourceFile,
        part_name: &Path,
        key: &str,
        queue: Arc<std::sync::Mutex<VecDeque<shaul_core::ByteRange>>>,
        resume_state: Arc<Mutex<TaskResume>>,
        checkpoint_lock: Arc<Mutex<()>>,
        range_broken: Arc<AtomicBool>,
        progress: Arc<AtomicU64>,
        cancel: CancellationToken,
        total: u64,
        if_range: Option<String>,
    ) -> std::result::Result<(), FetchError> {
        let mut part = tokio::fs::OpenOptions::new()
            .write(true)
            .open(part_name)
            .await
            .map_err(|error| FetchError::Fatal(Error::Io(error)))?;
        loop {
            if cancel.is_cancelled() || range_broken.load(Ordering::SeqCst) {
                break;
            }
            let range = {
                let mut queue = queue
                    .lock()
                    .map_err(|_| FetchError::Fatal(Error::Other("work queue poisoned".into())))?;
                queue.pop_front()
            };
            let Some(range) = range else {
                break;
            };
            match self
                .fetch_range(
                    request,
                    file,
                    range,
                    &mut part,
                    &progress,
                    &cancel,
                    total,
                    if_range.as_deref(),
                )
                .await
            {
                Ok(()) => {
                    let _guard = checkpoint_lock.lock().await;
                    let snapshot = {
                        let mut state = resume_state.lock().await;
                        if let Some(entry) = state.files.get_mut(key) {
                            entry.completed.push(range);
                            entry.normalize();
                        }
                        state.clone()
                    };
                    request
                        .checkpoint
                        .save(&snapshot)
                        .await
                        .map_err(FetchError::Fatal)?;
                }
                Err(FetchError::Cancelled) => break,
                Err(FetchError::RangeBroken) => {
                    range_broken.store(true, Ordering::SeqCst);
                    cancel.cancel();
                    break;
                }
                Err(error) => {
                    cancel.cancel();
                    return Err(error);
                }
            }
        }
        part.flush()
            .await
            .map_err(|error| FetchError::Fatal(Error::Io(error)))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_range(
        &self,
        request: &DownloadRequest,
        file: &ResourceFile,
        range: shaul_core::ByteRange,
        part: &mut tokio::fs::File,
        progress: &AtomicU64,
        cancel: &CancellationToken,
        total: u64,
        if_range: Option<&str>,
    ) -> std::result::Result<(), FetchError> {
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            let range_header = format!("bytes={}-{}", range.start, range.end - 1);
            let headers = build_headers(
                &request.options,
                &file.headers,
                Some(&range_header),
                if_range,
            )
            .map_err(FetchError::Fatal)?;
            let sent = self
                .send_follow(&file.url, &headers, &request.options, cancel)
                .await;
            let (response, captured, _) = match sent {
                Err(Error::Cancelled) => return Err(FetchError::Cancelled),
                Err(error) => {
                    if attempt >= request.options.retries {
                        return Err(FetchError::Fatal(error));
                    }
                    sleep_or_cancel(backoff(attempt), cancel)
                        .await
                        .map_err(FetchError::Fatal)?;
                    attempt += 1;
                    continue;
                }
                Ok(response) => response,
            };
            let status = response.status();
            let retry_after = retry_after(response.headers());

            if status == StatusCode::RANGE_NOT_SATISFIABLE {
                return Err(FetchError::Fatal(Error::Backend(format!(
                    "range {} is not satisfiable",
                    range_header
                ))));
            }
            if status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
            {
                drop(response);
                if attempt >= request.options.retries {
                    return Err(FetchError::Fatal(http_error(status, &captured)));
                }
                sleep_or_cancel(retry_after.unwrap_or_else(|| backoff(attempt)), cancel)
                    .await
                    .map_err(FetchError::Fatal)?;
                attempt += 1;
                continue;
            }

            let content_range = captured
                .get("content-range")
                .and_then(|value| parse_content_range(value));
            if status == StatusCode::OK && content_range.is_none() {
                return Err(FetchError::RangeBroken);
            }
            let is_partial = status == StatusCode::PARTIAL_CONTENT
                || (status == StatusCode::OK && content_range.is_some());
            if !is_partial {
                return Err(FetchError::Fatal(http_error(status, &captured)));
            }
            let parsed = content_range.ok_or_else(|| {
                FetchError::Fatal(Error::Backend(
                    "ranged response missing a valid Content-Range".to_string(),
                ))
            })?;
            if parsed.0 != range.start || parsed.1 + 1 != range.end {
                return Err(FetchError::Fatal(Error::Backend(format!(
                    "server returned range {}-{}, expected {}-{}",
                    parsed.0,
                    parsed.1,
                    range.start,
                    range.end - 1
                ))));
            }
            if let Some(server_total) = parsed.2
                && server_total != total
            {
                return Err(FetchError::Fatal(Error::Backend(format!(
                    "file size changed from {total} to {server_total}"
                ))));
            }

            part.seek(SeekFrom::Start(range.start))
                .await
                .map_err(|error| FetchError::Fatal(Error::Io(error)))?;
            let mut written = 0u64;
            let mut stream = response.bytes_stream();
            let retry_message;
            loop {
                let next = tokio::select! {
                    _ = cancel.cancelled() => return Err(FetchError::Cancelled),
                    next = tokio::time::timeout(request.options.timeout, stream.next()) => next,
                };
                match next {
                    Ok(Some(Ok(chunk))) => {
                        if written + chunk.len() as u64 > range.len() {
                            retry_message =
                                Some("server sent more data than requested".to_string());
                            break;
                        }
                        part.write_all(&chunk)
                            .await
                            .map_err(|error| FetchError::Fatal(Error::Io(error)))?;
                        written += chunk.len() as u64;
                        progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    }
                    Ok(Some(Err(error))) => {
                        retry_message = Some(format!("stream error: {error}"));
                        break;
                    }
                    Ok(None) => {
                        if written == range.len() {
                            return Ok(());
                        }
                        retry_message = Some(format!(
                            "short ranged response: {written} of {} bytes",
                            range.len()
                        ));
                        break;
                    }
                    Err(_) => {
                        retry_message = Some("read timed out".to_string());
                        break;
                    }
                }
            }

            // A retry rewrites this range from the beginning; remove the
            // partial bytes from the progress counter first.
            let current = progress.load(Ordering::Relaxed);
            progress.fetch_sub(written.min(current), Ordering::Relaxed);
            if attempt >= request.options.retries {
                return Err(FetchError::Retryable(
                    retry_message.unwrap_or_else(|| "range transfer failed".to_string()),
                ));
            }
            sleep_or_cancel(backoff(attempt), cancel)
                .await
                .map_err(FetchError::Fatal)?;
            attempt += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn transfer_single(
        &self,
        request: &DownloadRequest,
        file: &ResourceFile,
        part_name: &Path,
        key: &str,
        mut file_resume: FileResume,
        mut task_resume: TaskResume,
        progress: Arc<AtomicU64>,
        range_supported: bool,
    ) -> Result<FileResume> {
        let mut offset = if request.options.resume && range_supported {
            contiguous_end(&file_resume.completed)
        } else {
            0
        };
        if file_resume
            .total_size
            .map(|total| offset >= total)
            .unwrap_or(false)
        {
            return Ok(file_resume);
        }

        let mut part = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(part_name)
            .await?;
        if offset == 0 {
            part.set_len(0).await?;
        }
        part.seek(SeekFrom::Start(offset)).await?;

        let mut attempt = 0u32;
        loop {
            if request.cancel.is_cancelled() {
                checkpoint_single(request, &mut task_resume, key, &mut file_resume, offset).await?;
                return Err(Error::Cancelled);
            }
            let range = if offset > 0 && range_supported {
                Some(format!("bytes={offset}-"))
            } else {
                None
            };
            let if_range = if offset > 0 {
                strong_if_range(&file_resume.validator)
            } else {
                None
            };
            let headers = build_headers(
                &request.options,
                &file.headers,
                range.as_deref(),
                if_range.as_deref(),
            )?;
            let sent = self
                .send_follow(&file.url, &headers, &request.options, &request.cancel)
                .await;
            let (response, captured, _) = match sent {
                Err(Error::Cancelled) => {
                    checkpoint_single(request, &mut task_resume, key, &mut file_resume, offset)
                        .await?;
                    return Err(Error::Cancelled);
                }
                Err(error) => {
                    checkpoint_single(request, &mut task_resume, key, &mut file_resume, offset)
                        .await?;
                    if attempt >= request.options.retries {
                        return Err(error);
                    }
                    sleep_or_cancel(backoff(attempt), &request.cancel).await?;
                    attempt += 1;
                    continue;
                }
                Ok(response) => response,
            };
            let status = response.status();
            let retry_after = retry_after(response.headers());

            let content_range = captured
                .get("content-range")
                .and_then(|value| parse_content_range(value));
            if status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
            {
                drop(response);
                checkpoint_single(request, &mut task_resume, key, &mut file_resume, offset).await?;
                if attempt >= request.options.retries {
                    return Err(http_error(status, &captured));
                }
                sleep_or_cancel(
                    retry_after.unwrap_or_else(|| backoff(attempt)),
                    &request.cancel,
                )
                .await?;
                attempt += 1;
                continue;
            }

            let is_partial = status == StatusCode::PARTIAL_CONTENT
                || (status == StatusCode::OK && content_range.is_some());
            if is_partial {
                if let Some((start, _, _)) = content_range
                    && offset > 0
                    && start != offset
                {
                    return Err(Error::Backend(format!(
                        "server resumed at {start}, expected {offset}"
                    )));
                }
            } else if status == StatusCode::OK {
                if offset > 0 {
                    part.set_len(0).await?;
                    part.seek(SeekFrom::Start(0)).await?;
                    offset = 0;
                    progress.store(0, Ordering::Relaxed);
                }
            } else {
                return Err(http_error(status, &captured));
            }

            part.seek(SeekFrom::Start(offset)).await?;
            let mut stream = response.bytes_stream();
            let mut since_checkpoint = 0u64;
            let retry_message;
            loop {
                let next = tokio::select! {
                    _ = request.cancel.cancelled() => {
                        checkpoint_single(request, &mut task_resume, key, &mut file_resume, offset).await?;
                        part.sync_all().await?;
                        return Err(Error::Cancelled);
                    }
                    next = tokio::time::timeout(request.options.timeout, stream.next()) => next,
                };
                match next {
                    Ok(Some(Ok(chunk))) => {
                        if let Some(total) = file_resume.total_size
                            && offset + chunk.len() as u64 > total
                        {
                            return Err(Error::Backend(
                                "server sent more data than the known file size".to_string(),
                            ));
                        }
                        part.write_all(&chunk).await?;
                        offset += chunk.len() as u64;
                        progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                        since_checkpoint += chunk.len() as u64;
                        if since_checkpoint >= request.options.segment_size {
                            checkpoint_single(
                                request,
                                &mut task_resume,
                                key,
                                &mut file_resume,
                                offset,
                            )
                            .await?;
                            since_checkpoint = 0;
                        }
                    }
                    Ok(Some(Err(error))) => {
                        retry_message = Some(format!("stream error: {error}"));
                        break;
                    }
                    Ok(None) => {
                        if let Some(total) = file_resume.total_size
                            && offset < total
                        {
                            retry_message =
                                Some(format!("short response: {offset} of {total} bytes"));
                            break;
                        }
                        part.flush().await?;
                        part.sync_all().await?;
                        file_resume.total_size = file_resume.total_size.or(Some(offset));
                        file_resume.completed = if offset > 0 {
                            vec![shaul_core::ByteRange::new(0, offset)?]
                        } else {
                            Vec::new()
                        };
                        file_resume.normalize();
                        task_resume
                            .files
                            .insert(key.to_string(), file_resume.clone());
                        request.checkpoint.save(&task_resume).await?;
                        return Ok(file_resume);
                    }
                    Err(_) => {
                        retry_message = Some("read timed out".to_string());
                        break;
                    }
                }
            }

            part.flush().await?;
            checkpoint_single(request, &mut task_resume, key, &mut file_resume, offset).await?;
            if attempt >= request.options.retries {
                return Err(Error::Backend(
                    retry_message.unwrap_or_else(|| "download failed".to_string()),
                ));
            }
            sleep_or_cancel(backoff(attempt), &request.cancel).await?;
            attempt += 1;
        }
    }
}

#[async_trait]
impl DownloadBackend for HttpBackend {
    fn name(&self) -> &'static str {
        "http"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            segmented: true,
            resume: true,
            multi_file: true,
        }
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(&resource.kind, ResourceKind::Http)
    }

    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        let mut task_resume = request.resume.clone().unwrap_or_default();
        let mut outcomes = Vec::new();
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let outcome = self.download_file(&request, file, &mut task_resume).await?;
            outcomes.push(outcome);
            request.checkpoint.save(&task_resume).await?;
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }
}

struct Probe {
    final_url: Url,
    total_size: Option<u64>,
    range_supported: bool,
    filename: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    checksum: Option<Checksum>,
    headers: BTreeMap<String, String>,
}

impl Probe {
    fn validator(&self) -> Validator {
        Validator {
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
            size: self.total_size,
        }
    }
}

#[derive(Debug)]
enum FetchError {
    RangeBroken,
    Cancelled,
    Retryable(String),
    Fatal(Error),
}

fn build_headers(
    options: &shaul_core::ResolvedOptions,
    file_headers: &[Header],
    range: Option<&str>,
    if_range: Option<&str>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for header in &options.request.headers {
        if !is_reserved(&header.name) {
            insert_header(
                &mut headers,
                &header.name,
                &shaul_core::expand_env(&header.value)?,
            )?;
        }
    }
    for header in file_headers {
        if !is_reserved(&header.name) {
            insert_header(
                &mut headers,
                &header.name,
                &shaul_core::expand_env(&header.value)?,
            )?;
        }
    }

    if !headers.contains_key(AUTHORIZATION)
        && let Some(auth) = &options.request.auth
    {
        match auth {
            shaul_core::Auth::Bearer { token } => {
                insert_header(
                    &mut headers,
                    "authorization",
                    &format!("Bearer {}", shaul_core::expand_env(token)?),
                )?;
            }
            shaul_core::Auth::Basic { username, password } => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
                    "{}:{}",
                    shaul_core::expand_env(username)?,
                    shaul_core::expand_env(password)?
                ));
                insert_header(&mut headers, "authorization", &format!("Basic {encoded}"))?;
            }
            shaul_core::Auth::Header(header) => {
                insert_header(
                    &mut headers,
                    &header.name,
                    &shaul_core::expand_env(&header.value)?,
                )?;
            }
        }
    }

    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    insert_header(&mut headers, "user-agent", &options.user_agent)?;
    if let Some(range) = range {
        insert_header(&mut headers, "range", range)?;
    }
    if let Some(if_range) = if_range {
        insert_header(&mut headers, "if-range", if_range)?;
    }
    Ok(headers)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| Error::Config(format!("invalid header name: {name}")))?;
    let value = HeaderValue::from_str(value)
        .map_err(|_| Error::Config(format!("invalid value for header: {name}")))?;
    headers.insert(name, value);
    Ok(())
}

fn is_reserved(name: &str) -> bool {
    RESERVED_HEADERS
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

fn capture_headers(response: &Response, captured: &mut BTreeMap<String, String>) {
    for name in CAPTURE_HEADERS {
        if let Some(value) = response.headers().get(*name)
            && let Ok(value) = value.to_str()
        {
            captured.insert((*name).to_string(), value.to_string());
        }
    }
}

fn redacted_url(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let total = if total.trim() == "*" {
        None
    } else {
        Some(total.trim().parse::<u64>().ok()?)
    };
    if range.trim() == "*" {
        return Some((0, 0, total));
    }
    let (start, end) = range.split_once('-')?;
    Some((
        start.trim().parse::<u64>().ok()?,
        end.trim().parse::<u64>().ok()?,
        total,
    ))
}

fn parse_u64(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(60)))
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250u64.saturating_mul(1u64 << attempt.min(5)))
}

async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> Result<()> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

fn http_error(status: StatusCode, captured: &BTreeMap<String, String>) -> Error {
    let code = captured
        .get("x-error-code")
        .map(|code| format!(" ({code})"))
        .unwrap_or_default();
    Error::Backend(format!("HTTP {status}{code}"))
}

fn parse_digest(captured: &BTreeMap<String, String>) -> Option<Checksum> {
    if let Some(value) = captured.get("digest") {
        for part in value.split(',') {
            let Some((algorithm, encoded)) = part.trim().split_once('=') else {
                continue;
            };
            let algorithm = match algorithm.trim().to_ascii_lowercase().as_str() {
                "sha-256" | "sha256" => ChecksumAlgorithm::Sha256,
                "sha-512" | "sha512" => ChecksumAlgorithm::Sha512,
                "sha" | "sha-1" | "sha1" => ChecksumAlgorithm::Sha1,
                "md5" => ChecksumAlgorithm::Md5,
                _ => continue,
            };
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded.trim())
                && let Ok(checksum) = Checksum::new(algorithm, hex::encode(bytes))
            {
                return Some(checksum);
            }
        }
    }
    if let Some(value) = captured.get("content-md5")
        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value.trim())
        && let Ok(checksum) = Checksum::new(ChecksumAlgorithm::Md5, hex::encode(bytes))
    {
        return Some(checksum);
    }
    None
}

fn strong_if_range(validator: &Validator) -> Option<String> {
    if let Some(etag) = &validator.etag
        && !etag.starts_with("W/")
    {
        return Some(etag.clone());
    }
    validator.last_modified.clone()
}

fn contiguous_end(completed: &[shaul_core::ByteRange]) -> u64 {
    let merged = merge_ranges(completed.to_vec());
    if merged.len() == 1 && merged[0].start == 0 {
        merged[0].end
    } else {
        0
    }
}

fn manifest_path(destination: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        destination.join(path)
    }
}

fn path_for_manifest(destination: &Path, path: &Path) -> String {
    match path.strip_prefix(destination) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().to_string(),
    }
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

fn resolve_final_path(
    request: &DownloadRequest,
    file: &ResourceFile,
    probe_filename: Option<&str>,
    default_name: &str,
) -> Result<(PathBuf, String)> {
    let single = request.resource.files.len() == 1;
    let response_name = if file.use_response_filename {
        probe_filename.map(sanitize_file_name)
    } else {
        None
    };
    let name = response_name
        .clone()
        .unwrap_or_else(|| default_name.to_string());

    if let Some(output) = &request.options.output {
        if single {
            if output.is_dir() {
                let relative = if file.use_response_filename {
                    name.clone()
                } else {
                    file.path.clone()
                };
                let path = output.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
                return Ok((path.clone(), relative_path(&request.destination, &path)));
            }
            return Ok((output.clone(), relative_path(&request.destination, output)));
        }
        let path = output.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        return Ok((path.clone(), relative_path(&request.destination, &path)));
    }

    let relative = if single && file.use_response_filename {
        name
    } else {
        file.path.clone()
    };
    let path = request
        .destination
        .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
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

async fn checkpoint_single(
    request: &DownloadRequest,
    task_resume: &mut TaskResume,
    key: &str,
    file_resume: &mut FileResume,
    offset: u64,
) -> Result<()> {
    file_resume.completed = if offset > 0 {
        vec![shaul_core::ByteRange::new(0, offset)?]
    } else {
        Vec::new()
    };
    file_resume.normalize();
    task_resume
        .files
        .insert(key.to_string(), file_resume.clone());
    request.checkpoint.save(task_resume).await
}

fn spawn_progress_reporter(
    events: EventSink,
    task_id: TaskId,
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
