use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use shaul::{Auth, Checksum, DownloadEngine, Error, Header, Link, Result, TaskId, TaskState};
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

/// Parse a task state name used by `list` and batch commands.
pub fn parse_task_state(value: &str) -> Result<TaskState> {
    match value.trim().to_ascii_lowercase().as_str() {
        "pending" => Ok(TaskState::Pending),
        "running" => Ok(TaskState::Running),
        "paused" => Ok(TaskState::Paused),
        "completed" => Ok(TaskState::Completed),
        "cancelled" | "canceled" => Ok(TaskState::Cancelled),
        "failed" => Ok(TaskState::Failed),
        _ => Err(Error::Config(format!(
            "unknown task state: {value} (use pending, running, paused, completed, cancelled or failed)"
        ))),
    }
}

/// Validate an HTTP method token and reject methods that cannot return a
/// downloadable body.
pub fn validate_http_method(value: &str) -> Result<String> {
    let method = value.trim();
    if method.is_empty() {
        return Err(Error::Config("HTTP method must not be empty".to_string()));
    }
    if !method.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
    }) {
        return Err(Error::Config(format!("invalid HTTP method: {method}")));
    }
    if method.eq_ignore_ascii_case("HEAD") {
        return Err(Error::Config(
            "HEAD is not a download method; use GET or a method that returns a body".to_string(),
        ));
    }
    Ok(method.to_string())
}

pub fn collect_links(links: &[String], input_file: Option<&Path>) -> Result<Vec<String>> {
    let mut collected: Vec<String> = links
        .iter()
        .map(|link| link.trim())
        .filter(|link| !link.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(path) = input_file {
        let file = std::fs::File::open(path)?;
        for line in BufReader::new(file).lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            collected.push(trimmed.to_string());
        }
    }
    if collected.is_empty() {
        return Err(Error::Config("no links provided".to_string()));
    }
    Ok(collected)
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

/// Remove credentials, query strings and fragments from a URL in place.
pub fn redact_url(url: &mut url::Url) {
    if !url.username().is_empty() {
        let _ = url.set_username("redacted");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("redacted"));
    }
    url.set_query(None);
    url.set_fragment(None);
}

/// Redact every http(s)/ftp URL embedded in free-form text, such as a warning
/// or an error message.
pub fn redact_message(message: &str) -> String {
    const SCHEMES: &[&str] = &["http://", "https://", "ftp://", "socks5://", "socks5h://"];
    let mut output = String::with_capacity(message.len());
    let mut rest = message;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = SCHEMES.iter().filter_map(|scheme| lower.find(scheme)).min() else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, ')' | ']' | '}' | '>' | '"' | '\'' | ',' | ';')
            })
            .unwrap_or(tail.len());
        let candidate = &tail[..end];
        match url::Url::parse(candidate) {
            Ok(mut url) => {
                redact_url(&mut url);
                output.push_str(url.as_str());
            }
            Err(_) => {
                // Keep only the part before a query or fragment even when the
                // URL does not parse (for example, a trailing punctuation
                // character was not recognized as a delimiter).
                output.push_str(candidate.split(['?', '#']).next().unwrap_or(candidate));
            }
        }
        rest = &tail[end..];
    }
    output
}

/// Signed URLs commonly carry tokens in the query string. Keep magnet links
/// intact (their query holds the info hash) and strip HTTP query/fragment.
pub fn redact_link_text(raw: &str) -> String {
    if raw.starts_with("magnet:") {
        return raw.to_string();
    }
    let Ok(mut url) = url::Url::parse(raw) else {
        return raw.split(['?', '#']).next().unwrap_or(raw).to_string();
    };
    if url.username().is_empty() && url.password().is_none() {
        return raw.split(['?', '#']).next().unwrap_or(raw).to_string();
    }
    redact_url(&mut url);
    url.to_string()
}

pub fn redacted_task(task: &shaul::Task) -> shaul::Task {
    let mut task = task.clone();
    if let Ok(link) = Link::new(redact_link_text(task.link.as_str())) {
        task.link = link;
    }
    redact_request(&mut task.options.request);
    for mirror in &mut task.options.mirrors {
        redact_url(mirror);
    }
    if let Some(error) = &mut task.error {
        *error = redact_message(error);
    }
    for file in &mut task.files {
        if let Some(url) = &mut file.url {
            redact_url(url);
        }
    }
    if let Some(resource) = &mut task.resource {
        redact_resource(resource);
    }
    if let Some(resume) = &mut task.resume {
        for file in resume.files.values_mut() {
            file.url = redact_link_text(&file.url);
            for (key, value) in &mut file.metadata {
                if key.ends_with("url") || key.contains("cas") {
                    *value = redact_message(value);
                }
            }
        }
    }
    task
}

