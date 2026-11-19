//! Content-addressed cache for verified downloads.
//!
//! The cache is opt-in and only used when both a checksum and a size are
//! known. Entries are keyed by `<algorithm>/<hex>` and can be hardlinked or
//! copied into a task destination. A cache hit is verified again before it is
//! used, so a corrupted cache entry cannot silently satisfy a download.

use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use crate::checksum::{Checksum, compute_file_checksum};
use crate::error::{Error, Result};

/// A content-addressed cache rooted at a directory.
#[derive(Clone, Debug)]
pub struct ContentCache {
    root: PathBuf,
}

/// Aggregate size information for a content cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CacheStats {
    /// Number of regular cache entries.
    pub entries: u64,
    /// Total size of those entries in bytes.
    pub bytes: u64,
    /// Number of incomplete temporary files left behind by an interrupted
    /// insertion.
    pub temp_files: u64,
}

impl ContentCache {
    /// Creates a cache rooted at `root`, creating the directory if needed.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Returns the cache root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the path an entry with `checksum` would use.
    pub fn entry_path(&self, checksum: &Checksum) -> Result<PathBuf> {
        validate_checksum(checksum)?;
        let prefix = &checksum.hex[..2];
        Ok(self
            .root
            .join(checksum.algorithm.as_str())
            .join(prefix)
            .join(&checksum.hex))
    }

    /// Looks up an entry and verifies its digest and size. A mismatching entry
    /// is removed and reported as a miss.
    pub async fn lookup_verified(&self, checksum: &Checksum, size: u64) -> Result<Option<PathBuf>> {
        let path = self.entry_path(checksum)?;
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::Io(error)),
        };
        if !metadata.is_file() || metadata.len() != size {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
        let actual = compute_file_checksum(path.clone(), checksum.algorithm).await?;
        if checksum.matches(&actual) {
            Ok(Some(path))
        } else {
            let _ = tokio::fs::remove_file(&path).await;
            Ok(None)
        }
    }

    /// Inserts a verified file into the cache. If an entry already exists with
    /// the expected size it is reused.
    pub async fn insert(&self, checksum: &Checksum, source: &Path, size: u64) -> Result<PathBuf> {
        let target = self.entry_path(checksum)?;
        if source == target {
            return Ok(target);
        }
        let source_metadata = tokio::fs::metadata(source).await?;
        if !source_metadata.is_file() || source_metadata.len() != size {
            return Err(Error::Config(format!(
                "cache source {} has the wrong size",
                source.display()
            )));
        }
        match tokio::fs::metadata(&target).await {
            Ok(metadata) if metadata.is_file() && metadata.len() == size => {
                return Ok(target);
            }
            Ok(_) => {
                let _ = tokio::fs::remove_file(&target).await;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::Io(error)),
        }

        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temp = self.root.join(format!(".tmp-{}", Uuid::new_v4()));
        if tokio::fs::hard_link(source, &temp).await.is_err() {
            tokio::fs::copy(source, &temp).await?;
        }
        match tokio::fs::rename(&temp, &target).await {
            Ok(()) => Ok(target),
            Err(_) => {
                let _ = tokio::fs::remove_file(&temp).await;
                if let Ok(metadata) = tokio::fs::metadata(&target).await
                    && metadata.is_file()
                    && metadata.len() == size
                {
                    return Ok(target);
                }
                Err(Error::Io(std::io::Error::other(format!(
                    "could not publish cache entry {}",
                    target.display()
                ))))
            }
        }
    }

    /// Hardlinks or copies a cached file to `destination`. The caller is
    /// responsible for applying overwrite/rename conflict policy first.
    pub async fn materialize(&self, cached: &Path, destination: &Path, size: u64) -> Result<()> {
        if cached == destination {
            return Ok(());
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let _ = tokio::fs::remove_file(destination).await;
        match tokio::fs::hard_link(cached, destination).await {
            Ok(()) => Ok(()),
            Err(_) => {
                let temp = destination.with_file_name(format!(
                    ".{}.tmp-{}",
                    destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("download"),
                    Uuid::new_v4()
                ));
                tokio::fs::copy(cached, &temp).await?;
                match tokio::fs::rename(&temp, destination).await {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&temp).await;
                        Err(Error::Io(error))
                    }
                }
            }
        }
        .and_then(|()| {
            let metadata = std::fs::metadata(destination)?;
            if metadata.len() == size {
                Ok(())
            } else {
                Err(Error::Config(format!(
                    "cached file {} has the wrong size",
                    destination.display()
                )))
            }
        })
    }

    /// Returns aggregate entry and byte counts.
    pub async fn stats(&self) -> Result<CacheStats> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || stats_blocking(&root))
            .await
            .map_err(|error| Error::Other(format!("cache stats task failed: {error}")))?
    }

    /// Removes all cache entries and temporary files, returning the number of
    /// bytes removed.
    pub async fn clear(&self) -> Result<u64> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || clear_blocking(&root))
            .await
            .map_err(|error| Error::Other(format!("cache clean task failed: {error}")))?
    }

    /// Removes incomplete temporary files, returning the number removed.
    pub async fn prune_temp(&self) -> Result<u64> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || prune_temp_blocking(&root))
            .await
            .map_err(|error| Error::Other(format!("cache prune task failed: {error}")))?
    }
}

