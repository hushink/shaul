use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use url::Url;

use crate::error::{Error, Result};

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Make a single filename safe on every supported platform.
pub fn sanitize_file_name(name: &str) -> String {
    let decoded = percent_decode_str(name).decode_utf8_lossy();
    let mut output = String::with_capacity(decoded.len());
    for character in decoded.chars() {
        if character == '/'
            || character == '\\'
            || character == ':'
            || character == '*'
            || character == '?'
            || character == '"'
            || character == '<'
            || character == '>'
            || character == '|'
            || character.is_control()
        {
            output.push('_');
        } else {
            output.push(character);
        }
    }

    let output = output.trim().trim_end_matches(['.', ' ']).to_string();
    let output = if output.is_empty() {
        "download".to_string()
    } else {
        output
    };

    let stem = output
        .split('.')
        .next()
        .unwrap_or(&output)
        .to_ascii_uppercase();
    let output = if WINDOWS_RESERVED.contains(&stem.as_str()) {
        format!("_{output}")
    } else {
        output
    };

    truncate_file_name(&output, 200)
}

fn truncate_file_name(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let keep = max_chars.saturating_sub(extension.chars().count());
    let stem: String = name.chars().take(keep).collect();
    format!("{stem}{extension}")
}

/// Validate a multi-file relative path (torrent/HF snapshot style). Every
/// component must be a normal filename; traversal and absolute paths are
/// rejected rather than silently rewritten.
pub fn sanitize_relative_path(path: &str) -> Result<String> {
    let path = Path::new(path);
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_string_lossy();
                let safe = sanitize_file_name(&value);
                if safe.is_empty() || safe == "." || safe == ".." {
                    return Err(Error::InvalidLink(format!(
                        "unsafe path component: {value}"
                    )));
                }
                components.push(safe);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::InvalidLink(format!(
                    "unsafe relative path: {}",
                    path.display()
                )));
            }
        }
    }
    if components.is_empty() {
        return Err(Error::InvalidLink("empty path".to_string()));
    }
    Ok(components.join("/"))
}

/// Derive a filename from the last URL path segment.
pub fn filename_from_url(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|segments| segments.rev().find(|segment| !segment.is_empty()))
        .unwrap_or("download");
    sanitize_file_name(last)
}

/// Strip common archive extensions to obtain a stable extraction directory
/// name.
pub fn archive_stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string());
    for suffix in [".tar.gz", ".tar.zst", ".tar", ".zip", ".gz", ".zst"] {
        if let Some(stem) = name.strip_suffix(suffix)
            && !stem.is_empty()
        {
            return sanitize_file_name(stem);
        }
    }
    sanitize_file_name(&name)
}

/// Join a destination directory and a validated relative path.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = sanitize_relative_path(relative)?;
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_windows_reserved_names() {
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
    }

    #[test]
    fn rejects_traversal() {
        assert!(sanitize_relative_path("../../etc/passwd").is_err());
        assert!(sanitize_relative_path("/etc/passwd").is_err());
        assert_eq!(
            sanitize_relative_path("model/weights.bin").unwrap(),
            "model/weights.bin"
        );
    }

    #[test]
    fn derives_filename() {
        let url = Url::parse("https://example.com/files/model%20v1.bin?x=1").unwrap();
        assert_eq!(filename_from_url(&url), "model v1.bin");
    }

    #[test]
    fn strips_archive_suffixes() {
        assert_eq!(archive_stem(Path::new("data.tar.gz")), "data");
        assert_eq!(archive_stem(Path::new("data.zip")), "data");
    }
}
