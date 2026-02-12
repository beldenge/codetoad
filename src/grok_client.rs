use crate::protocol::{
    ChatChoice, ChatCompletionMessage, ChatCompletionResponse, ChatCompletionStreamChoice,
    ChatCompletionStreamChunk, ChatCompletionStreamDelta, ChatCompletionToolCallDelta,
    ChatCompletionToolCallFunctionDelta, ChatMessage, ChatTool, ChatToolCall, ChatToolCallFunction,
};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct GrokClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    current_model: String,
    max_tokens: u32,
    use_responses_api: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum SearchMode {
    Auto,
    Off,
}

impl SearchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
}

impl GrokClient {
    pub fn new(api_key: String, base_url: String, model: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(360))
            .build()
            .context("Failed building HTTP client")?;

        let normalized_base_url = base_url.trim_end_matches('/').to_string();
        let use_responses_api = normalized_base_url.to_lowercase().contains("api.x.ai");

        Ok(Self {
            http,
            api_key,
            base_url: normalized_base_url,
            current_model: model,
            max_tokens: std::env::var("GROK_MAX_TOKENS")
                .ok()
                .and_then(|raw| raw.parse::<u32>().ok())
                .filter(|val| *val > 0)
                .unwrap_or(1536),
            use_responses_api,
        })
    }

    pub fn set_model(&mut self, model: String) {
        self.current_model = model;
    }

    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse> {
        if self.use_responses_api {
            self.chat_with_responses_api(messages, tools, search_mode)
                .await
        } else {
            self.chat_with_chat_completions(messages, tools, search_mode)
                .await
        }
    }

    pub async fn stream_chat<F>(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
        cancel_token: &CancellationToken,
        mut on_chunk: F,
    ) -> Result<()>
    where
        F: FnMut(ChatCompletionStreamChunk) -> Result<()>,
    {
        if self.use_responses_api {
            self.stream_chat_with_responses_api(
                messages,
                tools,
                search_mode,
                cancel_token,
                &mut on_chunk,
            )
            .await
        } else {
            self.stream_chat_with_chat_completions(
                messages,
                tools,
                search_mode,
                cancel_token,
                &mut on_chunk,
            )
            .await
        }
    }

    pub async fn plain_completion(&self, prompt: &str) -> Result<String> {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: Some(prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = self.chat(&messages, &[], SearchMode::Off).await?;
        let content = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .unwrap_or_default();
        Ok(content.trim().to_string())
    }

    async fn chat_with_chat_completions(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse> {
        let payload = ChatCompletionsPayload::new(
            self.current_model.clone(),
            messages.to_vec(),
            tools.to_vec(),
            false,
            self.max_tokens,
            search_mode,
        );

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed sending chat completion request")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed reading chat response body")?;
        validate_status(status, &body)?;
        let parsed = serde_json::from_str::<ChatCompletionResponse>(&body)
            .context("Failed parsing chat completion response")?;
        Ok(parsed)
    }

    async fn chat_with_responses_api(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse> {
        let payload = ResponsesPayload::new(
            self.current_model.clone(),
            convert_messages_to_responses_input(messages),
            flatten_tools(tools),
            false,
            self.max_tokens,
            search_mode,
        );

        let response = self
            .http
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed sending responses request")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed reading responses body")?;
        validate_status(status, &body)?;
        convert_responses_body_to_chat_completion(&body)
    }

    async fn stream_chat_with_chat_completions<F>(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
        cancel_token: &CancellationToken,
        on_chunk: &mut F,
    ) -> Result<()>
    where
        F: FnMut(ChatCompletionStreamChunk) -> Result<()>,
    {
        let payload = ChatCompletionsPayload::new(
            self.current_model.clone(),
            messages.to_vec(),
            tools.to_vec(),
            true,
            self.max_tokens,
            search_mode,
        );

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed sending streaming chat completion request")?;

        let status = response.status();
        if status != StatusCode::OK {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown streaming error".to_string());
            bail!("Chat API returned {}: {}", status, body);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            if cancel_token.is_cancelled() {
                return Ok(());
            }

            let bytes = chunk.context("Failed reading streaming response chunk")?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(newline_idx) = buffer.find('\n') {
                let line = buffer[..newline_idx].trim().to_string();
                buffer = buffer[(newline_idx + 1)..].to_string();
                if line.is_empty() {
                    continue;
                }
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data == "[DONE]" {
                    return Ok(());
                }
                let parsed = serde_json::from_str::<ChatCompletionStreamChunk>(data)
                    .with_context(|| format!("Failed parsing streaming chunk: {data}"))?;
                on_chunk(parsed)?;
            }
        }
        Ok(())
    }

    async fn stream_chat_with_responses_api<F>(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
        cancel_token: &CancellationToken,
        on_chunk: &mut F,
    ) -> Result<()>
    where
        F: FnMut(ChatCompletionStreamChunk) -> Result<()>,
    {
        let payload = ResponsesPayload::new(
            self.current_model.clone(),
            convert_messages_to_responses_input(messages),
            flatten_tools(tools),
            true,
            self.max_tokens,
            search_mode,
        );

        let response = self
            .http
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed sending streaming responses request")?;

        let status = response.status();
        if status != StatusCode::OK {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown streaming error".to_string());
            bail!("Responses API returned {}: {}", status, body);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut current_event: Option<String> = None;
        let mut current_data = String::new();

        while let Some(chunk) = stream.next().await {
            if cancel_token.is_cancelled() {
                return Ok(());
            }

            let bytes = chunk.context("Failed reading streaming response chunk")?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(newline_idx) = buffer.find('\n') {
                let raw_line = buffer[..newline_idx].to_string();
                buffer = buffer[(newline_idx + 1)..].to_string();
                let line = raw_line.trim_end_matches('\r').trim();

                if line.is_empty() {
                    if !current_data.is_empty()
                        && handle_sse_event(current_event.as_deref(), &current_data, on_chunk)?
                    {
                        return Ok(());
                    }
                    current_event = None;
                    current_data.clear();
                    continue;
                }

                if let Some(event_name) = line.strip_prefix("event:") {
                    current_event = Some(event_name.trim().to_string());
                    continue;
                }

                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if !current_data.is_empty() {
                        current_data.push('\n');
                    }
                    current_data.push_str(data);
                    continue;
                }
            }
        }

        if !current_data.is_empty() {
            handle_sse_event(current_event.as_deref(), &current_data, on_chunk)?;
        }
        Ok(())
    }
}

