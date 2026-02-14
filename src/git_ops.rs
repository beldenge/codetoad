use crate::agent::Agent;
use crate::model_client::ModelClient;
use crate::tools::ToolResult;
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
    pub skip_push: bool,
}

pub async fn run_commit_and_push<C, F>(
    agent: Arc<Mutex<Agent<C>>>,
    options: CommitAndPushOptions,
    mut on_event: F,
) -> Result<CommitAndPushOutcome>
where
    C: ModelClient,
    F: FnMut(CommitAndPushEvent),
{
    let status = run_agent_bash(&agent, "git status --porcelain").await?;
    if !status.success {
        bail!(
            "git status failed: {}",
            status
                .error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }

    let status_output = normalize_command_output(status.output.unwrap_or_default());
    if status_output.trim().is_empty() {
        on_event(CommitAndPushEvent::NoChanges);
        return Ok(CommitAndPushOutcome::NoChanges);
    }
    on_event(CommitAndPushEvent::ChangesDetected);

    let add_command = "git add .".to_string();
    let add = run_agent_bash(&agent, &add_command).await?;
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

    let diff = run_agent_bash(&agent, "git diff --cached")
        .await
        .ok()
        .and_then(|r| r.output)
        .map(normalize_command_output)
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
    let commit = run_agent_bash(&agent, &commit_command).await?;
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

    if !options.skip_push {
        let mut push_command = "git push".to_string();
        let mut push = run_agent_bash(&agent, &push_command).await?;
        if !push.success
            && push
                .error
                .as_deref()
                .map(|e| e.contains("no upstream branch"))
                .unwrap_or(false)
        {
            push_command = "git push -u origin HEAD".to_string();
            push = run_agent_bash(&agent, &push_command).await?;
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
    }

    Ok(CommitAndPushOutcome::Completed)
}

async fn run_agent_bash<C: ModelClient>(
    agent: &Arc<Mutex<Agent<C>>>,
    command: &str,
) -> Result<ToolResult> {
    agent.lock().await.execute_bash_command(command).await
}

fn normalize_command_output(output: String) -> String {
    if output.trim() == "Command executed successfully (no output)" {
        String::new()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitAndPushEvent, CommitAndPushOptions, CommitAndPushOutcome, run_commit_and_push,
    };
    use crate::agent::Agent;
    use crate::grok_client::SearchMode;
    use crate::model_client::{ModelClient, StreamChunkHandler};
    use crate::protocol::{ChatCompletionResponse, ChatMessage, ChatTool};
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Mutex as AsyncMutex;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn run_commit_and_push_reports_no_changes_for_clean_repo() {
        let temp = TempDir::new("git-ops-no-changes");
        init_git_repo(temp.path());

        let agent = Agent::with_client(MockClient::new("model"), 1, temp.path()).expect("agent");
        let agent = Arc::new(AsyncMutex::new(agent));
        let mut events = Vec::new();

        let outcome = run_commit_and_push(
            agent,
            CommitAndPushOptions {
                default_commit_message: Some("chore: fallback".to_string()),
                skip_push: false,
            },
            |event| events.push(event),
        )
        .await
        .expect("commit and push");

        assert_eq!(outcome, CommitAndPushOutcome::NoChanges);
        assert!(matches!(
            events.first(),
            Some(CommitAndPushEvent::NoChanges)
        ));
    }

    #[tokio::test]
    async fn run_commit_and_push_completes_with_skip_push_when_changes_exist() {
        let repo = TempDir::new("git-ops-complete");
        init_git_repo(repo.path());
        fs::write(repo.path().join("README.md"), "updated\n").expect("write change");

        let agent = Agent::with_client(MockClient::new("model"), 1, repo.path()).expect("agent");
        let agent = Arc::new(AsyncMutex::new(agent));
        let mut events = Vec::new();

        let outcome = run_commit_and_push(
            agent,
            CommitAndPushOptions {
                default_commit_message: Some("chore: update readme".to_string()),
                skip_push: true,
            },
            |event| events.push(event),
        )
        .await
        .expect("commit and push");

        assert_eq!(outcome, CommitAndPushOutcome::Completed);
        let status = git_output(repo.path(), &["status", "--porcelain"]);
        assert!(status.trim().is_empty(), "repo should be clean after push");
        assert!(events.iter().any(|event| matches!(
            event,
            CommitAndPushEvent::GeneratedMessage(message) if !message.trim().is_empty()
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            CommitAndPushEvent::ToolResult {
                step: super::CommitAndPushStep::Add,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            CommitAndPushEvent::ToolResult {
                step: super::CommitAndPushStep::Commit,
                ..
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            CommitAndPushEvent::ToolResult {
                step: super::CommitAndPushStep::Push,
                ..
            }
        )));
    }

    fn init_git_repo(path: &Path) {
        git(path, &["init"]);
        git(path, &["config", "user.email", "test@example.com"]);
        git(path, &["config", "user.name", "Test User"]);
        fs::write(path.join("seed.txt"), "seed\n").expect("write seed file");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "chore: seed"]);
    }

    fn git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git command failed: git {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git command failed: git {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    struct MockClient {
        model: String,
    }

    impl MockClient {
        fn new(model: &str) -> Self {
            Self {
                model: model.to_string(),
            }
        }
    }

    #[async_trait]
    impl ModelClient for MockClient {
        fn set_model(&mut self, model: String) {
            self.model = model;
        }

        fn current_model(&self) -> &str {
            self.model.as_str()
        }

        async fn chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ChatTool],
            _search_mode: SearchMode,
        ) -> Result<ChatCompletionResponse> {
            Err(anyhow!("chat should not be called in git ops tests"))
        }

        async fn stream_chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ChatTool],
            _search_mode: SearchMode,
            _cancel_token: &CancellationToken,
            _on_chunk: &mut StreamChunkHandler<'_>,
        ) -> Result<()> {
            Err(anyhow!("stream_chat should not be called in git ops tests"))
        }

        async fn plain_completion(&self, _prompt: &str) -> Result<String> {
            Ok("chore: automated update".to_string())
        }
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
