use anyhow::{Context, Result};
use serde_json::Value;
use similar::TextDiff;
use std::fs;
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
        }
    }

    pub fn content_for_model(&self) -> String {
        if self.success {
            self.output.clone().unwrap_or_else(|| "Success".to_string())
        } else {
            self.error.clone().unwrap_or_else(|| "Error".to_string())
        }
    }
}

pub fn tool_result_from_error(err: anyhow::Error) -> ToolResult {
    ToolResult::err(format!("{err:#}"))
}

pub async fn execute_tool(name: &str, args: &Value) -> ToolResult {
    let result = match name {
        "view_file" => execute_view_file(args),
        "create_file" => execute_create_file(args),
        "str_replace_editor" => execute_str_replace_editor(args),
        "bash" => execute_bash_tool(args).await,
        _ => Ok(ToolResult::err(format!("Unknown tool: {name}"))),
    };

    match result {
        Ok(tool_result) => tool_result,
        Err(error) => tool_result_from_error(error),
    }
}

fn execute_view_file(args: &Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;

    let resolved = Path::new(path);
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

    let content = fs::read_to_string(resolved)
        .with_context(|| format!("Failed reading file {}", resolved.display()))?;
    let lines: Vec<&str> = content.lines().collect();

    let start = args
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let end = args
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize);

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

    let limit = 200usize;
    let display = lines
        .iter()
        .take(limit)
        .enumerate()
        .map(|(i, line)| format!("{}: {}", i + 1, line))
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

fn execute_create_file(args: &Value) -> Result<ToolResult> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .context("Missing 'path' argument")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .context("Missing 'content' argument")?;

    let resolved = Path::new(path);
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory {}", parent.display()))?;
    }
    fs::write(resolved, content)
        .with_context(|| format!("Failed writing file {}", resolved.display()))?;

    let created = TextDiff::from_lines("", content)
        .unified_diff()
        .header("/dev/null", &format!("b/{path}"))
        .to_string();

    Ok(ToolResult::ok(format!("Created {path}\n{created}")))
}

fn execute_str_replace_editor(args: &Value) -> Result<ToolResult> {
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

    let resolved = Path::new(path);
    if !resolved.exists() {
        return Ok(ToolResult::err(format!("File not found: {path}")));
    }

    let original = fs::read_to_string(resolved)
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

    fs::write(resolved, &updated)
        .with_context(|| format!("Failed writing file {}", resolved.display()))?;

    let diff = TextDiff::from_lines(&original, &updated)
        .unified_diff()
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();

    Ok(ToolResult::ok(format!("Updated {path}\n{diff}")))
}

async fn execute_bash_tool(args: &Value) -> Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("Missing 'command' argument")?;
    execute_bash_command(command).await
}

pub async fn execute_bash_command(command: &str) -> Result<ToolResult> {
    let trimmed = command.trim();
    if let Some(path) = trimmed.strip_prefix("cd ").map(str::trim) {
        std::env::set_current_dir(path)
            .with_context(|| format!("Failed to change directory to '{path}'"))?;
        return Ok(ToolResult::ok(format!(
            "Changed directory to: {}",
            std::env::current_dir()?.display()
        )));
    }

    let output = if cfg!(windows) {
        Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(trimmed)
            .output()
            .await
            .with_context(|| format!("Failed running command: {trimmed}"))?
    } else {
        Command::new("sh")
            .arg("-lc")
            .arg(trimmed)
            .output()
            .await
            .with_context(|| format!("Failed running command: {trimmed}"))?
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        if stdout.is_empty() && stderr.is_empty() {
            Ok(ToolResult::ok("Command executed successfully (no output)"))
        } else if stderr.is_empty() {
            Ok(ToolResult::ok(stdout))
        } else if stdout.is_empty() {
            Ok(ToolResult::ok(format!("STDERR:\n{stderr}")))
        } else {
            Ok(ToolResult::ok(format!("{stdout}\n\nSTDERR:\n{stderr}")))
        }
    } else {
        let message = if stderr.is_empty() {
            format!("Command failed: {trimmed}")
        } else {
            format!("Command failed: {stderr}")
        };
        Ok(ToolResult::err(message))
    }
}
