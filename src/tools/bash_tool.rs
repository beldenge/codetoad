use super::ToolResult;
use crate::tool_context;
use anyhow::{Context, Result};
use serde_json::Value;
use tokio::process::Command;

pub(super) async fn execute_bash_tool(args: &Value) -> Result<ToolResult> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .context("Missing 'command' argument")?;
    execute_bash_command(command).await
}

pub async fn execute_bash_command(command: &str) -> Result<ToolResult> {
    let trimmed = command.trim();
    if let Some(path) = trimmed.strip_prefix("cd ").map(str::trim) {
        return match tool_context::set_current_dir(path) {
            Ok(new_dir) => Ok(ToolResult::ok(format!(
                "Changed directory to: {}",
                new_dir.display()
            ))),
            Err(err) => Ok(ToolResult::err(err.to_string())),
        };
    }

    let cwd = match tool_context::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => return Ok(ToolResult::err(err.to_string())),
    };
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
                )))
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
                )))
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
