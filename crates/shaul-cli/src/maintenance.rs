use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use shaul::{
    Config, ContentCache, Error, ProxySetting, Result, SidecarStore, TaskState, TaskStore,
    hf_token_available, modelscope_credentials_available, part_path, validate_relative_path,
};

use crate::args::CleanArgs;

const HLS_TEMP_DIR_KEY: &str = "hls.temp_dir";
const HLS_TEMP_SUFFIX: &str = ".shaul.hls.parts";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CheckStatus {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Serialize)]
struct Check {
    name: String,
    status: CheckStatus,
    detail: String,
}

impl Check {
    fn new(name: impl Into<String>, status: CheckStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }
}

/// Check local configuration, directories, proxy settings and credentials.
pub async fn command_doctor(config: &Config, config_path: &Path, json: bool) -> Result<()> {
    let mut checks = Vec::new();

    checks.push(if config_path.exists() {
        Check::new("config", CheckStatus::Ok, config_path.display().to_string())
    } else {
        Check::new(
            "config",
            CheckStatus::Warn,
            format!(
                "{} does not exist; using defaults and environment",
                config_path.display()
            ),
        )
    });

    let state_dir = config.state_dir()?;
    checks.push(check_directory("state_dir", &state_dir).await);
    checks.push(check_directory("download_dir", &config.download.dir).await);
    let cache_dir = config.content_cache_dir()?;
    checks.push(check_directory("content_cache", &cache_dir).await);
    checks.push(match fs2::available_space(&config.download.dir) {
        Ok(available) => Check::new(
            "disk_space",
            CheckStatus::Ok,
            format!(
                "{} available ({} preflight)",
                crate::support::human_bytes(available),
                if config.download.disk_space_check {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
        ),
        Err(error) => Check::new(
            "disk_space",
            CheckStatus::Warn,
            format!("could not determine free space: {error}"),
        ),
    });

    checks.push(match config.proxy_setting() {
        ProxySetting::Ambient => Check::new(
            "proxy",
            CheckStatus::Ok,
            "not configured (environment proxy settings apply)",
        ),
        ProxySetting::Disabled => Check::new("proxy", CheckStatus::Ok, "disabled"),
        ProxySetting::Url(proxy) => match validate_proxy(&proxy) {
            Ok(()) => Check::new("proxy", CheckStatus::Ok, redact_proxy(&proxy)),
            Err(error) => Check::new("proxy", CheckStatus::Error, error.to_string()),
        },
        _ => Check::new("proxy", CheckStatus::Warn, "unknown proxy setting"),
    });

    checks.push(if hf_token_available() {
        Check::new(
            "hf_token",
            CheckStatus::Ok,
            "token available (environment or credential file)",
        )
    } else {
        Check::new(
            "hf_token",
            CheckStatus::Warn,
            "HF_TOKEN / HUGGING_FACE_HUB_TOKEN are not set and no credential file was found",
        )
    });
    checks.push(if modelscope_credentials_available() {
        Check::new(
            "modelscope_token",
            CheckStatus::Ok,
            "credentials available (environment or credential file)",
        )
    } else {
        Check::new(
            "modelscope_token",
            CheckStatus::Warn,
            "MODELSCOPE_API_TOKEN is not set and no credential file was found",
        )
    });

    let errors = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Error)
        .count();

    if json {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        for check in &checks {
            println!(
                "[{}] {}: {}",
                status_label(check.status),
                check.name,
                check.detail
            );
        }
        println!();
        if errors == 0 {
            println!("doctor: ok");
        } else {
            println!("doctor: {errors} problem(s) found");
        }
    }

    if errors == 0 {
        Ok(())
    } else {
        Err(Error::Other(format!("doctor found {errors} problem(s)")))
    }
}

/// Print task, byte and content-cache statistics.
pub async fn command_stats(config: &Config, json: bool) -> Result<()> {
    let store = SidecarStore::new(config.state_dir()?)?;
    let tasks = store.list().await?;
    let mut by_state = BTreeMap::new();
    let mut failures = BTreeMap::new();
    let mut known_bytes = 0u64;
    let mut downloaded_bytes = 0u64;
    for task in &tasks {
        *by_state
            .entry(state_label(&task.state).to_string())
            .or_insert(0usize) += 1;
        known_bytes += task
            .total_bytes()
            .unwrap_or_else(|| task.downloaded_bytes());
        downloaded_bytes += task.downloaded_bytes();
        if task.state == TaskState::Failed
            && let Some(error) = &task.error
        {
            *failures
                .entry(truncate(&crate::support::redact_message(error), 120))
                .or_insert(0usize) += 1;
        }
    }
    let cache_dir = config.content_cache_dir()?;
    let cache = ContentCache::new(&cache_dir)?;
    let cache_stats = cache.stats().await?;
    let pending = tasks
        .iter()
        .filter(|task| task.state == TaskState::Pending)
        .count();

    let report = StatsReport {
        tasks: TaskStats {
            total: tasks.len(),
            pending,
            by_state,
            known_bytes,
            downloaded_bytes,
        },
        cache: CacheReport {
            path: cache_dir.display().to_string(),
            entries: cache_stats.entries,
            bytes: cache_stats.bytes,
            temp_files: cache_stats.temp_files,
        },
        failures,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("tasks:       {}", report.tasks.total);
        println!("pending:     {}", report.tasks.pending);
        for (state, count) in &report.tasks.by_state {
            println!("  {state:<10} {count}");
        }
        println!(
            "bytes:       {} / {}",
            crate::support::human_bytes(report.tasks.downloaded_bytes),
            crate::support::human_bytes(report.tasks.known_bytes)
        );
        println!("cache:       {}", report.cache.path);
        println!(
            "  entries:   {} ({}), temp: {}",
            report.cache.entries,
            crate::support::human_bytes(report.cache.bytes),
            report.cache.temp_files
        );
        if !report.failures.is_empty() {
            println!("failures:");
            for (reason, count) in &report.failures {
                println!("  {count:>4}  {reason}");
            }
        }
    }
    Ok(())
}

/// Remove finished task sidecars and/or content-cache entries.
pub async fn command_clean(config: &Config, args: CleanArgs, json: bool) -> Result<()> {
    let remove_completed = args.completed || args.finished;
    let remove_cancelled = args.cancelled || args.finished;
    let remove_failed = args.failed || args.finished;
    if !remove_completed && !remove_cancelled && !remove_failed && !args.cache && !args.orphans {
        return Err(Error::Config(
            "nothing selected; use --finished, --completed, --cancelled, --failed, --cache or --orphans"
                .to_string(),
        ));
    }

    let store = SidecarStore::new(config.state_dir()?)?;
    let tasks = store.list().await?;
    let mut selected = Vec::new();
    for task in &tasks {
        let remove = match task.state {
            TaskState::Completed => remove_completed,
            TaskState::Cancelled => remove_cancelled,
            TaskState::Failed => remove_failed,
            _ => false,
        };
        if remove {
            selected.push(task.clone());
        }
    }

    let mut orphan_parts = BTreeSet::new();
    let mut orphan_dirs = BTreeSet::new();
    if args.orphans {
        for task in &tasks {
            if !matches!(task.state, TaskState::Completed | TaskState::Cancelled) {
                continue;
            }
            for file in &task.files {
                if let Some(path) = safe_orphan_part(&task.destination, &file.path) {
                    orphan_parts.insert(path);
                }
            }
            if let Some(resume) = &task.resume {
                for file in resume.files.values() {
                    if let Some(path) = safe_resume_part(&task.destination, &file.part_path) {
                        orphan_parts.insert(path);
                    }
                    if let Some(relative) = file.metadata.get(HLS_TEMP_DIR_KEY)
                        && let Some(path) = safe_hls_temp_dir(&task.destination, relative)
                    {
                        orphan_dirs.insert(path);
                    }
                }
            }
        }
    }

    let (removed_tasks, removed_task_bytes) =
        selected
            .iter()
            .fold((0usize, 0u64), |(count, bytes), task| {
                (
                    count + 1,
                    bytes
                        + task
                            .total_bytes()
                            .unwrap_or_else(|| task.downloaded_bytes()),
                )
            });
    if !args.dry_run {
        for task in &selected {
            store.delete(task.id).await?;
        }
    }

    let mut removed_orphan_parts = 0usize;
    let mut removed_orphan_bytes = 0u64;
    for path in orphan_parts {
        let Ok(metadata) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        removed_orphan_parts += 1;
        removed_orphan_bytes += metadata.len();
        if !args.dry_run {
            tokio::fs::remove_file(&path).await?;
        }
    }

    let mut removed_orphan_dirs = 0usize;
    let mut removed_orphan_dir_bytes = 0u64;
    for path in orphan_dirs {
        // `symlink_metadata` keeps a sidecar-planted symlink from being treated
        // as a temp directory and traversed.
        let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        removed_orphan_dirs += 1;
        removed_orphan_dir_bytes += directory_size(&path).await;
        if !args.dry_run {
            tokio::fs::remove_dir_all(&path).await?;
        }
    }

    let cache_dir = config.content_cache_dir()?;
    let cache = ContentCache::new(&cache_dir)?;
    let before = cache.stats().await?;
    let older_than = args
        .cache_older_than
        .as_deref()
        .map(parse_duration)
        .transpose()?;
    let max_bytes = args
        .cache_max_size
        .as_deref()
        .map(shaul::config::parse_bytes)
        .transpose()?;
    let mut removed_cache_entries = 0u64;
    let mut removed_cache_bytes = 0u64;
    let mut remaining_cache_entries = before.entries;
    let mut remaining_cache_bytes = before.bytes;
    if args.cache {
        if older_than.is_none() && max_bytes.is_none() {
            removed_cache_entries = before.entries;
            removed_cache_bytes = if args.dry_run {
                before.bytes
            } else {
                cache.clear().await?
            };
            remaining_cache_entries = 0;
            remaining_cache_bytes = 0;
        } else {
            let report = if args.dry_run {
                cache.prune_preview(older_than, max_bytes).await?
            } else {
                cache.prune(older_than, max_bytes).await?
            };
            removed_cache_entries = report.entries_removed;
            removed_cache_bytes = report.bytes_removed;
            remaining_cache_entries = report.entries_remaining;
            remaining_cache_bytes = report.bytes_remaining;
        }
    }

    let report = CleanReport {
        dry_run: args.dry_run,
        tasks: removed_tasks,
        task_bytes: removed_task_bytes,
        orphan_parts: removed_orphan_parts,
        orphan_bytes: removed_orphan_bytes,
        orphan_dirs: removed_orphan_dirs,
        orphan_dir_bytes: removed_orphan_dir_bytes,
        cache_entries: removed_cache_entries,
        cache_bytes: removed_cache_bytes,
        cache_entries_remaining: remaining_cache_entries,
        cache_bytes_remaining: remaining_cache_bytes,
        cache_temp_files: if args.cache { before.temp_files } else { 0 },
        cache_path: cache_dir.display().to_string(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if args.dry_run {
            "would remove"
        } else {
            "removed"
        };
        println!(
            "{verb} {} task(s) ({}), {} orphan part(s) ({}), {} orphan dir(s) ({}), {} cache entr(ies) ({})",
            report.tasks,
            crate::support::human_bytes(report.task_bytes),
            report.orphan_parts,
            crate::support::human_bytes(report.orphan_bytes),
            report.orphan_dirs,
            crate::support::human_bytes(report.orphan_dir_bytes),
            report.cache_entries,
            crate::support::human_bytes(report.cache_bytes)
        );
        if args.cache {
            println!(
                "cache remaining: {} entr(ies) ({})",
                report.cache_entries_remaining,
                crate::support::human_bytes(report.cache_bytes_remaining)
            );
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct StatsReport {
    tasks: TaskStats,
    cache: CacheReport,
    failures: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct TaskStats {
    total: usize,
    pending: usize,
    by_state: BTreeMap<String, usize>,
    known_bytes: u64,
    downloaded_bytes: u64,
}

#[derive(Debug, Serialize)]
struct CacheReport {
    path: String,
    entries: u64,
    bytes: u64,
    temp_files: u64,
}

#[derive(Debug, Serialize)]
struct CleanReport {
    dry_run: bool,
    tasks: usize,
    task_bytes: u64,
    orphan_parts: usize,
    orphan_bytes: u64,
    orphan_dirs: usize,
    orphan_dir_bytes: u64,
    cache_entries: u64,
    cache_bytes: u64,
    cache_entries_remaining: u64,
    cache_bytes_remaining: u64,
    cache_temp_files: u64,
    cache_path: String,
}

fn state_label(state: &TaskState) -> &'static str {
    match state {
        TaskState::Pending => "pending",
        TaskState::Running => "running",
        TaskState::Paused => "paused",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
        _ => "unknown",
    }
}

fn status_label(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "ok",
        CheckStatus::Warn => "warn",
        CheckStatus::Error => "error",
    }
}

async fn check_directory(name: &str, path: &Path) -> Check {
    if let Err(error) = tokio::fs::create_dir_all(path).await {
        return Check::new(
            name,
            CheckStatus::Error,
            format!("cannot create {}: {error}", path.display()),
        );
    }
    let probe = path.join(format!(".shaul-doctor-{}", uuid::Uuid::new_v4()));
    let result = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .await;
    match result {
        Ok(file) => {
            let _ = file.sync_all().await;
            drop(file);
            let _ = tokio::fs::remove_file(&probe).await;
            Check::new(name, CheckStatus::Ok, path.display().to_string())
        }
        Err(error) => Check::new(
            name,
            CheckStatus::Error,
            format!("{} is not writable: {error}", path.display()),
        ),
    }
}

fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim().to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = value.strip_suffix('w') {
        (number, 7 * 24 * 60 * 60)
    } else if let Some(number) = value.strip_suffix('d') {
        (number, 24 * 60 * 60)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 60 * 60)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1)
    } else {
        (value.as_str(), 1)
    };
    let number = number
        .trim()
        .parse::<f64>()
        .map_err(|_| Error::Config(format!("invalid duration: {value}")))?;
    if !number.is_finite() || number < 0.0 {
        return Err(Error::Config(format!("invalid duration: {value}")));
    }
    Ok(Duration::from_secs_f64(number * multiplier as f64))
}

fn validate_proxy(proxy: &str) -> Result<()> {
    let url = url::Url::parse(proxy)
        .map_err(|error| Error::Config(format!("invalid proxy URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(Error::Config(format!(
            "unsupported proxy scheme: {}",
            url.scheme()
        )));
    }
    Ok(())
}

