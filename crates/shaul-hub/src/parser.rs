use std::sync::Arc;

use async_trait::async_trait;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use percent_encoding::percent_decode_str;
use shaul_core::{
    Error, HubProvider, HubRepoType, Link, LinkParser, ParseOptions, Resource, ResourceFile,
    ResourceKind, Result, sanitize_file_name,
};
use url::Url;

use crate::client::{HubClient, HubEntry};

#[derive(Clone, Debug)]
struct HubSpec {
    repo: String,
    repo_type: HubRepoType,
    revision: String,
    file: Option<String>,
    implicit_include: Option<String>,
}

pub struct HuggingFaceParser {
    client: Arc<HubClient>,
}

impl HuggingFaceParser {
    pub fn new(client: Arc<HubClient>) -> Self {
        Self { client }
    }
}

impl ModelScopeParser {
    pub fn new(client: Arc<HubClient>) -> Self {
        Self { client }
    }
}

pub struct ModelScopeParser {
    client: Arc<HubClient>,
}

#[async_trait]
impl LinkParser for HuggingFaceParser {
    fn name(&self) -> &'static str {
        "huggingface"
    }

    fn supports(&self, link: &Link) -> bool {
        parse_hf_link(link, None, self.client.hf_endpoint())
            .ok()
            .flatten()
            .is_some()
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let spec = parse_hf_link(link, options.revision.as_deref(), self.client.hf_endpoint())?
            .ok_or_else(|| Error::InvalidLink(format!("not a Hugging Face link: {link}")))?;
        let metadata_request = self
            .client
            .request_options(HubProvider::HuggingFace, &options.request)?;
        // The resource request is persisted in the task sidecar, so keep
        // provider tokens out of it; the backend adds them at run time.
        let request = self.client.base_request_options(&options.request)?;

        if let Some(path) = &spec.file {
            let url =
                self.client
                    .hf_resolve_url(&spec.repo, spec.repo_type, &spec.revision, path)?;
            return Ok(Resource {
                kind: ResourceKind::Hub {
                    provider: HubProvider::HuggingFace,
                    repo: spec.repo.clone(),
                    repo_type: spec.repo_type,
                    revision: spec.revision.clone(),
                },
                name: repo_name(&spec.repo),
                files: vec![ResourceFile {
                    path: path.clone(),
                    url,
                    size: None,
                    checksum: options.checksum.clone(),
                    headers: Vec::new(),
                    metadata: Default::default(),
                    use_response_filename: false,
                }],
                request,
                checksum: options.checksum.clone(),
                revision: Some(spec.revision),
                metadata: Default::default(),
            });
        }

        let entries = self
            .client
            .hf_tree(
                &spec.repo,
                spec.repo_type,
                &spec.revision,
                &metadata_request,
            )
            .await?;
        let mut include = options.include.clone();
        if let Some(pattern) = spec.implicit_include {
            include.push(pattern);
        }
        let entries = filter_entries(entries, &include, &options.exclude)?;
        if entries.is_empty() {
            return Err(Error::Backend(
                "no files matched the repository filters".to_string(),
            ));
        }
        if entries.len() > 1 && options.checksum.is_some() {
            return Err(Error::Config(
                "--checksum applies to a single file, not a repository snapshot".to_string(),
            ));
        }
        let files = entries
            .into_iter()
            .map(|entry| {
                let url = self.client.hf_resolve_url(
                    &spec.repo,
                    spec.repo_type,
                    &spec.revision,
                    &entry.path,
                )?;
                Ok(ResourceFile {
                    path: entry.path,
                    url,
                    size: entry.size,
                    checksum: entry.checksum,
                    headers: Vec::new(),
                    metadata: entry.metadata,
                    use_response_filename: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Resource {
            kind: ResourceKind::Hub {
                provider: HubProvider::HuggingFace,
                repo: spec.repo.clone(),
                repo_type: spec.repo_type,
                revision: spec.revision.clone(),
            },
            name: repo_name(&spec.repo),
            files,
            request,
            checksum: None,
            revision: Some(spec.revision),
            metadata: Default::default(),
        })
    }
}

#[async_trait]
impl LinkParser for ModelScopeParser {
    fn name(&self) -> &'static str {
        "modelscope"
    }

    fn supports(&self, link: &Link) -> bool {
        parse_ms_link(link, None, self.client.ms_endpoint())
            .ok()
            .flatten()
            .is_some()
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let spec = parse_ms_link(link, options.revision.as_deref(), self.client.ms_endpoint())?
            .ok_or_else(|| Error::InvalidLink(format!("not a ModelScope link: {link}")))?;
        let metadata_request = self
            .client
            .request_options(HubProvider::ModelScope, &options.request)?;
        let request = self.client.base_request_options(&options.request)?;
        let entries = self
            .client
            .modelscope_files(
                &spec.repo,
                spec.repo_type,
                &spec.revision,
                &metadata_request,
            )
            .await?;
        let entries = filter_entries(entries, &options.include, &options.exclude)?;
        if entries.is_empty() {
            return Err(Error::Backend(
                "no files matched the repository filters".to_string(),
            ));
        }
        if entries.len() > 1 && options.checksum.is_some() {
            return Err(Error::Config(
                "--checksum applies to a single file, not a repository snapshot".to_string(),
            ));
        }
        let files = entries
            .into_iter()
            .map(|entry| {
                let url = self.client.modelscope_download_url(
                    &spec.repo,
                    spec.repo_type,
                    &spec.revision,
                    &entry.path,
                )?;
                Ok(ResourceFile {
                    path: entry.path,
                    url,
                    size: entry.size,
                    checksum: entry.checksum,
                    headers: Vec::new(),
                    metadata: entry.metadata,
                    use_response_filename: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Resource {
            kind: ResourceKind::Hub {
                provider: HubProvider::ModelScope,
                repo: spec.repo.clone(),
                repo_type: spec.repo_type,
                revision: spec.revision.clone(),
            },
            name: repo_name(&spec.repo),
            files,
            request,
            checksum: None,
            revision: Some(spec.revision),
            metadata: Default::default(),
        })
    }
}

fn parse_hf_link(
    link: &Link,
    option_revision: Option<&str>,
    endpoint: &Url,
) -> Result<Option<HubSpec>> {
    let raw = link.as_str();
    if let Some(rest) = raw.strip_prefix("hf://") {
        let parts = decode_parts(rest);
        let (repo_type, parts) = match parts.split_first() {
            Some((first, rest)) if first.as_str() == "datasets" => (HubRepoType::Dataset, rest),
            Some((first, _)) if first.as_str() == "spaces" => return Ok(None),
            _ => (HubRepoType::Model, parts.as_slice()),
        };
        let repo = match parts {
            [single] => single.to_string(),
            [org, name] => format!("{org}/{name}"),
            _ => return Ok(None),
        };
        if !valid_repo(&repo) {
            return Ok(None);
        }
        return Ok(Some(HubSpec {
            repo,
            repo_type,
            revision: option_revision.unwrap_or("main").to_string(),
            file: None,
            implicit_include: None,
        }));
    }

    let Ok(url) = Url::parse(raw) else {
        return Ok(None);
    };
    if !matches!(url.scheme(), "http" | "https") || !is_hf_host(url.host_str(), endpoint) {
        return Ok(None);
    }
    let parts = decode_parts(url.path());
    let (repo_type, parts) = match parts.split_first() {
        Some((first, rest)) if first.as_str() == "datasets" => (HubRepoType::Dataset, rest),
        Some((first, _)) if first.as_str() == "spaces" => return Ok(None),
        _ => (HubRepoType::Model, parts.as_slice()),
    };
    if parts.is_empty() {
        return Ok(None);
    }
    // A repository id may be `org/name` or a legacy single-segment id. If the
    // second segment is a known action, the repo is single-segment.
    let (repo, suffix) =
        if parts.len() >= 2 && !matches!(parts[1].as_str(), "tree" | "blob" | "resolve" | "raw") {
            (format!("{}/{}", parts[0], parts[1]), &parts[2..])
        } else {
            (parts[0].clone(), &parts[1..])
        };
    if !valid_repo(&repo) {
        return Ok(None);
    }
    let mut revision = option_revision.unwrap_or("main").to_string();
    if let Some(value) = query_value(&url, &["revision", "rev"]) {
        revision = value;
    }
    let mut file = None;
    let mut implicit_include = None;
    match suffix {
        [] => {}
        [kind, rev] if kind.as_str() == "tree" => {
            revision = rev.clone();
        }
        [kind, rev, rest @ ..] if kind.as_str() == "tree" && !rest.is_empty() => {
            revision = rev.clone();
            implicit_include = Some(format!("{}/**", rest.join("/")));
        }
        [kind, rev, rest @ ..]
            if matches!(kind.as_str(), "blob" | "resolve" | "raw") && !rest.is_empty() =>
        {
            revision = rev.clone();
            file = Some(rest.join("/"));
        }
        _ => return Ok(None),
    }
    Ok(Some(HubSpec {
        repo,
        repo_type,
        revision,
        file,
        implicit_include,
    }))
}

fn parse_ms_link(
    link: &Link,
    option_revision: Option<&str>,
    endpoint: &Url,
) -> Result<Option<HubSpec>> {
    let raw = link.as_str();
    if let Some(rest) = raw.strip_prefix("ms://") {
        let parts = decode_parts(rest);
        let (repo_type, parts) = match parts.split_first() {
            Some((first, rest)) if first.as_str() == "datasets" => (HubRepoType::Dataset, rest),
            _ => (HubRepoType::Model, parts.as_slice()),
        };
        let repo = match parts {
            [single] => single.to_string(),
            [org, name] => format!("{org}/{name}"),
            _ => return Ok(None),
        };
        if !valid_repo(&repo) {
            return Ok(None);
        }
        return Ok(Some(HubSpec {
            repo,
            repo_type,
            revision: option_revision.unwrap_or("master").to_string(),
            file: None,
            implicit_include: None,
        }));
    }

    let Ok(url) = Url::parse(raw) else {
        return Ok(None);
    };
    if !matches!(url.scheme(), "http" | "https") || !is_ms_host(url.host_str(), endpoint) {
        return Ok(None);
    }
    let parts = decode_parts(url.path());
    let (repo_type, parts) = match parts.split_first() {
        Some((first, rest)) if first.as_str() == "datasets" => (HubRepoType::Dataset, rest),
        Some((first, rest)) if first.as_str() == "models" => (HubRepoType::Model, rest),
        _ => return Ok(None),
    };
    if parts.len() < 2 {
        return Ok(None);
    }
    let repo = format!("{}/{}", parts[0], parts[1]);
    if !valid_repo(&repo) {
        return Ok(None);
    }
    if parts.len() > 3 || (parts.len() == 3 && !matches!(parts[2].as_str(), "summary" | "files")) {
        return Ok(None);
    }
    let revision = query_value(&url, &["Revision", "revision"])
        .or_else(|| option_revision.map(str::to_string))
        .unwrap_or_else(|| "master".to_string());
    Ok(Some(HubSpec {
        repo,
        repo_type,
        revision,
        file: None,
        implicit_include: None,
    }))
}

fn decode_parts(value: &str) -> Vec<String> {
    value
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| percent_decode_str(part).decode_utf8_lossy().to_string())
        .collect()
}

fn query_value(url: &Url, keys: &[&str]) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| {
            keys.iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
        })
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

fn is_hf_host(host: Option<&str>, endpoint: &Url) -> bool {
    let Some(host) = host.map(|value| value.to_ascii_lowercase()) else {
        return false;
    };
    if matches!(host.as_str(), "huggingface.co" | "www.huggingface.co") {
        return true;
    }
    endpoint
        .host_str()
        .map(|value| value.eq_ignore_ascii_case(&host))
        .unwrap_or(false)
}

fn is_ms_host(host: Option<&str>, endpoint: &Url) -> bool {
    let Some(host) = host.map(|value| value.to_ascii_lowercase()) else {
        return false;
    };
    if matches!(host.as_str(), "modelscope.cn" | "www.modelscope.cn") {
        return true;
    }
    endpoint
        .host_str()
        .map(|value| value.eq_ignore_ascii_case(&host))
        .unwrap_or(false)
}

fn valid_repo(repo: &str) -> bool {
    !repo.is_empty()
        && repo.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains('\\')
                && !part.contains('\0')
        })
}

