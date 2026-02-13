use crate::protocol::ChatMessage;
use serde_json::{Value, json};

pub fn to_chat_completions_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(chat_message_for_chat_completions)
        .collect()
}

pub fn to_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
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
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": responses_content_for_message(message),
                }));
            }
        }
    }

    input
}

fn responses_content_for_message(message: &ChatMessage) -> Vec<Value> {
    let mut content = Vec::<Value>::new();
    if let Some(text) = message.content.clone()
        && !text.trim().is_empty()
    {
        content.push(json!({
            "type": "input_text",
            "text": text,
        }));
    }

    if message.role == "user"
        && let Some(attachments) = &message.attachments
    {
        for attachment in attachments {
            content.push(json!({
                "type": "input_image",
                "image_url": attachment.data_url,
            }));
        }
    }

    if content.is_empty() {
        content.push(json!({
            "type": "input_text",
            "text": "",
        }));
    }
    content
}

fn chat_message_for_chat_completions(message: &ChatMessage) -> Value {
    let mut object = serde_json::Map::<String, Value>::new();
    object.insert("role".to_string(), Value::String(message.role.clone()));

    if let Some(tool_calls) = &message.tool_calls
        && let Ok(value) = serde_json::to_value(tool_calls)
    {
        object.insert("tool_calls".to_string(), value);
    }

    if let Some(tool_call_id) = &message.tool_call_id {
        object.insert(
            "tool_call_id".to_string(),
            Value::String(tool_call_id.clone()),
        );
    }

    let content_value = if message.role == "user" {
        let mut parts = Vec::<Value>::new();
        if let Some(text) = &message.content
            && !text.trim().is_empty()
        {
            parts.push(json!({
                "type": "text",
                "text": text,
            }));
        }

        if let Some(attachments) = &message.attachments {
            for attachment in attachments {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": attachment.data_url },
                }));
            }
        }

        if parts.is_empty() {
            Value::String(String::new())
        } else if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
            parts[0]
                .get("text")
                .and_then(Value::as_str)
                .map(|text| Value::String(text.to_string()))
                .unwrap_or_else(|| Value::Array(parts))
        } else {
            Value::Array(parts)
        }
    } else {
        Value::String(message.content.clone().unwrap_or_default())
    };
    object.insert("content".to_string(), content_value);

    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::{to_chat_completions_messages, to_responses_input};
    use crate::protocol::{ChatImageAttachment, ChatMessage};
    use serde_json::Value;

    #[test]
    fn responses_projection_includes_user_image_parts() {
        let messages = vec![ChatMessage::user_with_attachments(
            "What is shown here?",
            vec![ChatImageAttachment {
                filename: "shot.png".to_string(),
                mime_type: "image/png".to_string(),
                data_url: "data:image/png;base64,abc123".to_string(),
            }],
        )];
        let input = to_responses_input(&messages);
        let content = input[0]
            .get("content")
            .and_then(Value::as_array)
            .expect("content is array");
        assert_eq!(content.len(), 2);
        assert_eq!(
            content[1].get("type").and_then(Value::as_str),
            Some("input_image")
        );
    }

    #[test]
    fn chat_projection_includes_image_url_part() {
        let messages = vec![ChatMessage::user_with_attachments(
            "Describe this image",
            vec![ChatImageAttachment {
                filename: "snap.png".to_string(),
                mime_type: "image/png".to_string(),
                data_url: "data:image/png;base64,abc".to_string(),
            }],
        )];
        let projected = to_chat_completions_messages(&messages);
        let content = projected[0]
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
