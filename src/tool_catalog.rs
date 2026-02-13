use crate::confirmation::ConfirmationOperation;
use crate::protocol::{ChatTool, ChatToolFunction};
use serde_json::json;

pub const TOOL_VIEW_FILE: &str = "view_file";
pub const TOOL_CREATE_FILE: &str = "create_file";
pub const TOOL_STR_REPLACE_EDITOR: &str = "str_replace_editor";
pub const TOOL_BASH: &str = "bash";
pub const TOOL_SEARCH: &str = "search";
pub const TOOL_CREATE_TODO_LIST: &str = "create_todo_list";
pub const TOOL_UPDATE_TODO_LIST: &str = "update_todo_list";

pub fn default_tools() -> Vec<ChatTool> {
    vec![
        ChatTool {
            r#type: "function".to_string(),
            function: ChatToolFunction {
                name: TOOL_VIEW_FILE.to_string(),
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
                name: TOOL_CREATE_FILE.to_string(),
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
                name: TOOL_STR_REPLACE_EDITOR.to_string(),
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
                name: TOOL_BASH.to_string(),
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
                name: TOOL_SEARCH.to_string(),
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
                name: TOOL_CREATE_TODO_LIST.to_string(),
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
                name: TOOL_UPDATE_TODO_LIST.to_string(),
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
        TOOL_CREATE_FILE | TOOL_STR_REPLACE_EDITOR => Some(ConfirmationOperation::File),
        TOOL_BASH => Some(ConfirmationOperation::Bash),
        _ => None,
    }
}

pub fn tool_display_name(name: &str) -> &'static str {
    match name {
        TOOL_VIEW_FILE => "Read",
        TOOL_STR_REPLACE_EDITOR => "Update",
        TOOL_CREATE_FILE => "Create",
        TOOL_BASH => "Bash",
        TOOL_SEARCH => "Search",
        TOOL_CREATE_TODO_LIST => "TodoCreate",
        TOOL_UPDATE_TODO_LIST => "TodoUpdate",
        _ => "Tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn default_tools_have_unique_names() {
        let tools = default_tools();
        let mut seen = HashSet::new();
        for tool in tools {
            assert!(
                seen.insert(tool.function.name.clone()),
                "duplicate tool name: {}",
                tool.function.name
            );
        }
    }

    #[test]
    fn confirmation_mapping_matches_expected_tools() {
        assert_eq!(
            confirmation_operation_for_tool(TOOL_CREATE_FILE),
            Some(ConfirmationOperation::File)
        );
        assert_eq!(
            confirmation_operation_for_tool(TOOL_STR_REPLACE_EDITOR),
            Some(ConfirmationOperation::File)
        );
        assert_eq!(
            confirmation_operation_for_tool(TOOL_BASH),
            Some(ConfirmationOperation::Bash)
        );
        assert_eq!(confirmation_operation_for_tool(TOOL_VIEW_FILE), None);
    }

    #[test]
    fn display_names_defined_for_core_tools() {
        assert_eq!(tool_display_name(TOOL_VIEW_FILE), "Read");
        assert_eq!(tool_display_name(TOOL_CREATE_FILE), "Create");
        assert_eq!(tool_display_name(TOOL_STR_REPLACE_EDITOR), "Update");
        assert_eq!(tool_display_name(TOOL_BASH), "Bash");
        assert_eq!(tool_display_name(TOOL_SEARCH), "Search");
        assert_eq!(tool_display_name(TOOL_CREATE_TODO_LIST), "TodoCreate");
        assert_eq!(tool_display_name(TOOL_UPDATE_TODO_LIST), "TodoUpdate");
    }
}