fn handle_sse_event<F>(event: Option<&str>, data: &str, on_chunk: &mut F) -> Result<bool>
where
    F: FnMut(ChatCompletionStreamChunk) -> Result<()>,
{
    if data == "[DONE]" {
        return Ok(true);
    }

    match event.unwrap_or_default() {
        "response.done" | "response.completed" => return Ok(true),
        "response.output_text.delta" | "response.content_part.delta" => {
            let payload = serde_json::from_str::<Value>(data)
                .with_context(|| format!("Invalid responses delta payload: {data}"))?;
            let delta = payload
                .get("delta")
                .and_then(Value::as_str)
                .or_else(|| payload.get("text").and_then(Value::as_str))
                .or_else(|| {
                    payload
                        .get("part")
                        .and_then(|part| part.get("text"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default();

            if !delta.is_empty() {
                on_chunk(make_content_chunk(delta))?;
            }
            return Ok(false);
        }
        "response.output_item.added" | "response.output_item.done" => {
            if let Some(tool_chunk) = parse_tool_call_event(data)? {
                on_chunk(tool_chunk)?;
            }
            return Ok(false);
        }
        "response.error" => {
            let payload = serde_json::from_str::<Value>(data).unwrap_or_else(|_| json!({}));
            let message = payload
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| payload.get("message").and_then(Value::as_str))
                .unwrap_or("Unknown responses API error");
            bail!("Responses stream error: {message}");
        }
        _ => {}
    }

    if let Some(tool_chunk) = parse_tool_call_event(data)? {
        on_chunk(tool_chunk)?;
        return Ok(false);
    }

    if let Ok(chunk) = serde_json::from_str::<ChatCompletionStreamChunk>(data) {
        on_chunk(chunk)?;
    }
    Ok(false)
}

fn convert_messages_to_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    let mut input = Vec::new();

    for message in messages {
        match message.role.as_str() {
            "tool" => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": message.tool_call_id.clone().unwrap_or_else(|| "call_unknown".to_string()),
                    "output": message.content.clone().unwrap_or_default(),
                }));
            }
            "assistant" => {
                if let Some(content) = message.content.clone()
                    && !content.trim().is_empty()
                {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "input_text", "text": content }],
                    }));
                }
                if let Some(tool_calls) = message.tool_calls.clone() {
                    for tool_call in tool_calls {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tool_call.id,
                            "name": tool_call.function.name,
                            "arguments": tool_call.function.arguments,
                        }));
                    }
                }
            }
            role => {
                let content = message.content.clone().unwrap_or_default();
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": [{ "type": "input_text", "text": content }],
                }));
            }
        }
    }

    input
}

fn flatten_tools(tools: &[ChatTool]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": tool.r#type,
                "name": tool.function.name,
                "description": tool.function.description,
                "parameters": tool.function.parameters,
            })
        })
        .collect()
}

