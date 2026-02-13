use super::ToolResult;
use crate::tool_context;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::process::Command;

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

pub(super) async fn execute_search(args: &Value) -> Result<ToolResult> {
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
    cmd.arg("--files").arg("--no-require-git").arg("--follow");

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
