use crate::message_projection::to_chat_completions_messages;
use crate::model_client::{ModelClient, StreamChunkHandler};
use crate::protocol::{ChatCompletionResponse, ChatCompletionStreamChunk, ChatMessage, ChatTool};
use crate::provider::{ProviderKind, detect_provider};
use crate::responses_adapter::{
    convert_messages_to_responses_input, convert_responses_body_to_chat_completion, flatten_tools,
    handle_sse_event, server_side_search_tools, supports_server_side_tools,
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
    provider: ProviderKind,
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
        let provider = detect_provider(&normalized_base_url);
        let use_responses_api = matches!(provider, ProviderKind::Xai);

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
            provider,
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
        let messages = vec![ChatMessage::user(prompt.to_string())];

        let response = self.chat(&messages, &[], SearchMode::Off).await?;
        let content = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .unwrap_or_default();
        Ok(content.trim().to_string())
    }

    async fn post_json(
        &self,
        endpoint: &str,
        payload: &impl Serialize,
        request_context: &str,
        body_context: &str,
    ) -> Result<(StatusCode, String)> {
        let response = self
            .http
            .post(format!("{}/{}", self.base_url, endpoint))
            .bearer_auth(&self.api_key)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("Failed sending {request_context}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("Failed reading {body_context}"))?;
        Ok((status, body))
    }

    async fn post_json_stream(
        &self,
        endpoint: &str,
        payload: &impl Serialize,
        request_context: &str,
    ) -> Result<reqwest::Response> {
        self.http
            .post(format!("{}/{}", self.base_url, endpoint))
            .bearer_auth(&self.api_key)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("Failed sending {request_context}"))
    }

    async fn chat_with_chat_completions(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse> {
        let payload = ChatCompletionsPayload::new(
            self.current_model.clone(),
            to_chat_completions_messages(messages),
            tools.to_vec(),
            false,
            self.max_tokens,
            search_mode,
            matches!(self.provider, ProviderKind::Xai),
        );
        let (status, body) = self
            .post_json(
                "chat/completions",
                &payload,
                "chat completion request",
                "chat response body",
            )
            .await?;
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
        let model_for_request = self.responses_model_for(search_mode);
        let payload = ResponsesPayload::new(
            model_for_request,
            convert_messages_to_responses_input(messages),
            flatten_tools(tools),
            false,
            self.max_tokens,
            search_mode,
        );
        let (status, body) = self
            .post_json("responses", &payload, "responses request", "responses body")
            .await?;
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
            to_chat_completions_messages(messages),
            tools.to_vec(),
            true,
            self.max_tokens,
            search_mode,
            matches!(self.provider, ProviderKind::Xai),
        );
        let response = self
            .post_json_stream(
                "chat/completions",
                &payload,
                "streaming chat completion request",
            )
            .await?;

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
        let model_for_request = self.responses_model_for(search_mode);
        let payload = ResponsesPayload::new(
            model_for_request,
            convert_messages_to_responses_input(messages),
            flatten_tools(tools),
            true,
            self.max_tokens,
            search_mode,
        );
        let response = self
            .post_json_stream("responses", &payload, "streaming responses request")
            .await?;

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

    fn responses_model_for(&self, search_mode: SearchMode) -> String {
        if !matches!(search_mode, SearchMode::Auto) {
            return self.current_model.clone();
        }
        if supports_server_side_tools(&self.current_model) {
            return self.current_model.clone();
        }

        std::env::var("GROK_SEARCH_MODEL")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "grok-4-latest".to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionsPayload {
    model: String,
    messages: Vec<Value>,
    tools: Vec<ChatTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_parameters: Option<Value>,
}

impl ChatCompletionsPayload {
    fn new(
        model: String,
        messages: Vec<Value>,
        tools: Vec<ChatTool>,
        stream: bool,
        max_tokens: u32,
        search_mode: SearchMode,
        include_search_parameters: bool,
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
            search_parameters: include_search_parameters
                .then(|| json!({ "mode": search_mode.as_str() })),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    search_parameters: Option<Value>,
}

impl ResponsesPayload {
    fn new(
        model: String,
        input: Vec<Value>,
        mut tools: Vec<Value>,
        stream: bool,
        max_tokens: u32,
        search_mode: SearchMode,
    ) -> Self {
        if matches!(search_mode, SearchMode::Auto) && supports_server_side_tools(&model) {
            tools.extend(server_side_search_tools());
        }

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
            // xAI Responses API uses built-in server-side tools for live search.
            // Legacy search_parameters are intentionally omitted to avoid deprecation errors.
            search_parameters: None,
        }
    }
}

fn validate_status(status: StatusCode, body: &str) -> Result<()> {
    if status == StatusCode::OK {
        return Ok(());
    }
    bail!("API returned {}: {}", status, body);
}

#[async_trait::async_trait]
impl ModelClient for GrokClient {
    fn set_model(&mut self, model: String) {
        GrokClient::set_model(self, model);
    }

    fn current_model(&self) -> &str {
        GrokClient::current_model(self)
    }

    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
    ) -> Result<ChatCompletionResponse> {
        GrokClient::chat(self, messages, tools, search_mode).await
    }

    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[ChatTool],
        search_mode: SearchMode,
        cancel_token: &CancellationToken,
        on_chunk: &mut StreamChunkHandler<'_>,
    ) -> Result<()> {
        GrokClient::stream_chat(self, messages, tools, search_mode, cancel_token, |chunk| {
            on_chunk(chunk)
        })
        .await
    }

    async fn plain_completion(&self, prompt: &str) -> Result<String> {
        GrokClient::plain_completion(self, prompt).await
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatCompletionsPayload, SearchMode};
    use crate::message_projection::to_chat_completions_messages;
    use crate::protocol::ChatMessage;
    use crate::provider::{ProviderKind, detect_provider};
    use serde_json::Value;

    #[test]
    fn non_xai_payload_omits_search_parameters() {
        let payload = ChatCompletionsPayload::new(
            "gpt-4.1".to_string(),
            to_chat_completions_messages(&[ChatMessage::user("hello")]),
            Vec::new(),
            false,
            256,
            SearchMode::Auto,
            false,
        );

        let serialized = serde_json::to_value(&payload).expect("serializes");
        assert_eq!(serialized.get("search_parameters"), None);
    }

    #[test]
    fn xai_payload_includes_search_parameters() {
        let payload = ChatCompletionsPayload::new(
            "grok-4-latest".to_string(),
            to_chat_completions_messages(&[ChatMessage::user("hello")]),
            Vec::new(),
            false,
            256,
            SearchMode::Auto,
            true,
        );

        let serialized = serde_json::to_value(&payload).expect("serializes");
        let search = serialized
            .get("search_parameters")
            .and_then(Value::as_object)
            .expect("xai payload has search params");
        assert_eq!(search.get("mode").and_then(Value::as_str), Some("auto"));
    }

    #[test]
    fn xai_provider_uses_responses_api_detection() {
        assert_eq!(detect_provider("https://api.x.ai/v1"), ProviderKind::Xai);
    }

    #[test]
    fn chat_completions_maps_user_image_attachments() {
        let payload = ChatCompletionsPayload::new(
            "gpt-4.1".to_string(),
            to_chat_completions_messages(&[ChatMessage::user_with_attachments(
                "Describe this image",
                vec![crate::protocol::ChatImageAttachment {
                    filename: "snap.png".to_string(),
                    mime_type: "image/png".to_string(),
                    data_url: "data:image/png;base64,abc".to_string(),
                }],
            )]),
            Vec::new(),
            false,
            256,
            SearchMode::Off,
            false,
        );

        let serialized = serde_json::to_value(&payload).expect("serializes");
        let messages = serialized
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages are present");
        let content = messages[0]
            .get("content")
            .and_then(Value::as_array)
            .expect("multimodal content is array");
        assert_eq!(
            content[1]
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "image_url"
        );
    }
}
