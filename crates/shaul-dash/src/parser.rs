use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use percent_encoding::percent_decode_str;
use shaul_core::{
    Error, Link, LinkParser, ParseOptions, Resource, ResourceFile, ResourceKind, Result,
    sanitize_file_name,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::client::DashClient;
use crate::mpd::DashManifest;

/// Protocol identifier stored in [`ResourceKind::Custom`] for DASH resources.
pub const DASH_PROTOCOL: &str = "dash";

/// Parser for HTTP(S) `.mpd` links.
#[derive(Clone)]
pub struct DashParser {
    client: Arc<DashClient>,
}

impl DashParser {
    /// Creates a parser that fetches manifests with `client`.
    pub fn new(client: Arc<DashClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LinkParser for DashParser {
    fn name(&self) -> &'static str {
        DASH_PROTOCOL
    }

    fn supports(&self, link: &Link) -> bool {
        Url::parse(link.as_str())
            .map(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.path().to_ascii_lowercase().ends_with(".mpd")
            })
            .unwrap_or(false)
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let url = Url::parse(link.as_str())
            .map_err(|error| Error::InvalidLink(format!("invalid DASH URL: {error}")))?;
        if !self.supports(link) {
            return Err(Error::InvalidLink(
                "DASH parser received a non-DASH link".to_string(),
            ));
        }
        let (text, final_url) = self
            .client
            .fetch_text(&url, &options.request, &CancellationToken::new())
            .await?;
        let manifest = DashManifest::parse(&final_url, &text)?;
        let selected = manifest.select(&options.include, &options.exclude)?;

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "dash.origin".to_string(),
            url.origin().ascii_serialization(),
        );
        metadata.insert("dash.representation_id".to_string(), selected.id.clone());
        if let Some(bandwidth) = selected.bandwidth {
            metadata.insert("dash.bandwidth".to_string(), bandwidth.to_string());
        }
        if let Some(mime_type) = &selected.mime_type {
            metadata.insert("dash.mime_type".to_string(), mime_type.clone());
        }
        if let Some(codecs) = &selected.codecs {
            metadata.insert("dash.codecs".to_string(), codecs.clone());
        }

        let filename = output_name(&url);
        let file = ResourceFile {
            path: filename.clone(),
            url: final_url,
            size: None,
            checksum: options.checksum.clone(),
            headers: Vec::new(),
            metadata: metadata.clone(),
            use_response_filename: false,
        };
        Ok(Resource {
            kind: ResourceKind::Custom {
                protocol: DASH_PROTOCOL.to_string(),
            },
            name: filename,
            files: vec![file],
            request: options.request.clone(),
            checksum: options.checksum.clone(),
            revision: None,
            metadata,
        })
    }
}

fn output_name(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .unwrap_or_default();
    let decoded = percent_decode_str(last).decode_utf8_lossy();
    let stem = decoded
        .strip_suffix(".mpd")
        .or_else(|| decoded.strip_suffix(".MPD"))
        .unwrap_or(&decoded);
    if stem.is_empty() {
        return "stream".to_string();
    }
    let name = sanitize_file_name(stem);
    if name.is_empty() || name == "." {
        "stream".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_only_http_mpd_links() {
        let client =
            Arc::new(DashClient::new(&shaul_core::Config::default()).expect("client should build"));
        let parser = DashParser::new(client);
        assert!(parser.supports(&Link::new("https://example.com/a.mpd").unwrap()));
        assert!(parser.supports(&Link::new("https://example.com/a.MPD?x=1").unwrap()));
        assert!(!parser.supports(&Link::new("https://example.com/a.m3u8").unwrap()));
        assert!(!parser.supports(&Link::new("ftp://example.com/a.mpd").unwrap()));
        assert!(!parser.supports(&Link::new("magnet:?xt=urn:btih:abc").unwrap()));
    }

    #[test]
    fn derives_the_output_name() {
        let base = Url::parse("https://example.com/media/manifest.mpd").unwrap();
        assert_eq!(output_name(&base), "manifest");
        assert_eq!(
            output_name(&Url::parse("https://example.com/videos/my%20movie.mpd").unwrap()),
            "my movie"
        );
        assert_eq!(
            output_name(&Url::parse("https://example.com/videos/").unwrap()),
            "videos"
        );
        assert_eq!(
            output_name(&Url::parse("https://example.com/").unwrap()),
            "stream"
        );
    }
}