fn redact_proxy(proxy: &str) -> String {
    if proxy.trim().is_empty() {
        return proxy.to_string();
    }
    let Ok(mut url) = url::Url::parse(proxy) else {
        return "<redacted>".to_string();
    };
    if url.username().is_empty() && url.password().is_none() {
        return proxy.to_string();
    }
    if !url.username().is_empty() {
        let _ = url.set_username("redacted");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("redacted"));
    }
    url.to_string()
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let mut result: String = value.chars().take(max.saturating_sub(3)).collect();
        result.push_str("...");
        result
    }
}

fn safe_orphan_part(destination: &Path, relative: &str) -> Option<PathBuf> {
    validate_relative_path(relative).ok()?;
    Some(part_path(&destination.join(relative)))
}

fn safe_resume_part(destination: &Path, relative: &str) -> Option<PathBuf> {
    validate_relative_path(relative).ok()?;
    let path = destination.join(relative);
    let name = path.file_name()?.to_str()?;
    name.ends_with(".shaul.part").then_some(path)
}

fn safe_hls_temp_dir(destination: &Path, relative: &str) -> Option<PathBuf> {
    validate_relative_path(relative).ok()?;
    let path = destination.join(relative);
    let name = path.file_name()?.to_str()?;
    name.ends_with(HLS_TEMP_SUFFIX).then_some(path)
}

