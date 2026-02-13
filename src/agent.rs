use crate::agent_policy::{
    build_system_prompt, estimate_messages_tokens, estimate_text_tokens, search_mode_for,
};
use crate::agent_stream::{PartialToolCall, accumulate_tool_calls, merge_stream_text};
use crate::confirmation::ConfirmationOperation;
use crate::grok_client::GrokClient;
use crate::model_client::ModelClient;
use crate::protocol::{
    ChatCompletionStreamChunk, ChatMessage, ChatTool, ChatToolCall, ChatToolCallFunction,
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
use tokio::sync::mpsc;
use tokio::sync::Mutex;
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
}

impl<C: ModelClient> Agent<C> {
    pub fn with_client(client: C, max_tool_rounds: usize, cwd: &Path) -> Result<Self> {
        let system_prompt = build_system_prompt(cwd);
        let tool_session = ToolSessionState::new(cwd.to_path_buf())?;
        let messages = vec![ChatMessage {
            role: "system".to_string(),
            content: Some(system_prompt.clone()),
            tool_calls: None,
            tool_call_id: None,
        }];

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
        self.messages = vec![ChatMessage {
            role: "system".to_string(),
            content: Some(self.system_prompt.clone()),
            tool_calls: None,
            tool_call_id: None,
        }];
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

    pub(crate) fn restore_session_snapshot(&mut self, snapshot: AgentSessionSnapshot) -> Result<()> {
        self.client.set_model(snapshot.model);
        self.messages = if snapshot.messages.is_empty() {
            vec![ChatMessage {
                role: "system".to_string(),
                content: Some(self.system_prompt.clone()),
                tool_calls: None,
                tool_call_id: None,
            }]
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
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: Some(user_message.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });

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
            self.messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: Some(assistant_content.clone()),
                tool_calls: assistant_tool_calls.clone(),
                tool_call_id: None,
            });

            if let Some(tool_calls) = assistant_tool_calls {
                for call in tool_calls {
                    let parsed_args = parse_tool_arguments(&call.function.arguments);
                    let result =
                        execute_tool(&call.function.name, &parsed_args, &mut self.tool_session)
                            .await;
                    self.messages.push(ChatMessage {
                        role: "tool".to_string(),
                        content: Some(result.content_for_model()),
                        tool_calls: None,
                        tool_call_id: Some(call.id),
                    });
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
        cancel_token: CancellationToken,
        updates: mpsc::UnboundedSender<AgentEvent>,
        confirmation_rx: Option<Arc<Mutex<mpsc::UnboundedReceiver<ConfirmationDecision>>>>,
    ) -> Result<()> {
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: Some(user_message.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
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

            self.messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: if tool_calls.is_empty() {
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
                tool_call_id: None,
            });

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
                        self.messages.push(ChatMessage {
                            role: "tool".to_string(),
                            content: Some(result.content_for_model()),
                            tool_calls: None,
                            tool_call_id: Some(tool_call.id.clone()),
                        });
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

                self.messages.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(result.content_for_model()),
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id.clone()),
                });
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
