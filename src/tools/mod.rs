use crate::tool_catalog::{
    TOOL_BASH, TOOL_CREATE_FILE, TOOL_CREATE_TODO_LIST, TOOL_SEARCH, TOOL_STR_REPLACE_EDITOR,
    TOOL_UPDATE_TODO_LIST, TOOL_VIEW_FILE,
};
use anyhow::Result;
use serde_json::Value;

mod bash_tool;
mod file_ops;
mod search_tool;
mod todos;

use self::bash_tool::execute_bash_tool;
use self::file_ops::{execute_create_file, execute_str_replace_editor, execute_view_file};
use self::search_tool::execute_search;
use self::todos::{execute_create_todo_list, execute_update_todo_list};

pub use self::bash_tool::execute_bash_command;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
        }
    }

    pub fn content_for_model(&self) -> String {
        if self.success {
            self.output.clone().unwrap_or_else(|| "Success".to_string())
        } else {
            self.error.clone().unwrap_or_else(|| "Error".to_string())
        }
    }
}

pub fn tool_result_from_error(err: anyhow::Error) -> ToolResult {
    ToolResult::err(format!("{err:#}"))
}

pub async fn execute_tool(name: &str, args: &Value) -> ToolResult {
    let result: Result<ToolResult> = match name {
        TOOL_VIEW_FILE => execute_view_file(args),
        TOOL_CREATE_FILE => execute_create_file(args),
        TOOL_STR_REPLACE_EDITOR => execute_str_replace_editor(args),
        TOOL_BASH => execute_bash_tool(args).await,
        TOOL_SEARCH => execute_search(args).await,
        TOOL_CREATE_TODO_LIST => execute_create_todo_list(args),
        TOOL_UPDATE_TODO_LIST => execute_update_todo_list(args),
        _ => Ok(ToolResult::err(format!("Unknown tool: {name}"))),
    };

    match result {
        Ok(tool_result) => tool_result,
        Err(error) => tool_result_from_error(error),
    }
}
