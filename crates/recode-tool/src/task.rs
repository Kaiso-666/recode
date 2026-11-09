use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

/// Delegates to a child session. The agent list is injected at startup so
/// custom markdown agents appear in the schema instead of a hardcoded pair.
pub struct TaskTool {
    agents: Vec<String>,
    description: String,
}

impl Default for TaskTool {
    fn default() -> Self {
        Self::new(["general", "explore"])
    }
}

impl TaskTool {
    pub fn new(agents: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut agents: Vec<String> = agents.into_iter().map(Into::into).collect();
        if agents.is_empty() {
            agents = ["general".to_owned(), "explore".to_owned()].to_vec();
        }
        let description = format!(
            "Delegate a self-contained task to a subagent with its own context window. The \
             subagent can use the same tools you can (subject to permissions) and returns its \
             final answer. Use it to explore large areas, run independent investigations, or \
             parallelize work. Give it a complete, specific prompt: it cannot see this \
             conversation. Choose subagent_type from: {}. Defaults to the first one.",
            agents.join(", ")
        );
        Self {
            agents,
            description,
        }
    }
}

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
        &self.description
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
                    "enum": self.agents,
                    "description": "Which agent to run. Defaults to the first listed agent."
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
        let agent = input
            .subagent_type
            .unwrap_or_else(|| self.agents.first().cloned().unwrap_or_default());
        Ok(ToolOutcome {
            title: Some(format!("Subagent: {}", input.description)),
            output,
            metadata: Some(json!({"subagent": agent})),
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_the_configured_agents() {
        let tool = TaskTool::new(["general", "reviewer", "review/security"]);
        let schema = tool.schema();
        assert_eq!(
            schema.pointer("/properties/subagent_type/enum"),
            Some(&json!(["general", "reviewer", "review/security"]))
        );
        assert!(tool.description().contains("review/security"));
    }

    #[test]
    fn falls_back_to_the_default_agents() {
        let tool = TaskTool::new(Vec::<String>::new());
        assert_eq!(
            tool.schema().pointer("/properties/subagent_type/enum"),
            Some(&json!(["general", "explore"]))
        );
    }
}
