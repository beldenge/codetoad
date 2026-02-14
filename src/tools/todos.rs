use super::ToolResult;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TodoItem {
    id: String,
    content: String,
    status: String,
    priority: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoUpdate {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TodoStore {
    items: Vec<TodoItem>,
}

impl TodoStore {
    pub(crate) fn snapshot_value(&self) -> Result<Value> {
        serde_json::to_value(&self.items).context("Failed serializing todo store")
    }

    pub(crate) fn restore_value(&mut self, value: Value) -> Result<()> {
        let restored: Vec<TodoItem> =
            serde_json::from_value(value).context("Invalid todo store snapshot")?;
        for todo in &restored {
            if !is_valid_todo_item(todo) {
                anyhow::bail!(
                    "Todo snapshot contains invalid item. Expected id/content/status/priority values."
                );
            }
        }
        self.items = restored;
        Ok(())
    }
}

pub(super) fn execute_create_todo_list(args: &Value, store: &mut TodoStore) -> Result<ToolResult> {
    let todos_value = args
        .get("todos")
        .cloned()
        .context("Missing 'todos' argument")?;
    let todos: Vec<TodoItem> =
        serde_json::from_value(todos_value).context("Invalid 'todos' argument")?;

    for todo in &todos {
        if !is_valid_todo_item(todo) {
            return Ok(ToolResult::err(
                "Each todo must include id, content, status(pending|in_progress|completed), and priority(high|medium|low)",
            ));
        }
    }

    store.items = todos;
    Ok(ToolResult::ok(format_todo_list(&store.items)))
}

pub(super) fn execute_update_todo_list(args: &Value, store: &mut TodoStore) -> Result<ToolResult> {
    let updates_value = args
        .get("updates")
        .cloned()
        .context("Missing 'updates' argument")?;
    let updates: Vec<TodoUpdate> =
        serde_json::from_value(updates_value).context("Invalid 'updates' argument")?;

    for update in updates {
        let Some(todo) = store.items.iter_mut().find(|item| item.id == update.id) else {
            return Ok(ToolResult::err(format!(
                "Todo with id '{}' not found",
                update.id
            )));
        };

        if let Some(status) = update.status {
            if !is_valid_status(&status) {
                return Ok(ToolResult::err(format!(
                    "Invalid status: {status}. Must be pending, in_progress, or completed"
                )));
            }
            todo.status = status;
        }
        if let Some(content) = update.content {
            todo.content = content;
        }
        if let Some(priority) = update.priority {
            if !is_valid_priority(&priority) {
                return Ok(ToolResult::err(format!(
                    "Invalid priority: {priority}. Must be high, medium, or low"
                )));
            }
            todo.priority = priority;
        }
    }

    Ok(ToolResult::ok(format_todo_list(&store.items)))
}

fn is_valid_todo_item(item: &TodoItem) -> bool {
    !item.id.trim().is_empty()
        && !item.content.trim().is_empty()
        && is_valid_status(&item.status)
        && is_valid_priority(&item.priority)
}

fn is_valid_status(status: &str) -> bool {
    matches!(status, "pending" | "in_progress" | "completed")
}

fn is_valid_priority(priority: &str) -> bool {
    matches!(priority, "high" | "medium" | "low")
}

fn format_todo_list(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "No todos created yet".to_string();
    }

    let mut output = String::new();
    for (index, todo) in todos.iter().enumerate() {
        let marker = match todo.status.as_str() {
            "completed" => "●",
            "in_progress" => "◐",
            _ => "○",
        };
        let indent = if index == 0 { "" } else { "  " };
        output.push_str(&format!("{indent}{marker} {}\n", todo.content));
    }
    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::{TodoStore, execute_create_todo_list, execute_update_todo_list};
    use serde_json::json;

    #[test]
    fn create_todo_list_rejects_invalid_status_or_priority() {
        let mut store = TodoStore::default();
        let result = execute_create_todo_list(
            &json!({
                "todos": [
                    {
                        "id": "1",
                        "content": "task",
                        "status": "queued",
                        "priority": "medium"
                    }
                ]
            }),
            &mut store,
        )
        .expect("tool call should parse");

        assert!(!result.success);
        let message = result.error.expect("error should exist");
        assert!(message.contains("Each todo must include"));
    }

    #[test]
    fn create_todo_list_accepts_valid_items_and_formats_output() {
        let mut store = TodoStore::default();
        let result = execute_create_todo_list(
            &json!({
                "todos": [
                    {
                        "id": "1",
                        "content": "plan release",
                        "status": "pending",
                        "priority": "high"
                    },
                    {
                        "id": "2",
                        "content": "run tests",
                        "status": "completed",
                        "priority": "medium"
                    }
                ]
            }),
            &mut store,
        )
        .expect("tool call should parse");

        assert!(result.success);
        let output = result.output.expect("output should exist");
        assert!(output.contains("○ plan release"));
        assert!(output.contains("● run tests"));
    }

    #[test]
    fn update_todo_list_updates_existing_item() {
        let mut store = TodoStore::default();
        execute_create_todo_list(
            &json!({
                "todos": [
                    {
                        "id": "1",
                        "content": "task",
                        "status": "pending",
                        "priority": "low"
                    }
                ]
            }),
            &mut store,
        )
        .expect("seed todo");

        let result = execute_update_todo_list(
            &json!({
                "updates": [
                    {
                        "id": "1",
                        "status": "in_progress",
                        "priority": "high"
                    }
                ]
            }),
            &mut store,
        )
        .expect("update should parse");

        assert!(result.success);
        let output = result.output.expect("output should exist");
        assert!(output.contains("◐ task"));
    }

    #[test]
    fn update_todo_list_reports_missing_todo_id() {
        let mut store = TodoStore::default();
        let result = execute_update_todo_list(
            &json!({
                "updates": [
                    {
                        "id": "missing",
                        "status": "completed"
                    }
                ]
            }),
            &mut store,
        )
        .expect("update should parse");

        assert!(!result.success);
        assert_eq!(
            result.error.as_deref(),
            Some("Todo with id 'missing' not found")
        );
    }

    #[test]
    fn restore_snapshot_rejects_invalid_item_shape() {
        let mut store = TodoStore::default();
        let err = store
            .restore_value(json!([
                {
                    "id": "1",
                    "content": "task",
                    "status": "pending",
                    "priority": "urgent"
                }
            ]))
            .expect_err("invalid priority should fail");
        assert!(err.to_string().contains("invalid item"));
    }
}