fn validate_checksum(checksum: &Checksum) -> Result<()> {
    if checksum.hex.len() != checksum.algorithm.hex_len() || hex::decode(&checksum.hex).is_err() {
        return Err(Error::Config(format!(
            "invalid {} checksum: {}",
            checksum.algorithm, checksum.hex
        )));
    }
    Ok(())
}

fn stats_blocking(root: &Path) -> Result<CacheStats> {
    let mut stats = CacheStats::default();
    walk(root, &mut stats)?;
    Ok(stats)
}

fn walk(path: &Path, stats: &mut CacheStats) -> Result<()> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::Io(error)),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk(&path, stats)?;
        } else if metadata.is_file() {
            let temp = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".tmp-"));
            if temp {
                stats.temp_files += 1;
            } else {
                stats.entries += 1;
                stats.bytes += metadata.len();
            }
        }
    }
    Ok(())
}

fn clear_blocking(root: &Path) -> Result<u64> {
    let stats = stats_blocking(root)?;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(Error::Io(error)),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(stats.bytes)
}

fn prune_temp_blocking(root: &Path) -> Result<u64> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(Error::Io(error)),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".tmp-"))
        {
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::ChecksumAlgorithm;

    fn checksum(data: &[u8]) -> Checksum {
        Checksum {
            algorithm: ChecksumAlgorithm::Sha256,
            hex: {
                use sha2::{Digest, Sha256};
                hex::encode(Sha256::digest(data))
            },
        }
    }

    #[tokio::test]
    async fn inserts_and_looks_up_verified_content() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let data = b"cached content";
        tokio::fs::write(&source, data).await.unwrap();
        let checksum = checksum(data);
        let cache = ContentCache::new(dir.path().join("cache")).unwrap();

        let inserted = cache
            .insert(&checksum, &source, data.len() as u64)
            .await
            .unwrap();
        assert!(inserted.is_file());
        assert_eq!(
            cache
                .lookup_verified(&checksum, data.len() as u64)
                .await
                .unwrap(),
            Some(inserted)
        );
        assert!(
            cache
                .lookup_verified(&checksum, data.len() as u64 + 1)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejected_entries_are_removed_and_stats_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ContentCache::new(dir.path()).unwrap();
        let data = b"good";
        let checksum = checksum(data);
        let entry = cache.entry_path(&checksum).unwrap();
        tokio::fs::create_dir_all(entry.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&entry, b"bad!").await.unwrap();
        assert!(cache.lookup_verified(&checksum, 4).await.unwrap().is_none());
        assert!(!entry.exists());
    }

    #[tokio::test]
    async fn clear_removes_entries_and_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache_root = dir.path().join("cache");
        let cache = ContentCache::new(&cache_root).unwrap();
        let source = dir.path().join("source.bin");
        let data = b"clear me";
        tokio::fs::write(&source, data).await.unwrap();
        cache
            .insert(&checksum(data), &source, data.len() as u64)
            .await
            .unwrap();
        tokio::fs::write(cache_root.join(".tmp-leftover"), b"junk")
            .await
            .unwrap();
        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.temp_files, 1);
        assert_eq!(cache.clear().await.unwrap(), data.len() as u64);
        assert_eq!(cache.stats().await.unwrap(), CacheStats::default());
    }
}
