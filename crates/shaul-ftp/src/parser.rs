use std::collections::BTreeMap;

use async_trait::async_trait;
use percent_encoding::percent_decode_str;
use shaul_core::{
    Auth, Error, Link, LinkParser, ParseOptions, Resource, ResourceFile, ResourceKind, Result,
    sanitize_file_name,
};
use url::Url;

/// Protocol identifier stored in [`ResourceKind::Custom`] for FTP resources.
pub const FTP_PROTOCOL: &str = "ftp";

/// Parser for `ftp://` links.
pub struct FtpParser;

#[async_trait]
impl LinkParser for FtpParser {
    fn name(&self) -> &'static str {
        "ftp"
    }

    fn supports(&self, link: &Link) -> bool {
        Url::parse(link.as_str())
            .map(|url| url.scheme().eq_ignore_ascii_case(FTP_PROTOCOL))
            .unwrap_or(false)
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let mut url = Url::parse(link.as_str())
            .map_err(|error| Error::InvalidLink(format!("invalid FTP URL: {error}")))?;
        if !url.scheme().eq_ignore_ascii_case(FTP_PROTOCOL) {
            return Err(Error::InvalidLink(
                "FTP parser received a non-FTP link".to_string(),
            ));
        }
        if url.host_str().is_none() {
            return Err(Error::InvalidLink("FTP link is missing a host".to_string()));
        }

        let remote_path = percent_decode_str(url.path())
            .decode_utf8()
            .map_err(|_| Error::InvalidLink("FTP path is not valid UTF-8".to_string()))?;
        if remote_path.is_empty() || remote_path.ends_with('/') {
            return Err(Error::InvalidLink(
                "FTP link must point to a file, not a directory".to_string(),
            ));
        }
        let remote_name = remote_path
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Error::InvalidLink("FTP link has no file name".to_string()))?;
        let filename = sanitize_file_name(remote_name);

        let mut request = options.request.clone();
        let username = url.username().to_string();
        let password = url.password().map(str::to_string);
        if !username.is_empty() {
            if request.auth.is_none() {
                request.auth = Some(Auth::basic(
                    username.clone(),
                    password.clone().unwrap_or_default(),
                ));
            }
            url.set_username("")
                .map_err(|_| Error::InvalidLink("invalid FTP username".to_string()))?;
            url.set_password(None)
                .map_err(|_| Error::InvalidLink("invalid FTP password".to_string()))?;
        } else if password.is_some() {
            return Err(Error::InvalidLink(
                "FTP link has a password without a username".to_string(),
            ));
        }

        let checksum = options.checksum.clone();
        let file = ResourceFile {
            path: filename.clone(),
            url: url.clone(),
            size: None,
            checksum: checksum.clone(),
            headers: Vec::new(),
            metadata: BTreeMap::new(),
            use_response_filename: false,
        };
        Ok(Resource {
            kind: ResourceKind::Custom {
                protocol: FTP_PROTOCOL.to_string(),
            },
            name: filename,
            files: vec![file],
            request,
            checksum,
            revision: None,
            metadata: BTreeMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(link: &str) -> Result<Resource> {
        let link = Link::new(link).unwrap();
        let parser = FtpParser;
        parser.parse(&link, &ParseOptions::default()).await
    }

    #[tokio::test]
    async fn parses_a_file_link_and_strips_credentials() {
        let resource = parse("ftp://user:secret@example.com:2121/dir/model%20file.bin")
            .await
            .unwrap();
        assert!(matches!(
            &resource.kind,
            ResourceKind::Custom { protocol } if protocol == FTP_PROTOCOL
        ));
        assert_eq!(resource.name, "model file.bin");
        assert_eq!(resource.files.len(), 1);
        assert_eq!(resource.files[0].path, "model file.bin");
        assert_eq!(
            resource.files[0].url.as_str(),
            "ftp://example.com:2121/dir/model%20file.bin"
        );
        assert!(matches!(
            &resource.request.auth,
            Some(Auth::Basic { username, password })
                if username == "user" && password == "secret"
        ));
    }

    #[tokio::test]
    async fn anonymous_link_has_no_auth() {
        let resource = parse("ftp://example.com/pub/file.bin").await.unwrap();
        assert!(resource.request.auth.is_none());
    }

    #[tokio::test]
    async fn rejects_directory_links() {
        let link = Link::new("ftp://example.com/pub/").unwrap();
        let parser = FtpParser;
        assert!(parser.parse(&link, &ParseOptions::default()).await.is_err());
    }

    #[test]
    fn supports_only_ftp_scheme() {
        assert!(FtpParser.supports(&Link::new("ftp://example.com/file").unwrap()));
        assert!(!FtpParser.supports(&Link::new("https://example.com/file").unwrap()));
        assert!(!FtpParser.supports(&Link::new("not a url").unwrap()));
    }
}
