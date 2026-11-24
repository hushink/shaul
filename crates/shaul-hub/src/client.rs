use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use directories::BaseDirs;
use reqwest::header::{ACCEPT, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Client, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use shaul_core::{
    Auth, Checksum, ChecksumAlgorithm, Config, Error, Header, HubProvider, HubRepoType,
    ProxySetting, RequestOptions, Result, expand_env, sanitize_file_name, validate_relative_path,
};
use url::Url;

/// A file discovered through a provider's metadata API.
#[derive(Clone, Debug)]
pub struct HubEntry {
    /// Repository-relative path.
    pub path: String,
    /// File size reported by the provider, when available.
    pub size: Option<u64>,
    /// Provider checksum (for example an LFS SHA-256), when available.
    pub checksum: Option<Checksum>,
    /// Cache hints such as `hub.blob`.
    pub metadata: BTreeMap<String, String>,
}

/// HTTP client, endpoint and cache configuration shared by Hub parsers and
/// the Hub backend.
#[derive(Clone)]
pub struct HubClient {
    client: Client,
    hf_endpoint: Url,
    ms_endpoint: Url,
    hf_cache: PathBuf,
    ms_cache: PathBuf,
    global_headers: Vec<Header>,
    user_agent: String,
    timeout: Duration,
}

impl HubClient {
    /// Build a client from global configuration.
    pub fn new(config: &Config) -> Result<Self> {
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(
                config.network.connect_timeout_secs.max(1),
            ))
            .user_agent(config.network.user_agent.clone());
        if let Some(proxy) = proxy_from_config(config)? {
            builder = builder.proxy(proxy);
        } else if matches!(config.proxy_setting(), ProxySetting::Disabled) {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|error| Error::Backend(format!("failed to build Hub client: {error}")))?;
        Ok(Self {
            client,
            hf_endpoint: parse_endpoint(&config.hf_endpoint())?,
            ms_endpoint: parse_endpoint(&config.modelscope_endpoint())?,
            hf_cache: config.hf_cache_dir()?,
            ms_cache: config.modelscope_cache_dir()?,
            global_headers: config.header_defaults()?,
            user_agent: config.network.user_agent.clone(),
            timeout: Duration::from_secs(config.network.timeout_secs.max(1)),
        })
    }

    /// Configured Hugging Face endpoint.
    pub fn hf_endpoint(&self) -> &Url {
        &self.hf_endpoint
    }

    /// Configured ModelScope endpoint.
    pub fn ms_endpoint(&self) -> &Url {
        &self.ms_endpoint
    }

    /// Resolved Hugging Face cache root.
    pub fn hf_cache(&self) -> &std::path::Path {
        &self.hf_cache
    }

    /// Resolved ModelScope cache root.
    pub fn ms_cache(&self) -> &std::path::Path {
        &self.ms_cache
    }

    /// Global headers plus task overrides, without provider credentials.
    /// This is what gets persisted in a task sidecar.
    pub fn base_request_options(&self, request: &RequestOptions) -> Result<RequestOptions> {
        Ok(request.merged(&self.global_headers))
    }

    /// Merge config/task request overrides with provider credentials. User
    /// auth always wins; provider tokens are only added when none is present.
    /// Provider tokens are read at call time and are never persisted.
    pub fn request_options(
        &self,
        provider: HubProvider,
        request: &RequestOptions,
    ) -> Result<RequestOptions> {
        let mut options = self.base_request_options(request)?;
        let has_authorization = options
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("authorization"));
        if has_authorization || options.auth.is_some() {
            return Ok(options);
        }
        match provider {
            HubProvider::HuggingFace => {
                if let Some(token) = hf_token() {
                    options.auth = Some(Auth::bearer(token));
                }
            }
            HubProvider::ModelScope => {
                let credentials = modelscope_credentials();
                if let Some(token) = credentials.token {
                    options.auth = Some(Auth::bearer(token));
                }
                if let Some(cookie) = credentials.cookie {
                    let has_cookie = options
                        .headers
                        .iter()
                        .any(|header| header.name.eq_ignore_ascii_case("cookie"));
                    if !has_cookie {
                        options
                            .headers
                            .push(Header::new("Cookie", format!("m_session_id={cookie}"))?);
                    }
                }
            }
            _ => {}
        }
        Ok(options)
    }

    /// Build a Hugging Face `/resolve` URL for a repository file.
    pub fn hf_resolve_url(
        &self,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        path: &str,
    ) -> Result<Url> {
        let mut url = self.hf_endpoint.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::Backend("invalid Hugging Face endpoint".to_string()))?;
            match repo_type {
                HubRepoType::Model => {}
                HubRepoType::Dataset => {
                    segments.push("datasets");
                }
                _ => {
                    return Err(Error::Backend(
                        "unsupported Hugging Face repository type".to_string(),
                    ));
                }
            }
            push_repo(&mut segments, repo);
            segments.push("resolve");
            segments.push(revision);
            push_path(&mut segments, path);
        }
        Ok(url)
    }

    /// Build a ModelScope file download URL.
    pub fn modelscope_download_url(
        &self,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        path: &str,
    ) -> Result<Url> {
        let mut url = self.ms_endpoint.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::Backend("invalid ModelScope endpoint".to_string()))?;
            segments.push("api");
            segments.push("v1");
            let repo_segment = match repo_type {
                HubRepoType::Model => "models",
                HubRepoType::Dataset => "datasets",
                _ => {
                    return Err(Error::Backend("unsupported repository type".to_string()));
                }
            };
            segments.push(repo_segment);
            push_repo(&mut segments, repo);
            segments.push("repo");
        }
        url.query_pairs_mut()
            .append_pair("Revision", revision)
            .append_pair("FilePath", path);
        Ok(url)
    }

    /// Cache directory for a provider repository.
    pub fn cache_repo_dir(
        &self,
        provider: HubProvider,
        repo: &str,
        repo_type: HubRepoType,
    ) -> PathBuf {
        let prefix = match repo_type {
            HubRepoType::Model => "models",
            HubRepoType::Dataset => "datasets",
            _ => "repositories",
        };
        let repo = repo
            .split('/')
            .map(sanitize_file_name)
            .collect::<Vec<_>>()
            .join("--");
        let root = match provider {
            HubProvider::HuggingFace => &self.hf_cache,
            HubProvider::ModelScope => &self.ms_cache,
            _ => &self.hf_cache,
        };
        root.join(format!("{prefix}--{repo}"))
    }

    /// List files in a Hugging Face repository revision.
    pub async fn hf_tree(
        &self,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        request: &RequestOptions,
    ) -> Result<Vec<HubEntry>> {
        let mut url = self.hf_endpoint.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::Backend("invalid Hugging Face endpoint".to_string()))?;
            segments.push("api");
            let repo_segment = match repo_type {
                HubRepoType::Model => "models",
                HubRepoType::Dataset => "datasets",
                _ => {
                    return Err(Error::Backend("unsupported repository type".to_string()));
                }
            };
            segments.push(repo_segment);
            push_repo(&mut segments, repo);
            segments.push("tree");
            segments.push(revision);
        }
        url.query_pairs_mut()
            .append_pair("recursive", "true")
            .append_pair("expand", "true");

        let response: Vec<HfEntry> = self.get_json(url, request).await?;
        let mut entries = Vec::new();
        for entry in response {
            if entry.kind.as_deref() != Some("file") {
                continue;
            }
            let path = match validated_hub_path(&entry.path)? {
                Some(path) => path,
                None => continue,
            };
            let mut metadata = BTreeMap::new();
            if let Some(oid) = &entry.oid {
                metadata.insert("hub.oid".to_string(), oid.clone());
            }
            if let Some(xet_hash) = &entry.xet_hash {
                metadata.insert("hub.xet_hash".to_string(), xet_hash.clone());
            }
            let (size, checksum, blob) = match entry.lfs {
                Some(lfs) => {
                    metadata.insert("hub.lfs".to_string(), "true".to_string());
                    metadata.insert("hub.blob".to_string(), lfs.oid.clone());
                    let checksum = Checksum::new(ChecksumAlgorithm::Sha256, &lfs.oid).ok();
                    (lfs.size.or(entry.size), checksum, Some(lfs.oid))
                }
                None => (entry.size, None, entry.oid.clone()),
            };
            if let Some(blob) = blob {
                metadata.entry("hub.blob".to_string()).or_insert(blob);
            }
            entries.push(HubEntry {
                path,
                size,
                checksum,
                metadata,
            });
        }
        Ok(entries)
    }

    /// List files in a ModelScope repository revision.
    pub async fn modelscope_files(
        &self,
        repo: &str,
        repo_type: HubRepoType,
        revision: &str,
        request: &RequestOptions,
    ) -> Result<Vec<HubEntry>> {
        let mut url = self.ms_endpoint.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| Error::Backend("invalid ModelScope endpoint".to_string()))?;
            segments.push("api");
            segments.push("v1");
            let repo_segment = match repo_type {
                HubRepoType::Model => "models",
                HubRepoType::Dataset => "datasets",
                _ => {
                    return Err(Error::Backend("unsupported repository type".to_string()));
                }
            };
            segments.push(repo_segment);
            push_repo(&mut segments, repo);
            segments.push("repo");
            segments.push("files");
        }
        url.query_pairs_mut()
            .append_pair("Revision", revision)
            .append_pair("Recursive", "True");

        let response: MsResponse = self.get_json(url, request).await?;
        if response.code.is_some_and(|code| code != 200) {
            return Err(Error::Backend(format!(
                "ModelScope API error: {}",
                response
                    .message
                    .unwrap_or_else(|| "unknown error".to_string())
            )));
        }
        let files = response.data.map(|data| data.files).unwrap_or_default();
        let mut entries = Vec::new();
        for file in files {
            if file.kind.as_deref() == Some("tree") {
                continue;
            }
            let Some(path) = validated_hub_path(&file.path)? else {
                continue;
            };
            let checksum = file
                .sha256
                .as_deref()
                .and_then(|value| Checksum::new(ChecksumAlgorithm::Sha256, value).ok());
            let mut metadata = BTreeMap::new();
            if let Some(sha256) = &file.sha256 {
                metadata.insert("hub.blob".to_string(), sha256.clone());
                metadata.insert("hub.sha256".to_string(), sha256.clone());
            }
            entries.push(HubEntry {
                path,
                size: file.size,
                checksum,
                metadata,
            });
        }
        Ok(entries)
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url, request: &RequestOptions) -> Result<T> {
        let response = self.send(url, request).await?;
        let status = response.status();
        if !status.is_success() {
            let error_code = response
                .headers()
                .get("x-error-code")
                .and_then(|value| value.to_str().ok())
                .map(|value| format!(" ({value})"))
                .unwrap_or_default();
            let body = tokio::time::timeout(self.timeout, response.text())
                .await
                .ok()
                .and_then(|result| result.ok())
                .unwrap_or_default();
            let snippet: String = body.chars().take(300).collect();
            return Err(Error::Backend(format!(
                "Hub API HTTP {status}{error_code}: {snippet}"
            )));
        }
        let text = tokio::time::timeout(self.timeout, response.text())
            .await
            .map_err(|_| Error::Backend("Hub API response timed out".to_string()))?
            .map_err(|error| {
                Error::Backend(format!("Hub API response: {}", error.without_url()))
            })?;
        serde_json::from_str(&text)
            .map_err(|error| Error::Backend(format!("Hub API returned invalid JSON: {error}")))
    }

    async fn send(&self, url: Url, request: &RequestOptions) -> Result<Response> {
        let mut builder = self.client.get(url);
        for header in &request.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|_| Error::Config(format!("invalid header name: {}", header.name)))?;
            let value = HeaderValue::from_str(&expand_env(&header.value)?)
                .map_err(|_| Error::Config(format!("invalid value for header {}", header.name)))?;
            builder = builder.header(name, value);
        }
        if let Some(auth) = &request.auth {
            builder = match auth {
                Auth::Bearer { token } => builder.bearer_auth(expand_env(token)?),
                Auth::Basic { username, password } => {
                    builder.basic_auth(expand_env(username)?, Some(expand_env(password)?))
                }
                Auth::Header(header) => {
                    let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|_| {
                        Error::Config(format!("invalid header name: {}", header.name))
                    })?;
                    let value =
                        HeaderValue::from_str(&expand_env(&header.value)?).map_err(|_| {
                            Error::Config(format!("invalid value for header {}", header.name))
                        })?;
                    builder.header(name, value)
                }
            };
        }
        builder = builder
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, &self.user_agent);
        tokio::time::timeout(self.timeout, builder.send())
            .await
            .map_err(|_| Error::Backend("Hub API request timed out".to_string()))?
            .map_err(|error| {
                Error::Backend(format!("Hub API request failed: {}", error.without_url()))
            })
    }
}

