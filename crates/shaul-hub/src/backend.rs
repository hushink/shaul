use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use shaul_core::{
    BackendCapabilities, Config, DownloadBackend, DownloadOutcome, DownloadRequest, Error,
    EventKind, FileOutcome, HubProvider, HubRepoType, Resource, ResourceFile, ResourceKind, Result,
    ResumeCheckpoint, TaskResume, compute_file_checksum, sanitize_file_name,
};
use shaul_http::HttpBackend;
use tokio::sync::Mutex;

use crate::cache;
use crate::client::HubClient;

/// Backend for Hugging Face and ModelScope repository snapshots.
#[derive(Clone)]
pub struct HubBackend {
    client: Arc<HubClient>,
    http: HttpBackend,
}

impl HubBackend {
    /// Build a Hub backend, reusing the shared segmented HTTP backend for file
    /// transfers.
    pub fn new(client: Arc<HubClient>, config: &Config) -> Result<Self> {
        Ok(Self {
            client,
            http: HttpBackend::new(config)?,
        })
    }

    /// Build a Hub backend around an existing HTTP backend, so direct and Hub
    /// transfers share connection and bandwidth limits.
    pub fn with_http(client: Arc<HubClient>, http: HttpBackend) -> Self {
        Self { client, http }
    }

    async fn download_one(
        &self,
        provider: HubProvider,
        request: &DownloadRequest,
        task_resume: &mut TaskResume,
        file: &ResourceFile,
        relative: &str,
    ) -> Result<FileOutcome> {
        // Provider credentials are resolved here, at run time, so they are
        // never written into the task sidecar.
        let effective_request = self
            .client
            .request_options(provider, &request.options.request)?;
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
            let relative = format!("{repo_name}/{}", file.path);
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
            let relative = format!("{repo_name}/{}", file.path);
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
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok((index, outcome))) => outcomes[index] = Some(outcome),
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
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

fn warning(request: &DownloadRequest, message: String) {
    request.events.emit(EventKind::Warning {
        task_id: Some(request.task_id),
        message,
    });
}
