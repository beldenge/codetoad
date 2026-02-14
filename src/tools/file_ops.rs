use super::ToolResult;
use crate::tool_context::ToolContext;
use anyhow::{Context, Result};
use serde_json::Value;
use similar::TextDiff;
use std::fs;

pub(super) fn execute_view_file(args: &Value, tool_context: &ToolContext) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;

    let resolved = tool_context.resolve_path(path)?;
    if !resolved.exists() {
        return Ok(ToolResult::err(format!(
            "File or directory not found: {path}"
        )));
    }

    if resolved.is_dir() {
        let mut names = Vec::new();
        for entry in fs::read_dir(resolved)? {
            let entry = entry?;
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        names.sort();
        return Ok(ToolResult::ok(format!(
            "Directory contents of {path}:\n{}",
            names.join("\n")
        )));
    }

    let content = fs::read_to_string(&resolved)
        .with_context(|| format!("Failed reading file {}", resolved.display()))?;
    let lines: Vec<&str> = content.lines().collect();

    let start = args
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let end = args
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|value| value as usize);

    if let (Some(start), Some(end)) = (start, end) {
        if start == 0 || end < start {
            return Ok(ToolResult::err("Invalid line range"));
        }
        let selected: Vec<String> = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                let line_no = index + 1;
                if line_no >= start && line_no <= end {
                    Some(format!("{line_no}: {line}"))
                } else {
                    None
                }
            })
            .collect();
        return Ok(ToolResult::ok(format!(
            "Lines {start}-{end} of {path}:\n{}",
            selected.join("\n")
        )));
    }

    let limit = 10usize;
    let display = lines
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, line)| format!("{}: {}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = if lines.len() > limit {
        format!("\n... +{} lines", lines.len() - limit)
    } else {
        String::new()
    };

    Ok(ToolResult::ok(format!(
        "Contents of {path}:\n{display}{suffix}"
    )))
}

pub(super) fn execute_create_file(args: &Value, tool_context: &ToolContext) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .context("Missing 'content' argument")?;

    let resolved = tool_context.resolve_path(path)?;
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory {}", parent.display()))?;
    }
    fs::write(&resolved, content)
        .with_context(|| format!("Failed writing file {}", resolved.display()))?;

    let created = TextDiff::from_lines("", content)
        .unified_diff()
        .header("/dev/null", &format!("b/{path}"))
        .to_string();

    Ok(ToolResult::ok(format!("Created {path}\n{created}")))
}

pub(super) fn execute_str_replace_editor(
    args: &Value,
    tool_context: &ToolContext,
) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;
    let old_str = args
        .get("old_str")
        .and_then(Value::as_str)
        .context("Missing 'old_str' argument")?;
    let new_str = args
        .get("new_str")
        .and_then(Value::as_str)
        .context("Missing 'new_str' argument")?;
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let resolved = tool_context.resolve_path(path)?;
    if !resolved.exists() {
        return Ok(ToolResult::err(format!("File not found: {path}")));
    }

    let original = fs::read_to_string(&resolved)
        .with_context(|| format!("Failed reading file {}", resolved.display()))?;

    if !original.contains(old_str) {
        return Ok(ToolResult::err(format!(
            "String not found in file: \"{old_str}\""
        )));
    }

    let updated = if replace_all {
        original.replace(old_str, new_str)
    } else {
        original.replacen(old_str, new_str, 1)
    };

    fs::write(&resolved, &updated)
        .with_context(|| format!("Failed writing file {}", resolved.display()))?;

    let diff = TextDiff::from_lines(&original, &updated)
        .unified_diff()
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();

    Ok(ToolResult::ok(format!("Updated {path}\n{diff}")))
}

#[cfg(test)]
mod tests {
    use super::{execute_create_file, execute_str_replace_editor, execute_view_file};
    use crate::tool_context::ToolContext;
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn view_file_returns_limited_preview_with_line_numbers() {
        let temp = TempDir::new("file-ops-view-preview");
        let file_path = temp.path().join("notes.txt");
        let content = (1..=12)
            .map(|n| format!("line-{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&file_path, content).expect("write fixture");

        let context = ToolContext::new(temp.path().to_path_buf()).expect("tool context");
        let result = execute_view_file(&json!({ "path": "notes.txt" }), &context).expect("view");

        assert!(result.success);
        let output = result.output.expect("output");
        assert!(output.contains("Contents of notes.txt:"));
        assert!(output.contains("1: line-1"));
        assert!(output.contains("10: line-10"));
        assert!(output.contains("... +2 lines"));
    }

    #[test]
    fn view_file_rejects_invalid_line_ranges() {
        let temp = TempDir::new("file-ops-view-range");
        fs::write(temp.path().join("notes.txt"), "a\nb\nc").expect("write fixture");
        let context = ToolContext::new(temp.path().to_path_buf()).expect("tool context");

        let result = execute_view_file(
            &json!({ "path": "notes.txt", "start_line": 0, "end_line": 1 }),
            &context,
        )
        .expect("view");
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("Invalid line range"));
    }

    #[test]
    fn create_file_writes_content_and_returns_diff() {
        let temp = TempDir::new("file-ops-create");
        let context = ToolContext::new(temp.path().to_path_buf()).expect("tool context");

        let result = execute_create_file(
            &json!({
                "path": "src/new.txt",
                "content": "hello\nworld\n"
            }),
            &context,
        )
        .expect("create");
        assert!(result.success);
        let output = result.output.expect("output");
        assert!(output.contains("Created src/new.txt"));
        assert!(output.contains("+++ b/src/new.txt"));

        let created = fs::read_to_string(temp.path().join("src").join("new.txt")).expect("read");
        assert_eq!(created, "hello\nworld\n");
    }

    #[test]
    fn str_replace_editor_replaces_first_or_all_matches() {
        let temp = TempDir::new("file-ops-replace");
        let file_path = temp.path().join("doc.txt");
        fs::write(&file_path, "foo foo foo").expect("write fixture");
        let context = ToolContext::new(temp.path().to_path_buf()).expect("tool context");

        let first = execute_str_replace_editor(
            &json!({
                "path": "doc.txt",
                "old_str": "foo",
                "new_str": "bar"
            }),
            &context,
        )
        .expect("replace first");
        assert!(first.success);
        let after_first = fs::read_to_string(&file_path).expect("read after first");
        assert_eq!(after_first, "bar foo foo");

        let all = execute_str_replace_editor(
            &json!({
                "path": "doc.txt",
                "old_str": "foo",
                "new_str": "baz",
                "replace_all": true
            }),
            &context,
        )
        .expect("replace all");
        assert!(all.success);
        let after_all = fs::read_to_string(&file_path).expect("read after all");
        assert_eq!(after_all, "bar baz baz");
    }

    #[test]
    fn str_replace_editor_errors_when_old_string_missing() {
        let temp = TempDir::new("file-ops-replace-missing");
        fs::write(temp.path().join("doc.txt"), "hello").expect("write fixture");
        let context = ToolContext::new(temp.path().to_path_buf()).expect("tool context");

        let result = execute_str_replace_editor(
            &json!({
                "path": "doc.txt",
                "old_str": "missing",
                "new_str": "x"
            }),
            &context,
        )
        .expect("replace");

        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|msg| msg.contains("String not found in file"))
        );
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos();
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("grok-build-{prefix}-{pid}-{nonce}"));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
