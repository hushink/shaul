use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use shaul::{Auth, Checksum, DownloadEngine, Error, Header, Link, Result, TaskId};
use uuid::Uuid;

pub fn parse_headers(values: &[String]) -> Result<Vec<Header>> {
    values
        .iter()
        .map(|value| {
            let (name, header_value) = value.split_once(':').ok_or_else(|| {
                Error::Config(format!(
                    "invalid header (expected \"Name: value\"): {value}"
                ))
            })?;
            Header::new(name, header_value)
        })
        .collect()
}

pub fn read_header_file(path: &Path) -> Result<Vec<Header>> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut headers = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (name, value) = trimmed.split_once(':').ok_or_else(|| {
            Error::Config(format!("invalid header in {}: {trimmed}", path.display()))
        })?;
        headers.push(Header::new(name, value)?);
    }
    Ok(headers)
}

pub fn parse_basic_auth(value: &str) -> Result<Auth> {
    let (username, password) = value
        .split_once(':')
        .ok_or_else(|| Error::Config("--basic expects user:password".to_string()))?;
    Ok(Auth::basic(username, password))
}

pub fn parse_checksum(value: &Option<String>, file: &Option<PathBuf>) -> Result<Option<Checksum>> {
    match (value, file) {
        (Some(_), Some(_)) => Err(Error::Config(
            "use either --checksum or --checksum-file, not both".to_string(),
        )),
        (Some(value), None) => Ok(Some(Checksum::parse(value)?)),
        (None, Some(path)) => {
            let text = std::fs::read_to_string(path)?;
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let token = line.split_whitespace().next().unwrap_or(line);
                return Ok(Some(Checksum::parse(token)?));
            }
            Err(Error::Config(format!(
                "no checksum found in {}",
                path.display()
            )))
        }
        (None, None) => Ok(None),
    }
}

pub async fn resolve_task_id(engine: &DownloadEngine, value: &str) -> Result<TaskId> {
    if let Ok(id) = Uuid::parse_str(value) {
        return Ok(id);
    }
    let tasks = engine.list().await?;
    let matches: Vec<_> = tasks
        .into_iter()
        .filter(|task| {
            let full = task.id.to_string();
            let compact = full.replace('-', "");
            full.starts_with(value) || compact.starts_with(value) || compact.ends_with(value)
        })
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id),
        0 => Err(Error::TaskNotFound(value.to_string())),
        _ => Err(Error::Other(format!(
            "task id prefix is ambiguous: {value}"
        ))),
    }
}

pub fn acquire_run_lock(state_dir: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(state_dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(state_dir.join("run.lock"))?;
    file.try_lock_exclusive()
        .map_err(|_| Error::Other("another `shaul run` worker is already active".to_string()))?;
    Ok(file)
}

/// UUIDv7 values created in the same millisecond share their timestamp
/// prefix, so display the random suffix instead of the timestamp prefix.
pub fn short_id(id: &TaskId) -> String {
    let compact = id.to_string().replace('-', "");
    compact[compact.len().saturating_sub(8)..].to_string()
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn display_link(link: &Link) -> String {
    redact_link_text(link.as_str())
}

/// Signed URLs commonly carry tokens in the query string. Keep magnet links
/// intact (their query holds the info hash) and strip HTTP query/fragment.
pub fn redact_link_text(raw: &str) -> String {
    if raw.starts_with("magnet:") {
        return raw.to_string();
    }
    raw.split(['?', '#']).next().unwrap_or(raw).to_string()
}

pub fn redacted_task(task: &shaul::Task) -> shaul::Task {
    let mut task = task.clone();
    if let Ok(link) = Link::new(redact_link_text(task.link.as_str())) {
        task.link = link;
    }
    redact_request(&mut task.options.request);
    for file in &mut task.files {
        if let Some(url) = &mut file.url
            && matches!(url.scheme(), "http" | "https")
        {
            url.set_query(None);
            url.set_fragment(None);
        }
    }
    if let Some(resource) = &mut task.resource {
        redact_request(&mut resource.request);
        for value in resource.metadata.values_mut() {
            if value.contains("://") {
                *value = redact_link_text(value);
            }
        }
        for file in &mut resource.files {
            if matches!(file.url.scheme(), "http" | "https") {
                file.url.set_query(None);
                file.url.set_fragment(None);
            }
            for header in &mut file.headers {
                if header.is_sensitive() {
                    header.value = "<redacted>".to_string();
                }
            }
            for value in file.metadata.values_mut() {
                if value.contains("://") {
                    *value = redact_link_text(value);
                }
            }
        }
    }
    if let Some(resume) = &mut task.resume {
        for file in resume.files.values_mut() {
            file.url = redact_link_text(&file.url);
            for (key, value) in &mut file.metadata {
                if key.ends_with("url") || key.contains("cas") {
                    *value = redact_link_text(value);
                }
            }
        }
    }
    task
}

fn redact_request(request: &mut shaul::RequestOptions) {
    if request.auth.is_some() {
        request.auth = Some(Auth::Bearer {
            token: "<redacted>".to_string(),
        });
    }
    for header in &mut request.headers {
        if header.is_sensitive() {
            header.value = "<redacted>".to_string();
        }
    }
}
