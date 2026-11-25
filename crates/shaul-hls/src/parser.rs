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

use crate::client::HlsClient;
use crate::playlist::{Encryption, MediaPlaylist, Playlist};

/// Protocol identifier stored in [`ResourceKind::Custom`] for HLS resources.
pub const HLS_PROTOCOL: &str = "hls";

/// Parser for HTTP(S) `.m3u8` links.
#[derive(Clone)]
pub struct HlsParser {
    client: Arc<HlsClient>,
}

impl HlsParser {
    /// Creates a parser that fetches playlists with `client`.
    pub fn new(client: Arc<HlsClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LinkParser for HlsParser {
    fn name(&self) -> &'static str {
        HLS_PROTOCOL
    }

    fn supports(&self, link: &Link) -> bool {
        Url::parse(link.as_str())
            .map(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.path().to_ascii_lowercase().ends_with(".m3u8")
            })
            .unwrap_or(false)
    }

    async fn parse(&self, link: &Link, options: &ParseOptions) -> Result<Resource> {
        let url = Url::parse(link.as_str())
            .map_err(|error| Error::InvalidLink(format!("invalid HLS URL: {error}")))?;
        if !self.supports(link) {
            return Err(Error::InvalidLink(
                "HLS parser received a non-HLS link".to_string(),
            ));
        }
        let (text, final_url) = self
            .client
            .fetch_text(&url, &options.request, &CancellationToken::new())
            .await?;
        let playlist = Playlist::parse(&final_url, &text)?;
        let mut metadata = BTreeMap::new();
        metadata.insert("hls.origin".to_string(), url.origin().ascii_serialization());
        let media_url = match playlist {
            Playlist::Master(master) => {
                let variant = master.select_variant(&options.include, &options.exclude)?;
                metadata.insert("hls.bandwidth".to_string(), variant.bandwidth.to_string());
                if let Some(name) = &variant.name {
                    metadata.insert("hls.variant_name".to_string(), name.clone());
                }
                if let Some((width, height)) = variant.resolution {
                    metadata.insert("hls.resolution".to_string(), format!("{width}x{height}"));
                }
                variant.uri.clone()
            }
            Playlist::Media(media) => {
                validate_media(&media)?;
                final_url
            }
        };
        let filename = output_name(&url);
        let file = ResourceFile {
            path: filename.clone(),
            url: media_url,
            size: None,
            checksum: options.checksum.clone(),
            headers: Vec::new(),
            metadata: metadata.clone(),
            use_response_filename: false,
        };
        Ok(Resource {
            kind: ResourceKind::Custom {
                protocol: HLS_PROTOCOL.to_string(),
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
        .strip_suffix(".m3u8")
        .or_else(|| decoded.strip_suffix(".M3U8"))
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

pub(crate) fn validate_media(media: &MediaPlaylist) -> Result<()> {
    if !media.is_vod() {
        return Err(Error::Backend(
            "live HLS playlists are not supported; only VOD with #EXT-X-ENDLIST is supported"
                .to_string(),
        ));
    }
    if media.init_changed {
        return Err(Error::Backend(
            "HLS playlists with multiple different #EXT-X-MAP values are not supported".to_string(),
        ));
    }
    if media.init.is_some() && media.init_key.as_ref().is_some_and(|key| key.iv.is_none()) {
        return Err(Error::Backend(
            "encrypted #EXT-X-MAP without an IV is not supported".to_string(),
        ));
    }
    match &media.encryption {
        Encryption::None | Encryption::Aes128 => Ok(()),
        Encryption::Other(method) => Err(Error::Backend(format!(
            "unsupported HLS encryption method: {method}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playlist::Playlist;

    fn base() -> Url {
        Url::parse("https://example.com/media/master.m3u8").unwrap()
    }

    #[test]
    fn supports_only_http_m3u8_links() {
        let client =
            Arc::new(HlsClient::new(&shaul_core::Config::default()).expect("client should build"));
        let parser = HlsParser::new(client);
        assert!(parser.supports(&Link::new("https://example.com/a.m3u8").unwrap()));
        assert!(parser.supports(&Link::new("https://example.com/a.M3U8?x=1").unwrap()));
        assert!(!parser.supports(&Link::new("https://example.com/a.mp4").unwrap()));
        assert!(!parser.supports(&Link::new("ftp://example.com/a.m3u8").unwrap()));
        assert!(!parser.supports(&Link::new("magnet:?xt=urn:btih:abc").unwrap()));
    }

    #[test]
    fn derives_the_output_name() {
        assert_eq!(output_name(&base()), "master");
        assert_eq!(
            output_name(&Url::parse("https://example.com/videos/my%20movie.m3u8").unwrap()),
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

    #[test]
    fn validates_live_and_encryption_methods() {
        let live = "#EXTM3U\n#EXTINF:4.0,\nseg.ts\n";
        let Playlist::Media(media) = Playlist::parse(&base(), live).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(validate_media(&media).is_err());

        let aes128 = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:4.0,\nseg.ts\n#EXT-X-ENDLIST\n";
        let Playlist::Media(media) = Playlist::parse(&base(), aes128).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(validate_media(&media).is_ok());

        let sample_aes = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\"\n#EXTINF:4.0,\nseg.ts\n#EXT-X-ENDLIST\n";
        let Playlist::Media(media) = Playlist::parse(&base(), sample_aes).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(validate_media(&media).is_err());

        let encrypted_init = r#"#EXTM3U
#EXT-X-KEY:METHOD=AES-128,URI="key"
#EXT-X-MAP:URI="init.mp4"
#EXTINF:4.0,
seg.ts
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), encrypted_init).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(validate_media(&media).is_err());
    }

    #[test]
    fn accepts_a_vod_media_playlist() {
        let text = "#EXTM3U\n#EXTINF:4.0,\nseg.ts\n#EXT-X-ENDLIST\n";
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(validate_media(&media).is_ok());
    }
}
