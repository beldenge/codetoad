use crate::custom_instructions::load_custom_instructions;
use crate::grok_client::{GrokClient, SearchMode};
use crate::protocol::{
    ChatCompletionMessage, ChatCompletionToolCallDelta, ChatMessage, ChatTool, ChatToolCall,
    ChatToolCallFunction, ChatToolFunction,
};
use crate::tools::{ToolResult, execute_tool};
use anyhow::{Context, Result};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationOperation {
    File,
    Bash,
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

#[derive(Debug, Clone)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Clone)]
pub struct Agent {
    client: GrokClient,
    messages: Vec<ChatMessage>,
    system_prompt: String,
    max_tool_rounds: usize,
    tools: Vec<ChatTool>,
    auto_edit_enabled: bool,
    session_allow_file_ops: bool,
    session_allow_bash_ops: bool,
}

impl Agent {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        max_tool_rounds: usize,
        cwd: &Path,
    ) -> Result<Self> {
        let client = GrokClient::new(api_key, base_url, model)?;
        let system_prompt = build_system_prompt(cwd);
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
            auto_edit_enabled: false,
            session_allow_file_ops: false,
            session_allow_bash_ops: false,
        })
    }

    pub fn current_model(&self) -> &str {
        self.client.current_model()
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

    pub async fn process_user_message(&mut self, user_message: &str) -> Result<String> {
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: Some(user_message.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });

        for _ in 0..self.max_tool_rounds {
            let search_mode = if should_use_search_for(user_message) {
                SearchMode::Auto
            } else {
                SearchMode::Off
            };

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
                    let parsed_args = serde_json::from_str::<Value>(&call.function.arguments)
                        .unwrap_or_else(|_| json!({}));
                    let result = execute_tool(&call.function.name, &parsed_args).await;
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
                updates
                    .send(AgentEvent::Content(
                        "\n\n[Operation cancelled by user]".to_string(),
                    ))
                    .ok();
                updates.send(AgentEvent::Done).ok();
                return Ok(());
            }

            let mut content = String::new();
            let mut partial_calls: Vec<PartialToolCall> = Vec::new();
            let mut last_token_emit = std::time::Instant::now();
            let search_mode = if should_use_search_for(&user_message) {
                SearchMode::Auto
            } else {
                SearchMode::Off
            };

            self.client
                .stream_chat(
                    &self.messages,
                    &self.tools,
                    search_mode,
                    &cancel_token,
                    |chunk| {
                        for choice in chunk.choices {
                            if let Some(piece) = choice.delta.content
                                && let Some(incremental) = merge_stream_text(&mut content, &piece)
                            {
                                updates.send(AgentEvent::Content(incremental)).ok();
                                if last_token_emit.elapsed()
                                    >= std::time::Duration::from_millis(250)
                                {
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
                    },
                )
                .await?;

            if cancel_token.is_cancelled() {
                updates
                    .send(AgentEvent::Content(
                        "\n\n[Operation cancelled by user]".to_string(),
                    ))
                    .ok();
                updates.send(AgentEvent::Done).ok();
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
                    updates
                        .send(AgentEvent::Content(
                            "\n\n[Operation cancelled by user]".to_string(),
                        ))
                        .ok();
                    updates.send(AgentEvent::Done).ok();
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

                let parsed_args = serde_json::from_str::<Value>(&tool_call.arguments)
                    .with_context(|| format!("Invalid tool call arguments for {}", tool_call.name))
                    .unwrap_or_else(|_| json!({}));
                let result = execute_tool(&tool_call.name, &parsed_args).await;

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

fn accumulate_tool_calls(
    target: &mut Vec<PartialToolCall>,
    deltas: &[ChatCompletionToolCallDelta],
) {
    for delta in deltas {
        while target.len() <= delta.index {
            target.push(PartialToolCall {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        }

        let entry = &mut target[delta.index];
        if let Some(id) = &delta.id {
            entry.id.push_str(id);
        }
        if let Some(function) = &delta.function {
            if let Some(name) = &function.name {
                merge_stream_field(&mut entry.name, name);
            }
            if let Some(arguments) = &function.arguments {
                merge_stream_field(&mut entry.arguments, arguments);
            }
        }

        if entry.id.is_empty() {
            entry.id = format!("call_{}", delta.index);
        }
    }
}

fn merge_stream_field(target: &mut String, delta: &str) {
    if delta.is_empty() {
        return;
    }
    if target.is_empty() {
        target.push_str(delta);
        return;
    }

    // Some providers emit full field values repeatedly instead of token deltas.
    // Replace with the longer prefix form rather than duplicating content.
    if delta.starts_with(target.as_str()) {
        *target = delta.to_string();
        return;
    }
    if target.as_str() == delta {
        return;
    }
    append_with_overlap(target, delta);
}

fn merge_stream_text(target: &mut String, incoming: &str) -> Option<String> {
    if incoming.is_empty() {
        return None;
    }
    if target.is_empty() {
        target.push_str(incoming);
        return Some(incoming.to_string());
    }

    // Some streams send complete snapshots repeatedly instead of deltas.
    if incoming == target.as_str() {
        return None;
    }
    if incoming.starts_with(target.as_str()) {
        let suffix = &incoming[target.len()..];
        if suffix.is_empty() {
            return None;
        }
        target.push_str(suffix);
        return Some(suffix.to_string());
    }

    let appended = append_with_overlap(target, incoming);
    if appended.is_empty() {
        None
    } else {
        Some(appended)
    }
}

fn append_with_overlap(target: &mut String, incoming: &str) -> String {
    if incoming.is_empty() {
        return String::new();
    }

    let mut overlap_len = 0usize;
    let mut boundaries = Vec::new();
    boundaries.push(0usize);
    boundaries.extend(incoming.char_indices().map(|(idx, _)| idx).skip(1));
    boundaries.push(incoming.len());

    for size in boundaries.into_iter().rev() {
        if size == 0 || size > target.len() {
            continue;
        }
        if target.ends_with(&incoming[..size]) {
            overlap_len = size;
            break;
        }
    }

    let suffix = &incoming[overlap_len..];
    target.push_str(suffix);
    suffix.to_string()
}

fn build_system_prompt(cwd: &Path) -> String {
    let custom = load_custom_instructions(cwd)
        .map(|instructions| {
            format!(
                "\n\nCUSTOM INSTRUCTIONS:\n{}\n\nFollow the custom instructions above while respecting the tool safety constraints below.\n",
                instructions
            )
        })
        .unwrap_or_default();

    format!(
        "You are Grok CLI, an AI coding assistant in a terminal environment.{custom}
You can use these tools:
- view_file: Read file contents or list directories.
- create_file: Create a new file.
- str_replace_editor: Replace text in an existing file.
- bash: Run shell commands.

Important behavior:
- Use view_file before editing when practical.
- Use str_replace_editor for existing files instead of create_file.
- Keep responses concise and directly tied to the task.
- Use bash for file discovery and command execution when useful.

Current working directory: {}",
        cwd.display()
    )
}

fn default_tools() -> Vec<ChatTool> {
    vec![
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "view_file".to_string(),
                description: "View contents of a file or list directory contents".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to file or directory" },
                        "start_line": { "type": "number", "description": "Optional start line" },
                        "end_line": { "type": "number", "description": "Optional end line" }
                    },
                    "required": ["path"]
                }),
            },
        },
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "create_file".to_string(),
                description: "Create a new file with specified content".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            },
        },
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "str_replace_editor".to_string(),
                description: "Replace text in an existing file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old_str": { "type": "string" },
                        "new_str": { "type": "string" },
                        "replace_all": { "type": "boolean" }
                    },
                    "required": ["path", "old_str", "new_str"]
                }),
            },
        },
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "bash".to_string(),
                description: "Execute a shell command".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"]
                }),
            },
        },
    ]
}

