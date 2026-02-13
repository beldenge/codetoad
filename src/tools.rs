use crate::tool_catalog::{
    TOOL_BASH, TOOL_CREATE_FILE, TOOL_CREATE_TODO_LIST, TOOL_SEARCH, TOOL_STR_REPLACE_EDITOR,
    TOOL_UPDATE_TODO_LIST, TOOL_VIEW_FILE,
};
use crate::tool_context;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use similar::TextDiff;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
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

#[derive(Debug, Clone, Deserialize)]
struct SearchOptions {
    query: String,
    #[serde(default)]
    search_type: Option<String>,
    #[serde(default)]
    include_pattern: Option<String>,
    #[serde(default)]
    exclude_pattern: Option<String>,
    #[serde(default)]
    case_sensitive: Option<bool>,
    #[serde(default)]
    whole_word: Option<bool>,
    #[serde(default)]
    regex: Option<bool>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    file_types: Option<Vec<String>>,
    #[serde(default)]
    include_hidden: Option<bool>,
}

#[derive(Debug, Clone)]
struct SearchTextResult {
    file: String,
}

#[derive(Debug, Clone)]
struct FileSearchResult {
    path: String,
    score: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoItem {
    id: String,
    content: String,
    status: String,
    priority: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoUpdate {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

static TODO_ITEMS: OnceLock<Mutex<Vec<TodoItem>>> = OnceLock::new();

fn todo_items() -> &'static Mutex<Vec<TodoItem>> {
    TODO_ITEMS.get_or_init(|| Mutex::new(Vec::new()))
}

pub async fn execute_tool(name: &str, args: &Value) -> ToolResult {
    let result = match name {
        TOOL_VIEW_FILE => execute_view_file(args),
        TOOL_CREATE_FILE => execute_create_file(args),
        TOOL_STR_REPLACE_EDITOR => execute_str_replace_editor(args),
        TOOL_BASH => execute_bash_tool(args).await,
        TOOL_SEARCH => execute_search(args).await,
        TOOL_CREATE_TODO_LIST => execute_create_todo_list(args),
        TOOL_UPDATE_TODO_LIST => execute_update_todo_list(args),
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

    let limit = 10usize;
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

async fn execute_search(args: &Value) -> Result<ToolResult> {
    let options: SearchOptions =
        serde_json::from_value(args.clone()).context("Invalid search arguments")?;
    let query = options.query.trim();
    if query.is_empty() {
        return Ok(ToolResult::err("Missing or empty 'query' argument"));
    }

    let search_type = options
        .search_type
        .as_deref()
        .unwrap_or("both")
        .to_ascii_lowercase();
    if !matches!(search_type.as_str(), "text" | "files" | "both") {
        return Ok(ToolResult::err(
            "Invalid search_type. Expected one of: text, files, both",
        ));
    }

    let max_results = options.max_results.unwrap_or(50).clamp(1, 200);
    let mut text_results = Vec::new();
    let mut file_results = Vec::new();

    if matches!(search_type.as_str(), "text" | "both") {
        text_results = search_text(query, &options, max_results).await?;
    }
    if matches!(search_type.as_str(), "files" | "both") {
        file_results = search_files(query, &options, max_results).await?;
    }

    Ok(ToolResult::ok(format_search_results(
        query,
        &text_results,
        &file_results,
    )))
}

async fn search_text(
    query: &str,
    options: &SearchOptions,
    max_results: usize,
) -> Result<Vec<SearchTextResult>> {
    let mut cmd = Command::new("rg");
    cmd.arg("--json")
        .arg("--with-filename")
        .arg("--line-number")
        .arg("--column")
        .arg("--no-heading")
        .arg("--color=never")
        .arg("--max-count")
        .arg(max_results.to_string())
        .arg("--no-require-git")
        .arg("--follow");

    apply_default_exclude_globs(&mut cmd);

    if !options.case_sensitive.unwrap_or(false) {
        cmd.arg("--ignore-case");
    }
    if options.whole_word.unwrap_or(false) {
        cmd.arg("--word-regexp");
    }
    if !options.regex.unwrap_or(false) {
        cmd.arg("--fixed-strings");
    }
    apply_search_filters(&mut cmd, options);

    cmd.arg(query).arg(".");

    let output = cmd
        .current_dir(tool_context::current_dir()?)
        .output()
        .await
        .with_context(|| format!("Failed running search command for query '{query}'"))?;
    let status_code = output.status.code().unwrap_or_default();
    if !output.status.success() && status_code != 1 {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow::anyhow!("Search command failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    for line in stdout.lines() {
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if parsed.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }

        let data = parsed.get("data").unwrap_or(&Value::Null);
        let file = data
            .get("path")
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if file.is_empty() {
            continue;
        }

        results.push(SearchTextResult { file });

        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

async fn search_files(
    query: &str,
    options: &SearchOptions,
    max_results: usize,
) -> Result<Vec<FileSearchResult>> {
    let mut cmd = Command::new("rg");
    cmd.arg("--files")
        .arg("--no-require-git")
        .arg("--follow");

    apply_default_exclude_globs(&mut cmd);
    apply_search_filters(&mut cmd, options);

    cmd.arg(".");

    let output = cmd
        .current_dir(tool_context::current_dir()?)
        .output()
        .await
        .context("Failed running file search command")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow::anyhow!("File search command failed: {stderr}"));
    }

    let query_lower = query.to_ascii_lowercase();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    for line in stdout.lines() {
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }
        let path = normalize_file_path(raw);
        let file_name = PathBuf::from(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let score = calculate_file_score(&file_name, &path, &query_lower);
        if score > 0 {
            results.push(FileSearchResult { path, score });
        }
    }

    results.sort_by(|a, b| match b.score.cmp(&a.score) {
        Ordering::Equal => a.path.cmp(&b.path),
        ordering => ordering,
    });
    results.truncate(max_results);
    Ok(results)
}

fn apply_default_exclude_globs(cmd: &mut Command) {
    for pattern in ["!.git/**", "!node_modules/**", "!.DS_Store", "!*.log"] {
        cmd.arg("--glob").arg(pattern);
    }
}

fn apply_search_filters(cmd: &mut Command, options: &SearchOptions) {
    if options.include_hidden.unwrap_or(false) {
        cmd.arg("--hidden");
    }
    if let Some(include_pattern) = options.include_pattern.as_deref() {
        cmd.arg("--glob").arg(include_pattern);
    }
    if let Some(exclude_pattern) = options.exclude_pattern.as_deref() {
        cmd.arg("--glob").arg(format!("!{exclude_pattern}"));
    }
    append_file_type_globs(cmd, options.file_types.as_deref());
}

fn append_file_type_globs(cmd: &mut Command, file_types: Option<&[String]>) {
    if let Some(file_types) = file_types {
        for file_type in file_types {
            let trimmed = file_type.trim().trim_start_matches('.');
            if !trimmed.is_empty() {
                cmd.arg("--glob").arg(format!("*.{trimmed}"));
            }
        }
    }
}

fn normalize_file_path(path: &str) -> String {
    path.strip_prefix("./")
        .map(|trimmed| trimmed.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn calculate_file_score(file_name: &str, file_path: &str, pattern: &str) -> i32 {
    let lower_name = file_name.to_ascii_lowercase();
    let lower_path = file_path.to_ascii_lowercase();

    if lower_name == pattern {
        return 100;
    }
    if lower_name.contains(pattern) {
        return 80;
    }
    if lower_path.contains(pattern) {
        return 60;
    }

    let mut pattern_index = 0usize;
    let pattern_chars = pattern.chars().collect::<Vec<_>>();
    for ch in lower_name.chars() {
        if pattern_index < pattern_chars.len() && ch == pattern_chars[pattern_index] {
            pattern_index += 1;
        }
    }

    if pattern_index == pattern_chars.len() {
        let length_penalty = lower_name.chars().count() as i32 - pattern_chars.len() as i32;
        return (40 - length_penalty).max(10);
    }

    0
}

fn format_search_results(
    query: &str,
    text_results: &[SearchTextResult],
    file_results: &[FileSearchResult],
) -> String {
    if text_results.is_empty() && file_results.is_empty() {
        return format!("No results found for \"{query}\"");
    }

    let mut match_counts: HashMap<String, usize> = HashMap::new();
    for result in text_results {
        *match_counts.entry(result.file.clone()).or_insert(0) += 1;
    }

    let mut ordered_files = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for result in text_results {
        if seen.insert(result.file.clone()) {
            ordered_files.push(result.file.clone());
        }
    }
    for result in file_results {
        if seen.insert(result.path.clone()) {
            ordered_files.push(result.path.clone());
        }
    }

    let display_limit = 8usize;
    let mut lines = vec![format!("Search results for \"{query}\":")];
    for file in ordered_files.iter().take(display_limit) {
        let match_count = match_counts.get(file).copied().unwrap_or(0);
        if match_count > 0 {
            lines.push(format!("  {file} ({match_count} matches)"));
        } else {
            lines.push(format!("  {file}"));
        }
    }
    if ordered_files.len() > display_limit {
        lines.push(format!("  ... +{} more", ordered_files.len() - display_limit));
    }

    lines.join("\n")
}

fn execute_create_todo_list(args: &Value) -> Result<ToolResult> {
    let todos_value = args
        .get("todos")
        .cloned()
        .context("Missing 'todos' argument")?;
    let todos: Vec<TodoItem> =
        serde_json::from_value(todos_value).context("Invalid 'todos' argument")?;

    for todo in &todos {
        if !is_valid_todo_item(todo) {
            return Ok(ToolResult::err(
                "Each todo must include id, content, status(pending|in_progress|completed), and priority(high|medium|low)",
            ));
        }
    }

    let mut store = todo_items()
        .lock()
        .map_err(|_| anyhow::anyhow!("Todo list storage is unavailable"))?;
    *store = todos.clone();
    Ok(ToolResult::ok(format_todo_list(&store)))
}

fn execute_update_todo_list(args: &Value) -> Result<ToolResult> {
    let updates_value = args
        .get("updates")
        .cloned()
        .context("Missing 'updates' argument")?;
    let updates: Vec<TodoUpdate> =
        serde_json::from_value(updates_value).context("Invalid 'updates' argument")?;

    let mut store = todo_items()
        .lock()
        .map_err(|_| anyhow::anyhow!("Todo list storage is unavailable"))?;
    for update in updates {
        let Some(todo) = store.iter_mut().find(|item| item.id == update.id) else {
            return Ok(ToolResult::err(format!(
                "Todo with id '{}' not found",
                update.id
            )));
        };

        if let Some(status) = update.status {
            if !is_valid_status(&status) {
                return Ok(ToolResult::err(format!(
                    "Invalid status: {status}. Must be pending, in_progress, or completed"
                )));
            }
            todo.status = status;
        }
        if let Some(content) = update.content {
            todo.content = content;
        }
        if let Some(priority) = update.priority {
            if !is_valid_priority(&priority) {
                return Ok(ToolResult::err(format!(
                    "Invalid priority: {priority}. Must be high, medium, or low"
                )));
            }
            todo.priority = priority;
        }
    }

    Ok(ToolResult::ok(format_todo_list(&store)))
}

fn is_valid_todo_item(item: &TodoItem) -> bool {
    !item.id.trim().is_empty()
        && !item.content.trim().is_empty()
        && is_valid_status(&item.status)
        && is_valid_priority(&item.priority)
}

fn is_valid_status(status: &str) -> bool {
    matches!(status, "pending" | "in_progress" | "completed")
}

fn is_valid_priority(priority: &str) -> bool {
    matches!(priority, "high" | "medium" | "low")
}

fn format_todo_list(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "No todos created yet".to_string();
    }

    let mut output = String::new();
    for (index, todo) in todos.iter().enumerate() {
        let marker = match todo.status.as_str() {
            "completed" => "●",
            "in_progress" => "◐",
            _ => "○",
        };
        let indent = if index == 0 { "" } else { "  " };
        output.push_str(&format!("{indent}{marker} {}\n", todo.content));
    }
    output.trim_end().to_string()
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
        let new_dir = tool_context::set_current_dir(path)?;
        return Ok(ToolResult::ok(format!(
            "Changed directory to: {}",
            new_dir.display()
        )));
    }

    let cwd = tool_context::current_dir()?;
    let output = if cfg!(windows) {
        Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(trimmed)
            .current_dir(&cwd)
            .output()
            .await
            .with_context(|| format!("Failed running command: {trimmed}"))?
    } else {
        Command::new("sh")
            .arg("-lc")
            .arg(trimmed)
            .current_dir(&cwd)
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
