use std::path::{Path, PathBuf};

use shaul_core::{Checksum, Error, ResolvedOptions, Result, compute_file_checksum};

pub enum DestinationAction {
    Reuse,
    Create(PathBuf),
}

pub async fn prepare_destination(
    path: &Path,
    expected_size: Option<u64>,
    options: &ResolvedOptions,
) -> Result<DestinationAction> {
    if !tokio::fs::try_exists(path).await? {
        ensure_parent(path).await?;
        return Ok(DestinationAction::Create(path.to_path_buf()));
    }
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.is_file()
        && let Some(size) = expected_size
        && metadata.len() == size
    {
        return Ok(DestinationAction::Reuse);
    }
    if options.overwrite {
        remove_file_if_exists(path).await?;
        ensure_parent(path).await?;
        return Ok(DestinationAction::Create(path.to_path_buf()));
    }
    if options.rename_on_conflict {
        let unique = unique_path(path).await?;
        ensure_parent(&unique).await?;
        return Ok(DestinationAction::Create(unique));
    }
    Err(Error::Backend(format!(
        "destination already exists: {} (use --force or --rename)",
        path.display()
    )))
}

pub async fn validate_cached(
    path: &Path,
    expected_size: Option<u64>,
    expected_checksum: Option<&Checksum>,
) -> Result<bool> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(false);
    }
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Ok(false);
    }
    if let Some(size) = expected_size
        && metadata.len() != size
    {
        return Ok(false);
    }
    if let Some(expected) = expected_checksum {
        let actual = compute_file_checksum(path.to_path_buf(), expected.algorithm).await?;
        return Ok(expected.matches(&actual));
    }
    Ok(true)
}

pub async fn link_or_copy(source: &Path, target: &Path) -> Result<()> {
    if source == target {
        return Ok(());
    }
    ensure_parent(target).await?;
    remove_file_if_exists(target).await?;
    match tokio::fs::hard_link(source, target).await {
        Ok(()) => Ok(()),
        Err(_) => {
            tokio::fs::copy(source, target).await?;
            Ok(())
        }
    }
}

pub async fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

pub async fn remove_file_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

pub async fn unique_path(path: &Path) -> Result<PathBuf> {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..10_000 {
        let candidate = path.with_file_name(format!("{stem}-{index}{extension}"));
        if !tokio::fs::try_exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    Err(Error::Backend(
        "could not find a free destination name".to_string(),
    ))
}
