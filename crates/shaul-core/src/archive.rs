use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::path::archive_stem;

const MAX_ARCHIVE_ENTRIES: u64 = 100_000;
const MAX_EXTRACTED_BYTES: u64 = 100 * 1024 * 1024 * 1024;

/// Archive formats recognized by [`detect_archive`] and handled by
/// [`extract_archive`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArchiveKind {
    /// ZIP archive.
    Zip,
    /// Uncompressed tar archive.
    Tar,
    /// Gzip-compressed tar archive.
    TarGz,
    /// Zstandard-compressed tar archive.
    TarZst,
    /// Single gzip-compressed file.
    Gzip,
    /// Single zstd-compressed file.
    Zstd,
}

impl ArchiveKind {
    /// Returns `true` for formats that wrap one file rather than a directory
    /// tree.
    pub fn is_single_file(self) -> bool {
        matches!(self, Self::Gzip | Self::Zstd)
    }
}

/// Summary of an extraction.
#[derive(Clone, Debug)]
pub struct ExtractReport {
    /// Path the archive contents were written to. Custom extractors must return
    /// a path under [`ExtractorRequest::extract_root`].
    pub target: PathBuf,
    /// Number of file entries extracted.
    pub files_extracted: u64,
    /// Non-fatal issues, such as skipped symlinks.
    pub warnings: Vec<String>,
}

/// Read-only inputs supplied to an [`ArchiveExtractor`].
#[non_exhaustive]
pub struct ExtractorRequest<'a> {
    /// Archive file to read.
    pub path: &'a Path,
    /// Directory the extractor may write into. Writes outside this directory
    /// are rejected by the registry after extraction, and are a contract
    /// violation for the implementation.
    pub extract_root: &'a Path,
    /// Whether an existing extraction target may be replaced.
    pub overwrite: bool,
}

impl<'a> ExtractorRequest<'a> {
    /// Creates a request for `path` rooted at `extract_root`.
    pub fn new(path: &'a Path, extract_root: &'a Path, overwrite: bool) -> Self {
        Self {
            path,
            extract_root,
            overwrite,
        }
    }
}

/// One archive format implementation used by [`ExtractorRegistry`].
///
/// Extractors run on a blocking worker. Implementations must keep every write
/// under [`ExtractorRequest::extract_root`], reject unsafe entry paths, and
/// should extract into a temporary directory before renaming the final target
/// into place.
pub trait ArchiveExtractor: Send + Sync {
    /// Stable name used in diagnostics.
    fn name(&self) -> &'static str;

    /// Returns `true` when this extractor handles `path`.
    ///
    /// The registry calls this in order and the first match owns the file.
    /// Implementations should be cheap and free of side effects; return
    /// `Ok(false)` for unsupported files and reserve `Err` for real I/O or
    /// detection failures.
    fn supports(&self, path: &Path) -> Result<bool>;

    /// Extracts the archive described by `request`.
    fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport>;
}

/// Extractor for the archive formats built into shaul: zip, tar, tar.gz,
/// tar.zst, gzip and zstd.
pub struct BuiltinExtractor;

impl ArchiveExtractor for BuiltinExtractor {
    fn name(&self) -> &'static str {
        "builtin"
    }

    fn supports(&self, path: &Path) -> Result<bool> {
        Ok(detect_archive(path)?.is_some())
    }

    fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
        let kind = detect_archive(request.path)?.ok_or_else(|| {
            Error::Extraction(format!("unsupported archive: {}", request.path.display()))
        })?;
        extract_archive(request.path, kind, request.extract_root, request.overwrite)
    }
}

/// Ordered list of archive extractors.
///
/// [`ExtractorRegistry::default`] contains [`BuiltinExtractor`]. Use
/// [`ExtractorRegistry::with`] to try a custom extractor before the built-ins,
/// or [`ExtractorRegistry::new`] for an explicit list.
#[derive(Clone)]
pub struct ExtractorRegistry {
    extractors: Vec<Arc<dyn ArchiveExtractor>>,
}

impl Default for ExtractorRegistry {
    fn default() -> Self {
        Self {
            extractors: vec![Arc::new(BuiltinExtractor)],
        }
    }
}

impl ExtractorRegistry {
    /// Creates a registry containing exactly `extractors`, in order.
    pub fn new(extractors: Vec<Arc<dyn ArchiveExtractor>>) -> Self {
        Self { extractors }
    }

