use std::collections::BTreeMap;

use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};
use shaul_core::{Error, Result};
use url::Url;

/// A parsed HLS playlist.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Playlist {
    /// A master playlist that lists variants.
    Master(MasterPlaylist),
    /// A media playlist that lists segments.
    Media(MediaPlaylist),
}

/// A master playlist and its variants.
#[derive(Clone, Debug)]
pub struct MasterPlaylist {
    /// Variants in declaration order.
    pub variants: Vec<Variant>,
}

/// One master-playlist variant.
#[derive(Clone, Debug)]
pub struct Variant {
    /// Resolved media playlist URL.
    pub uri: Url,
    /// Declared peak bandwidth in bits per second.
    pub bandwidth: u64,
    /// Declared average bandwidth, when present.
    pub average_bandwidth: Option<u64>,
    /// Video resolution as `(width, height)`.
    pub resolution: Option<(u32, u32)>,
    /// Optional `NAME` attribute.
    pub name: Option<String>,
    /// Optional `CODECS` attribute.
    pub codecs: Option<String>,
}

/// A media playlist and its segments.
#[derive(Clone, Debug)]
pub struct MediaPlaylist {
    /// Segments in playback order.
    pub segments: Vec<Segment>,
    /// Initialization segment from `#EXT-X-MAP`, for fMP4 playlists.
    pub init: Option<Url>,
    /// Key that applies to the initialization segment, when encrypted.
    pub init_key: Option<Key>,
    /// Whether a later `#EXT-X-MAP` changed the initialization segment.
    pub init_changed: bool,
    /// Whether `#EXT-X-MAP` carried a byte range.
    pub init_byte_range: bool,
    /// Whether `#EXT-X-ENDLIST` was present.
    pub end_list: bool,
    /// `#EXT-X-MEDIA-SEQUENCE`, when present.
    pub media_sequence: u64,
    /// `#EXT-X-PLAYLIST-TYPE`, when present.
    pub playlist_type: Option<String>,
    /// Overall encryption summary for the playlist.
    pub encryption: Encryption,
}

/// Overall encryption state of a media playlist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Encryption {
    /// Every segment is unencrypted.
    #[default]
    None,
    /// At least one segment uses AES-128.
    Aes128,
    /// A method other than AES-128, such as `SAMPLE-AES`.
    Other(String),
}

/// An AES-128 key that applies to one or more segments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// Resolved key URI.
    pub uri: Url,
    /// Explicit IV from `#EXT-X-KEY`, when present.
    pub iv: Option<[u8; 16]>,
}

/// One media segment.
#[derive(Clone, Debug)]
pub struct Segment {
    /// Resolved segment URL.
    pub uri: Url,
    /// `#EXTINF` duration in seconds.
    pub duration: f64,
    /// `#EXT-X-BYTERANGE` range, when present.
    pub byte_range: Option<ByteRange>,
    /// Whether `#EXT-X-DISCONTINUITY` preceded this segment.
    pub discontinuity: bool,
    /// AES-128 key that applies to this segment, when encrypted.
    pub key: Option<Key>,
    /// Media sequence number of this segment.
    pub sequence: u64,
}

/// A parsed `#EXT-X-BYTERANGE` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    /// Length in bytes.
    pub length: u64,
    /// Offset in bytes, when explicitly provided.
    pub offset: Option<u64>,
}

