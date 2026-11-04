use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct TaskTool;

#[derive(Deserialize)]
struct Input {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Delegate a self-contained task to a subagent with its own context window. The \
         subagent can use the same tools you can (subject to permissions) and returns its \
         final answer. Use it to explore large areas, run independent investigations, or \
         parallelize work. Give it a complete, specific prompt: it cannot see this \
         conversation. Choose subagent_type `explore` for read-only search, `general` for \
         work that may edit files."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short (3-5 words) description shown to the user."
                },
                "prompt": {
                    "type": "string",
                    "description": "Complete instructions for the subagent, including all context it needs."
                },
                "subagent_type": {
                    "type": "string",
                    "enum": ["general", "explore"],
                    "description": "Which agent to run. Defaults to general."
                }
            },
            "required": ["description", "prompt"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "task"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn title(&self, input: &Value) -> String {
        input
            .get("description")
            .and_then(Value::as_str)
            .map(|description| format!("Subagent: {description}"))
            .unwrap_or_else(|| "Subagent".to_owned())
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        input
            .get("description")
            .and_then(Value::as_str)
            .map(|description| vec![description.to_owned()])
            .unwrap_or_default()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid task arguments: {error}")))?;
        if input.prompt.trim().is_empty() {
            return Err(ToolFailure::new("prompt must not be empty"));
        }
        let runner = context
            .subagent
            .as_ref()
            .ok_or_else(|| ToolFailure::new("subagents are not available in this session"))?;
        let output = runner
            .run(
                input.description.clone(),
                input.prompt,
                input.subagent_type.clone(),
            )
            .await?;
        Ok(ToolOutcome {
            title: Some(format!("Subagent: {}", input.description)),
            output,
            metadata: Some(
                json!({"subagent": input.subagent_type.unwrap_or_else(|| "general".into())}),
            ),
            attachments: Vec::new(),
        })
    }
}