    /// Prepends `extractor` so it is tried before the existing entries.
    pub fn with(mut self, extractor: Arc<dyn ArchiveExtractor>) -> Self {
        self.extractors.insert(0, extractor);
        self
    }

    /// Returns `true` when an extractor claims `path`.
    pub fn supports(&self, path: &Path) -> Result<bool> {
        for extractor in &self.extractors {
            if extractor.supports(path)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Extracts `path` with the first matching extractor.
    ///
    /// Returns `Ok(None)` when no extractor supports the file. The final target
    /// reported by the extractor is checked to be inside `extract_root`.
    pub fn extract(
        &self,
        path: &Path,
        extract_root: &Path,
        overwrite: bool,
    ) -> Result<Option<ExtractReport>> {
        for extractor in &self.extractors {
            if extractor.supports(path)? {
                std::fs::create_dir_all(extract_root)?;
                let request = ExtractorRequest::new(path, extract_root, overwrite);
                let report = extractor.extract(&request)?;
                validate_extract_target(extract_root, &report.target)?;
                return Ok(Some(report));
            }
        }
        Ok(None)
    }
}

fn validate_extract_target(extract_root: &Path, target: &Path) -> Result<()> {
    let root = extract_root.canonicalize()?;
    let target = target.canonicalize()?;
    if !target.starts_with(&root) {
        return Err(Error::Extraction(format!(
            "extractor wrote outside the extraction root: {}",
            target.display()
        )));
    }
    Ok(())
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
                let decoder = flate2::read::GzDecoder::new(input);
                let mut limited = decoder.take(MAX_EXTRACTED_BYTES + 1);
                let copied = std::io::copy(&mut limited, &mut output)?;
                ensure_extracted_bytes(copied)?;
            }
            ArchiveKind::Zstd => {
                let decoder = ruzstd::decoding::StreamingDecoder::new(input)
                    .map_err(|error| Error::Extraction(format!("zstd: {error}")))?;
                let mut limited = decoder.take(MAX_EXTRACTED_BYTES + 1);
                let copied = std::io::copy(&mut limited, &mut output)?;
                ensure_extracted_bytes(copied)?;
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
    ensure_archive_entries(archive.len() as u64)?;
    let mut files = 0u64;
    let mut extracted_bytes = 0u64;
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
        let remaining = MAX_EXTRACTED_BYTES.saturating_sub(extracted_bytes);
        let mut writer = File::create(&output)?;
        let copied = std::io::copy(&mut (&mut entry).take(remaining + 1), &mut writer)?;
        ensure_extracted_bytes(extracted_bytes.saturating_add(copied))?;
        extracted_bytes += copied;
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
    let mut entries_seen = 0u64;
    let mut extracted_bytes = 0u64;
    for entry in archive
        .entries()
        .map_err(|error| Error::Extraction(format!("tar: {error}")))?
    {
        entries_seen += 1;
        ensure_archive_entries(entries_seen)?;
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
        let size = entry.header().size().unwrap_or(0);
        extracted_bytes = extracted_bytes.saturating_add(size);
        ensure_extracted_bytes(extracted_bytes)?;
        entry
            .unpack(&output)
            .map_err(|error| Error::Extraction(format!("tar unpack: {error}")))?;
        files += 1;
    }
    Ok(files)
}

fn ensure_archive_entries(entries: u64) -> Result<()> {
    if entries > MAX_ARCHIVE_ENTRIES {
        return Err(Error::Extraction(format!(
            "archive has more than {MAX_ARCHIVE_ENTRIES} entries"
        )));
    }
    Ok(())
}

fn ensure_extracted_bytes(bytes: u64) -> Result<()> {
    if bytes > MAX_EXTRACTED_BYTES {
        return Err(Error::Extraction(format!(
            "archive expands beyond {MAX_EXTRACTED_BYTES} bytes"
        )));
    }
    Ok(())
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
    fn rejects_archives_over_extraction_limits() {
        assert!(ensure_archive_entries(MAX_ARCHIVE_ENTRIES).is_ok());
        assert!(ensure_archive_entries(MAX_ARCHIVE_ENTRIES + 1).is_err());
        assert!(ensure_extracted_bytes(MAX_EXTRACTED_BYTES).is_ok());
        assert!(ensure_extracted_bytes(MAX_EXTRACTED_BYTES + 1).is_err());
    }

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

    struct DemoExtractor;

    impl ArchiveExtractor for DemoExtractor {
        fn name(&self) -> &'static str {
            "demo"
        }

        fn supports(&self, path: &Path) -> Result<bool> {
            Ok(path.extension().and_then(|extension| extension.to_str()) == Some("demo"))
        }

        fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
            let target = request.extract_root.join("demo-out");
            std::fs::create_dir_all(&target)?;
            std::fs::write(target.join("hello.txt"), b"demo")?;
            Ok(ExtractReport {
                target,
                files_extracted: 1,
                warnings: Vec::new(),
            })
        }
    }

