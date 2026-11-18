use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use digest::Digest;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

use crate::error::{Error, Result};

/// Digest algorithms supported for checksum verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ChecksumAlgorithm {
    /// SHA-256, 64 hex characters.
    Sha256,
    /// SHA-512, 128 hex characters.
    Sha512,
    /// SHA-1, 40 hex characters.
    Sha1,
    /// MD5, 32 hex characters.
    Md5,
}

impl ChecksumAlgorithm {
    /// Length of the digest in hex characters.
    pub fn hex_len(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha512 => 128,
            Self::Sha1 => 40,
            Self::Md5 => 32,
        }
    }

    /// Lowercase algorithm name as used in checksum strings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Sha1 => "sha1",
            Self::Md5 => "md5",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().replace('-', "").as_str() {
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            "sha1" => Some(Self::Sha1),
            "md5" => Some(Self::Md5),
            _ => None,
        }
    }

    fn from_hex_len(len: usize) -> Option<Self> {
        match len {
            64 => Some(Self::Sha256),
            128 => Some(Self::Sha512),
            40 => Some(Self::Sha1),
            32 => Some(Self::Md5),
            _ => None,
        }
    }
}

impl FromStr for ChecksumAlgorithm {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::from_token(value)
            .ok_or_else(|| Error::Config(format!("unknown checksum algorithm: {value}")))
    }
}

impl fmt::Display for ChecksumAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An expected content digest, for example `sha256:abc...`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksum {
    /// Algorithm that produced the digest.
    pub algorithm: ChecksumAlgorithm,
    /// Lowercase hexadecimal digest.
    pub hex: String,
}

impl Checksum {
    /// Validates `hex` against `algorithm` and builds a checksum.
    pub fn new(algorithm: ChecksumAlgorithm, hex: impl AsRef<str>) -> Result<Self> {
        let hex = hex.as_ref().trim().trim_matches('"');
        let hex = hex.strip_prefix("0x").unwrap_or(hex).to_ascii_lowercase();
        if hex.len() != algorithm.hex_len() || hex::decode(&hex).is_err() {
            return Err(Error::Config(format!(
                "invalid {} checksum: {hex}",
                algorithm.as_str()
            )));
        }
        Ok(Self { algorithm, hex })
    }

    /// Accepts `sha256:<hex>`, `sha-256:<hex>` or a bare hex digest whose
    /// length identifies the algorithm.
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if let Some((algorithm, hex)) = value.split_once(':') {
            let algorithm = ChecksumAlgorithm::from_str(algorithm)?;
            return Self::new(algorithm, hex);
        }

        let algorithm = ChecksumAlgorithm::from_hex_len(value.len()).ok_or_else(|| {
            Error::Config(format!(
                "cannot infer checksum algorithm from {value}; use <algorithm>:<hex>"
            ))
        })?;
        Self::new(algorithm, value)
    }

    /// Hugging Face LFS exposes the SHA-256 digest through `X-Linked-Etag`
    /// (usually quoted). Weak ETags are not checksums.
    pub fn from_etag(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.starts_with("W/") {
            return None;
        }
        let value = value.trim_matches('"');
        let algorithm = ChecksumAlgorithm::from_hex_len(value.len())?;
        Self::new(algorithm, value).ok()
    }

    /// Returns whether both checksums use the same algorithm and digest,
    /// ignoring hex case.
    pub fn matches(&self, actual: &Checksum) -> bool {
        self.algorithm == actual.algorithm && self.hex.eq_ignore_ascii_case(&actual.hex)
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hex)
    }
}

impl FromStr for Checksum {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

/// Compute a file digest without blocking the async runtime.
pub async fn compute_file_checksum(
    path: PathBuf,
    algorithm: ChecksumAlgorithm,
) -> Result<Checksum> {
    tokio::task::spawn_blocking(move || compute_file_checksum_blocking(&path, algorithm))
        .await
        .map_err(|error| Error::Other(format!("checksum task failed: {error}")))?
}

/// Compute a file digest on the calling thread.
///
/// Use [`compute_file_checksum`] inside async code so the read does not block
/// the runtime.
pub fn compute_file_checksum_blocking(
    path: &Path,
    algorithm: ChecksumAlgorithm,
) -> Result<Checksum> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file);
    let mut hasher = AnyHasher::new(algorithm);
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Checksum {
        algorithm,
        hex: hasher.finalize_hex(),
    })
}

enum AnyHasher {
    Sha256(Sha256),
    Sha512(Sha512),
    Sha1(Sha1),
    Md5(md5::Md5),
}

impl AnyHasher {
    fn new(algorithm: ChecksumAlgorithm) -> Self {
        match algorithm {
            ChecksumAlgorithm::Sha256 => Self::Sha256(Sha256::new()),
            ChecksumAlgorithm::Sha512 => Self::Sha512(Sha512::new()),
            ChecksumAlgorithm::Sha1 => Self::Sha1(Sha1::new()),
            ChecksumAlgorithm::Md5 => Self::Md5(md5::Md5::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha256(hasher) => hasher.update(data),
            Self::Sha512(hasher) => hasher.update(data),
            Self::Sha1(hasher) => hasher.update(data),
            Self::Md5(hasher) => hasher.update(data),
        }
    }

    fn finalize_hex(self) -> String {
        match self {
            Self::Sha256(hasher) => hex::encode(hasher.finalize()),
            Self::Sha512(hasher) => hex::encode(hasher.finalize()),
            Self::Sha1(hasher) => hex::encode(hasher.finalize()),
            Self::Md5(hasher) => hex::encode(hasher.finalize()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_and_bare_checksums() {
        let prefixed = Checksum::parse(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        assert_eq!(prefixed.algorithm, ChecksumAlgorithm::Sha256);
        let bare = Checksum::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(bare.algorithm, ChecksumAlgorithm::Md5);
    }

    #[test]
    fn rejects_invalid_checksum() {
        assert!(Checksum::parse("sha256:xyz").is_err());
    }

    #[test]
    fn etag_is_sha256() {
        let etag = "\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"";
        let checksum = Checksum::from_etag(etag).unwrap();
        assert_eq!(checksum.algorithm, ChecksumAlgorithm::Sha256);
    }
}
