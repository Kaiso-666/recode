use crate::{Tool, ToolContext};
use async_trait::async_trait;
use parking_lot::Mutex;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Session-scoped todo list. Todos are a planning aid for the model and are
/// intentionally volatile: they live for the process, not the transcript.
/// The TUI can query [`todos_for`] to render a checklist.
pub struct TodoTool;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    #[serde(default)]
    pub status: TodoStatus,
    #[serde(default)]
    pub priority: TodoPriority,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoPriority {
    #[default]
    Medium,
    High,
    Low,
}

fn store() -> &'static Mutex<HashMap<String, Vec<TodoItem>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Vec<TodoItem>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Current todos for a session, for UI rendering.
pub fn todos_for(session_id: &recode_schema::SessionId) -> Vec<TodoItem> {
    store()
        .lock()
        .get(session_id.as_str())
        .cloned()
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct Input {
    todos: Vec<TodoItem>,
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todowrite"
    }

    fn description(&self) -> &str {
        "Replace the session todo list. Use it for multi-step tasks: keep exactly one item \
         in_progress while working, mark items completed as you finish them, and rewrite the \
         list when the plan changes. This does not modify any files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete todo list, in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "Imperative description of the task."},
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Current state of the task."
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["high", "medium", "low"],
                                "description": "Optional ordering hint."
                            }
                        },
                        "required": ["content", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        })
    }

    fn default_decision(&self) -> Decision {
        Decision::Allow
    }

    fn title(&self, input: &Value) -> String {
        let count = input
            .get("todos")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        format!("Update todos ({count})")
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid todowrite arguments: {error}")))?;
        if input.todos.len() > 100 {
            return Err(ToolFailure::new("todo list is limited to 100 items"));
        }
        store()
            .lock()
            .insert(context.session_id.as_str().to_owned(), input.todos.clone());
        let mut output = String::new();
        for todo in &input.todos {
            let marker = match todo.status {
                TodoStatus::Completed => "x",
                TodoStatus::InProgress => "~",
                TodoStatus::Cancelled => "-",
                TodoStatus::Pending => " ",
            };
            let priority = match todo.priority {
                TodoPriority::High => "!",
                TodoPriority::Medium => "",
                TodoPriority::Low => "·",
            };
            output.push_str(&format!("[{marker}] {priority}{}\n", todo.content));
        }
        if output.is_empty() {
            output.push_str("(todo list cleared)\n");
        }
        Ok(ToolOutcome {
            title: Some(format!("{} todos", input.todos.len())),
            output,
            metadata: Some(json!({"count": input.todos.len()})),
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolRegistry;
    use recode_schema::id::{MessageId, SessionId};

    #[tokio::test]
    async fn stores_todos_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let context = ToolContext::new(
            session.clone(),
            MessageId::new(),
            "call",
            dir.path().to_path_buf(),
        );
        let registry = ToolRegistry::builtin();
        registry
            .execute(
                "todowrite",
                json!({"todos": [
                    {"content": "inspect", "status": "completed"},
                    {"content": "write", "status": "in_progress", "priority": "high"}
                ]}),
                &context,
            )
            .await
            .unwrap();
        let todos = todos_for(&session);
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[1].status, TodoStatus::InProgress);
        assert_eq!(todos[1].priority, TodoPriority::High);
    }
}
