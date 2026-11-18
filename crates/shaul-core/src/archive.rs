use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::path::archive_stem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    TarZst,
    Gzip,
    Zstd,
}

impl ArchiveKind {
    pub fn is_single_file(self) -> bool {
        matches!(self, Self::Gzip | Self::Zstd)
    }
}

#[derive(Clone, Debug)]
pub struct ExtractReport {
    pub target: PathBuf,
    pub files_extracted: u64,
    pub warnings: Vec<String>,
}

/// Detect by magic bytes, using the extension only for legacy tarballs that
/// lack the `ustar` marker.
pub fn detect_archive(path: &Path) -> Result<Option<ArchiveKind>> {
    let mut file = File::open(path)?;
    let mut prefix = [0u8; 512];
    let read = file.read(&mut prefix)?;
    let prefix = &prefix[..read];

    if prefix.starts_with(b"PK\x03\x04")
        || prefix.starts_with(b"PK\x05\x06")
        || prefix.starts_with(b"PK\x07\x08")
    {
        return Ok(Some(ArchiveKind::Zip));
    }

    if prefix.starts_with(&[0x1f, 0x8b]) {
        return Ok(Some(if decompressed_is_tar_gzip(prefix) {
            ArchiveKind::TarGz
        } else {
            ArchiveKind::Gzip
        }));
    }

    if prefix.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Ok(Some(if decompressed_is_tar_zstd(prefix) {
            ArchiveKind::TarZst
        } else {
            ArchiveKind::Zstd
        }));
    }

    if is_tar(prefix) {
        return Ok(Some(ArchiveKind::Tar));
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "tar" {
        return Ok(Some(ArchiveKind::Tar));
    }

    Ok(None)
}

fn is_tar(bytes: &[u8]) -> bool {
    bytes.len() >= 262 && &bytes[257..262] == b"ustar"
}

fn decompressed_is_tar_gzip(prefix: &[u8]) -> bool {
    let mut decoder = flate2::read::GzDecoder::new(prefix);
    let mut decoded = [0u8; 512];
    let read = decoder.read(&mut decoded).unwrap_or(0);
    is_tar(&decoded[..read])
}

fn decompressed_is_tar_zstd(prefix: &[u8]) -> bool {
    let Ok(mut decoder) = ruzstd::decoding::StreamingDecoder::new(prefix) else {
        return false;
    };
    let mut decoded = [0u8; 512];
    let read = decoder.read(&mut decoded).unwrap_or(0);
    is_tar(&decoded[..read])
}

/// Extract into `extract_root`. Multi-entry archives land under a directory
/// named after the archive stem; single-file `.gz`/`.zst` archives land at
/// `<stem>`. Extraction is performed in a temporary directory and renamed
/// into place only after it succeeds.
pub fn extract_archive(
    path: &Path,
    kind: ArchiveKind,
    extract_root: &Path,
    overwrite: bool,
) -> Result<ExtractReport> {
    std::fs::create_dir_all(extract_root)?;
    let stem = archive_stem(path);
    if kind.is_single_file() {
        extract_single_file(path, kind, extract_root, &stem, overwrite)
    } else {
        extract_directory(path, kind, extract_root, &stem, overwrite)
    }
}

