use super::ToolResult;
use crate::tool_context;
use anyhow::{Context, Result};
use serde_json::Value;
use similar::TextDiff;
use std::fs;

pub(super) fn execute_view_file(args: &Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;

    let resolved = tool_context::resolve_path(path)?;
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

pub(super) fn execute_create_file(args: &Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .context("Missing 'content' argument")?;

    let resolved = tool_context::resolve_path(path)?;
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

pub(super) fn execute_str_replace_editor(args: &Value) -> Result<ToolResult> {
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

    let resolved = tool_context::resolve_path(path)?;
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
