//! Destination filesystem checks.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Checks that the filesystem containing `destination` has at least
/// `required` bytes available. Missing destination directories are handled by
/// walking to the nearest existing ancestor. Filesystems that cannot report
/// free space are treated as unknown and do not fail the caller.
pub fn ensure_free_space(destination: &Path, required: u64) -> Result<()> {
    if required == 0 {
        return Ok(());
    }
    let start = if destination.is_file() {
        destination.parent().unwrap_or(destination)
    } else {
        destination
    };
    let Some(probe) = nearest_existing_path(start) else {
        return Ok(());
    };
    let available = match fs2::available_space(&probe) {
        Ok(available) => available,
        Err(error) => {
            tracing::debug!(path = %probe.display(), %error, "could not determine free disk space");
            return Ok(());
        }
    };
    if available < required {
        return Err(Error::Backend(format!(
            "insufficient disk space at {}: need {required} bytes, {available} available",
            probe.display()
        )));
    }
    Ok(())
}

fn nearest_existing_path(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.as_os_str().is_empty() {
            return None;
        }
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_insufficient_space() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_free_space(dir.path(), 1).is_ok());
        let error = ensure_free_space(dir.path(), u64::MAX).unwrap_err();
        assert!(
            error.to_string().contains("insufficient disk space"),
            "{error}"
        );
    }
}
