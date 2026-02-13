use crate::tool_catalog::{
    TOOL_BASH, TOOL_CREATE_FILE, TOOL_CREATE_TODO_LIST, TOOL_SEARCH, TOOL_STR_REPLACE_EDITOR,
    TOOL_UPDATE_TODO_LIST, TOOL_VIEW_FILE,
};
use crate::tool_context::ToolContext;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

mod bash_tool;
mod file_ops;
mod search_tool;
mod todos;

use self::bash_tool::execute_bash_tool;
use self::file_ops::{execute_create_file, execute_str_replace_editor, execute_view_file};
use self::search_tool::execute_search;
use self::todos::{TodoStore, execute_create_todo_list, execute_update_todo_list};

pub(crate) struct ToolSessionState {
    tool_context: ToolContext,
    todo_store: TodoStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolSessionSnapshot {
    current_dir: String,
    todos: Value,
}

impl ToolSessionState {
    pub(crate) fn new(project_root: PathBuf) -> Result<Self> {
        Ok(Self {
            tool_context: ToolContext::new(project_root)?,
            todo_store: TodoStore::default(),
        })
    }

    pub(crate) fn snapshot(&self) -> Result<ToolSessionSnapshot> {
        Ok(ToolSessionSnapshot {
            current_dir: self.tool_context.relative_current_dir(),
            todos: self.todo_store.snapshot_value()?,
        })
    }

    pub(crate) fn restore(&mut self, snapshot: ToolSessionSnapshot) -> Result<()> {
        self.tool_context
            .restore_relative_current_dir(&snapshot.current_dir)
            .context("Failed restoring working directory state")?;
        self.todo_store
            .restore_value(snapshot.todos)
            .context("Failed restoring todo state")?;
        Ok(())
    }
}

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

pub(crate) async fn execute_tool(
    name: &str,
    args: &Value,
    session: &mut ToolSessionState,
) -> ToolResult {
    let result: Result<ToolResult> = match name {
        TOOL_VIEW_FILE => execute_view_file(args, &session.tool_context),
        TOOL_CREATE_FILE => execute_create_file(args, &session.tool_context),
        TOOL_STR_REPLACE_EDITOR => execute_str_replace_editor(args, &session.tool_context),
        TOOL_BASH => execute_bash_tool(args, &mut session.tool_context).await,
        TOOL_SEARCH => execute_search(args, &session.tool_context).await,
        TOOL_CREATE_TODO_LIST => execute_create_todo_list(args, &mut session.todo_store),
        TOOL_UPDATE_TODO_LIST => execute_update_todo_list(args, &mut session.todo_store),
        _ => Ok(ToolResult::err(format!("Unknown tool: {name}"))),
    };

    match result {
        Ok(tool_result) => tool_result,
        Err(error) => tool_result_from_error(error),
    }
}

pub(crate) async fn execute_bash_command(
    command: &str,
    session: &mut ToolSessionState,
) -> Result<ToolResult> {
    bash_tool::execute_bash_command(command, &mut session.tool_context).await
}
