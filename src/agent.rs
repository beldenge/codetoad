use crate::agent_policy::{
    build_system_prompt, estimate_messages_tokens, estimate_text_tokens, search_mode_for,
};
use crate::agent_stream::{PartialToolCall, accumulate_tool_calls, merge_stream_text};
use crate::confirmation::ConfirmationOperation;
use crate::grok_client::GrokClient;
use crate::model_client::ModelClient;
use crate::protocol::{
    ChatCompletionStreamChunk, ChatImageAttachment, ChatMessage, ChatTool, ChatToolCall,
    ChatToolCallFunction,
};
use crate::tool_catalog::{confirmation_operation_for_tool, default_tools};
use crate::tools::{
    ToolResult, ToolSessionSnapshot, ToolSessionState, execute_bash_command, execute_tool,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ToolCallSummary {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub enum ConfirmationDecision {
    Approve {
        tool_call_id: String,
        remember_for_session: bool,
    },
    Reject {
        tool_call_id: String,
        feedback: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Content(String),
    TokenCount(usize),
    ConfirmationRequest {
        tool_call: ToolCallSummary,
        operation: ConfirmationOperation,
    },
    ToolCalls(Vec<ToolCallSummary>),
    ToolResult {
        tool_call: ToolCallSummary,
        result: ToolResult,
    },
    Error(String),
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentSessionSnapshot {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tool_session: ToolSessionSnapshot,
    pub auto_edit_enabled: bool,
    pub session_allow_file_ops: bool,
    pub session_allow_bash_ops: bool,
}

pub struct Agent<C: ModelClient = GrokClient> {
    client: C,
    messages: Vec<ChatMessage>,
    system_prompt: String,
    max_tool_rounds: usize,
    tools: Vec<ChatTool>,
    tool_session: ToolSessionState,
    auto_edit_enabled: bool,
    session_allow_file_ops: bool,
    session_allow_bash_ops: bool,
}

impl Agent<GrokClient> {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        max_tool_rounds: usize,
        cwd: &Path,
    ) -> Result<Self> {
        let client = GrokClient::new(api_key, base_url, model)?;
        Self::with_client(client, max_tool_rounds, cwd)
    }

    pub fn reconfigure_provider(&mut self, api_key: String, base_url: String, model: String) {
        self.client.reconfigure_connection(api_key, base_url);
        self.client.set_model(model);
    }
}

impl<C: ModelClient> Agent<C> {
    pub fn with_client(client: C, max_tool_rounds: usize, cwd: &Path) -> Result<Self> {
        let system_prompt = build_system_prompt(cwd);
        let tool_session = ToolSessionState::new(cwd.to_path_buf())?;
        let messages = vec![ChatMessage::system(system_prompt.clone())];

        Ok(Self {
            client,
            messages,
            system_prompt,
            max_tool_rounds,
            tools: default_tools(),
            tool_session,
            auto_edit_enabled: false,
            session_allow_file_ops: false,
            session_allow_bash_ops: false,
        })
    }

    pub fn current_model(&self) -> &str {
        self.client.current_model()
    }

    pub fn auto_edit_enabled(&self) -> bool {
        self.auto_edit_enabled
    }

    pub fn set_model(&mut self, model: String) {
        self.client.set_model(model);
    }

    pub fn set_auto_edit_enabled(&mut self, enabled: bool) {
        self.auto_edit_enabled = enabled;
        if enabled {
            self.session_allow_file_ops = true;
            self.session_allow_bash_ops = true;
        } else {
            self.session_allow_file_ops = false;
            self.session_allow_bash_ops = false;
        }
    }

    pub fn is_operation_auto_approved(&self, operation: ConfirmationOperation) -> bool {
        if self.auto_edit_enabled {
            return true;
        }
        match operation {
            ConfirmationOperation::File => self.session_allow_file_ops,
            ConfirmationOperation::Bash => self.session_allow_bash_ops,
        }
    }

    pub fn remember_operation_for_session(&mut self, operation: ConfirmationOperation) {
        match operation {
            ConfirmationOperation::File => self.session_allow_file_ops = true,
            ConfirmationOperation::Bash => self.session_allow_bash_ops = true,
        }
    }

    pub fn reset_conversation(&mut self) {
        self.messages = vec![ChatMessage::system(self.system_prompt.clone())];
    }

    pub async fn generate_plain_text(&self, prompt: &str) -> Result<String> {
        self.client.plain_completion(prompt).await
    }

    pub(crate) fn session_snapshot(&self) -> Result<AgentSessionSnapshot> {
        Ok(AgentSessionSnapshot {
            model: self.current_model().to_string(),
            messages: self.messages.clone(),
            tool_session: self.tool_session.snapshot()?,
            auto_edit_enabled: self.auto_edit_enabled,
            session_allow_file_ops: self.session_allow_file_ops,
            session_allow_bash_ops: self.session_allow_bash_ops,
        })
    }

    pub(crate) fn restore_session_snapshot(
        &mut self,
        snapshot: AgentSessionSnapshot,
    ) -> Result<()> {
        self.client.set_model(snapshot.model);
        self.messages = if snapshot.messages.is_empty() {
            vec![ChatMessage::system(self.system_prompt.clone())]
        } else {
            snapshot.messages
        };
        self.tool_session.restore(snapshot.tool_session)?;
        self.auto_edit_enabled = snapshot.auto_edit_enabled;
        self.session_allow_file_ops = snapshot.session_allow_file_ops;
        self.session_allow_bash_ops = snapshot.session_allow_bash_ops;
        Ok(())
    }

    pub async fn execute_bash_command(&mut self, command: &str) -> Result<ToolResult> {
        execute_bash_command(command, &mut self.tool_session).await
    }

    pub async fn process_user_message(&mut self, user_message: &str) -> Result<String> {
        self.process_user_message_with_attachments(user_message, Vec::new())
            .await
    }

    pub async fn process_user_message_with_attachments(
        &mut self,
        user_message: &str,
        attachments: Vec<ChatImageAttachment>,
    ) -> Result<String> {
        self.messages.push(ChatMessage::user_with_attachments(
            user_message.to_string(),
            attachments,
        ));

        for _ in 0..self.max_tool_rounds {
            let search_mode = search_mode_for(user_message);

            let response = self
                .client
                .chat(&self.messages, &self.tools, search_mode)
                .await?;
            let message = response
                .choices
                .first()
                .map(|choice| choice.message.clone())
                .context("No chat response choices")?;

            let assistant_content = message.content.clone().unwrap_or_default();
            let assistant_tool_calls = message.tool_calls.clone();
            self.messages.push(ChatMessage::assistant(
                assistant_content.clone(),
                assistant_tool_calls.clone(),
            ));

            if let Some(tool_calls) = assistant_tool_calls {
                for call in tool_calls {
                    let parsed_args = parse_tool_arguments(&call.function.arguments);
                    let result =
                        execute_tool(&call.function.name, &parsed_args, &mut self.tool_session)
                            .await;
                    self.messages
                        .push(ChatMessage::tool(call.id, result.content_for_model()));
                }
                continue;
            }

            return Ok(assistant_content);
        }

        Ok("Maximum tool execution rounds reached.".to_string())
    }

    pub async fn process_user_message_stream(
        &mut self,
        user_message: String,
        attachments: Vec<ChatImageAttachment>,
        cancel_token: CancellationToken,
        updates: mpsc::UnboundedSender<AgentEvent>,
        confirmation_rx: Option<Arc<Mutex<mpsc::UnboundedReceiver<ConfirmationDecision>>>>,
    ) -> Result<()> {
        self.messages.push(ChatMessage::user_with_attachments(
            user_message.clone(),
            attachments,
        ));
        let mut input_tokens = estimate_messages_tokens(&self.messages);
        updates.send(AgentEvent::TokenCount(input_tokens)).ok();

        for _ in 0..self.max_tool_rounds {
            if cancel_token.is_cancelled() {
                send_cancelled(&updates);
                return Ok(());
            }

            let mut content = String::new();
            let mut partial_calls: Vec<PartialToolCall> = Vec::new();
            let mut last_token_emit = std::time::Instant::now();
            let search_mode = search_mode_for(&user_message);

            let mut on_chunk = |chunk: ChatCompletionStreamChunk| {
                for choice in chunk.choices {
                    if let Some(piece) = choice.delta.content
                        && let Some(incremental) = merge_stream_text(&mut content, &piece)
                    {
                        updates.send(AgentEvent::Content(incremental)).ok();
                        if last_token_emit.elapsed() >= std::time::Duration::from_millis(250) {
                            let output_tokens = estimate_text_tokens(&content);
                            updates
                                .send(AgentEvent::TokenCount(input_tokens + output_tokens))
                                .ok();
                            last_token_emit = std::time::Instant::now();
                        }
                    }

                    if let Some(tool_calls) = choice.delta.tool_calls {
                        accumulate_tool_calls(&mut partial_calls, &tool_calls);
                    }
                }
                Ok(())
            };

            self.client
                .stream_chat(
                    &self.messages,
                    &self.tools,
                    search_mode,
                    &cancel_token,
                    &mut on_chunk,
                )
                .await?;

            if cancel_token.is_cancelled() {
                send_cancelled(&updates);
                return Ok(());
            }

            let tool_calls = partial_calls
                .into_iter()
                .filter(|call| !call.name.trim().is_empty())
                .map(|call| ToolCallSummary {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect::<Vec<_>>();
            let tool_call_tokens = tool_calls
                .iter()
                .map(|call| estimate_text_tokens(&call.arguments))
                .sum::<usize>();
            updates
                .send(AgentEvent::TokenCount(
                    input_tokens + estimate_text_tokens(&content) + tool_call_tokens,
                ))
                .ok();

            self.messages.push(ChatMessage::assistant(
                content,
                if tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        tool_calls
                            .iter()
                            .map(|call| ChatToolCall {
                                id: call.id.clone(),
                                r#type: "function".to_string(),
                                function: ChatToolCallFunction {
                                    name: call.name.clone(),
                                    arguments: call.arguments.clone(),
                                },
                            })
                            .collect(),
                    )
                },
            ));

            if tool_calls.is_empty() {
                updates.send(AgentEvent::Done).ok();
                return Ok(());
            }

            updates.send(AgentEvent::ToolCalls(tool_calls.clone())).ok();

            for tool_call in tool_calls {
                if cancel_token.is_cancelled() {
                    send_cancelled(&updates);
                    return Ok(());
                }

                let operation = confirmation_operation_for_tool(&tool_call.name);
                if let Some(operation) = operation {
                    let decision = self
                        .confirm_tool_call(
                            tool_call.clone(),
                            operation,
                            &updates,
                            confirmation_rx.as_ref(),
                            &cancel_token,
                        )
                        .await;
                    if let Some(rejection_message) = decision {
                        let result = ToolResult::err(rejection_message);
                        self.messages.push(ChatMessage::tool(
                            tool_call.id.clone(),
                            result.content_for_model(),
                        ));
                        input_tokens = estimate_messages_tokens(&self.messages);
                        updates.send(AgentEvent::TokenCount(input_tokens)).ok();
                        updates
                            .send(AgentEvent::ToolResult { tool_call, result })
                            .ok();
                        continue;
                    }
                }

                let parsed_args = parse_tool_arguments(&tool_call.arguments);
                let result =
                    execute_tool(&tool_call.name, &parsed_args, &mut self.tool_session).await;

                self.messages.push(ChatMessage::tool(
                    tool_call.id.clone(),
                    result.content_for_model(),
                ));
                input_tokens = estimate_messages_tokens(&self.messages);
                updates.send(AgentEvent::TokenCount(input_tokens)).ok();

                updates
                    .send(AgentEvent::ToolResult { tool_call, result })
                    .ok();
            }
        }

        updates
            .send(AgentEvent::Content(
                "\n\nMaximum tool execution rounds reached. Stopping to prevent infinite loops."
                    .to_string(),
            ))
            .ok();
        updates.send(AgentEvent::Done).ok();
        Ok(())
    }

    async fn confirm_tool_call(
        &mut self,
        tool_call: ToolCallSummary,
        operation: ConfirmationOperation,
        updates: &mpsc::UnboundedSender<AgentEvent>,
        confirmation_rx: Option<&Arc<Mutex<mpsc::UnboundedReceiver<ConfirmationDecision>>>>,
        cancel_token: &CancellationToken,
    ) -> Option<String> {
        if self.auto_edit_enabled {
            return None;
        }

        if operation == ConfirmationOperation::File && self.session_allow_file_ops {
            return None;
        }
        if operation == ConfirmationOperation::Bash && self.session_allow_bash_ops {
            return None;
        }

        let confirmation_rx = confirmation_rx?;

        updates
            .send(AgentEvent::ConfirmationRequest {
                tool_call: tool_call.clone(),
                operation,
            })
            .ok();

        loop {
            if cancel_token.is_cancelled() {
                return Some("Operation cancelled by user".to_string());
            }

            let decision = {
                let mut guard = confirmation_rx.lock().await;
                guard.recv().await
            };

            let Some(decision) = decision else {
                return Some("Operation cancelled: confirmation channel closed".to_string());
            };

            match decision {
                ConfirmationDecision::Approve {
                    tool_call_id,
                    remember_for_session,
                } if tool_call_id == tool_call.id => {
                    if remember_for_session {
                        match operation {
                            ConfirmationOperation::File => self.session_allow_file_ops = true,
                            ConfirmationOperation::Bash => self.session_allow_bash_ops = true,
                        }
                    }
                    return None;
                }
                ConfirmationDecision::Reject {
                    tool_call_id,
                    feedback,
                } if tool_call_id == tool_call.id => {
                    return Some(
                        feedback.unwrap_or_else(|| "Operation cancelled by user".to_string()),
                    );
                }
                _ => {}
            }
        }
    }
}

fn parse_tool_arguments(arguments: &str) -> Value {
    serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}))
}

