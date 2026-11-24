use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "xet")]
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "xet")]
use base64::Engine;
use shaul_core::{
    BackendCapabilities, Config, ContentCache, DownloadBackend, DownloadOutcome, DownloadRequest,
    Error, EventKind, FileOutcome, HubProvider, HubRepoType, Resource, ResourceFile, ResourceKind,
    Result, ResumeCheckpoint, TaskResume, compute_file_checksum, metrics, sanitize_file_name,
    validate_relative_path,
};
#[cfg(feature = "xet")]
use shaul_core::{CancellationToken, ChecksumAlgorithm, expand_env};
use shaul_http::HttpBackend;
use tokio::sync::Mutex;

use crate::cache;
use crate::client::HubClient;

#[cfg(feature = "xet")]
use xet::xet_session::{HeaderMap, HeaderValue, XetFileInfo, XetSession, XetSessionBuilder};

/// Backend for Hugging Face and ModelScope repository snapshots.
#[derive(Clone)]
pub struct HubBackend {
    client: Arc<HubClient>,
    http: HttpBackend,
    #[cfg(feature = "xet")]
    xet: Arc<std::sync::OnceLock<Option<XetSession>>>,
}

#[cfg(feature = "xet")]
fn build_xet_session() -> Option<XetSession> {
    let builder = XetSessionBuilder::new();
    let builder = if let Ok(handle) = tokio::runtime::Handle::try_current() {
        builder.with_tokio_handle(handle)
    } else {
        builder
    };
    match builder.build() {
        Ok(session) => Some(session),
        Err(error) => {
            tracing::warn!(%error, "could not initialize Xet; Hub downloads will use HTTP");
            None
        }
    }
}

#[cfg(feature = "xet")]
fn xet_file_info(file: &ResourceFile) -> Option<XetFileInfo> {
    let hash = file
        .metadata
        .get("hub.xet_hash")
        .filter(|value| !value.is_empty())?
        .clone();
    let sha256 = file
        .checksum
        .as_ref()
        .filter(|checksum| checksum.algorithm == ChecksumAlgorithm::Sha256)
        .map(|checksum| checksum.hex.clone());
    Some(XetFileInfo {
        hash,
        file_size: file.size,
        sha256,
    })
}

#[cfg(feature = "xet")]
fn xet_headers(request: &shaul_core::RequestOptions) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for header in &request.headers {
        let value = expand_env(&header.value)?;
        insert_xet_header(&mut headers, &header.name, &value)?;
    }
    if let Some(auth) = &request.auth {
        match auth {
            shaul_core::Auth::Bearer { token } => {
                insert_xet_header(&mut headers, "authorization", &format!("Bearer {token}"))?
            }
            shaul_core::Auth::Basic { username, password } => {
                let encoded = base64::engine::general_purpose::STANDARD
                    .encode(format!("{username}:{password}"));
                insert_xet_header(&mut headers, "authorization", &format!("Basic {encoded}"))?;
            }
            shaul_core::Auth::Header(header) => {
                let value = expand_env(&header.value)?;
                insert_xet_header(&mut headers, &header.name, &value)?;
            }
        }
    }
    Ok(headers)
}

#[cfg(feature = "xet")]
fn insert_xet_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    let name = http::HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| Error::Config(format!("invalid header name: {name}")))?;
    let value = HeaderValue::from_str(value)
        .map_err(|_| Error::Config(format!("invalid header value for {name}")))?;
    headers.insert(name, value);
    Ok(())
}

impl HubBackend {
    /// Build a Hub backend, reusing the shared segmented HTTP backend for file
    /// transfers.
    pub fn new(client: Arc<HubClient>, config: &Config) -> Result<Self> {
        Ok(Self {
            client,
            http: HttpBackend::new(config)?,
            #[cfg(feature = "xet")]
            xet: Arc::new(std::sync::OnceLock::new()),
        })
    }