fn parse_endpoint(value: &str) -> Result<Url> {
    let url = Url::parse(value.trim())
        .map_err(|error| Error::Config(format!("invalid Hub endpoint {value}: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Config(format!(
            "Hub endpoint must be http(s): {value}"
        )));
    }
    Ok(url)
}

fn push_repo(segments: &mut url::PathSegmentsMut<'_>, repo: &str) {
    for part in repo.split('/') {
        if !part.is_empty() {
            segments.push(part);
        }
    }
}

fn push_path(segments: &mut url::PathSegmentsMut<'_>, path: &str) {
    for part in path.split('/') {
        if !part.is_empty() {
            segments.push(part);
        }
    }
}

/// Trim and validate a repository-relative path returned by provider metadata.
/// Returns `None` for paths that are empty after trimming.
fn validated_hub_path(raw: &str) -> Result<Option<String>> {
    let path = raw.trim_start_matches('/').to_string();
    if path.is_empty() {
        return Ok(None);
    }
    validate_relative_path(&path)
        .map_err(|_| Error::Backend(format!("unsafe Hub file path: {raw:?}")))?;
    Ok(Some(path))
}

#[derive(Deserialize)]
struct HfEntry {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    path: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    oid: Option<String>,
    #[serde(rename = "xetHash", default)]
    xet_hash: Option<String>,
    #[serde(default)]
    lfs: Option<HfLfs>,
}