fn send_cancelled(updates: &mpsc::UnboundedSender<AgentEvent>) {
    updates
        .send(AgentEvent::Content(
            "\n\n[Operation cancelled by user]".to_string(),
        ))
        .ok();
    updates.send(AgentEvent::Done).ok();
}

#[cfg(test)]
mod tests {
    use super::{Agent, AgentEvent, ConfirmationDecision, ToolCallSummary, parse_tool_arguments};
    use crate::confirmation::ConfirmationOperation;
    use crate::grok_client::SearchMode;
    use crate::model_client::{ModelClient, StreamChunkHandler};
    use crate::protocol::{
        ChatChoice, ChatCompletionMessage, ChatCompletionResponse, ChatCompletionStreamChoice,
        ChatCompletionStreamChunk, ChatCompletionStreamDelta, ChatMessage, ChatTool, ChatToolCall,
        ChatToolCallFunction,
    };
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn parse_tool_arguments_returns_empty_object_for_invalid_json() {
        let value = parse_tool_arguments("not json");
        assert_eq!(value, json!({}));
    }

    #[test]
    fn auto_edit_and_session_permissions_control_auto_approval() {
        let temp = TempDir::new("agent-auto-approve");
        let client = MockClient::new("grok-code-fast-1");
        let mut agent = Agent::with_client(client, 2, temp.path()).expect("agent");

        assert!(!agent.is_operation_auto_approved(ConfirmationOperation::File));
        assert!(!agent.is_operation_auto_approved(ConfirmationOperation::Bash));

        agent.remember_operation_for_session(ConfirmationOperation::Bash);
        assert!(agent.is_operation_auto_approved(ConfirmationOperation::Bash));
        assert!(!agent.is_operation_auto_approved(ConfirmationOperation::File));

        agent.set_auto_edit_enabled(true);
        assert!(agent.is_operation_auto_approved(ConfirmationOperation::File));
        assert!(agent.is_operation_auto_approved(ConfirmationOperation::Bash));

        agent.set_auto_edit_enabled(false);
        assert!(!agent.is_operation_auto_approved(ConfirmationOperation::File));
        assert!(!agent.is_operation_auto_approved(ConfirmationOperation::Bash));
    }