    /// Build a Hub backend around an existing HTTP backend, so direct and Hub
    /// transfers share connection and bandwidth limits. Xet is not enabled
    /// through this constructor; use [`HubBackend::with_http_and_xet`].
    pub fn with_http(client: Arc<HubClient>, http: HttpBackend) -> Self {
        Self {
            client,
            http,
            #[cfg(feature = "xet")]
            xet: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Build a Hub backend around an existing HTTP backend and also enable the
    /// optional Xet session when the `xet` feature is enabled.
    pub fn with_http_and_xet(client: Arc<HubClient>, http: HttpBackend) -> Self {
        Self::with_http(client, http)
    }

    #[cfg(feature = "xet")]
    fn xet_token_url(&self, resource: &Resource) -> Result<String> {
        let ResourceKind::Hub {
            provider: HubProvider::HuggingFace,
            repo,
            repo_type,
            revision,
        } = &resource.kind
        else {
            return Err(Error::Backend(
                "Xet is only available for Hugging Face Hub resources".to_string(),
            ));
        };
        let repo_segment = match repo_type {
            HubRepoType::Model => "models",
            HubRepoType::Dataset => "datasets",
            _ => {
                return Err(Error::Backend(
                    "unsupported Hugging Face repository type for Xet".to_string(),
                ));
            }
        };
        let revision = if revision.trim().is_empty() {
            "main"
        } else {
            revision.as_str()
        };
        let mut url = self.client.hf_endpoint().clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::Backend("invalid Hugging Face endpoint".to_string()))?;
            segments.push("api").push(repo_segment);
            for part in repo.split('/').filter(|part| !part.is_empty()) {
                segments.push(part);
            }
            segments.push("xet-read-token").push(revision);
        }
        Ok(url.to_string())
    }

