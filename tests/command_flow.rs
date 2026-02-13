use anyhow::{Result, bail};
use grok_build::agent::{Agent, AgentEvent, ConfirmationDecision};
use grok_build::confirmation::ConfirmationOperation;
use grok_build::grok_client::SearchMode;
use grok_build::model_client::{ModelClient, StreamChunkHandler};
use grok_build::protocol::{
    ChatChoice, ChatCompletionMessage, ChatCompletionResponse, ChatCompletionStreamChoice,
    ChatCompletionStreamChunk, ChatCompletionStreamDelta, ChatCompletionToolCallDelta,
    ChatCompletionToolCallFunctionDelta, ChatMessage, ChatTool,
};
use grok_build::slash_commands::{
    CommandGroup, ParsedSlashCommand, append_help_section, parse_slash_command,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct FakeClient {
    model: String,
    stream_rounds: Arc<Mutex<VecDeque<Vec<ChatCompletionStreamChunk>>>>,
}

impl FakeClient {
    fn with_rounds(rounds: Vec<Vec<ChatCompletionStreamChunk>>) -> Self {
        Self {
            model: "fake-model".to_string(),
            stream_rounds: Arc::new(Mutex::new(VecDeque::from(rounds))),
        }
    }
}

#[async_trait::async_trait]
impl ModelClient for FakeClient {
    fn set_model(&mut self, model: String) {
        self.model = model;
    }

    fn current_model(&self) -> &str {
        &self.model
    }

    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ChatTool],
        _search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse> {
        Ok(ChatCompletionResponse {
            choices: vec![ChatChoice {
                message: ChatCompletionMessage {
                    content: Some("ok".to_string()),
                    tool_calls: None,
                },
            }],
        })
    }

    async fn stream_chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ChatTool],
        _search_mode: SearchMode,
        cancel_token: &CancellationToken,
        on_chunk: &mut StreamChunkHandler<'_>,
    ) -> Result<()> {
        if cancel_token.is_cancelled() {
            return Ok(());
        }

        let mut guard = self.stream_rounds.lock().await;
        let Some(round) = guard.pop_front() else {
            bail!("No scripted stream round available");
        };
        drop(guard);

        for chunk in round {
            if cancel_token.is_cancelled() {
                return Ok(());
            }
            on_chunk(chunk)?;
        }
        Ok(())
    }

    async fn plain_completion(&self, _prompt: &str) -> Result<String> {
        Ok("fake commit message".to_string())
    }
}

#[tokio::test]
async fn streaming_flow_emits_confirmation_then_rejection_result_then_done() {
    let first_round = vec![ChatCompletionStreamChunk {
        choices: vec![ChatCompletionStreamChoice {
            delta: ChatCompletionStreamDelta {
                content: None,
                tool_calls: Some(vec![ChatCompletionToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    _type: Some("function".to_string()),
                    function: Some(ChatCompletionToolCallFunctionDelta {
                        name: Some("bash".to_string()),
                        arguments: Some("{\"command\":\"echo hi\"}".to_string()),
                    }),
                }]),
            },
        }],
    }];
    let second_round = vec![ChatCompletionStreamChunk {
        choices: vec![ChatCompletionStreamChoice {
            delta: ChatCompletionStreamDelta {
                content: Some("final answer".to_string()),
                tool_calls: None,
            },
        }],
    }];

    let temp = TempDir::new("agent-flow");
    let cwd = temp.path().to_path_buf();
    let fake = FakeClient::with_rounds(vec![first_round, second_round]);
    let mut agent = Agent::with_client(fake, 4, &cwd).expect("agent setup");

    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (confirm_tx, confirm_rx) = mpsc::unbounded_channel::<ConfirmationDecision>();
    let confirm_rx = Arc::new(Mutex::new(confirm_rx));

    let task = tokio::spawn(async move {
        agent
            .process_user_message_stream(
                "run a command".to_string(),
                Vec::new(),
                CancellationToken::new(),
                updates_tx,
                Some(confirm_rx),
            )
            .await
    });

    let mut events = Vec::new();
    while let Some(event) = updates_rx.recv().await {
        if let AgentEvent::ConfirmationRequest {
            tool_call,
            operation,
        } = &event
        {
            assert_eq!(*operation, ConfirmationOperation::Bash);
            confirm_tx
                .send(ConfirmationDecision::Reject {
                    tool_call_id: tool_call.id.clone(),
                    feedback: None,
                })
                .expect("confirmation channel open");
        }
        let done = matches!(event, AgentEvent::Done);
        events.push(event);
        if done {
            break;
        }
    }

    task.await.expect("task join").expect("agent run");

    let tool_calls_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolCalls(_)))
        .expect("tool calls event");
    let confirm_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ConfirmationRequest { .. }))
        .expect("confirmation request event");
    let tool_result_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolResult { .. }))
        .expect("tool result event");
    let done_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::Done))
        .expect("done event");

    assert!(tool_calls_idx < confirm_idx);
    assert!(confirm_idx < tool_result_idx);
    assert!(tool_result_idx < done_idx);

    let rejection = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } => result.error.clone(),
            _ => None,
        })
        .expect("rejection output");
    assert!(rejection.contains("Operation cancelled by user"));

    let final_content = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Content(text) if text.contains("final answer") => Some(text.clone()),
            _ => None,
        })
        .expect("final model content");
    assert_eq!(final_content, "final answer");
}

#[test]
fn slash_command_parser_and_help_are_consistent() {
    assert!(matches!(
        parse_slash_command("/help"),
        Some(ParsedSlashCommand::Help)
    ));
    assert!(matches!(
        parse_slash_command("/clear"),
        Some(ParsedSlashCommand::Clear)
    ));
    assert!(matches!(
        parse_slash_command("/models"),
        Some(ParsedSlashCommand::Models)
    ));
    assert!(matches!(
        parse_slash_command("/models grok-4-latest"),
        Some(ParsedSlashCommand::SetModel(model)) if model == "grok-4-latest"
    ));
    assert!(matches!(
        parse_slash_command("/resume"),
        Some(ParsedSlashCommand::Resume)
    ));
    assert!(matches!(
        parse_slash_command("/commit-and-push"),
        Some(ParsedSlashCommand::CommitAndPush)
    ));
    assert!(matches!(
        parse_slash_command("/exit"),
        Some(ParsedSlashCommand::Exit)
    ));

    let mut built_in = String::new();
    append_help_section(&mut built_in, "Built-in Commands", CommandGroup::BuiltIn);
    assert!(built_in.contains("/help"));
    assert!(built_in.contains("/clear"));
    assert!(built_in.contains("/models"));
    assert!(built_in.contains("/exit"));

    let mut git = String::new();
    append_help_section(&mut git, "Git Commands", CommandGroup::Git);
    assert!(git.contains("/commit-and-push"));
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