impl Playlist {
    /// Parse a playlist relative to `base`.
    pub fn parse(base: &Url, text: &str) -> Result<Self> {
        let mut lines = text
            .lines()
            .map(|line| line.trim_end_matches('\r').trim())
            .filter(|line| !line.is_empty())
            .peekable();
        let Some(first) = lines.peek() else {
            return Err(Error::InvalidLink("empty HLS playlist".to_string()));
        };
        if *first != "#EXTM3U" {
            return Err(Error::InvalidLink(
                "HLS playlist must start with #EXTM3U".to_string(),
            ));
        }
        let _ = lines.next();

        let mut master = Vec::new();
        let mut media = MediaState::default();
        let mut pending_inf: Option<f64> = None;
        let mut pending_range: Option<ByteRange> = None;
        let mut pending_discontinuity = false;

        while let Some(line) = lines.next() {
            if let Some(value) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                let attributes = parse_attributes(value);
                let Some(uri_line) = lines.next() else {
                    return Err(Error::InvalidLink(
                        "HLS variant is missing its URI".to_string(),
                    ));
                };
                if uri_line.starts_with('#') {
                    return Err(Error::InvalidLink(
                        "HLS variant is missing its URI".to_string(),
                    ));
                }
                let uri = resolve_uri(base, uri_line)?;
                let bandwidth = attributes
                    .get("BANDWIDTH")
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or_else(|| {
                        Error::InvalidLink("HLS variant is missing BANDWIDTH".to_string())
                    })?;
                master.push(Variant {
                    uri,
                    bandwidth,
                    average_bandwidth: attributes
                        .get("AVERAGE-BANDWIDTH")
                        .and_then(|value| value.parse::<u64>().ok()),
                    resolution: attributes
                        .get("RESOLUTION")
                        .and_then(|value| parse_resolution(value)),
                    name: attributes.get("NAME").cloned(),
                    codecs: attributes.get("CODECS").cloned(),
                });
                continue;
            }
            if let Some(value) = line.strip_prefix("#EXTINF:") {
                let duration = value
                    .split(',')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| {
                        Error::InvalidLink(format!("invalid #EXTINF duration: {value}"))
                    })?;
                pending_inf = Some(duration);
                continue;
            }
            if let Some(value) = line.strip_prefix("#EXT-X-BYTERANGE:") {
                pending_range = Some(parse_byte_range(value)?);
                continue;
            }
            if line == "#EXT-X-DISCONTINUITY" {
                pending_discontinuity = true;
                continue;
            }
            if line == "#EXT-X-ENDLIST" {
                media.end_list = true;
                continue;
            }
            if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                if !media.segments.is_empty() {
                    return Err(Error::InvalidLink(
                        "#EXT-X-MEDIA-SEQUENCE must appear before segments".to_string(),
                    ));
                }
                media.media_sequence = value.trim().parse::<u64>().map_err(|_| {
                    Error::InvalidLink(format!("invalid #EXT-X-MEDIA-SEQUENCE: {value}"))
                })?;
                continue;
            }
            if let Some(value) = line.strip_prefix("#EXT-X-PLAYLIST-TYPE:") {
                media.playlist_type = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = line.strip_prefix("#EXT-X-MAP:") {
                let attributes = parse_attributes(value);
                let Some(uri) = attributes.get("URI") else {
                    return Err(Error::InvalidLink("#EXT-X-MAP is missing URI".to_string()));
                };
                let uri = resolve_uri(base, uri)?;
                let key_changed =
                    media.init.is_some() && media.init_key.as_ref() != media.current_key.as_ref();
                if media.init.as_ref().is_some_and(|existing| *existing != uri) || key_changed {
                    media.init_changed = true;
                } else if media.init.is_none() {
                    media.init = Some(uri);
                    media.init_key = media.current_key.clone();
                }
                if attributes.contains_key("BYTERANGE") {
                    media.init_byte_range = true;
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("#EXT-X-KEY:") {
                match parse_key(base, value)? {
                    ParsedKey::None => media.current_key = None,
                    ParsedKey::Aes128(key) => {
                        media.current_key = Some(key);
                        if matches!(media.encryption, Encryption::None) {
                            media.encryption = Encryption::Aes128;
                        }
                    }
                    ParsedKey::Unsupported(method) => {
                        media.current_key = None;
                        if !matches!(media.encryption, Encryption::Other(_)) {
                            media.encryption = Encryption::Other(method);
                        }
                    }
                }
                continue;
            }
            if line.starts_with("#EXT-X-MEDIA:") || line.starts_with("#EXT-X-I-FRAME-STREAM-INF:") {
                // Alternate renditions and I-frame playlists are out of scope
                // for phase 1.
                continue;
            }
            if line.starts_with('#') {
                // Unknown tags and comments are ignored for forward
                // compatibility.
                continue;
            }

            let Some(duration) = pending_inf.take() else {
                return Err(Error::InvalidLink(format!(
                    "HLS segment has no #EXTINF: {line}"
                )));
            };
            let uri = resolve_uri(base, line)?;
            let sequence = media.media_sequence + media.segments.len() as u64;
            media.segments.push(Segment {
                uri,
                duration,
                byte_range: pending_range.take(),
                discontinuity: pending_discontinuity,
                key: media.current_key.clone(),
                sequence,
            });
            pending_discontinuity = false;
        }

        if !master.is_empty() {
            if !media.segments.is_empty() || media.init.is_some() {
                return Err(Error::InvalidLink(
                    "HLS playlist mixes variants and media segments".to_string(),
                ));
            }
            return Ok(Playlist::Master(MasterPlaylist { variants: master }));
        }
        if media.segments.is_empty() {
            return Err(Error::InvalidLink(
                "HLS media playlist has no segments".to_string(),
            ));
        }
        Ok(Playlist::Media(MediaPlaylist {
            segments: media.segments,
            init: media.init,
            init_key: media.init_key,
            init_changed: media.init_changed,
            init_byte_range: media.init_byte_range,
            end_list: media.end_list,
            media_sequence: media.media_sequence,
            playlist_type: media.playlist_type,
            encryption: media.encryption,
        }))
    }
}

