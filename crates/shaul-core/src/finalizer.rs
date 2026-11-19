use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;

use crate::archive::ExtractorRegistry;
use crate::backend::FileOutcome;
use crate::cache::ContentCache;
use crate::checksum::compute_file_checksum;
use crate::config::ResolvedOptions;
use crate::error::{Error, Result};
use crate::event::{EventKind, EventSink};
use crate::task::{TaskId, VerifyMode};
use crate::telemetry::metrics;

/// Outcome accumulated by a finalizer or by a whole pipeline.
#[derive(Clone, Debug, Default)]
pub struct FinalizeResult {
    /// Whether the file's checksum was verified.
    pub checksum_verified: bool,
    /// Directory the archive was extracted into, if extraction ran.
    pub extracted_to: Option<PathBuf>,
}

/// Read-only inputs available to each finalizer.
pub struct FinalizeContext<'a> {
    /// Task that produced the file.
    pub task_id: TaskId,
    /// Directory the task was asked to download into.
    pub destination: &'a Path,
    /// The completed file and how it was written.
    pub file: &'a FileOutcome,
    /// Resolved options controlling verification and extraction.
    pub options: &'a ResolvedOptions,
    /// Sink for verification, extraction and warning events.
    pub events: &'a EventSink,
}

/// A single post-download step, run once all bytes have been written.
#[async_trait]
pub trait Finalizer: Send + Sync {
    /// Stable name used in diagnostics.
    fn name(&self) -> &'static str;
    /// Runs this step for the completed file.
    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult>;
}

/// Fixed order: verify before extraction. A corrupt archive is never
/// unpacked.
pub struct FinalizerPipeline {
    finalizers: Vec<Arc<dyn Finalizer>>,
}

impl Default for FinalizerPipeline {
    fn default() -> Self {
        Self {
            finalizers: vec![Arc::new(VerifyFinalizer), Arc::new(ExtractFinalizer)],
        }
    }
}

impl FinalizerPipeline {
    /// Builds a pipeline that runs the given finalizers in order.
    pub fn new(finalizers: Vec<Arc<dyn Finalizer>>) -> Self {
        Self { finalizers }
    }

    /// Builds the default verification step plus extraction through a custom
    /// [`ExtractorRegistry`].
    pub fn with_extractor_registry(registry: Arc<ExtractorRegistry>) -> Self {
        Self::new(vec![
            Arc::new(VerifyFinalizer),
            Arc::new(ExtractorFinalizer::new(registry)),
        ])
    }

    /// Runs every finalizer in order and merges their results, stopping at the
    /// first error.
    pub async fn run(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        let mut result = FinalizeResult::default();
        for finalizer in &self.finalizers {
            let step = finalizer.finalize(context).await?;
            result.checksum_verified |= step.checksum_verified;
            if step.extracted_to.is_some() {
                result.extracted_to = step.extracted_to;
            }
        }
        Ok(result)
    }
}

/// Verifies the downloaded file against the expected checksum, honoring
/// [`VerifyMode`].
pub struct VerifyFinalizer;

#[async_trait]
impl Finalizer for VerifyFinalizer {
    fn name(&self) -> &'static str {
        "verify"
    }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        if context.file.checksum_verified {
            return Ok(FinalizeResult {
                checksum_verified: true,
                extracted_to: None,
            });
        }
        let expected = context
            .options
            .checksum
            .clone()
            .or_else(|| context.file.checksum.clone());

        match (context.options.verify, expected) {
            (VerifyMode::Disabled, _) => Ok(FinalizeResult::default()),
            (VerifyMode::Require, None) => Err(Error::ChecksumMissing),
            (VerifyMode::Auto, None) => Ok(FinalizeResult::default()),
            (_, Some(expected)) => {
                context.events.emit(EventKind::VerificationStarted {
                    task_id: context.task_id,
                    file: context.file.relative_path.clone(),
                    algorithm: expected.algorithm.to_string(),
                    expected: expected.hex.clone(),
                });
                let actual =
                    compute_file_checksum(context.file.path.clone(), expected.algorithm).await?;
                let ok = expected.matches(&actual);
                context.events.emit(EventKind::VerificationCompleted {
                    task_id: context.task_id,
                    file: context.file.relative_path.clone(),
                    ok,
                    actual: actual.hex.clone(),
                });
                if !ok {
                    return Err(Error::Verification {
                        expected: expected.hex,
                        actual: actual.hex,
                    });
                }
                cache_verified_file(context, &expected).await;
                Ok(FinalizeResult {
                    checksum_verified: true,
                    extracted_to: None,
                })
            }
        }
    }
}

async fn cache_verified_file(context: &FinalizeContext<'_>, checksum: &crate::checksum::Checksum) {
    if !context.options.content_cache {
        return;
    }
    let cache = match ContentCache::new(&context.options.content_cache_dir) {
        Ok(cache) => cache,
        Err(error) => {
            context.events.emit(EventKind::Warning {
                task_id: Some(context.task_id),
                message: format!("could not open content cache: {error}"),
            });
            return;
        }
    };
    match cache
        .insert(checksum, &context.file.path, context.file.size)
        .await
    {
        Ok(_) => context
            .events
            .telemetry()
            .counter(metrics::CACHE_INSERTS, 1, &[]),
        Err(error) => context.events.emit(EventKind::Warning {
            task_id: Some(context.task_id),
            message: format!("could not populate content cache: {error}"),
        }),
    }
}

