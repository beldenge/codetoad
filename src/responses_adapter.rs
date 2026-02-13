use crate::protocol::{
    ChatChoice, ChatCompletionMessage, ChatCompletionResponse, ChatCompletionStreamChoice,
    ChatCompletionStreamChunk, ChatCompletionStreamDelta, ChatCompletionToolCallDelta,
    ChatCompletionToolCallFunctionDelta, ChatMessage, ChatTool, ChatToolCall, ChatToolCallFunction,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub fn handle_sse_event<F>(event: Option<&str>, data: &str, on_chunk: &mut F) -> Result<bool>
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
        "response.output_text.done" | "response.content_part.done" => {
            let payload = serde_json::from_str::<Value>(data)
                .with_context(|| format!("Invalid responses done payload: {data}"))?;
            let text = payload
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| {
                    payload
                        .get("part")
                        .and_then(|part| part.get("text"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default();
            if !text.is_empty() {
                on_chunk(make_content_chunk(text))?;
            }
            return Ok(false);
        }
        "response.output_item.added" => {
            // Responses API may emit both `added` and `done` for the same item.
            // We process tool calls on `done` to avoid duplicating full payloads.
            return Ok(false);
        }
        "response.output_item.done" => {
            if let Some(text) = parse_message_item_text(data)?
                && !text.is_empty()
            {
                on_chunk(make_content_chunk(&text))?;
            }
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

    if let Some(text) = parse_message_item_text(data)?
        && !text.is_empty()
    {
        on_chunk(make_content_chunk(&text))?;
        return Ok(false);
    }

    if let Ok(chunk) = serde_json::from_str::<ChatCompletionStreamChunk>(data) {
        on_chunk(chunk)?;
    }
    Ok(false)
}

pub fn convert_messages_to_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
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

pub fn flatten_tools(tools: &[ChatTool]) -> Vec<Value> {
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

pub fn convert_responses_body_to_chat_completion(body: &str) -> Result<ChatCompletionResponse> {
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
                let Some(name) = function_call_name(item) else {
                    continue;
                };
                let arguments = function_call_arguments(item);
                let id = function_call_id(item, &format!("call_{index}"));

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

pub fn supports_server_side_tools(model: &str) -> bool {
    model.to_ascii_lowercase().contains("grok-4")
}

pub fn server_side_search_tools() -> Vec<Value> {
    vec![json!({ "type": "web_search" }), json!({ "type": "x_search" })]
}

fn parse_message_item_text(data: &str) -> Result<Option<String>> {
    let payload = serde_json::from_str::<Value>(data).unwrap_or_else(|_| json!({}));
    let item = payload
        .get("item")
        .cloned()
        .unwrap_or_else(|| payload.clone());
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();

    if item_type != "message" {
        return Ok(None);
    }

    let mut parts = Vec::new();
    if let Some(content_items) = item.get("content").and_then(Value::as_array) {
        for part in content_items {
            let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
            if matches!(part_type, "output_text" | "text" | "input_text")
                && let Some(text) = part.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                parts.push(text.to_string());
            }
        }
    }

    if parts.is_empty() {
        return Ok(item
            .get("text")
            .and_then(Value::as_str)
            .map(|s| s.to_string()));
    }

    Ok(Some(parts.join("")))
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

    let Some(name) = function_call_name(&item) else {
        return Ok(None);
    };
    let arguments = function_call_arguments(&item);
    let id = function_call_id(&item, "call_0");
    let index = payload
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;

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

fn function_call_name(item: &Value) -> Option<String> {
    item.get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
}

fn function_call_arguments(item: &Value) -> String {
    item.get("arguments")
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("function")
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
        })
        .unwrap_or("{}")
        .to_string()
}

fn function_call_id(item: &Value, fallback: &str) -> String {
    item.get("id")
        .and_then(Value::as_str)
        .or_else(|| item.get("call_id").and_then(Value::as_str))
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn supports_server_side_tools_only_for_grok4_family() {
        assert!(supports_server_side_tools("grok-4-latest"));
        assert!(supports_server_side_tools("GROK-4"));
        assert!(!supports_server_side_tools("grok-code-fast-1"));
    }

    #[test]
    fn convert_responses_body_maps_message_and_tool_calls() {
        let body = json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        { "type": "output_text", "text": "hello " },
                        { "type": "output_text", "text": "world" }
                    ]
                },
                {
                    "type": "function_call",
                    "call_id": "call_123",
                    "name": "bash",
                    "arguments": "{\"command\":\"pwd\"}"
                }
            ]
        })
        .to_string();

        let completion = convert_responses_body_to_chat_completion(&body).expect("valid payload");
        let message = &completion.choices[0].message;
        assert_eq!(message.content.as_deref(), Some("hello world"));
        let tool_calls = message.tool_calls.as_ref().expect("tool call exists");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_123");
        assert_eq!(tool_calls[0].function.name, "bash");
    }

    #[test]
    fn handle_sse_event_emits_text_delta() {
        let mut chunks = Vec::new();
        let done = handle_sse_event(
            Some("response.output_text.delta"),
            r#"{"delta":"abc"}"#,
            &mut |chunk| {
                chunks.push(chunk);
                Ok(())
            },
        )
        .expect("delta parsing succeeds");

        assert!(!done);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].choices[0].delta.content.as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn handle_sse_event_emits_tool_call_from_done_event() {
        let mut chunks = Vec::new();
        let done = handle_sse_event(
            Some("response.output_item.done"),
            r#"{"item":{"type":"function_call","call_id":"call_9","name":"view_file","arguments":"{\"path\":\"src/main.rs\"}"},"output_index":2}"#,
            &mut |chunk| {
                chunks.push(chunk);
                Ok(())
            },
        )
        .expect("tool event parsing succeeds");

        assert!(!done);
        assert_eq!(chunks.len(), 1);
        let delta = &chunks[0].choices[0].delta;
        let tool_calls = delta.tool_calls.as_ref().expect("tool calls emitted");
        assert_eq!(tool_calls[0].index, 2);
        assert_eq!(tool_calls[0].id.as_deref(), Some("call_9"));
        assert_eq!(
            tool_calls[0]
                .function
                .as_ref()
                .and_then(|f| f.name.as_deref()),
            Some("view_file")
        );
    }
}
