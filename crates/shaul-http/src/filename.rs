use percent_encoding::percent_decode_str;

/// Parse RFC 6266 `Content-Disposition`, preferring the UTF-8 `filename*`
/// form. The result is not yet sanitized; callers must do that.
pub fn parse_content_disposition(value: &str) -> Option<String> {
    for part in value.split(';').skip(1) {
        let part = part.trim();
        let lower = part.to_ascii_lowercase();
        if lower.starts_with("filename*=") {
            let rest = part["filename*=".len()..].trim().trim_matches('"');
            let mut pieces = rest.splitn(3, '\'');
            let _charset = pieces.next();
            let _language = pieces.next();
            if let Some(encoded) = pieces.next() {
                return Some(percent_decode_str(encoded).decode_utf8_lossy().to_string());
            }
        } else if lower.starts_with("filename=") {
            let rest = part["filename=".len()..].trim().trim_matches('"');
            return Some(percent_decode_str(rest).decode_utf8_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_filename() {
        assert_eq!(
            parse_content_disposition("attachment; filename=\"model.bin\""),
            Some("model.bin".to_string())
        );
    }

    #[test]
    fn parses_utf8_filename() {
        assert_eq!(
            parse_content_disposition("attachment; filename*=UTF-8''%E6%A8%A1%E5%9E%8B.bin"),
            Some("模型.bin".to_string())
        );
    }
}
