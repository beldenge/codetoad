use crate::protocol::{ChatCompletionResponse, ChatCompletionStreamChunk, ChatMessage, ChatTool};
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

        Ok(Self {
            http,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            current_model: model,
            max_tokens: std::env::var("GROK_MAX_TOKENS")
                .ok()
                .and_then(|raw| raw.parse::<u32>().ok())
                .filter(|val| *val > 0)
                .unwrap_or(1536),
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
        let payload = ChatPayload::new(
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
        validate_status(status, body.clone())?;
        let parsed = serde_json::from_str::<ChatCompletionResponse>(&body)
            .context("Failed parsing chat completion response")?;
        Ok(parsed)
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
        let payload = ChatPayload::new(
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
}

#[derive(Debug, Clone, Serialize)]
struct ChatPayload {
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

impl ChatPayload {
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

fn validate_status(status: StatusCode, body: String) -> Result<()> {
    if status == StatusCode::OK {
        return Ok(());
    }
    bail!("Chat API returned {}: {}", status, body);
}