    async fn try_content_cache(
        &self,
        request: &DownloadRequest,
        task_resume: &TaskResume,
        file: &ResourceFile,
        relative: &str,
    ) -> Result<Option<FileOutcome>> {
        if !request.options.content_cache {
            return Ok(None);
        }
        let checksum = request
            .options
            .checksum
            .clone()
            .or_else(|| file.checksum.clone())
            .or_else(|| {
                task_resume
                    .files
                    .get(file.url.as_str())
                    .and_then(|entry| entry.checksum.clone())
            });
        let (Some(checksum), Some(size)) = (checksum, file.size) else {
            return Ok(None);
        };
        let cache = match ContentCache::new(&request.options.content_cache_dir) {
            Ok(cache) => cache,
            Err(error) => {
                warning(request, format!("could not open content cache: {error}"));
                return Ok(None);
            }
        };
        let Some(cached) = cache.lookup_verified(&checksum, size).await? else {
            request
                .events
                .telemetry()
                .counter(metrics::CACHE_MISSES, 1, &[]);
            return Ok(None);
        };
        let destination = request
            .destination
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let action = cache::prepare_destination(&destination, Some(size), &request.options).await?;
        let final_path = match action {
            cache::DestinationAction::Reuse => return Ok(None),
            cache::DestinationAction::Create(path) => path,
        };
        if let Err(error) = cache.materialize(&cached, &final_path, size).await {
            warning(
                request,
                format!("could not materialize cached file: {error}"),
            );
            return Ok(None);
        }
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
            file: relative.to_string(),
            total_bytes: Some(size),
        });
        request.events.emit(EventKind::Progress {
            task_id: request.task_id,
            file: relative.to_string(),
            downloaded_bytes: size,
            total_bytes: Some(size),
        });
        Ok(Some(FileOutcome::new(
            final_path,
            relative.to_string(),
            size,
            Some(checksum),
            true,
        )))
    }

    #[cfg(feature = "xet")]
    async fn try_xet(
        &self,
        request: &DownloadRequest,
        file: &ResourceFile,
        relative: &str,
        effective_request: &shaul_core::RequestOptions,
    ) -> Result<Option<FileOutcome>> {
        let Some(session) = self.xet.get_or_init(build_xet_session).as_ref() else {
            return Ok(None);
        };
        let Some(info) = xet_file_info(file) else {
            return Ok(None);
        };
        let token_url = match self.xet_token_url(&request.resource) {
            Ok(url) => url,
            Err(error) => {
                tracing::debug!(%error, "Xet token URL is unavailable; using HTTP");
                return Ok(None);
            }
        };
        let headers = match xet_headers(effective_request) {
            Ok(headers) => headers,
            Err(error) => {
                warning(request, format!("could not build Xet headers: {error}"));
                return Ok(None);
            }
        };
        let destination = request
            .destination
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let action = cache::prepare_destination(&destination, file.size, &request.options).await?;
        let final_path = match action {
            cache::DestinationAction::Reuse => {
                let size = tokio::fs::metadata(&destination).await?.len();
                request.events.emit(EventKind::FileStarted {
                    task_id: request.task_id,
                    file: relative.to_string(),
                    total_bytes: Some(size),
                });
                return Ok(Some(FileOutcome::new(
                    destination,
                    relative.to_string(),
                    size,
                    file.checksum
                        .clone()
                        .or_else(|| request.options.checksum.clone()),
                    false,
                )));
            }
            cache::DestinationAction::Create(path) => path,
        };
        if request.options.disk_space_check
            && let Some(size) = file.size
        {
            shaul_core::ensure_free_space(&request.destination, size)?;
        }

        let group = match session.new_file_download_group() {
            Ok(builder) => builder
                .with_token_refresh_url(token_url, headers)
                .build()
                .await
                .map_err(|error| Error::Backend(format!("Xet group: {error}")))?,
            Err(error) => {
                warning(request, format!("could not start Xet download: {error}"));
                return Ok(None);
            }
        };
        request.events.emit(EventKind::FileStarted {
            task_id: request.task_id,
            file: relative.to_string(),
            total_bytes: file.size,
        });
        let stop = CancellationToken::new();
        let progress_group = group.clone();
        let progress_events = request.events.clone();
        let progress_file = relative.to_string();
        let progress_task_id = request.task_id;
        let progress_total = file.size;
        let progress_stop = stop.clone();
        let progress = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = progress_stop.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {
                        let report = progress_group.progress();
                        progress_events.emit(EventKind::Progress {
                            task_id: progress_task_id,
                            file: progress_file.clone(),
                            downloaded_bytes: report.total_bytes_completed,
                            total_bytes: progress_total.or(Some(report.total_bytes)),
                        });
                    }
                }
            }
        });
        if let Err(error) = group.download_file_to_path(info, final_path.clone()).await {
            let _ = group.abort();
            stop.cancel();
            let _ = progress.await;
            let _ = cache::remove_file_if_exists(&final_path).await;
            warning(request, format!("Xet download failed: {error}"));
            return Ok(None);
        }
        let finish_group = group.clone();
        let finish = finish_group.finish();
        let result = tokio::select! {
            result = finish => result,
            _ = request.cancel.cancelled() => {
                let _ = group.abort();
                stop.cancel();
                let _ = progress.await;
                let _ = cache::remove_file_if_exists(&final_path).await;
                return Err(Error::Cancelled);
            }
        };
        stop.cancel();
        let _ = progress.await;
        match result {
            Ok(_) => {
                let size = tokio::fs::metadata(&final_path).await?.len();
                Ok(Some(FileOutcome::new(
                    final_path,
                    relative.to_string(),
                    size,
                    file.checksum
                        .clone()
                        .or_else(|| request.options.checksum.clone()),
                    false,
                )))
            }
            Err(error) => {
                let _ = cache::remove_file_if_exists(&final_path).await;
                warning(request, format!("Xet download failed: {error}"));
                Ok(None)
            }
        }
    }

    async fn download_one(
        &self,
        provider: HubProvider,
        request: &DownloadRequest,
        task_resume: &mut TaskResume,
        file: &ResourceFile,
        relative: &str,
    ) -> Result<FileOutcome> {
        if let Some(outcome) = self
            .try_content_cache(request, task_resume, file, relative)
            .await?
        {
            return Ok(outcome);
        }
        // Provider credentials are resolved here, at run time, so they are
        // never written into the task sidecar.
        let effective_request = self
            .client
            .request_options(provider, &request.options.request)?;
        #[cfg(feature = "xet")]
        {
            // Xet transfers cannot be resumed and must not start when
            // `--verify` requires a checksum that is not known yet; the HTTP
            // path can still discover one from response headers.
            let checksum_known = request.options.checksum.is_some()
                || file.checksum.is_some()
                || task_resume
                    .files
                    .get(file.url.as_str())
                    .and_then(|entry| entry.checksum.as_ref())
                    .is_some();
            if (request.options.verify != shaul_core::VerifyMode::Require || checksum_known)
                && let Some(outcome) = self
                    .try_xet(request, file, relative, &effective_request)
                    .await?
            {
                return Ok(outcome);
            }
        }
        let single_file = ResourceFile {
            path: relative.to_string(),
            url: file.url.clone(),
            size: file.size,
            checksum: file.checksum.clone(),
            headers: file.headers.clone(),
            metadata: file.metadata.clone(),
            use_response_filename: false,
        };
        let resource = Resource {
            kind: ResourceKind::Http,
            name: relative.to_string(),
            files: vec![single_file.clone()],
            request: effective_request.clone(),
            checksum: file.checksum.clone(),
            revision: request.resource.revision.clone(),
            metadata: Default::default(),
        };
        let mut options = request.options.clone();
        options.output = None;
        options.request = effective_request;
        options.checksum = file.checksum.clone();
        let single_request = DownloadRequest {
            task_id: request.task_id,
            resource,
            destination: request.destination.clone(),
            options,
            resume: Some(task_resume.clone()),
            cancel: request.cancel.clone(),
            events: request.events.clone(),
            checkpoint: request.checkpoint.clone(),
        };
        self.http
            .download_file(&single_request, &single_file, task_resume)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_with_cache(
        &self,
        request: &DownloadRequest,
        task_resume: &mut TaskResume,
        provider: HubProvider,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        file: &ResourceFile,
        relative: &str,
    ) -> Result<FileOutcome> {
        let Some(blob_name) = file
            .metadata
            .get("hub.blob")
            .filter(|value| !value.is_empty())
        else {
            return self
                .download_one(provider, request, task_resume, file, relative)
                .await;
        };
        let cache_repo = self.client.cache_repo_dir(provider, repo, repo_type);
        let blob_path = cache_repo.join("blobs").join(sanitize_file_name(blob_name));
        let destination = request
            .destination
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let expected_size = file.size;
        let cached_checksum = task_resume
            .files
            .get(file.url.as_str())
            .and_then(|entry| entry.checksum.clone());
        let checksum = file
            .checksum
            .clone()
            .or_else(|| request.options.checksum.clone())
            .or(cached_checksum);

        if tokio::fs::try_exists(&blob_path).await? {
            match cache::validate_cached(&blob_path, expected_size, checksum.as_ref()).await {
                Ok(true) => {
                    let action =
                        cache::prepare_destination(&destination, expected_size, &request.options)
                            .await?;
                    let final_path = match action {
                        cache::DestinationAction::Reuse => destination.clone(),
                        cache::DestinationAction::Create(path) => {
                            cache::link_or_copy(&blob_path, &path).await?;
                            path
                        }
                    };
                    let size = match expected_size {
                        Some(size) => size,
                        None => tokio::fs::metadata(&final_path).await?.len(),
                    };
                    if let Err(error) = self
                        .create_snapshot(
                            provider,
                            repo,
                            repo_type,
                            revision,
                            file,
                            &blob_path,
                            task_resume,
                        )
                        .await
                    {
                        warning(request, format!("could not update cache snapshot: {error}"));
                    }
                    request.events.emit(EventKind::FileStarted {
                        task_id: request.task_id,
                        file: relative.to_string(),
                        total_bytes: Some(size),
                    });
                    request.events.emit(EventKind::Progress {
                        task_id: request.task_id,
                        file: relative.to_string(),
                        downloaded_bytes: size,
                        total_bytes: Some(size),
                    });
                    return Ok(FileOutcome::new(
                        final_path,
                        relative.to_string(),
                        size,
                        checksum,
                        false,
                    ));
                }
                Ok(false) => {
                    warning(
                        request,
                        "cached blob failed validation; downloading it again".to_string(),
                    );
                    let _ = tokio::fs::remove_file(&blob_path).await;
                }
                Err(error) => {
                    warning(request, format!("could not validate cached blob: {error}"));
                }
            }
        }

        let outcome = self
            .download_one(provider, request, task_resume, file, relative)
            .await?;
        let mut cacheable = true;
        let verify_checksum = checksum.clone().or_else(|| outcome.checksum.clone());
        if let Some(expected) = &verify_checksum {
            match compute_file_checksum(outcome.path.clone(), expected.algorithm).await {
                Ok(actual) if expected.matches(&actual) => {}
                Ok(actual) => {
                    cacheable = false;
                    warning(
                        request,
                        format!(
                            "not caching {}: checksum mismatch (expected {}, got {})",
                            relative, expected.hex, actual.hex
                        ),
                    );
                }
                Err(error) => {
                    cacheable = false;
                    warning(request, format!("not caching {relative}: {error}"));
                }
            }
        }
        if cacheable
            && let Err(error) = self
                .populate_cache(
                    provider,
                    repo,
                    repo_type,
                    revision,
                    file,
                    &outcome.path,
                    &blob_path,
                    task_resume,
                )
                .await
        {
            warning(request, format!("could not populate cache: {error}"));
        }
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    async fn populate_cache(
        &self,
        provider: HubProvider,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        file: &ResourceFile,
        source: &Path,
        blob_path: &Path,
        task_resume: &TaskResume,
    ) -> Result<()> {
        cache::ensure_parent(blob_path).await?;
        cache::remove_file_if_exists(blob_path).await?;
        cache::link_or_copy(source, blob_path).await?;
        self.create_snapshot(
            provider,
            repo,
            repo_type,
            revision,
            file,
            blob_path,
            task_resume,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_snapshot(
        &self,
        provider: HubProvider,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        file: &ResourceFile,
        blob_path: &Path,
        task_resume: &TaskResume,
    ) -> Result<()> {
        let cache_repo = self.client.cache_repo_dir(provider, repo, repo_type);
        let ref_path = cache_repo.join("refs").join(sanitize_file_name(revision));
        let existing_commit = tokio::fs::read_to_string(&ref_path)
            .await
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let commit = task_resume
            .files
            .get(file.url.as_str())
            .and_then(|entry| entry.metadata.get("x-repo-commit"))
            .cloned()
            .filter(|value| !value.is_empty())
            .or(existing_commit);
        let snapshot_revision = commit.clone().unwrap_or_else(|| revision.to_string());
        validate_relative_path(&file.path)
            .map_err(|_| Error::Backend(format!("unsafe Hub file path: {:?}", file.path)))?;
        let snapshot_path = cache_repo
            .join("snapshots")
            .join(sanitize_file_name(&snapshot_revision))
            .join(&file.path);
        cache::link_or_copy(blob_path, &snapshot_path).await?;

        if provider == HubProvider::HuggingFace
            && let Some(commit) = commit
        {
            cache::ensure_parent(&ref_path).await?;
            let current = tokio::fs::read_to_string(&ref_path).await.ok();
            if current.as_deref() != Some(commit.as_str()) {
                tokio::fs::write(&ref_path, commit).await?;
            }
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "xet"))]
mod xet_tests {
    use super::*;
    use shaul_core::{Checksum, Header, RequestOptions};

    #[test]
    fn builds_xet_file_info_from_tree_metadata() {
        let checksum = Checksum::new(ChecksumAlgorithm::Sha256, "a".repeat(64)).unwrap();
        let mut metadata = std::collections::BTreeMap::new();
        metadata.insert("hub.xet_hash".to_string(), "xet-hash".to_string());
        let file = ResourceFile {
            path: "model.bin".to_string(),
            url: "https://huggingface.co/org/repo/resolve/main/model.bin"
                .parse()
                .unwrap(),
            size: Some(42),
            checksum: Some(checksum),
            headers: Vec::new(),
            metadata,
            use_response_filename: false,
        };
        let info = xet_file_info(&file).unwrap();
        assert_eq!(info.hash, "xet-hash");
        assert_eq!(info.file_size, Some(42));
        assert_eq!(info.sha256, Some("a".repeat(64)));
    }

    #[test]
    fn builds_authorization_headers_for_xet() {
        let request = RequestOptions {
            headers: vec![Header::new("X-Test", "yes").unwrap()],
            auth: Some(shaul_core::Auth::bearer("secret")),
            allow_cross_origin_auth: false,
        };
        let headers = xet_headers(&request).unwrap();
        assert_eq!(headers.get("authorization").unwrap(), "Bearer secret");
        assert_eq!(headers.get("x-test").unwrap(), "yes");
    }
}

#[async_trait]
impl DownloadBackend for HubBackend {
    fn name(&self) -> &'static str {
        "hub"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::new(true, true, true)
    }

    fn supports(&self, resource: &Resource) -> bool {
        matches!(&resource.kind, ResourceKind::Hub { .. })
    }

    #[tracing::instrument(
        name = "shaul.hub.download",
        level = "debug",
        skip(self, request),
        fields(task_id = %request.task_id, files = request.resource.files.len())
    )]
    async fn download(&self, request: DownloadRequest) -> Result<DownloadOutcome> {
        if request.options.output.is_some() {
            return Err(Error::Config(
                "--output is not supported for Hub downloads; use --dir".to_string(),
            ));
        }
        let (provider, repo, repo_type, revision) = match &request.resource.kind {
            ResourceKind::Hub {
                provider,
                repo,
                repo_type,
                revision,
            } => (*provider, repo.clone(), *repo_type, revision.clone()),
            _ => {
                return Err(Error::NoBackend(
                    "HubBackend received a non-Hub resource".to_string(),
                ));
            }
        };
        if request.options.file_jobs > 1 && request.resource.files.len() > 1 {
            self.download_concurrent(&request, provider, &repo, repo_type, &revision)
                .await
        } else {
            self.download_sequential(&request, provider, &repo, repo_type, &revision)
                .await
        }
    }
}

