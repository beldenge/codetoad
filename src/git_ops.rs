use crate::agent::Agent;
use crate::tools::{ToolResult, execute_bash_command};
use anyhow::{Result, bail};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAndPushStep {
    Add,
    Commit,
    Push,
}

#[derive(Debug, Clone)]
pub enum CommitAndPushEvent {
    NoChanges,
    ChangesDetected,
    GeneratedMessage(String),
    ToolResult {
        step: CommitAndPushStep,
        command: String,
        result: ToolResult,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAndPushOutcome {
    NoChanges,
    Completed,
}

#[derive(Debug, Clone, Default)]
pub struct CommitAndPushOptions {
    pub default_commit_message: Option<String>,
}

pub async fn run_commit_and_push<F>(
    agent: Arc<Mutex<Agent>>,
    options: CommitAndPushOptions,
    mut on_event: F,
) -> Result<CommitAndPushOutcome>
where
    F: FnMut(CommitAndPushEvent),
{
    let status = execute_bash_command("git status --porcelain").await?;
    if !status.success {
        bail!(
            "git status failed: {}",
            status
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }

    let status_output = status.output.unwrap_or_default();
    if status_output.trim().is_empty() {
        on_event(CommitAndPushEvent::NoChanges);
        return Ok(CommitAndPushOutcome::NoChanges);
    }
    on_event(CommitAndPushEvent::ChangesDetected);

    let add_command = "git add .".to_string();
    let add = execute_bash_command(&add_command).await?;
    on_event(CommitAndPushEvent::ToolResult {
        step: CommitAndPushStep::Add,
        command: add_command.clone(),
        result: add.clone(),
    });
    if !add.success {
        bail!(
            "git add failed: {}",
            add.error.unwrap_or_else(|| "unknown error".to_string())
        );
    }

    let diff = execute_bash_command("git diff --cached")
        .await
        .ok()
        .and_then(|r| r.output)
        .unwrap_or_default();
    let prompt = format!(
        "Generate a concise conventional commit message under 72 characters.\n\nGit Status:\n{}\n\nGit Diff:\n{}\n\nRespond with only the commit message.",
        status_output, diff
    );

    let generated = match agent.lock().await.generate_plain_text(&prompt).await {
        Ok(text) => text.trim().trim_matches('"').to_string(),
        Err(err) => {
            if let Some(default) = options.default_commit_message.clone() {
                default
            } else {
                return Err(err);
            }
        }
    };
    let commit_message = if generated.trim().is_empty() {
        if let Some(default) = options.default_commit_message.clone() {
            default
        } else {
            bail!("Failed to generate commit message");
        }
    } else {
        generated
    };

    on_event(CommitAndPushEvent::GeneratedMessage(commit_message.clone()));

    let commit_command = format!("git commit -m \"{}\"", commit_message.replace('"', "\\\""));
    let commit = execute_bash_command(&commit_command).await?;
    on_event(CommitAndPushEvent::ToolResult {
        step: CommitAndPushStep::Commit,
        command: commit_command.clone(),
        result: commit.clone(),
    });
    if !commit.success {
        bail!(
            "git commit failed: {}",
            commit.error.unwrap_or_else(|| "unknown error".to_string())
        );
    }

    let mut push_command = "git push".to_string();
    let mut push = execute_bash_command(&push_command).await?;
    if !push.success
        && push
            .error
            .as_deref()
            .map(|e| e.contains("no upstream branch"))
            .unwrap_or(false)
    {
        push_command = "git push -u origin HEAD".to_string();
        push = execute_bash_command(&push_command).await?;
    }

    on_event(CommitAndPushEvent::ToolResult {
        step: CommitAndPushStep::Push,
        command: push_command,
        result: push.clone(),
    });
    if !push.success {
        bail!(
            "git push failed: {}",
            push.error.unwrap_or_else(|| "unknown error".to_string())
        );
    }

    Ok(CommitAndPushOutcome::Completed)
}
