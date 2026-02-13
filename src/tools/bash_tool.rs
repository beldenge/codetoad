use super::ToolResult;
use crate::tool_context::ToolContext;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub(super) async fn execute_bash_tool(
    args: &Value,
    tool_context: &mut ToolContext,
) -> Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("Missing 'command' argument")?;
    execute_bash_command(command, tool_context).await
}

pub async fn execute_bash_command(
    command: &str,
    tool_context: &mut ToolContext,
) -> Result<ToolResult> {
    let trimmed = command.trim();
    if let Some(path) = trimmed.strip_prefix("cd ").map(str::trim) {
        return match tool_context.set_current_dir(path) {
            Ok(new_dir) => Ok(ToolResult::ok(format!(
                "Changed directory to: {}",
                new_dir.display()
            ))),
            Err(err) => Ok(ToolResult::err(err.to_string())),
        };
    }

    if let Err(reason) = validate_command_paths(trimmed, tool_context) {
        return Ok(ToolResult::err(format!(
            "Blocked by shell sandbox policy: {reason}"
        )));
    }

    let cwd = tool_context.current_dir().to_path_buf();
    let output = if cfg!(windows) {
        match Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(trimmed)
            .current_dir(&cwd)
            .output()
            .await
        {
            Ok(output) => output,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "Failed running command: {trimmed}: {err}"
                )));
            }
        }
    } else {
        match Command::new("sh")
            .arg("-lc")
            .arg(trimmed)
            .current_dir(&cwd)
            .output()
            .await
        {
            Ok(output) => output,
            Err(err) => {
                return Ok(ToolResult::err(format!(
                    "Failed running command: {trimmed}: {err}"
                )));
            }
        }
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

fn validate_command_paths(command: &str, tool_context: &ToolContext) -> Result<(), String> {
    if has_dynamic_path_expansion(command) {
        return Err(
            "dynamic path expansion is not allowed (e.g. ~, $VAR/path, %VAR%\\path, $(...))"
                .to_string(),
        );
    }

    let mut expect_redirection_target = false;
    for token in split_shell_like(command) {
        if let Some(candidate) = extract_path_candidate(&token, &mut expect_redirection_target) {
            if candidate.contains("://") {
                continue;
            }
            let sanitized = sanitize_path_token(&candidate);
            if sanitized.is_empty() {
                continue;
            }
            if contains_glob_pattern(&sanitized) {
                if let Some(prefix) = prefix_before_glob(&sanitized)
                    && looks_like_path(&prefix)
                {
                    tool_context
                        .resolve_path(&prefix)
                        .map_err(|err| err.to_string())?;
                }
                continue;
            }
            if looks_like_path(&sanitized) {
                tool_context
                    .resolve_path(&sanitized)
                    .map_err(|err| err.to_string())?;
            }
        }
    }

    Ok(())
}

fn has_dynamic_path_expansion(command: &str) -> bool {
    command.contains("$(")
        || command.contains("${")
        || command.contains('`')
        || command.contains("~/")
        || command.contains("~\\")
        || contains_variable_path_like(command)
}

fn contains_variable_path_like(command: &str) -> bool {
    let bytes = command.as_bytes();
    for i in 0..bytes.len() {
        let ch = bytes[i] as char;
        if ch == '$' && i + 1 < bytes.len() {
            let next = bytes[i + 1] as char;
            if next.is_ascii_alphabetic() || next == '_' {
                let suffix = &command[i + 1..];
                if suffix.contains('/') || suffix.contains('\\') {
                    return true;
                }
            }
        }
        if ch == '%'
            && let Some(end) = command[i + 1..].find('%')
        {
            let after = i + 1 + end + 1;
            if after < command.len() {
                let rest = &command[after..];
                if rest.starts_with('\\') || rest.starts_with('/') {
                    return true;
                }
            }
        }
    }
    false
}

fn split_shell_like(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in command.chars() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '"' | '\'' => quote = Some(ch),
            ch if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn extract_path_candidate(token: &str, expect_redirection_target: &mut bool) -> Option<String> {
    if token.is_empty() {
        return None;
    }

    if *expect_redirection_target {
        *expect_redirection_target = false;
        return Some(token.to_string());
    }

    if matches!(token, ">" | ">>" | "<" | "1>" | "1>>" | "2>" | "2>>") {
        *expect_redirection_target = true;
        return None;
    }

    for prefix in ["2>>", "1>>", ">>", "2>", "1>", ">", "<"] {
        if let Some(rest) = token.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }

    if looks_like_path(token) {
        Some(token.to_string())
    } else {
        None
    }
}

fn looks_like_path(token: &str) -> bool {
    if token.is_empty() || token == "-" {
        return false;
    }
    if token.starts_with('-') && !token.contains('/') && !token.contains('\\') {
        return false;
    }

    let path = Path::new(token);
    if path.is_absolute() {
        return true;
    }

    token == "."
        || token == ".."
        || token.starts_with("./")
        || token.starts_with(".\\")
        || token.starts_with("../")
        || token.starts_with("..\\")
        || token.contains('/')
        || token.contains('\\')
        || is_windows_drive_path(token)
}

fn is_windows_drive_path(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn sanitize_path_token(token: &str) -> String {
    token
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']'))
        .to_string()
}

fn contains_glob_pattern(token: &str) -> bool {
    token.contains('*') || token.contains('?')
}

fn prefix_before_glob(token: &str) -> Option<String> {
    let first_glob = token
        .char_indices()
        .find(|(_, ch)| *ch == '*' || *ch == '?')
        .map(|(index, _)| index)?;
    let prefix = token[..first_glob].trim();
    if prefix.is_empty() {
        None
    } else {
        Some(prefix.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        has_dynamic_path_expansion, looks_like_path, sanitize_path_token, validate_command_paths,
    };
    use crate::tool_context::ToolContext;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_absolute_out_of_root_path() {
        let temp = TempDir::new("bash-sandbox-reject");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let context = ToolContext::new(root).expect("tool context");

        let err =
            validate_command_paths("cat C:\\Windows\\system32\\drivers\\etc\\hosts", &context)
                .expect_err("must reject");
        assert!(err.contains("Path escapes project root"));
    }

    #[test]
    fn allows_relative_in_project_paths() {
        let temp = TempDir::new("bash-sandbox-allow");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        fs::create_dir_all(root.join("src")).expect("create src");
        let context = ToolContext::new(root).expect("tool context");

        validate_command_paths("cat src/main.rs", &context).expect("path should be allowed");
    }

    #[test]
    fn rejects_dynamic_path_expansion_patterns() {
        assert!(has_dynamic_path_expansion("cat $HOME/.ssh/id_rsa"));
        assert!(has_dynamic_path_expansion("cat %USERPROFILE%\\secret.txt"));
        assert!(has_dynamic_path_expansion("cat ~/secret.txt"));
        assert!(has_dynamic_path_expansion("cat $(pwd)/file"));
    }

    #[test]
    fn identifies_path_like_tokens() {
        assert!(looks_like_path("src/main.rs"));
        assert!(looks_like_path("../outside"));
        assert!(looks_like_path("C:\\repo\\file.txt"));
        assert!(!looks_like_path("--color=never"));
        assert!(!looks_like_path("echo"));
    }

    #[test]
    fn sanitizes_wrapped_tokens() {
        assert_eq!(
            sanitize_path_token("\"src/main.rs\","),
            "src/main.rs".to_string()
        );
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
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
