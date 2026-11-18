use async_trait::async_trait;
use shaul_core::{Error, Link, LinkParser, ParseOptions, Resource, Result};
use url::Url;

/// Default parser: every http(s) URL is a direct download.
#[derive(Clone, Debug, Default)]
pub struct DirectLinkParser;

#[async_trait]
impl LinkParser for DirectLinkParser {
    fn name(&self) -> &'static str {
        "direct-http"
    }

    fn supports(&self, link: &Link) -> bool {
        let lower = link.as_str().to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let url = Url::parse(link.as_str())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::InvalidLink(format!(
                "unsupported URL scheme: {}",
                url.scheme()
            )));
        }
        let mut resource =
            Resource::http_file(url, options.request.clone(), options.checksum.clone())?;
        if let Some(checksum) = &options.checksum
            && let Some(file) = resource.files.first_mut()
        {
            file.checksum = Some(checksum.clone());
        }
        Ok(resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_http_url() {
        let parser = DirectLinkParser;
        let link = Link::new("https://example.com/files/model.bin?token=1").unwrap();
        let resource = parser.parse(&link, &ParseOptions::default()).await.unwrap();
        assert_eq!(resource.files.len(), 1);
        assert_eq!(resource.files[0].path, "model.bin");
    }
}
