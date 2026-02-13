use crate::confirmation::ConfirmationOperation;
use crate::protocol::{ChatTool, ChatToolFunction};
use serde_json::json;

pub fn default_tools() -> Vec<ChatTool> {
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
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "search".to_string(),
                description: "Unified search for text content and files".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text query or file name pattern" },
                        "search_type": {
                            "type": "string",
                            "enum": ["text", "files", "both"],
                            "description": "Search mode (default: both)"
                        },
                        "include_pattern": { "type": "string", "description": "Optional include glob pattern" },
                        "exclude_pattern": { "type": "string", "description": "Optional exclude glob pattern" },
                        "case_sensitive": { "type": "boolean", "description": "Enable case sensitive text matching" },
                        "whole_word": { "type": "boolean", "description": "Match whole words only for text search" },
                        "regex": { "type": "boolean", "description": "Treat query as regex for text search" },
                        "max_results": { "type": "number", "description": "Maximum number of results" },
                        "file_types": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional file type filters (e.g. ['rs', 'ts'])"
                        },
                        "include_hidden": { "type": "boolean", "description": "Include hidden files" }
                    },
                    "required": ["query"]
                }),
            },
        },
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "create_todo_list".to_string(),
                description: "Create a new todo list for planning and tracking tasks".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "todos": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "content": { "type": "string" },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "completed"]
                                    },
                                    "priority": {
                                        "type": "string",
                                        "enum": ["high", "medium", "low"]
                                    }
                                },
                                "required": ["id", "content", "status", "priority"]
                            }
                        }
                    },
                    "required": ["todos"]
                }),
            },
        },
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: "update_todo_list".to_string(),
                description: "Update existing todo items".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "updates": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "completed"]
                                    },
                                    "content": { "type": "string" },
                                    "priority": {
                                        "type": "string",
                                        "enum": ["high", "medium", "low"]
                                    }
                                },
                                "required": ["id"]
                            }
                        }
                    },
                    "required": ["updates"]
                }),
            },
        },
    ]
}

pub fn confirmation_operation_for_tool(tool_name: &str) -> Option<ConfirmationOperation> {
    match tool_name {
        "create_file" | "str_replace_editor" => Some(ConfirmationOperation::File),
        "bash" => Some(ConfirmationOperation::Bash),
        _ => None,
    }
}

pub fn tool_display_name(name: &str) -> &'static str {
    match name {
        "view_file" => "Read",
        "str_replace_editor" => "Update",
        "create_file" => "Create",
        "bash" => "Bash",
        "search" => "Search",
        "create_todo_list" => "TodoCreate",
        "update_todo_list" => "TodoUpdate",
        _ => "Tool",
    }
}