#[derive(Default)]
struct MediaState {
    segments: Vec<Segment>,
    init: Option<Url>,
    init_key: Option<Key>,
    init_changed: bool,
    init_byte_range: bool,
    end_list: bool,
    media_sequence: u64,
    playlist_type: Option<String>,
    current_key: Option<Key>,
    encryption: Encryption,
}

impl MasterPlaylist {
    /// Select the highest-bandwidth variant that matches `include` and does
    /// not match `exclude`. Empty `include` matches every variant.
    pub fn select_variant(&self, include: &[String], exclude: &[String]) -> Result<&Variant> {
        let include = glob_set(include)?;
        let exclude = glob_set(exclude)?;
        let mut selected: Option<&Variant> = None;
        for variant in &self.variants {
            if matches_any(&exclude, variant) {
                continue;
            }
            if (include.is_empty() || matches_any(&include, variant))
                && selected.is_none_or(|current| variant.bandwidth > current.bandwidth)
            {
                selected = Some(variant);
            }
        }
        selected.ok_or_else(|| {
            Error::InvalidLink("no HLS variant matched --include/--exclude".to_string())
        })
    }
}

impl MediaPlaylist {
    /// Whether the playlist is a complete video-on-demand list.
    pub fn is_vod(&self) -> bool {
        self.end_list && self.playlist_type.as_deref() != Some("EVENT")
    }
}

