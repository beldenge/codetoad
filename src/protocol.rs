use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTool {
    pub r#type: String,
    pub function: ChatToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    pub r#type: String,
    pub function: ChatToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<ChatImageAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatImageAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatChoice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatChoice {
    pub message: ChatCompletionMessage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionMessage {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionStreamChunk {
    pub choices: Vec<ChatCompletionStreamChoice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionStreamChoice {
    pub delta: ChatCompletionStreamDelta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionStreamDelta {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ChatCompletionToolCallDelta>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub _type: Option<String>,
    pub function: Option<ChatCompletionToolCallFunctionDelta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionToolCallFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}
