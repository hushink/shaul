use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use shaul_core::{
    BitTorrentSource, Error, Link, LinkParser, ParseOptions, Resource, ResourceKind, Result,
    sanitize_file_name,
};
use url::Url;

/// Recognizes magnet links, bare v1 info hashes and local `.torrent` files.
#[derive(Clone, Debug, Default)]
pub struct TorrentParser;

#[async_trait]
impl LinkParser for TorrentParser {
    fn name(&self) -> &'static str {
        "bittorrent"
    }

    fn supports(&self, link: &Link) -> bool {
        let raw = link.as_str();
        if torrent_path(raw).is_some() {
            return true;
        }
        is_magnet_candidate(raw) && librqbit::Magnet::parse(raw).is_ok()
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        if options.checksum.is_some() {
            return Err(Error::Config(
                "--checksum is not supported for BitTorrent tasks".to_string(),
            ));
        }
        let raw = link.as_str();
        if let Some(path) = torrent_path(raw) {
            let absolute = absolute_path(&path);
            let name = path
                .file_stem()
                .map(|value| sanitize_file_name(&value.to_string_lossy()))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "torrent".to_string());
            let mut metadata = BTreeMap::new();
            metadata.insert(
                "bt.torrent_path".to_string(),
                absolute.to_string_lossy().to_string(),
            );
            return Ok(Resource {
                kind: ResourceKind::BitTorrent {
                    source: BitTorrentSource::TorrentFile,
                    name: Some(name.clone()),
                },
                name,
                files: Vec::new(),
                request: options.request.clone(),
                checksum: None,
                revision: None,
                metadata,
            });
        }

        let magnet = librqbit::Magnet::parse(raw)
            .map_err(|error| Error::InvalidLink(format!("invalid magnet link {raw}: {error:#}")))?;
        let name = magnet
            .name
            .clone()
            .or_else(|| magnet_display_name(raw))
            .map(|value| sanitize_file_name(&value))
            .filter(|value| !value.is_empty());
        let mut metadata = BTreeMap::new();
        metadata.insert("bt.magnet".to_string(), raw.to_string());
        Ok(Resource {
            kind: ResourceKind::BitTorrent {
                source: BitTorrentSource::Magnet,
                name: name.clone(),
            },
            name: name.unwrap_or_else(|| "torrent".to_string()),
            files: Vec::new(),
            request: options.request.clone(),
            checksum: None,
            revision: None,
            metadata,
        })
    }
}

fn is_magnet_candidate(raw: &str) -> bool {
    raw.starts_with("magnet:") || (raw.len() == 40 && raw.chars().all(|c| c.is_ascii_hexdigit()))
}

fn magnet_display_name(raw: &str) -> Option<String> {
    Url::parse(raw)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "dn")
                .map(|(_, value)| value.to_string())
        })
        .filter(|value| !value.is_empty())
}

fn torrent_path(raw: &str) -> Option<PathBuf> {
    if raw.starts_with("file://") {
        let path = Url::parse(raw).ok()?.to_file_path().ok()?;
        return is_torrent_file(&path).then_some(path);
    }
    let path = PathBuf::from(raw);
    is_torrent_file(&path).then_some(path)
}

fn is_torrent_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case("torrent"))
            .unwrap_or(false)
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_magnet_links() {
        let parser = TorrentParser;
        let link =
            Link::new("magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=hello")
                .unwrap();
        assert!(parser.supports(&link));
        let resource = parser.parse(&link, &ParseOptions::default()).await.unwrap();
        assert_eq!(resource.name, "hello");
        assert!(matches!(
            resource.kind,
            ResourceKind::BitTorrent {
                source: BitTorrentSource::Magnet,
                ..
            }
        ));
        assert_eq!(
            resource.metadata.get("bt.magnet").map(String::as_str),
            Some(link.as_str())
        );
    }

    #[tokio::test]
    async fn parses_local_torrent_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.torrent");
        std::fs::write(&path, b"d4:infod4:name4:testee").unwrap();
        let parser = TorrentParser;
        let link = Link::new(path.to_string_lossy()).unwrap();
        assert!(parser.supports(&link));
        let resource = parser.parse(&link, &ParseOptions::default()).await.unwrap();
        assert_eq!(resource.name, "sample");
        assert!(matches!(
            resource.kind,
            ResourceKind::BitTorrent {
                source: BitTorrentSource::TorrentFile,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn rejects_checksums_for_torrents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.torrent");
        std::fs::write(&path, b"x").unwrap();
        let parser = TorrentParser;
        let link = Link::new(path.to_string_lossy()).unwrap();
        let options = ParseOptions {
            checksum: Some(
                shaul_core::Checksum::parse(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                )
                .unwrap(),
            ),
            ..ParseOptions::default()
        };
        let error = parser.parse(&link, &options).await.unwrap_err();
        assert!(error.to_string().contains("--checksum"));
    }
}