#[derive(Deserialize)]
struct HfLfs {
    oid: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
struct MsResponse {
    #[serde(rename = "Code", default)]
    code: Option<i64>,
    #[serde(rename = "Data", default)]
    data: Option<MsData>,
    #[serde(rename = "Message", default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct MsData {
    #[serde(rename = "Files", default)]
    files: Vec<MsFile>,
}

#[derive(Deserialize)]
struct MsFile {
    #[serde(rename = "Path", default)]
    path: String,
    #[serde(rename = "Size", default)]
    size: Option<u64>,
    #[serde(rename = "Sha256", default)]
    sha256: Option<String>,
    #[serde(rename = "Type", default)]
    kind: Option<String>,
}

/// Whether a Hugging Face token is available from the environment or an
/// official credential file. Used by `shaul doctor`.
pub fn hf_token_available() -> bool {
    hf_token().is_some()
}

/// Whether ModelScope credentials (a token or a session cookie) are available
/// from the environment or the official credential files.
pub fn modelscope_credentials_available() -> bool {
    let credentials = modelscope_credentials();
    credentials.token.is_some() || credentials.cookie.is_some()
}

fn hf_token() -> Option<String> {
    for name in ["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    let mut paths = Vec::new();
    if let Ok(path) = std::env::var("HF_TOKEN_PATH")
        && !path.trim().is_empty()
    {
        paths.push(PathBuf::from(path));
    }
    if let Ok(home) = std::env::var("HF_HOME")
        && !home.trim().is_empty()
    {
        paths.push(PathBuf::from(home).join("token"));
    }
    if let Some(base) = BaseDirs::new() {
        paths.push(
            base.home_dir()
                .join(".cache")
                .join("huggingface")
                .join("token"),
        );
    }
    for path in paths {
        if let Ok(text) = std::fs::read_to_string(path) {
            let token = text.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

#[derive(Default)]
struct ModelScopeCredentials {
    token: Option<String>,
    cookie: Option<String>,
}

fn modelscope_credentials() -> ModelScopeCredentials {
    let mut credentials = ModelScopeCredentials::default();
    if let Ok(value) = std::env::var("MODELSCOPE_API_TOKEN") {
        let value = value.trim();
        if !value.is_empty() {
            credentials.token = Some(value.to_string());
        }
    }
    let Some(base) = BaseDirs::new() else {
        return credentials;
    };
    let path = base
        .home_dir()
        .join(".modelscope")
        .join("credentials")
        .join("cookies");
    let Ok(text) = std::fs::read_to_string(path) else {
        return credentials;
    };
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if credentials.token.is_none() {
            credentials.token = find_json_string(
                &value,
                &["token", "api_token", "access_token", "accessToken"],
            );
        }
        if credentials.cookie.is_none() {
            credentials.cookie = find_cookie(&value);
        }
        return credentials;
    }
    for line in text.lines() {
        if let Some(cookie) = parse_cookie_line(line) {
            credentials.cookie = Some(cookie);
            break;
        }
    }
    credentials
}

fn find_json_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for key in keys {
        if let Some(value) = object.get(*key).and_then(|value| value.as_str())
            && !value.trim().is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

fn find_cookie(value: &serde_json::Value) -> Option<String> {
    if let Some(cookie) = value
        .get("m_session_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        return Some(cookie.to_string());
    }
    let cookies = value.get("cookies")?;
    if let Some(cookie) = cookies
        .get("m_session_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    {
        return Some(cookie.to_string());
    }
    if let Some(array) = cookies.as_array() {
        for item in array {
            if item.get("name").and_then(|value| value.as_str()) == Some("m_session_id")
                && let Some(value) = item.get("value").and_then(|value| value.as_str())
                && !value.is_empty()
            {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn parse_cookie_line(line: &str) -> Option<String> {
    let index = line.find("m_session_id")?;
    let rest = &line[index + "m_session_id".len()..];
    let rest = rest.trim_start().trim_start_matches('"');
    let rest = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
    let value = rest
        .trim_start()
        .trim_start_matches('"')
        .split([';', '"', ','])
        .next()
        .unwrap_or("")
        .trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn proxy_from_config(config: &Config) -> Result<Option<reqwest::Proxy>> {
    let ProxySetting::Url(url) = config.proxy_setting() else {
        return Ok(None);
    };
    let mut proxy = reqwest::Proxy::all(&url)
        .map_err(|error| Error::Config(format!("invalid proxy URL: {error}")))?;
    if let Some(value) = config.network.no_proxy.as_deref()
        && let Some(no_proxy) = reqwest::NoProxy::from_string(value)
    {
        proxy = proxy.no_proxy(Some(no_proxy));
    }
    Ok(Some(proxy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cookie_line() {
        assert_eq!(
            parse_cookie_line("m_session_id=abc123; Path=/"),
            Some("abc123".to_string())
        );
        assert_eq!(
            parse_cookie_line("{\"m_session_id\": \"xyz\"}"),
            Some("xyz".to_string())
        );
    }

    #[test]
    fn parses_xet_hash_from_tree_entries() {
        let entry: HfEntry =
            serde_json::from_str(r#"{"type":"file","path":"a","size":1,"xetHash":"abc"}"#).unwrap();
        assert_eq!(entry.xet_hash.as_deref(), Some("abc"));
    }

    #[test]
    fn proxy_configuration_supports_disabling_ambient_proxies() {
        let mut config = Config::default();
        assert!(proxy_from_config(&config).unwrap().is_none());

        config.network.proxy = Some(String::new());
        assert_eq!(config.proxy_setting(), ProxySetting::Disabled);
        assert!(proxy_from_config(&config).unwrap().is_none());

        config.network.proxy = Some("http://127.0.0.1:8080".to_string());
        assert!(proxy_from_config(&config).unwrap().is_some());
    }

    #[test]
    fn validates_hub_paths() {
        assert_eq!(
            validated_hub_path("weights/model.bin").unwrap().as_deref(),
            Some("weights/model.bin")
        );
        assert_eq!(
            validated_hub_path("/weights/model.bin").unwrap().as_deref(),
            Some("weights/model.bin")
        );
        assert!(validated_hub_path("").unwrap().is_none());
        assert!(validated_hub_path("../escape.bin").is_err());
        assert!(validated_hub_path("..\\..\\escape.bin").is_err());
        assert!(validated_hub_path("C:/Windows/System32/evil.dll").is_err());
    }

    #[test]
    fn builds_cache_dirs() {
        let mut config = Config::default();
        config.hub.hf_cache = Some(PathBuf::from("cache"));
        let client = HubClient::new(&config).unwrap();
        assert_eq!(
            client.cache_repo_dir(HubProvider::HuggingFace, "org/repo", HubRepoType::Model),
            PathBuf::from("cache").join("models--org--repo")
        );
    }
}