/// Return a stable SHA-256 fingerprint for a playlist body.
pub fn fingerprint(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern)
            .map_err(|error| Error::Config(format!("invalid glob {pattern}: {error}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| Error::Config(format!("invalid glob set: {error}")))
}

fn matches_any(set: &GlobSet, variant: &Variant) -> bool {
    if set.is_empty() {
        return false;
    }
    let mut candidates = vec![variant.uri.as_str().to_string()];
    if let Some(name) = &variant.name {
        candidates.push(name.clone());
    }
    if let Some(codecs) = &variant.codecs {
        candidates.push(codecs.clone());
    }
    if let Some((width, height)) = variant.resolution {
        candidates.push(format!("{width}x{height}"));
    }
    candidates.push(variant.bandwidth.to_string());
    if let Some(average) = variant.average_bandwidth {
        candidates.push(average.to_string());
    }
    candidates.iter().any(|candidate| set.is_match(candidate))
}

fn resolve_uri(base: &Url, value: &str) -> Result<Url> {
    let url = base
        .join(value.trim())
        .map_err(|error| Error::InvalidLink(format!("invalid HLS URI {value}: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidLink(format!(
            "HLS URI must use http or https: {value}"
        )));
    }
    Ok(url)
}

fn parse_resolution(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn parse_byte_range(value: &str) -> Result<ByteRange> {
    let (length, offset) = match value.trim().split_once('@') {
        Some((length, offset)) => (
            length,
            Some(offset.parse::<u64>().map_err(|_| {
                Error::InvalidLink(format!("invalid #EXT-X-BYTERANGE offset: {value}"))
            })?),
        ),
        None => (value.trim(), None),
    };
    let length = length
        .parse::<u64>()
        .map_err(|_| Error::InvalidLink(format!("invalid #EXT-X-BYTERANGE length: {value}")))?;
    Ok(ByteRange { length, offset })
}

enum ParsedKey {
    None,
    Aes128(Key),
    Unsupported(String),
}

fn parse_key(base: &Url, value: &str) -> Result<ParsedKey> {
    let attributes = parse_attributes(value);
    let method = attributes
        .get("METHOD")
        .map(String::as_str)
        .unwrap_or_default();
    match method {
        "" | "NONE" => Ok(ParsedKey::None),
        "AES-128" => {
            let key_format = attributes
                .get("KEYFORMAT")
                .map(String::as_str)
                .unwrap_or("identity");
            if !key_format.eq_ignore_ascii_case("identity") {
                return Ok(ParsedKey::Unsupported(format!("KEYFORMAT={key_format}")));
            }
            let Some(uri) = attributes.get("URI") else {
                return Err(Error::InvalidLink(
                    "AES-128 #EXT-X-KEY is missing URI".to_string(),
                ));
            };
            let iv = attributes
                .get("IV")
                .map(|value| parse_iv(value))
                .transpose()?;
            Ok(ParsedKey::Aes128(Key {
                uri: resolve_uri(base, uri)?,
                iv,
            }))
        }
        other => Ok(ParsedKey::Unsupported(other.to_string())),
    }
}

fn parse_iv(value: &str) -> Result<[u8; 16]> {
    let trimmed = value.trim();
    let hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    let bytes = hex::decode(hex)
        .map_err(|_| Error::InvalidLink(format!("invalid #EXT-X-KEY IV: {value}")))?;
    bytes
        .try_into()
        .map_err(|_| Error::InvalidLink(format!("HLS IV must be 16 bytes: {value}")))
}

/// Parse an HLS attribute list. Commas inside quoted values are preserved.
pub fn parse_attributes(input: &str) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut parts = Vec::new();
    for character in input.chars() {
        match character {
            '"' => {
                in_quotes = !in_quotes;
                current.push(character);
            }
            ',' if !in_quotes => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    for part in parts {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        attributes.insert(name.trim().to_string(), value.to_string());
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/media/master.m3u8").unwrap()
    }

    #[test]
    fn parses_a_master_playlist_and_selects_the_highest_bandwidth() {
        let text = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720,CODECS="avc1.4d401f,mp4a.40.2"
720p/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2400000,RESOLUTION=1920x1080,NAME="1080p"
1080p/index.m3u8
"#;
        let Playlist::Master(master) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a master playlist");
        };
        assert_eq!(master.variants.len(), 2);
        assert_eq!(master.variants[0].resolution, Some((1280, 720)));
        assert_eq!(
            master.variants[0].codecs.as_deref(),
            Some("avc1.4d401f,mp4a.40.2")
        );
        let selected = master.select_variant(&[], &[]).unwrap();
        assert_eq!(selected.bandwidth, 2_400_000);
        assert_eq!(
            selected.uri.as_str(),
            "https://example.com/media/1080p/index.m3u8"
        );
        let selected = master.select_variant(&["*720*".to_string()], &[]).unwrap();
        assert_eq!(selected.bandwidth, 800_000);
    }

    #[test]
    fn parses_a_media_playlist_with_relative_and_absolute_segments() {
        let text = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXT-X-MEDIA-SEQUENCE:42
#EXT-X-PLAYLIST-TYPE:VOD
#EXTINF:6.0,
seg-000.ts
#EXTINF:5.5,title
https://cdn.example.com/seg-001.ts
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(media.is_vod());
        assert_eq!(media.media_sequence, 42);
        assert_eq!(media.segments.len(), 2);
        assert_eq!(
            media.segments[0].uri.as_str(),
            "https://example.com/media/seg-000.ts"
        );
        assert_eq!(media.segments[1].duration, 5.5);
        assert_eq!(
            media.segments[1].uri.as_str(),
            "https://cdn.example.com/seg-001.ts"
        );
    }

    #[test]
    fn parses_fmp4_init_maps_and_byte_ranges() {
        let text = r#"#EXTM3U
#EXT-X-MAP:URI="init.mp4"
#EXT-X-BYTERANGE:1000@0
#EXTINF:4.0,
media.mp4
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert_eq!(
            media.init.as_ref().map(Url::as_str),
            Some("https://example.com/media/init.mp4")
        );
        assert_eq!(
            media.segments[0].byte_range,
            Some(ByteRange {
                length: 1000,
                offset: Some(0)
            })
        );
    }

    #[test]
    fn exposes_aes_128_keys() {
        let text = r#"#EXTM3U
#EXT-X-KEY:METHOD=AES-128,URI="key.bin",IV=0x000102030405060708090a0b0c0d0e0f
#EXTINF:4.0,
seg.ts
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert_eq!(media.encryption, Encryption::Aes128);
        let key = media.segments[0].key.as_ref().unwrap();
        assert_eq!(key.uri.as_str(), "https://example.com/media/key.bin");
        assert_eq!(
            key.iv,
            Some([
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f,
            ])
        );
    }

    #[test]
    fn keeps_encryption_when_a_later_key_is_none() {
        let text = r#"#EXTM3U
#EXT-X-KEY:METHOD=AES-128,URI="key.bin"
#EXTINF:4.0,
seg-0.ts
#EXT-X-KEY:METHOD=NONE
#EXTINF:4.0,
seg-1.ts
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert_eq!(media.encryption, Encryption::Aes128);
        assert!(media.segments[0].key.is_some());
        assert!(media.segments[1].key.is_none());
        assert_eq!(media.segments[0].sequence, 0);
        assert_eq!(media.segments[1].sequence, 1);
    }

    #[test]
    fn tracks_key_rotation_and_media_sequence() {
        let text = r#"#EXTM3U
#EXT-X-MEDIA-SEQUENCE:10
#EXT-X-KEY:METHOD=AES-128,URI="key1.bin"
#EXTINF:4.0,
seg-0.ts
#EXT-X-KEY:METHOD=AES-128,URI="key2.bin",IV=0x000102030405060708090a0b0c0d0e0f
#EXTINF:4.0,
seg-1.ts
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert_eq!(media.encryption, Encryption::Aes128);
        assert_eq!(media.segments[0].sequence, 10);
        assert_eq!(
            media.segments[0].key.as_ref().unwrap().uri.as_str(),
            "https://example.com/media/key1.bin"
        );
        assert!(media.segments[0].key.as_ref().unwrap().iv.is_none());
        assert_eq!(media.segments[1].sequence, 11);
        assert_eq!(
            media.segments[1].key.as_ref().unwrap().uri.as_str(),
            "https://example.com/media/key2.bin"
        );
        assert!(media.segments[1].key.as_ref().unwrap().iv.is_some());
    }

    #[test]
    fn rejects_unsupported_key_formats_and_bad_ivs() {
        let keyformat = r#"#EXTM3U
#EXT-X-KEY:METHOD=AES-128,URI="key.bin",KEYFORMAT="com.example.drm"
#EXTINF:4.0,
seg.ts
#EXT-X-ENDLIST
"#;
        let Playlist::Media(media) = Playlist::parse(&base(), keyformat).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(matches!(media.encryption, Encryption::Other(_)));

        let bad_iv = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x1234\n#EXTINF:4.0,\nseg.ts\n#EXT-X-ENDLIST\n";
        assert!(Playlist::parse(&base(), bad_iv).is_err());
    }

    #[test]
    fn rejects_live_playlists_through_is_vod() {
        let text = "#EXTM3U\n#EXTINF:4.0,\nseg.ts\n";
        let Playlist::Media(media) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a media playlist");
        };
        assert!(!media.is_vod());
    }

    #[test]
    fn rejects_a_missing_header_and_a_mixed_playlist() {
        assert!(Playlist::parse(&base(), "#EXTINF:4.0,\nseg.ts\n").is_err());
        let mixed = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1
variant.m3u8
#EXTINF:4.0,
seg.ts
"#;
        assert!(Playlist::parse(&base(), mixed).is_err());
    }

    #[test]
    fn rejects_non_http_uris() {
        let text = "#EXTM3U\n#EXTINF:4.0,\nfile:///tmp/seg.ts\n#EXT-X-ENDLIST\n";
        assert!(Playlist::parse(&base(), text).is_err());
    }

    #[test]
    fn parses_quoted_attribute_commas() {
        let attributes = parse_attributes(r#"BANDWIDTH=1,CODECS="a,b",NAME="x""#);
        assert_eq!(attributes.get("BANDWIDTH").map(String::as_str), Some("1"));
        assert_eq!(attributes.get("CODECS").map(String::as_str), Some("a,b"));
        assert_eq!(attributes.get("NAME").map(String::as_str), Some("x"));
    }

    #[test]
    fn variant_selection_can_fail() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8\n";
        let Playlist::Master(master) = Playlist::parse(&base(), text).unwrap() else {
            panic!("expected a master playlist");
        };
        assert!(
            master
                .select_variant(&["missing".to_string()], &[])
                .is_err()
        );
    }
}