/// Extracts supported archives according to the resolved extraction options
/// using the built-in [`ExtractorRegistry::default`] formats.
pub struct ExtractFinalizer;

#[async_trait]
impl Finalizer for ExtractFinalizer {
    fn name(&self) -> &'static str {
        "extract"
    }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        static REGISTRY: OnceLock<Arc<ExtractorRegistry>> = OnceLock::new();
        let registry = REGISTRY
            .get_or_init(|| Arc::new(ExtractorRegistry::default()))
            .clone();
        run_extraction(context, registry).await
    }
}

/// Extracts archives with a caller-supplied [`ExtractorRegistry`].
pub struct ExtractorFinalizer {
    registry: Arc<ExtractorRegistry>,
}

impl ExtractorFinalizer {
    /// Creates a finalizer that chooses extractors from `registry`.
    pub fn new(registry: Arc<ExtractorRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Finalizer for ExtractorFinalizer {
    fn name(&self) -> &'static str {
        "extract"
    }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        run_extraction(context, self.registry.clone()).await
    }
}

async fn run_extraction(
    context: &FinalizeContext<'_>,
    registry: Arc<ExtractorRegistry>,
) -> Result<FinalizeResult> {
    if !context.options.extract {
        return Ok(FinalizeResult::default());
    }
    if !registry.supports(&context.file.path)? {
        context.events.emit(EventKind::Warning {
            task_id: Some(context.task_id),
            message: format!(
                "extraction requested but {} is not a supported archive",
                context.file.path.display()
            ),
        });
        return Ok(FinalizeResult::default());
    }

    let extract_root = context
        .options
        .extract_dir
        .clone()
        .unwrap_or_else(|| context.destination.to_path_buf());
    context.events.emit(EventKind::ExtractionStarted {
        task_id: context.task_id,
        file: context.file.relative_path.clone(),
        target: extract_root.clone(),
    });

    let archive_path = context.file.path.clone();
    let extract_root_for_task = extract_root.clone();
    let overwrite = context.options.overwrite;
    let report = tokio::task::spawn_blocking(move || {
        registry.extract(&archive_path, &extract_root_for_task, overwrite)
    })
    .await
    .map_err(|error| Error::Extraction(format!("extract task failed: {error}")))??;

    let Some(report) = report else {
        context.events.emit(EventKind::Warning {
            task_id: Some(context.task_id),
            message: format!(
                "extraction requested but {} is no longer supported",
                context.file.path.display()
            ),
        });
        return Ok(FinalizeResult::default());
    };

    for warning in &report.warnings {
        context.events.emit(EventKind::Warning {
            task_id: Some(context.task_id),
            message: warning.clone(),
        });
    }

    if context.options.delete_archive
        && let Err(error) = tokio::fs::remove_file(&context.file.path).await
    {
        context.events.emit(EventKind::Warning {
            task_id: Some(context.task_id),
            message: format!(
                "could not delete archive {}: {error}",
                context.file.path.display()
            ),
        });
    }

    context.events.emit(EventKind::ExtractionCompleted {
        task_id: context.task_id,
        file: context.file.relative_path.clone(),
        target: report.target.clone(),
        files_extracted: report.files_extracted,
    });

    Ok(FinalizeResult {
        checksum_verified: false,
        extracted_to: Some(report.target),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{ArchiveExtractor, ExtractReport, ExtractorRequest};
    use crate::config::Config;
    use crate::task::TaskOptions;

    struct DemoExtractor;

    impl ArchiveExtractor for DemoExtractor {
        fn name(&self) -> &'static str {
            "demo"
        }

        fn supports(&self, path: &Path) -> Result<bool> {
            Ok(path.extension().and_then(|extension| extension.to_str()) == Some("demo"))
        }

        fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
            let target = request.extract_root.join("demo-out");
            std::fs::create_dir_all(&target)?;
            std::fs::write(target.join("hello.txt"), b"hello")?;
            Ok(ExtractReport {
                target,
                files_extracted: 1,
                warnings: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn extractor_finalizer_uses_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("payload.demo");
        std::fs::write(&archive, b"payload").unwrap();
        let destination = dir.path().join("dest");
        std::fs::create_dir_all(&destination).unwrap();

        let task_options = TaskOptions {
            extract: Some(true),
            ..TaskOptions::default()
        };
        let options = Config::default()
            .resolve(&destination, &task_options)
            .unwrap();
        let (events, _receiver) = EventSink::channel(16);
        let file = FileOutcome::new(archive, "payload.demo".to_string(), 7, None, false);
        let context = FinalizeContext {
            task_id: uuid::Uuid::new_v4(),
            destination: &destination,
            file: &file,
            options: &options,
            events: &events,
        };

        let registry = ExtractorRegistry::default().with(Arc::new(DemoExtractor));
        let result = ExtractorFinalizer::new(Arc::new(registry))
            .finalize(&context)
            .await
            .unwrap();

        let extracted = result.extracted_to.expect("extracted path");
        assert_eq!(extracted, destination.join("demo-out"));
        assert_eq!(
            std::fs::read_to_string(extracted.join("hello.txt")).unwrap(),
            "hello"
        );
    }
}
