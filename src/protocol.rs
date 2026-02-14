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

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            attachments: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            attachments: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user_with_attachments(
        content: impl Into<String>,
        attachments: Vec<ChatImageAttachment>,
    ) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            attachments: if attachments.is_empty() {
                None
            } else {
                Some(attachments)
            },
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Option<Vec<ChatToolCall>>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.into()),
            attachments: None,
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            attachments: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{ChatImageAttachment, ChatMessage, ChatToolCall, ChatToolCallFunction};

    #[test]
    fn user_message_constructor_sets_expected_fields() {
        let message = ChatMessage::user("hello");

        assert_eq!(message.role, "user");
        assert_eq!(message.content.as_deref(), Some("hello"));
        assert!(message.attachments.is_none());
        assert!(message.tool_calls.is_none());
        assert!(message.tool_call_id.is_none());
    }

    #[test]
    fn user_with_attachments_omits_empty_attachment_list() {
        let message = ChatMessage::user_with_attachments("hello", Vec::new());
        assert!(message.attachments.is_none());
    }

    #[test]
    fn user_with_attachments_preserves_non_empty_attachment_list() {
        let attachment = ChatImageAttachment {
            filename: "screen.png".to_string(),
            mime_type: "image/png".to_string(),
            data_url: "data:image/png;base64,AAAA".to_string(),
        };
        let message = ChatMessage::user_with_attachments("hello", vec![attachment.clone()]);

        assert_eq!(message.attachments.as_ref().map(Vec::len), Some(1));
        let attached = &message.attachments.expect("attachments should be present")[0];
        assert_eq!(attached.filename, attachment.filename);
        assert_eq!(attached.mime_type, attachment.mime_type);
        assert_eq!(attached.data_url, attachment.data_url);
    }

    #[test]
    fn assistant_constructor_preserves_tool_calls() {
        let call = ChatToolCall {
            id: "call_1".to_string(),
            r#type: "function".to_string(),
            function: ChatToolCallFunction {
                name: "view_file".to_string(),
                arguments: "{\"path\":\"README.md\"}".to_string(),
            },
        };
        let message = ChatMessage::assistant("working", Some(vec![call.clone()]));

        assert_eq!(message.role, "assistant");
        assert_eq!(message.content.as_deref(), Some("working"));
        assert_eq!(message.tool_calls.as_ref().map(Vec::len), Some(1));
        let stored = &message.tool_calls.expect("tool calls should be present")[0];
        assert_eq!(stored.id, call.id);
        assert_eq!(stored.function.name, call.function.name);
        assert_eq!(stored.function.arguments, call.function.arguments);
        assert!(message.tool_call_id.is_none());
    }

    #[test]
    fn tool_constructor_sets_tool_call_id_and_content() {
        let message = ChatMessage::tool("call_1", "done");

        assert_eq!(message.role, "tool");
        assert_eq!(message.content.as_deref(), Some("done"));
        assert_eq!(message.tool_call_id.as_deref(), Some("call_1"));
        assert!(message.attachments.is_none());
        assert!(message.tool_calls.is_none());
    }
}