fn convert_responses_body_to_chat_completion(body: &str) -> Result<ChatCompletionResponse> {
    let payload = serde_json::from_str::<Value>(body)
        .with_context(|| format!("Invalid response JSON: {body}"))?;

    let output = payload
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for (index, item) in output.iter().enumerate() {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match item_type {
            "message" => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        let part_type =
                            part.get("type").and_then(Value::as_str).unwrap_or_default();
                        if matches!(part_type, "output_text" | "text" | "input_text")
                            && let Some(text) = part.get("text").and_then(Value::as_str)
                            && !text.trim().is_empty()
                        {
                            content_parts.push(text.to_string());
                        }
                    }
                }
            }
            "function_call" => {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        item.get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default()
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        item.get("function")
                            .and_then(|function| function.get("arguments"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or("{}")
                    .to_string();
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("call_id").and_then(Value::as_str))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("call_{index}"));

                if !name.trim().is_empty() {
                    tool_calls.push(ChatToolCall {
                        id,
                        r#type: "function".to_string(),
                        function: ChatToolCallFunction { name, arguments },
                    });
                }
            }
            _ => {}
        }
    }

    if content_parts.is_empty()
        && let Some(top_level_text) = payload.get("output_text").and_then(Value::as_str)
        && !top_level_text.trim().is_empty()
    {
        content_parts.push(top_level_text.to_string());
    }

    Ok(ChatCompletionResponse {
        choices: vec![ChatChoice {
            message: ChatCompletionMessage {
                content: if content_parts.is_empty() {
                    None
                } else {
                    Some(content_parts.join(""))
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
            },
        }],
    })
}

fn parse_tool_call_event(data: &str) -> Result<Option<ChatCompletionStreamChunk>> {
    let payload = serde_json::from_str::<Value>(data).unwrap_or_else(|_| json!({}));
    let item = payload
        .get("item")
        .cloned()
        .unwrap_or_else(|| payload.clone());
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();

    if item_type != "function_call" {
        return Ok(None);
    }

    let name = item
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string();
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("function")
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
        })
        .unwrap_or("{}")
        .to_string();
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| item.get("call_id").and_then(Value::as_str))
        .unwrap_or("call_0")
        .to_string();
    let index = payload
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;

    if name.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(ChatCompletionStreamChunk {
        choices: vec![ChatCompletionStreamChoice {
            delta: ChatCompletionStreamDelta {
                content: None,
                tool_calls: Some(vec![ChatCompletionToolCallDelta {
                    index,
                    id: Some(id),
                    _type: Some("function".to_string()),
                    function: Some(ChatCompletionToolCallFunctionDelta {
                        name: Some(name),
                        arguments: Some(arguments),
                    }),
                }]),
            },
        }],
    }))
}

fn make_content_chunk(delta: &str) -> ChatCompletionStreamChunk {
    ChatCompletionStreamChunk {
        choices: vec![ChatCompletionStreamChoice {
            delta: ChatCompletionStreamDelta {
                content: Some(delta.to_string()),
                tool_calls: None,
            },
        }],
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionsPayload {
    model: String,
    messages: Vec<ChatMessage>,
    tools: Vec<ChatTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    search_parameters: Value,
}

impl ChatCompletionsPayload {
    fn new(
        model: String,
        messages: Vec<ChatMessage>,
        tools: Vec<ChatTool>,
        stream: bool,
        max_tokens: u32,
        search_mode: SearchMode,
    ) -> Self {
        let tool_choice = if tools.is_empty() {
            None
        } else {
            Some("auto".to_string())
        };

        Self {
            model,
            messages,
            tools,
            tool_choice,
            temperature: 0.7,
            max_tokens,
            stream,
            search_parameters: json!({ "mode": search_mode.as_str() }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ResponsesPayload {
    model: String,
    input: Vec<Value>,
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    temperature: f32,
    max_output_tokens: u32,
    stream: bool,
    search_parameters: Value,
}

impl ResponsesPayload {
    fn new(
        model: String,
        input: Vec<Value>,
        tools: Vec<Value>,
        stream: bool,
        max_tokens: u32,
        search_mode: SearchMode,
    ) -> Self {
        let tool_choice = if tools.is_empty() {
            None
        } else {
            Some("auto".to_string())
        };

        Self {
            model,
            input,
            tools,
            tool_choice,
            temperature: 0.7,
            max_output_tokens: max_tokens,
            stream,
            search_parameters: json!({ "mode": search_mode.as_str() }),
        }
    }
}

fn validate_status(status: StatusCode, body: &str) -> Result<()> {
    if status == StatusCode::OK {
        return Ok(());
    }
    bail!("API returned {}: {}", status, body);
}