/// Clone a resource with request credentials and URL secrets removed.
pub fn redacted_resource(resource: &shaul::Resource) -> shaul::Resource {
    let mut resource = resource.clone();
    redact_resource(&mut resource);
    resource
}

fn redact_resource(resource: &mut shaul::Resource) {
    redact_request(&mut resource.request);
    for value in resource.metadata.values_mut() {
        if value.contains("://") {
            *value = redact_message(value);
        }
    }
    for file in &mut resource.files {
        redact_url(&mut file.url);
        for header in &mut file.headers {
            if header.is_sensitive() {
                header.value = "<redacted>".to_string();
            }
        }
        for value in file.metadata.values_mut() {
            if value.contains("://") {
                *value = redact_message(value);
            }
        }
    }
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
    if request.body.is_some() {
        request.body = Some("<redacted>".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_urls_inside_messages() {
        let message = "request failed for url (https://example.com/file?token=secret) and ftp://user:pass@example.com/x using socks5://user:socks-secret@127.0.0.1:1080";
        let redacted = redact_message(message);
        assert!(!redacted.contains("secret"), "{redacted}");
        assert!(!redacted.contains("pass"), "{redacted}");
        assert!(!redacted.contains("socks-secret"), "{redacted}");
        assert!(redacted.contains("https://example.com/file"), "{redacted}");
        assert!(redacted.contains("redacted"), "{redacted}");
        assert_eq!(redact_message("plain text"), "plain text");
    }

    #[test]
    fn redacted_resource_hides_credentials_and_url_secrets() {
        use std::collections::BTreeMap;

        use shaul::{Header, RequestOptions, Resource, ResourceFile, ResourceKind};

        let resource = Resource {
            kind: ResourceKind::Http,
            name: "file.bin".to_string(),
            files: vec![ResourceFile {
                path: "file.bin".to_string(),
                url: url::Url::parse(
                    "https://user:url-secret@example.com/file.bin?token=query-secret",
                )
                .unwrap(),
                size: None,
                checksum: None,
                headers: vec![Header::new("Authorization", "Bearer header-secret").unwrap()],
                metadata: BTreeMap::from([(
                    "source".to_string(),
                    "https://example.com/x?token=meta-secret".to_string(),
                )]),
                use_response_filename: true,
            }],
            request: RequestOptions {
                headers: vec![Header::new("Authorization", "Bearer request-secret").unwrap()],
                auth: Some(shaul::Auth::bearer("auth-secret")),
                allow_cross_origin_auth: false,
                body: Some("body-secret".to_string()),
                ..RequestOptions::default()
            },
            checksum: None,
            revision: None,
            metadata: BTreeMap::new(),
        };

        let text = serde_json::to_string(&redacted_resource(&resource)).unwrap();
        for secret in [
            "url-secret",
            "query-secret",
            "header-secret",
            "request-secret",
            "auth-secret",
            "meta-secret",
            "body-secret",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(text.contains("<redacted>"), "{text}");
    }

    #[test]
    fn redacted_task_hides_mirror_and_error_secrets() {
        use std::path::PathBuf;

        use shaul::{Link, Task, TaskOptions};

        let mut task = Task::new(
            Link::new("https://example.com/file?token=link-secret").unwrap(),
            PathBuf::from("."),
            TaskOptions {
                mirrors: vec![
                    url::Url::parse(
                        "https://user:mirror-secret@mirror.example/x?token=mirror-query",
                    )
                    .unwrap(),
                ],
                ..TaskOptions::default()
            },
        );
        task.error = Some("failed for url (https://example.com/x?token=error-secret)".to_string());

        let text = serde_json::to_string(&redacted_task(&task)).unwrap();
        for secret in [
            "link-secret",
            "mirror-secret",
            "mirror-query",
            "error-secret",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }

    #[test]
    fn parses_task_states() {
        assert_eq!(parse_task_state("paused").unwrap(), TaskState::Paused);
        assert_eq!(parse_task_state("CANCELLED").unwrap(), TaskState::Cancelled);
        assert_eq!(parse_task_state("canceled").unwrap(), TaskState::Cancelled);
        assert!(parse_task_state("unknown").is_err());
    }

    #[test]
    fn validates_http_methods() {
        assert_eq!(validate_http_method(" post ").unwrap(), "post");
        assert_eq!(validate_http_method("PURGE").unwrap(), "PURGE");
        assert!(validate_http_method("").is_err());
        assert!(validate_http_method("BAD METHOD").is_err());
        assert!(validate_http_method("HEAD").is_err());
    }
}