async fn directory_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            // `file_type()` does not follow symlinks, so the walk cannot leave
            // the temp directory through a link.
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file()
                && let Ok(metadata) = entry.metadata().await
            {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphan_paths_must_be_relative_parts() {
        let destination = Path::new("task");
        assert!(safe_orphan_part(destination, "file.bin").is_some());
        assert!(safe_orphan_part(destination, "../escape.bin").is_none());
        assert!(safe_orphan_part(destination, "C:/Windows/file").is_none());

        assert!(safe_resume_part(destination, "file.bin.shaul.part").is_some());
        assert!(safe_resume_part(destination, "file.bin").is_none());
        assert!(safe_resume_part(destination, "../escape.shaul.part").is_none());
        assert!(safe_resume_part(destination, "C:/tmp/x.shaul.part").is_none());
    }

    #[test]
    fn hls_temp_dirs_must_be_relative_markers() {
        let destination = Path::new("task");
        assert!(safe_hls_temp_dir(destination, "video.ts.shaul.hls.parts").is_some());
        assert!(safe_hls_temp_dir(destination, "nested/video.ts.shaul.hls.parts").is_some());
        assert!(safe_hls_temp_dir(destination, "../video.ts.shaul.hls.parts").is_none());
        assert!(safe_hls_temp_dir(destination, "C:/video.ts.shaul.hls.parts").is_none());
        assert!(safe_hls_temp_dir(destination, "video.ts").is_none());
    }
}