    #[tokio::test]
    async fn process_user_message_returns_assistant_content_without_tools() {
        let temp = TempDir::new("agent-process-simple");
        let client =
            MockClient::with_chat("grok-code-fast-1", vec![chat_response("hello back", None)]);
        let mut agent = Agent::with_client(client, 2, temp.path()).expect("agent");

        let output = agent.process_user_message("hello").await.expect("response");
        assert_eq!(output, "hello back");
        assert_eq!(agent.messages.len(), 3);
        assert_eq!(agent.messages[1].role, "user");
        assert_eq!(agent.messages[2].role, "assistant");
    }

    #[tokio::test]
    async fn process_user_message_executes_tool_round_then_returns_final_answer() {
        let temp = TempDir::new("agent-process-tool-round");
        let responses = vec![
            chat_response(
                "",
                Some(vec![tool_call("call_1", "not_a_real_tool", r#"{"x":1}"#)]),
            ),
            chat_response("done", None),
        ];
        let client = MockClient::with_chat("grok-code-fast-1", responses);
        let mut agent = Agent::with_client(client, 3, temp.path()).expect("agent");

        let output = agent.process_user_message("run").await.expect("response");
        assert_eq!(output, "done");
        assert_eq!(agent.messages.len(), 5);
        assert_eq!(agent.messages[3].role, "tool");
        assert_eq!(
            agent.messages[3].content.as_deref(),
            Some("Unknown tool: not_a_real_tool")
        );
    }

    #[tokio::test]
    async fn process_user_message_returns_max_round_message_when_tool_loop_never_finishes() {
        let temp = TempDir::new("agent-process-max-rounds");
        let client = MockClient::with_chat(
            "grok-code-fast-1",
            vec![chat_response(
                "",
                Some(vec![tool_call("call_1", "not_a_real_tool", "{}")]),
            )],
        );
        let mut agent = Agent::with_client(client, 1, temp.path()).expect("agent");

        let output = agent.process_user_message("loop").await.expect("response");
        assert_eq!(output, "Maximum tool execution rounds reached.");
    }

    #[tokio::test]
    async fn session_snapshot_restores_model_messages_and_flags() {
        let temp = TempDir::new("agent-snapshot");
        let mut agent =
            Agent::with_client(MockClient::new("model-a"), 2, temp.path()).expect("agent one");
        agent.set_auto_edit_enabled(true);
        agent.messages.push(ChatMessage::user("hello"));

        let snapshot = agent.session_snapshot().expect("snapshot");

        let mut restored =
            Agent::with_client(MockClient::new("model-b"), 2, temp.path()).expect("agent two");
        restored
            .restore_session_snapshot(snapshot)
            .expect("restore snapshot");

        assert_eq!(restored.current_model(), "model-a");
        assert!(restored.auto_edit_enabled());
        assert!(restored.is_operation_auto_approved(ConfirmationOperation::File));
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.messages[1].content.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn confirm_tool_call_handles_reject_and_approve_with_memory() {
        let temp = TempDir::new("agent-confirm");
        let mut agent =
            Agent::with_client(MockClient::new("model"), 2, temp.path()).expect("agent");
        let cancel = CancellationToken::new();

        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel::<ConfirmationDecision>();
        let confirm_rx = Arc::new(tokio::sync::Mutex::new(confirm_rx));

        confirm_tx
            .send(ConfirmationDecision::Reject {
                tool_call_id: "call_reject".to_string(),
                feedback: Some("nope".to_string()),
            })
            .ok();

        let rejected = agent
            .confirm_tool_call(
                ToolCallSummary {
                    id: "call_reject".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"echo hi"}"#.to_string(),
                },
                ConfirmationOperation::Bash,
                &updates_tx,
                Some(&confirm_rx),
                &cancel,
            )
            .await;
        assert_eq!(rejected.as_deref(), Some("nope"));
        let confirmation_event = updates_rx.recv().await.expect("confirmation event");
        assert!(matches!(
            confirmation_event,
            AgentEvent::ConfirmationRequest { .. }
        ));

        let (updates_tx2, mut updates_rx2) = mpsc::unbounded_channel::<AgentEvent>();
        let (confirm_tx2, confirm_rx2) = mpsc::unbounded_channel::<ConfirmationDecision>();
        let confirm_rx2 = Arc::new(tokio::sync::Mutex::new(confirm_rx2));
        confirm_tx2
            .send(ConfirmationDecision::Approve {
                tool_call_id: "call_approve".to_string(),
                remember_for_session: true,
            })
            .ok();

        let approved = agent
            .confirm_tool_call(
                ToolCallSummary {
                    id: "call_approve".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"echo ok"}"#.to_string(),
                },
                ConfirmationOperation::Bash,
                &updates_tx2,
                Some(&confirm_rx2),
                &cancel,
            )
            .await;
        assert!(approved.is_none());
        assert!(agent.is_operation_auto_approved(ConfirmationOperation::Bash));
        let confirmation_event2 = updates_rx2.recv().await.expect("confirmation event");
        assert!(matches!(
            confirmation_event2,
            AgentEvent::ConfirmationRequest { .. }
        ));
    }

    #[tokio::test]
    async fn process_user_message_stream_emits_content_and_done() {
        let temp = TempDir::new("agent-stream-content");
        let stream_chunks = vec![vec![stream_content_chunk("hello")]];
        let client = MockClient::with_stream("model", stream_chunks);
        let mut agent = Agent::with_client(client, 2, temp.path()).expect("agent");
        let cancel = CancellationToken::new();
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();

        agent
            .process_user_message_stream("prompt".to_string(), Vec::new(), cancel, updates_tx, None)
            .await
            .expect("stream call");

        let mut saw_content = false;
        let mut saw_done = false;
        while let Ok(event) = updates_rx.try_recv() {
            match event {
                AgentEvent::Content(chunk) if chunk.contains("hello") => saw_content = true,
                AgentEvent::Done => saw_done = true,
                _ => {}
            }
        }
        assert!(saw_content);
        assert!(saw_done);
    }

    #[tokio::test]
    async fn process_user_message_stream_honors_pre_cancelled_token() {
        let temp = TempDir::new("agent-stream-cancel");
        let client = MockClient::new("model");
        let mut agent = Agent::with_client(client, 2, temp.path()).expect("agent");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();

        agent
            .process_user_message_stream("prompt".to_string(), Vec::new(), cancel, updates_tx, None)
            .await
            .expect("stream call");

        let mut saw_cancel_message = false;
        let mut saw_done = false;
        while let Ok(event) = updates_rx.try_recv() {
            match event {
                AgentEvent::Content(text) if text.contains("Operation cancelled by user") => {
                    saw_cancel_message = true
                }
                AgentEvent::Done => saw_done = true,
                _ => {}
            }
        }
        assert!(saw_cancel_message);
        assert!(saw_done);
    }

    fn chat_response(
        content: &str,
        tool_calls: Option<Vec<ChatToolCall>>,
    ) -> ChatCompletionResponse {
        ChatCompletionResponse {
            choices: vec![ChatChoice {
                message: ChatCompletionMessage {
                    content: Some(content.to_string()),
                    tool_calls,
                },
            }],
        }
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> ChatToolCall {
        ChatToolCall {
            id: id.to_string(),
            r#type: "function".to_string(),
            function: ChatToolCallFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn stream_content_chunk(content: &str) -> ChatCompletionStreamChunk {
        ChatCompletionStreamChunk {
            choices: vec![ChatCompletionStreamChoice {
                delta: ChatCompletionStreamDelta {
                    content: Some(content.to_string()),
                    tool_calls: None,
                },
            }],
        }
    }

    struct MockClient {
        model: String,
        chat_responses: Arc<Mutex<VecDeque<ChatCompletionResponse>>>,
        stream_sequences: Arc<Mutex<VecDeque<Vec<ChatCompletionStreamChunk>>>>,
        plain_text: Arc<Mutex<String>>,
    }

    impl MockClient {
        fn new(model: &str) -> Self {
            Self {
                model: model.to_string(),
                chat_responses: Arc::new(Mutex::new(VecDeque::new())),
                stream_sequences: Arc::new(Mutex::new(VecDeque::new())),
                plain_text: Arc::new(Mutex::new("ok".to_string())),
            }
        }

        fn with_chat(model: &str, responses: Vec<ChatCompletionResponse>) -> Self {
            let client = Self::new(model);
            *client.chat_responses.lock().expect("lock chat responses") =
                responses.into_iter().collect();
            client
        }

        fn with_stream(model: &str, sequences: Vec<Vec<ChatCompletionStreamChunk>>) -> Self {
            let client = Self::new(model);
            *client
                .stream_sequences
                .lock()
                .expect("lock stream sequences") = sequences.into_iter().collect();
            client
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
            self.chat_responses
                .lock()
                .expect("lock chat responses")
                .pop_front()
                .ok_or_else(|| anyhow!("No queued chat response"))
        }

        async fn stream_chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ChatTool],
            _search_mode: SearchMode,
            _cancel_token: &CancellationToken,
            on_chunk: &mut StreamChunkHandler<'_>,
        ) -> Result<()> {
            let chunks = self
                .stream_sequences
                .lock()
                .expect("lock stream sequences")
                .pop_front()
                .unwrap_or_default();
            for chunk in chunks {
                on_chunk(chunk)?;
            }
            Ok(())
        }

        async fn plain_completion(&self, _prompt: &str) -> Result<String> {
            Ok(self.plain_text.lock().expect("lock plain text").clone())
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