    struct OverrideBuiltin;

    impl ArchiveExtractor for OverrideBuiltin {
        fn name(&self) -> &'static str {
            "override"
        }

        fn supports(&self, path: &Path) -> Result<bool> {
            Ok(path.to_string_lossy().ends_with(".tar.gz"))
        }

        fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
            let target = request.extract_root.join("override-out");
            std::fs::create_dir_all(&target)?;
            std::fs::write(target.join("override.txt"), b"override")?;
            Ok(ExtractReport {
                target,
                files_extracted: 1,
                warnings: Vec::new(),
            })
        }
    }

    #[test]
    fn custom_extractor_is_selected_and_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.demo");
        std::fs::write(&path, b"demo").unwrap();
        let registry = ExtractorRegistry::default().with(Arc::new(DemoExtractor));

        assert!(registry.supports(&path).unwrap());
        let report = registry.extract(&path, dir.path(), false).unwrap().unwrap();
        assert_eq!(report.files_extracted, 1);
        assert_eq!(
            std::fs::read_to_string(report.target.join("hello.txt")).unwrap(),
            "demo"
        );
    }

    #[test]
    fn custom_extractor_precedes_builtins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.tar.gz");
        std::fs::write(&path, b"not really a tarball").unwrap();
        let registry = ExtractorRegistry::default().with(Arc::new(OverrideBuiltin));

        let report = registry.extract(&path, dir.path(), false).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(report.target.join("override.txt")).unwrap(),
            "override"
        );
    }

    #[test]
    fn registry_returns_none_when_no_extractor_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.bin");
        std::fs::write(&path, b"plain").unwrap();
        let registry = ExtractorRegistry::new(Vec::new());

        assert!(!registry.supports(&path).unwrap());
        assert!(
            registry
                .extract(&path, dir.path(), false)
                .unwrap()
                .is_none()
        );
    }

    struct EscapingExtractor;

    impl ArchiveExtractor for EscapingExtractor {
        fn name(&self) -> &'static str {
            "escaping"
        }

        fn supports(&self, _path: &Path) -> Result<bool> {
            Ok(true)
        }

        fn extract(&self, request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
            let target = request.extract_root.parent().unwrap().join("escaped");
            std::fs::create_dir_all(&target)?;
            std::fs::write(target.join("outside.txt"), b"outside")?;
            Ok(ExtractReport {
                target,
                files_extracted: 1,
                warnings: Vec::new(),
            })
        }
    }

    #[test]
    fn extractor_target_must_stay_under_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("input.bin");
        std::fs::write(&path, b"input").unwrap();
        let registry = ExtractorRegistry::new(vec![Arc::new(EscapingExtractor)]);

        let error = registry.extract(&path, &root, false).unwrap_err();
        assert!(matches!(error, Error::Extraction(_)));
    }

    struct FailingExtractor;

    impl ArchiveExtractor for FailingExtractor {
        fn name(&self) -> &'static str {
            "failing"
        }

        fn supports(&self, _path: &Path) -> Result<bool> {
            Ok(true)
        }

        fn extract(&self, _request: &ExtractorRequest<'_>) -> Result<ExtractReport> {
            Err(Error::Extraction("boom".to_string()))
        }
    }

    #[test]
    fn a_claiming_extractor_failure_does_not_fall_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.tar.gz");
        std::fs::write(&path, b"not really a tarball").unwrap();
        let registry = ExtractorRegistry::default().with(Arc::new(FailingExtractor));

        let error = registry.extract(&path, dir.path(), false).unwrap_err();
        assert!(matches!(error, Error::Extraction(message) if message == "boom"));
    }
}
