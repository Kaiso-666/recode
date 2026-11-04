use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct LsTool;

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    path: Option<String>,
}

const MAX_ENTRIES: usize = 500;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List a directory. Directories are shown with a trailing slash. Sorted by name, \
         directories first. Use glob for recursive file discovery."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory to list; defaults to the project root."}
            },
            "additionalProperties": false
        })
    }

    fn default_decision(&self) -> Decision {
        Decision::Allow
    }

    fn title(&self, input: &Value) -> String {
        format!(
            "List {}",
            input.get("path").and_then(Value::as_str).unwrap_or(".")
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid ls arguments: {error}")))?;
        let directory = match input.path.as_deref() {
            Some(path) => context.resolve_path(path)?,
            None => context.directory.clone(),
        };
        let mut entries = tokio::fs::read_dir(&directory).await.map_err(|error| {
            ToolFailure::new(format!("cannot list {}: {error}", directory.display()))
        })?;
        let mut rows = Vec::new();
        let mut truncated = false;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| ToolFailure::new(format!("cannot read directory entry: {error}")))?
        {
            if rows.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let metadata = entry.metadata().await.ok();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = metadata.as_ref().map(|meta| meta.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
            rows.push((is_dir, name, size));
        }
        rows.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        let mut output = format!("{}\n", directory.display());
        for (is_dir, name, size) in rows {
            if is_dir {
                output.push_str(&format!("{name}/\n"));
            } else {
                output.push_str(&format!("{name}  ({size} bytes)\n"));
            }
        }
        if truncated {
            output.push_str(&format!("… [truncated at {MAX_ENTRIES} entries]\n"));
        }
        Ok(ToolOutcome {
            title: Some(format!("List {}", directory.display())),
            output,
            metadata: Some(json!({"path": directory, "truncated": truncated})),
            attachments: Vec::new(),
        })
    }
}