fn repo_name(repo: &str) -> String {
    repo.rsplit('/')
        .next()
        .map(sanitize_file_name)
        .unwrap_or_else(|| "repo".to_string())
}

fn filter_entries(
    entries: Vec<HubEntry>,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<HubEntry>> {
    let includes = build_globset(include)?;
    let excludes = build_globset(exclude)?;
    Ok(entries
        .into_iter()
        .filter(|entry| {
            (include.is_empty() || includes.is_match(&entry.path))
                && (exclude.is_empty() || !excludes.is_match(&entry.path))
        })
        .collect())
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|error| Error::Config(format!("invalid glob {pattern}: {error}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| Error::Config(format!("invalid glob set: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn parses_hf_repo_and_file_links() {
        let endpoint = endpoint("https://huggingface.co");
        let spec = parse_hf_link(
            &Link::new("https://huggingface.co/bert-base-uncased").unwrap(),
            None,
            &endpoint,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.repo, "bert-base-uncased");
        assert_eq!(spec.revision, "main");

        let spec = parse_hf_link(
            &Link::new("https://huggingface.co/org/repo/tree/dev/sub").unwrap(),
            None,
            &endpoint,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.revision, "dev");
        assert_eq!(spec.implicit_include.as_deref(), Some("sub/**"));

        let spec = parse_hf_link(
            &Link::new("https://huggingface.co/org/repo/resolve/main/model.bin").unwrap(),
            None,
            &endpoint,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.file.as_deref(), Some("model.bin"));
    }

    #[test]
    fn parses_hf_scheme_and_datasets() {
        let endpoint = endpoint("https://huggingface.co");
        let spec = parse_hf_link(
            &Link::new("hf://datasets/org/data").unwrap(),
            Some("v1"),
            &endpoint,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.repo, "org/data");
        assert_eq!(spec.repo_type, HubRepoType::Dataset);
        assert_eq!(spec.revision, "v1");
    }

    #[test]
    fn rejects_non_repo_pages() {
        let endpoint = endpoint("https://huggingface.co");
        assert!(
            parse_hf_link(
                &Link::new("https://huggingface.co/org/repo/settings").unwrap(),
                None,
                &endpoint
            )
            .unwrap()
            .is_none()
        );
        assert!(
            parse_hf_link(
                &Link::new("https://example.com/org/repo").unwrap(),
                None,
                &endpoint
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn parses_modelscope_links() {
        let endpoint = endpoint("https://modelscope.cn");
        let spec = parse_ms_link(
            &Link::new("https://modelscope.cn/models/Qwen/Qwen2.5-0.5B-Instruct").unwrap(),
            None,
            &endpoint,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.repo, "Qwen/Qwen2.5-0.5B-Instruct");
        assert_eq!(spec.revision, "master");

        let spec = parse_ms_link(
            &Link::new("ms://datasets/org/data").unwrap(),
            Some("v2"),
            &endpoint,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.repo_type, HubRepoType::Dataset);
        assert_eq!(spec.revision, "v2");
    }

    #[test]
    fn filters_with_globs() {
        let entries = vec![
            HubEntry {
                path: "a.bin".to_string(),
                size: None,
                checksum: None,
                metadata: Default::default(),
            },
            HubEntry {
                path: "dir/b.bin".to_string(),
                size: None,
                checksum: None,
                metadata: Default::default(),
            },
            HubEntry {
                path: "dir/c.txt".to_string(),
                size: None,
                checksum: None,
                metadata: Default::default(),
            },
        ];
        let filtered = filter_entries(
            entries,
            &["**/*.bin".to_string()],
            &["dir/b.bin".to_string()],
        )
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "a.bin");
    }
}
