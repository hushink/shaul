use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::archive::{detect_archive, extract_archive};
use crate::backend::FileOutcome;
use crate::checksum::compute_file_checksum;
use crate::config::ResolvedOptions;
use crate::error::{Error, Result};
use crate::event::{EventKind, EventSink};
use crate::task::{TaskId, VerifyMode};

#[derive(Clone, Debug, Default)]
pub struct FinalizeResult {
    pub checksum_verified: bool,
    pub extracted_to: Option<PathBuf>,
}

pub struct FinalizeContext<'a> {
    pub task_id: TaskId,
    pub destination: &'a Path,
    pub file: &'a FileOutcome,
    pub options: &'a ResolvedOptions,
    pub events: &'a EventSink,
}

#[async_trait]
pub trait Finalizer: Send + Sync {
    fn name(&self) -> &'static str;
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
    pub fn new(finalizers: Vec<Arc<dyn Finalizer>>) -> Self {
        Self { finalizers }
    }

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
                Ok(FinalizeResult {
                    checksum_verified: true,
                    extracted_to: None,
                })
            }
        }
    }
}

pub struct ExtractFinalizer;

#[async_trait]
impl Finalizer for ExtractFinalizer {
    fn name(&self) -> &'static str {
        "extract"
    }

    async fn finalize(&self, context: &FinalizeContext<'_>) -> Result<FinalizeResult> {
        if !context.options.extract {
            return Ok(FinalizeResult::default());
        }
        let Some(kind) = detect_archive(&context.file.path)? else {
            context.events.emit(EventKind::Warning {
                task_id: Some(context.task_id),
                message: format!(
                    "extraction requested but {} is not a supported archive",
                    context.file.path.display()
                ),
            });
            return Ok(FinalizeResult::default());
        };

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
            extract_archive(&archive_path, kind, &extract_root_for_task, overwrite)
        })
        .await
        .map_err(|error| Error::Extraction(format!("extract task failed: {error}")))??;

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
}