fn should_use_search_for(message: &str) -> bool {
    let lowered = message.to_lowercase();
    let keywords = [
        "today",
        "latest",
        "news",
        "trending",
        "current",
        "recent",
        "price",
        "release notes",
        "changelog",
    ];
    keywords.iter().any(|k| lowered.contains(k))
}

fn confirmation_operation_for_tool(tool_name: &str) -> Option<ConfirmationOperation> {
    match tool_name {
        "create_file" | "str_replace_editor" => Some(ConfirmationOperation::File),
        "bash" => Some(ConfirmationOperation::Bash),
        _ => None,
    }
}

fn estimate_messages_tokens(messages: &[ChatMessage]) -> usize {
    let mut chars = 0usize;
    for message in messages {
        chars += message.role.chars().count();
        if let Some(content) = &message.content {
            chars += content.chars().count();
        }
        if let Some(tool_id) = &message.tool_call_id {
            chars += tool_id.chars().count();
        }
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                chars += call.id.chars().count();
                chars += call.function.name.chars().count();
                chars += call.function.arguments.chars().count();
            }
        }
    }
    estimate_chars_tokens(chars)
}

fn estimate_text_tokens(text: &str) -> usize {
    estimate_chars_tokens(text.chars().count())
}

fn estimate_chars_tokens(char_count: usize) -> usize {
    if char_count == 0 {
        0
    } else {
        // Rough token approximation for streaming UX feedback.
        char_count.div_ceil(4)
    }
}

#[allow(dead_code)]
fn _assistant_to_message(message: ChatCompletionMessage) -> ChatMessage {
    ChatMessage {
        role: "assistant".to_string(),
        content: message.content,
        tool_calls: message.tool_calls,
        tool_call_id: None,
    }
}
