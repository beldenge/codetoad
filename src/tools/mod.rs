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

#[cfg(test)]
mod tests {
    use super::{ToolResult, ToolSessionState, execute_tool, tool_result_from_error};
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn content_for_model_prefers_output_for_success() {
        let result = ToolResult::ok("all good");
        assert_eq!(result.content_for_model(), "all good");
    }

    #[test]
    fn content_for_model_prefers_error_for_failure() {
        let result = ToolResult::err("failed");
        assert_eq!(result.content_for_model(), "failed");
    }

    #[test]
    fn tool_result_from_error_uses_display_chain() {
        let err = anyhow::anyhow!("top-level failure");
        let result = tool_result_from_error(err);
        assert!(!result.success);
        let message = result.error.expect("error should exist");
        assert!(message.contains("top-level failure"));
    }

    #[tokio::test]
    async fn execute_tool_returns_unknown_tool_error() {
        let temp = TempDir::new("tools-unknown");
        let mut session = ToolSessionState::new(temp.path().to_path_buf()).expect("session");

        let result = execute_tool("not_a_real_tool", &json!({}), &mut session).await;
        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some("Unknown tool: not_a_real_tool")
        );
    }

    #[test]
    fn session_snapshot_round_trip_preserves_relative_cwd_and_todos() {
        let temp = TempDir::new("tools-session-snapshot");
        fs::create_dir_all(temp.path().join("nested")).expect("create nested dir");

        let mut session = ToolSessionState::new(temp.path().to_path_buf()).expect("session");
        session
            .tool_context
            .set_current_dir("nested")
            .expect("set relative cwd");
        session
            .todo_store
            .restore_value(
                json!([{ "id": "1", "content": "task", "status": "pending", "priority": "medium" }]),
            )
            .expect("seed todos");

        let snapshot = session.snapshot().expect("snapshot");

        let mut restored = ToolSessionState::new(temp.path().to_path_buf()).expect("restored");
        restored.restore(snapshot).expect("restore");

        assert_eq!(restored.tool_context.relative_current_dir(), "nested");
        let todos = restored.todo_store.snapshot_value().expect("todos");
        assert_eq!(
            todos,
            json!([{ "id": "1", "content": "task", "status": "pending", "priority": "medium" }])
        );
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos();
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("grok-build-{prefix}-{pid}-{nonce}"));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
