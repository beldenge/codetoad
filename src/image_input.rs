use crate::protocol::ChatImageAttachment;
use base64::Engine as _;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PreparedImageAttachment {
    pub chat_attachment: ChatImageAttachment,
    pub display_path: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct PreparedUserInput {
    pub text: String,
    pub attachments: Vec<PreparedImageAttachment>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AttachmentNotice {
    pub display_path: String,
    pub size_bytes: usize,
}

impl PreparedUserInput {
    pub fn into_chat_request(self) -> (String, Vec<ChatImageAttachment>) {
        (
            self.text,
            self.attachments
                .into_iter()
                .map(|attachment| attachment.chat_attachment)
                .collect(),
        )
    }

    pub fn attachment_notices(&self) -> Vec<AttachmentNotice> {
        self.attachments
            .iter()
            .map(|attachment| AttachmentNotice {
                display_path: attachment.display_path.clone(),
                size_bytes: attachment.size_bytes,
            })
            .collect()
    }
}

pub fn prepare_user_input(raw: &str, cwd: &Path) -> PreparedUserInput {
    let text = raw.trim().to_string();
    let mut warnings = Vec::new();
    let mut attachments = Vec::new();
    let mut seen = HashSet::<PathBuf>::new();

    let mut candidates = extract_markdown_image_candidates(&text);
    candidates.extend(extract_path_like_candidates(&text));

    for candidate in candidates {
        if let Some(prepared) = try_prepare_attachment(&candidate, cwd, &mut warnings)
            && seen.insert(prepared.0.clone())
        {
            attachments.push(prepared.1);
        }
    }

    PreparedUserInput {
        text,
        attachments,
        warnings,
    }
}

fn try_prepare_attachment(
    candidate: &str,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Option<(PathBuf, PreparedImageAttachment)> {
    let cleaned = trim_wrapping_quotes(candidate.trim());
    if cleaned.is_empty() {
        return None;
    }

    let path_like = maybe_decode_file_url(cleaned).unwrap_or_else(|| cleaned.to_string());
    let path = PathBuf::from(path_like);
    let resolved = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };

    let canonical = std::fs::canonicalize(&resolved).unwrap_or(resolved.clone());
    if !canonical.is_file() || !is_supported_image_path(&canonical) {
        return None;
    }

    let bytes = match std::fs::read(&canonical) {
        Ok(bytes) => bytes,
        Err(err) => {
            warnings.push(format!("Skipping image '{}': {err}", canonical.display()));
            return None;
        }
    };

    if bytes.len() > MAX_IMAGE_BYTES {
        warnings.push(format!(
            "Skipping image '{}': file is larger than {} MB",
            canonical.display(),
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
        return None;
    }

    let mime_type = mime_type_for_path(&canonical).unwrap_or("application/octet-stream");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let filename = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_string();
    let attachment = PreparedImageAttachment {
        chat_attachment: ChatImageAttachment {
            filename,
            mime_type: mime_type.to_string(),
            data_url: format!("data:{mime_type};base64,{encoded}"),
        },
        display_path: canonical.display().to_string(),
        size_bytes: bytes.len(),
    };

    Some((canonical, attachment))
}

fn extract_path_like_candidates(input: &str) -> Vec<String> {
    let mut candidates = tokenize_preserving_quotes(input)
        .into_iter()
        .map(|token| trim_path_punctuation(&token))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    candidates.extend(extract_absolute_image_path_candidates(input));
    candidates
}

fn extract_absolute_image_path_candidates(input: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut idx = 0usize;

    while idx < input.len() {
        let rest = &input[idx..];
        let mut consumed = 1usize;
        if let Some(path) = try_scan_file_url(rest) {
            consumed = path.len().max(1);
            candidates.push(path);
        } else if let Some(path) = try_scan_absolute_fs_path(rest) {
            consumed = path.len().max(1);
            candidates.push(path);
        }
        idx += consumed;
    }

    candidates
}

fn try_scan_file_url(rest: &str) -> Option<String> {
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with("file://") {
        return None;
    }

    let end = find_image_path_end(rest)?;
    Some(trim_path_punctuation(&rest[..end]))
}

fn try_scan_absolute_fs_path(rest: &str) -> Option<String> {
    let bytes = rest.as_bytes();
    let starts_with_unix_abs = bytes.first().is_some_and(|ch| *ch == b'/');
    let starts_with_windows_abs = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && is_path_sep(bytes[2]);
    if !starts_with_unix_abs && !starts_with_windows_abs {
        return None;
    }

    let end = find_image_path_end(rest)?;
    Some(trim_path_punctuation(&rest[..end]))
}

fn find_image_path_end(rest: &str) -> Option<usize> {
    const EXTENSIONS: [&str; 6] = [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp"];

    let lower = rest.to_ascii_lowercase();
    let mut best_end: Option<usize> = None;

    for ext in EXTENSIONS {
        let mut cursor = 0usize;
        while cursor < lower.len() {
            let Some(found_rel) = lower[cursor..].find(ext) else {
                break;
            };
            let ext_start = cursor + found_rel;
            let ext_end = ext_start + ext.len();
            let trailing = rest[ext_end..].chars().next();
            if trailing.is_none_or(is_path_terminator) {
                best_end = Some(best_end.map_or(ext_end, |current| current.min(ext_end)));
                break;
            }
            cursor = ext_end;
        }
    }

    best_end
}

fn is_path_terminator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '>' | ',' | ';')
}

fn is_path_sep(ch: u8) -> bool {
    ch == b'\\' || ch == b'/'
}

fn extract_markdown_image_candidates(input: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let bytes = input.as_bytes();
    let mut idx = 0usize;

    while idx + 1 < bytes.len() {
        if bytes[idx] == b'!' && bytes[idx + 1] == b'[' {
            let Some(alt_close) = input[idx + 2..].find(']') else {
                break;
            };
            let alt_close_idx = idx + 2 + alt_close;
            if input.as_bytes().get(alt_close_idx + 1) != Some(&b'(') {
                idx += 2;
                continue;
            }
            let path_start = alt_close_idx + 2;
            let Some(path_close_rel) = input[path_start..].find(')') else {
                break;
            };
            let path_end = path_start + path_close_rel;
            let candidate = input[path_start..path_end].trim();
            if !candidate.is_empty() {
                candidates.push(candidate.to_string());
            }
            idx = path_end + 1;
            continue;
        }
        idx += 1;
    }

    candidates
}

fn tokenize_preserving_quotes(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in input.chars() {
        match quote {
            Some(active) if ch == active => {
                quote = None;
                current.push(ch);
            }
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            None => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn trim_wrapping_quotes(value: &str) -> &str {
    value
        .trim()
        .trim_start_matches('"')
        .trim_end_matches('"')
        .trim_start_matches('\'')
        .trim_end_matches('\'')
}

fn trim_path_punctuation(value: &str) -> String {
    value
        .trim()
        .trim_end_matches([',', ';', ':', ')', ']'])
        .trim_start_matches(['(', '['])
        .to_string()
}

fn maybe_decode_file_url(value: &str) -> Option<String> {
    let normalized = value.strip_prefix("file://")?;
    let without_host = if let Some(rest) = normalized.strip_prefix('/') {
        if rest.len() >= 2 && rest.as_bytes()[1] == b':' {
            rest
        } else {
            normalized
        }
    } else {
        normalized
    };
    Some(percent_decode(without_host))
}

fn percent_decode(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%'
            && idx + 2 < bytes.len()
            && let Some(decoded) = decode_hex_pair(bytes[idx + 1], bytes[idx + 2])
        {
            result.push(decoded as char);
            idx += 3;
            continue;
        }
        result.push(bytes[idx] as char);
        idx += 1;
    }
    result
}

fn decode_hex_pair(high: u8, low: u8) -> Option<u8> {
    let high = from_hex(high)?;
    let low = from_hex(low)?;
    Some((high << 4) | low)
}

fn from_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn is_supported_image_path(path: &Path) -> bool {
    mime_type_for_path(path).is_some()
}

fn mime_type_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        extract_absolute_image_path_candidates, extract_markdown_image_candidates,
        maybe_decode_file_url, percent_decode, prepare_user_input,
    };
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_markdown_image_paths() {
        let text = "Review this ![shot](./screens/error.png) please";
        let paths = extract_markdown_image_candidates(text);
        assert_eq!(paths, vec!["./screens/error.png"]);
    }

    #[test]
    fn decodes_file_url() {
        assert_eq!(
            maybe_decode_file_url("file:///C:/tmp/test%20image.png").as_deref(),
            Some("C:/tmp/test image.png")
        );
    }

    #[test]
    fn percent_decode_keeps_plain_text() {
        assert_eq!(percent_decode("hello_world"), "hello_world");
    }

    #[test]
    fn extracts_unquoted_windows_absolute_path_with_spaces() {
        let text = "what is in C:\\Users\\george\\Desktop\\Screenshot 2026-02-12 164555.png now";
        let paths = extract_absolute_image_path_candidates(text);
        assert_eq!(
            paths,
            vec!["C:\\Users\\george\\Desktop\\Screenshot 2026-02-12 164555.png"]
        );
    }

    #[test]
    fn extracts_file_url_with_spaces() {
        let text = "check file:///C:/Users/george/Desktop/Screenshot%202026-02-12%20164555.png";
        let paths = extract_absolute_image_path_candidates(text);
        assert_eq!(
            paths,
            vec!["file:///C:/Users/george/Desktop/Screenshot%202026-02-12%20164555.png"]
        );
    }

    #[test]
    fn prepare_user_input_attaches_absolute_image_outside_cwd() {
        let cwd_dir = TempDir::new("image-input-cwd");
        let image_dir = TempDir::new("image-input-img");
        let image_path = image_dir.path().join("Screenshot 2026-02-12 164555.png");
        std::fs::write(&image_path, b"fake-png").expect("write image");

        let input = format!("what do you see {}", image_path.display());
        let prepared = prepare_user_input(&input, cwd_dir.path());
        assert_eq!(prepared.attachments.len(), 1);
        assert_eq!(prepared.warnings.len(), 0);
    }

    #[test]
    fn prepare_user_input_deduplicates_same_image_candidate() {
        let cwd_dir = TempDir::new("image-input-dedup-cwd");
        let image_dir = TempDir::new("image-input-dedup-img");
        let image_path = image_dir.path().join("shot.png");
        std::fs::write(&image_path, b"fake-png").expect("write image");

        let input = format!(
            "review {} and again ![same]({})",
            image_path.display(),
            image_path.display()
        );
        let prepared = prepare_user_input(&input, cwd_dir.path());
        assert_eq!(prepared.attachments.len(), 1);
    }

    #[test]
    fn absolute_path_candidate_ignores_non_image_extensions() {
        let text = "look at C:\\tmp\\notes.txt";
        let paths = extract_absolute_image_path_candidates(text);
        assert!(paths.is_empty());
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            path.push(format!(
                "codetoad-{}-{}-{}",
                prefix,
                std::process::id(),
                nanos
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