fn extract_single_file(
    path: &Path,
    kind: ArchiveKind,
    extract_root: &Path,
    stem: &str,
    overwrite: bool,
) -> Result<ExtractReport> {
    let target = extract_root.join(stem);
    prepare_target(&target, overwrite)?;

    let temp_dir = extract_root.join(format!(".shaul-extract-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)?;
    let temp_file = temp_dir.join(stem);

    let result = (|| -> Result<()> {
        let input = File::open(path)?;
        let mut output = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_file)?;
        match kind {
            ArchiveKind::Gzip => {
                let mut decoder = flate2::read::GzDecoder::new(input);
                std::io::copy(&mut decoder, &mut output)?;
            }
            ArchiveKind::Zstd => {
                let mut decoder = ruzstd::decoding::StreamingDecoder::new(input)
                    .map_err(|error| Error::Extraction(format!("zstd: {error}")))?;
                std::io::copy(&mut decoder, &mut output)?;
            }
            other => {
                return Err(Error::Extraction(format!(
                    "unsupported single-file archive: {other:?}"
                )));
            }
        }
        output.flush()?;
        output.sync_all()?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(error);
    }

    if target.exists() {
        remove_existing(&target)?;
    }
    std::fs::rename(&temp_file, &target)?;
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(ExtractReport {
        target,
        files_extracted: 1,
        warnings: Vec::new(),
    })
}

fn extract_directory(
    path: &Path,
    kind: ArchiveKind,
    extract_root: &Path,
    stem: &str,
    overwrite: bool,
) -> Result<ExtractReport> {
    let target = extract_root.join(stem);
    prepare_target(&target, overwrite)?;

    let temp_dir = extract_root.join(format!(".shaul-extract-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)?;

    let mut warnings = Vec::new();
    let result = match kind {
        ArchiveKind::Zip => extract_zip(path, &temp_dir, &mut warnings),
        ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarZst => {
            extract_tar(path, kind, &temp_dir, &mut warnings)
        }
        other => Err(Error::Extraction(format!(
            "unsupported archive kind: {other:?}"
        ))),
    };

    let files_extracted = match result {
        Ok(count) => count,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(error);
        }
    };

    if target.exists() {
        remove_existing(&target)?;
    }
    std::fs::rename(&temp_dir, &target)?;
    Ok(ExtractReport {
        target,
        files_extracted,
        warnings,
    })
}

fn prepare_target(target: &Path, overwrite: bool) -> Result<()> {
    if target.exists() && !overwrite {
        return Err(Error::Extraction(format!(
            "extraction target already exists: {} (use --force to overwrite)",
            target.display()
        )));
    }
    Ok(())
}

fn remove_existing(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn extract_zip(path: &Path, target: &Path, warnings: &mut Vec<String>) -> Result<u64> {
    let file = File::open(path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| Error::Extraction(format!("zip: {error}")))?;
    let mut files = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::Extraction(format!("zip entry {index}: {error}")))?;
        let Some(name) = entry.enclosed_name() else {
            let display = entry
                .name()
                .map(|name| name.to_string())
                .unwrap_or_else(|_| "<invalid>".to_string());
            return Err(Error::Extraction(format!(
                "unsafe zip entry path: {display}"
            )));
        };
        if name.as_os_str().is_empty() {
            continue;
        }
        if entry.is_symlink() {
            warnings.push(format!("skipped symlink entry: {}", name.display()));
            continue;
        }
        let output = target.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if !path_is_safe(&name) {
            return Err(Error::Extraction(format!(
                "unsafe zip entry path: {}",
                name.display()
            )));
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer = File::create(&output)?;
        std::io::copy(&mut entry, &mut writer)?;
        files += 1;
    }
    Ok(files)
}

fn extract_tar(
    path: &Path,
    kind: ArchiveKind,
    target: &Path,
    warnings: &mut Vec<String>,
) -> Result<u64> {
    let file = File::open(path)?;
    let reader: Box<dyn Read> = match kind {
        ArchiveKind::Tar => Box::new(file),
        ArchiveKind::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
        ArchiveKind::TarZst => Box::new(
            ruzstd::decoding::StreamingDecoder::new(file)
                .map_err(|error| Error::Extraction(format!("zstd: {error}")))?,
        ),
        other => {
            return Err(Error::Extraction(format!(
                "unsupported tar kind: {other:?}"
            )));
        }
    };
    let mut archive = tar::Archive::new(reader);
    let mut files = 0u64;
    for entry in archive
        .entries()
        .map_err(|error| Error::Extraction(format!("tar: {error}")))?
    {
        let mut entry = entry.map_err(|error| Error::Extraction(format!("tar entry: {error}")))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink()
            || entry_type.is_hard_link()
            || entry_type.is_block_special()
            || entry_type.is_character_special()
            || entry_type.is_fifo()
        {
            let path = entry
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "<invalid>".to_string());
            warnings.push(format!("skipped special tar entry: {path}"));
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| Error::Extraction(format!("tar path: {error}")))?
            .into_owned();
        if !path_is_safe(&path) {
            return Err(Error::Extraction(format!(
                "unsafe tar entry path: {}",
                path.display()
            )));
        }
        let output = target.join(&path);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry
            .unpack(&output)
            .map_err(|error| Error::Extraction(format!("tar unpack: {error}")))?;
        files += 1;
    }
    Ok(files)
}

fn path_is_safe(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[allow(dead_code)]
fn _unused_read_write(_: &mut dyn Read, _: &mut dyn Write) {}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn extracts_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("data.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "hello.txt", &b"hello"[..])
            .unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();

        assert_eq!(
            detect_archive(&archive_path).unwrap(),
            Some(ArchiveKind::TarGz)
        );
        let report = extract_archive(&archive_path, ArchiveKind::TarGz, dir.path(), false).unwrap();
        assert!(report.target.join("hello.txt").exists());
        assert_eq!(
            std::fs::read_to_string(report.target.join("hello.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn extracts_single_gzip() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("hello.txt.gz");
        let mut encoder =
            GzEncoder::new(File::create(&archive_path).unwrap(), Compression::default());
        encoder.write_all(b"hello").unwrap();
        encoder.finish().unwrap();

        assert_eq!(
            detect_archive(&archive_path).unwrap(),
            Some(ArchiveKind::Gzip)
        );
        let report = extract_archive(&archive_path, ArchiveKind::Gzip, dir.path(), false).unwrap();
        assert_eq!(std::fs::read_to_string(report.target).unwrap(), "hello");
    }

    #[test]
    fn does_not_overwrite_without_permission() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("data.tar.gz");
        let file = File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "file", &b"x"[..]).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
        extract_archive(&archive_path, ArchiveKind::TarGz, dir.path(), false).unwrap();
        assert!(extract_archive(&archive_path, ArchiveKind::TarGz, dir.path(), false).is_err());
    }

    #[test]
    fn extracts_zip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.zip");
        {
            let file = File::create(&path).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            archive.start_file("hello.txt", options).unwrap();
            archive.write_all(b"hello").unwrap();
            archive.finish().unwrap();
        }
        assert_eq!(detect_archive(&path).unwrap(), Some(ArchiveKind::Zip));
        let report = extract_archive(&path, ArchiveKind::Zip, dir.path(), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(report.target.join("hello.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn detects_nothing_for_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.bin");
        std::fs::write(&path, [0u8; 512]).unwrap();
        assert_eq!(detect_archive(&path).unwrap(), None);
    }
}
