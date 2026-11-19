use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use shaul::{Config, ContentCache, Error, Result, SidecarStore, TaskState, TaskStore};

use crate::args::CleanArgs;

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

    checks.push(match &config.network.proxy {
        None => Check::new("proxy", CheckStatus::Ok, "not configured"),
        Some(proxy) => match validate_proxy(proxy) {
            Ok(()) => Check::new("proxy", CheckStatus::Ok, redact_proxy(proxy)),
            Err(error) => Check::new("proxy", CheckStatus::Error, error.to_string()),
        },
    });

    checks.push(token_check(
        "hf_token",
        &["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"],
    ));
    checks.push(token_check("modelscope_token", &["MODELSCOPE_API_TOKEN"]));

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
            *failures.entry(truncate(error, 120)).or_insert(0usize) += 1;
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
    if !remove_completed && !remove_cancelled && !remove_failed && !args.cache {
        return Err(Error::Config(
            "nothing selected; use --finished, --completed, --cancelled, --failed or --cache"
                .to_string(),
        ));
    }

    let store = SidecarStore::new(config.state_dir()?)?;
    let tasks = store.list().await?;
    let mut selected = Vec::new();
    for task in tasks {
        let remove = match task.state {
            TaskState::Completed => remove_completed,
            TaskState::Cancelled => remove_cancelled,
            TaskState::Failed => remove_failed,
            _ => false,
        };
        if remove {
            selected.push(task);
        }
    }

    let mut removed_tasks = 0usize;
    let mut removed_task_bytes = 0u64;
    for task in &selected {
        removed_tasks += 1;
        removed_task_bytes += task
            .total_bytes()
            .unwrap_or_else(|| task.downloaded_bytes());
        if !args.dry_run {
            store.delete(task.id).await?;
        }
    }

    let cache_dir = config.content_cache_dir()?;
    let cache = ContentCache::new(&cache_dir)?;
    let before = cache.stats().await?;
    let mut removed_cache_bytes = 0u64;
    if args.cache {
        removed_cache_bytes = if args.dry_run {
            before.bytes
        } else {
            cache.clear().await?
        };
    }

    let report = CleanReport {
        dry_run: args.dry_run,
        tasks: removed_tasks,
        task_bytes: removed_task_bytes,
        cache_entries: if args.cache { before.entries } else { 0 },
        cache_bytes: removed_cache_bytes,
        cache_temp_files: if args.cache { before.temp_files } else { 0 },
        cache_path: cache_dir.display().to_string(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.dry_run {
        println!(
            "would remove {} task(s) ({}), {} cache entr(ies) ({})",
            report.tasks,
            crate::support::human_bytes(report.task_bytes),
            report.cache_entries,
            crate::support::human_bytes(report.cache_bytes)
        );
    } else {
        println!(
            "removed {} task(s) ({}), {} cache entr(ies) ({})",
            report.tasks,
            crate::support::human_bytes(report.task_bytes),
            report.cache_entries,
            crate::support::human_bytes(report.cache_bytes)
        );
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
    cache_entries: u64,
    cache_bytes: u64,
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

fn token_check(name: &str, variables: &[&str]) -> Check {
    match variables
        .iter()
        .find(|variable| std::env::var_os(variable).is_some_and(|value| !value.is_empty()))
    {
        Some(variable) => Check::new(name, CheckStatus::Ok, format!("{variable} is set")),
        None => Check::new(
            name,
            CheckStatus::Warn,
            format!("{} is not set", variables.join(" / ")),
        ),
    }
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