impl HubBackend {
    async fn download_sequential(
        &self,
        request: &DownloadRequest,
        provider: HubProvider,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
    ) -> Result<DownloadOutcome> {
        let mut task_resume = request.resume.clone().unwrap_or_default();
        let mut outcomes = Vec::new();
        let repo_name = sanitize_file_name(repo.rsplit('/').next().unwrap_or("repo"));
        for file in &request.resource.files {
            if request.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let relative = hub_relative_path(&repo_name, file)?;
            let outcome = if request.options.cache {
                self.download_with_cache(
                    request,
                    &mut task_resume,
                    provider,
                    repo,
                    repo_type,
                    revision,
                    file,
                    &relative,
                )
                .await?
            } else {
                self.download_one(provider, request, &mut task_resume, file, &relative)
                    .await?
            };
            outcomes.push(outcome);
            request.checkpoint.save(&task_resume).await?;
        }
        Ok(DownloadOutcome::Completed(outcomes))
    }

    async fn download_concurrent(
        &self,
        request: &DownloadRequest,
        provider: HubProvider,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
    ) -> Result<DownloadOutcome> {
        let shared = Arc::new(Mutex::new(request.resume.clone().unwrap_or_default()));
        let cancel = request.cancel.child_token();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            request.options.file_jobs.max(1),
        ));
        let repo_name = sanitize_file_name(repo.rsplit('/').next().unwrap_or("repo"));
        let mut tasks = tokio::task::JoinSet::new();
        for (index, file) in request.resource.files.iter().cloned().enumerate() {
            let relative = hub_relative_path(&repo_name, &file)?;
            let key = file.url.as_str().to_string();
            let mut local = TaskResume::default();
            if let Some(stored) = shared.lock().await.files.get(&key).cloned() {
                local.files.insert(key, stored);
            }
            let mut file_request = request.clone();
            file_request.cancel = cancel.clone();
            let checkpoint = Arc::new(FileCheckpoint {
                shared: shared.clone(),
                inner: request.checkpoint.clone(),
            });
            file_request.checkpoint = checkpoint.clone();
            let backend = self.clone();
            let repo = repo.to_string();
            let revision = revision.to_string();
            let permit = semaphore.clone();
            tasks.spawn(async move {
                let _permit = permit
                    .acquire_owned()
                    .await
                    .map_err(|_| Error::Other("file job semaphore closed".to_string()))?;
                let mut local = local;
                let outcome = if file_request.options.cache {
                    backend
                        .download_with_cache(
                            &file_request,
                            &mut local,
                            provider,
                            &repo,
                            repo_type,
                            &revision,
                            &file,
                            &relative,
                        )
                        .await?
                } else {
                    backend
                        .download_one(provider, &file_request, &mut local, &file, &relative)
                        .await?
                };
                checkpoint.save(&local).await?;
                Ok::<_, Error>((index, outcome))
            });
        }

        let mut outcomes: Vec<Option<FileOutcome>> =
            (0..request.resource.files.len()).map(|_| None).collect();
        let mut first_error = None;
        let mut cancelled = false;
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok((index, outcome))) => outcomes[index] = Some(outcome),
                Ok(Err(error)) => {
                    if error.is_cancelled() {
                        cancelled = true;
                    } else {
                        first_error.get_or_insert(error);
                    }
                    cancel.cancel();
                }
                Err(error) => {
                    first_error
                        .get_or_insert_with(|| Error::Other(format!("file job failed: {error}")));
                    cancel.cancel();
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if cancelled || request.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let outcomes = outcomes
            .into_iter()
            .map(|outcome| outcome.ok_or_else(|| Error::Other("missing file outcome".to_string())))
            .collect::<Result<Vec<_>>>()?;
        Ok(DownloadOutcome::Completed(outcomes))
    }
}

struct FileCheckpoint {
    shared: Arc<Mutex<TaskResume>>,
    inner: Arc<dyn ResumeCheckpoint>,
}

#[async_trait]
impl ResumeCheckpoint for FileCheckpoint {
    async fn save(&self, resume: &TaskResume) -> Result<()> {
        let mut shared = self.shared.lock().await;
        for (key, file) in &resume.files {
            shared.files.insert(key.clone(), file.clone());
        }
        self.inner.save(&shared).await
    }
}

fn hub_relative_path(repo_name: &str, file: &ResourceFile) -> Result<String> {
    validate_relative_path(&file.path)
        .map_err(|_| Error::Backend(format!("unsafe Hub file path: {:?}", file.path)))?;
    Ok(format!("{repo_name}/{}", file.path))
}

fn warning(request: &DownloadRequest, message: String) {
    request.events.emit(EventKind::Warning {
        task_id: Some(request.task_id),
        message,
    });
}
