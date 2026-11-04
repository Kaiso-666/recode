use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct ReadTool;

#[derive(Deserialize)]
struct Input {
    file_path: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_LINES: u64 = 2000;
const MAX_LINE_CHARS: usize = 2000;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a text file from the project. Returns line-numbered content. Use offset \
         (1-based line) and limit to page through large files. Binary files are rejected; \
         use bash for those. Always read a file before editing it."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file, relative to the project root or absolute."
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line number to start at.",
                    "minimum": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return (default 2000).",
                    "minimum": 1
                }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    fn default_decision(&self) -> Decision {
        Decision::Allow
    }

    fn title(&self, input: &Value) -> String {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| format!("Read {path}"))
            .unwrap_or_else(|| "Read file".to_owned())
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| vec![path.to_owned()])
            .unwrap_or_default()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid read arguments: {error}")))?;
        let path = context.resolve_path(&input.file_path)?;
        let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
            ToolFailure::new(format!("cannot read {}: {error}", path.display()))
        })?;
        if metadata.is_dir() {
            return Err(ToolFailure::new(format!(
                "{} is a directory; use the ls tool or read a file inside it",
                path.display()
            )));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(ToolFailure::new(format!(
                "{} is too large to read ({} bytes, limit {} bytes); use bash with head/tail or a range",
                path.display(),
                metadata.len(),
                MAX_FILE_BYTES
            )));
        }
        let bytes = tokio::fs::read(&path).await.map_err(|error| {
            ToolFailure::new(format!("cannot read {}: {error}", path.display()))
        })?;
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return Err(ToolFailure::new(format!(
                "{} appears to be a binary file",
                path.display()
            )));
        }
        let text = String::from_utf8_lossy(&bytes);
        let total_lines = text.lines().count();
        let offset = input.offset.unwrap_or(1).max(1) as usize;
        let limit = input.limit.unwrap_or(DEFAULT_LINES).max(1) as usize;
        let start = offset - 1;
        if start >= total_lines && total_lines > 0 {
            return Err(ToolFailure::new(format!(
                "offset {offset} is past the end of the file ({total_lines} lines)"
            )));
        }
        let selected: Vec<&str> = text.lines().skip(start).take(limit).collect();
        let mut output = String::new();
        for (index, line) in selected.iter().enumerate() {
            let number = offset + index;
            let line = if line.chars().count() > MAX_LINE_CHARS {
                let mut truncated: String = line.chars().take(MAX_LINE_CHARS).collect();
                truncated.push('…');
                truncated
            } else {
                (*line).to_owned()
            };
            output.push_str(&format!("{number:>6}| {line}\n"));
        }
        let shown_end = offset + selected.len().saturating_sub(1);
        if offset > 1 || shown_end < total_lines {
            output.push_str(&format!(
                "\n[lines {offset}-{shown_end} of {total_lines}]\n"
            ));
        }
        Ok(ToolOutcome {
            title: Some(format!(
                "Read {} ({total_lines} lines)",
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string())
            )),
            output,
            metadata: Some(json!({
                "path": path,
                "lines": total_lines,
                "offset": offset,
                "limit": limit
            })),
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
    async fn numbers_lines_and_pages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let registry = ToolRegistry::builtin();
        let context = ToolContext::new(
            SessionId::new(),
            MessageId::new(),
            "call",
            dir.path().to_path_buf(),
        );
        let outcome = registry
            .execute(
                "read",
                json!({"file_path": "a.txt", "offset": 2, "limit": 1}),
                &context,
            )
            .await
            .unwrap();
        assert!(outcome.output.contains("     2| two"));
        assert!(!outcome.output.contains("one"));
    }

    #[tokio::test]
    async fn rejects_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), [0u8, 1, 2, 3]).unwrap();
        let registry = ToolRegistry::builtin();
        let context = ToolContext::new(
            SessionId::new(),
            MessageId::new(),
            "call",
            dir.path().to_path_buf(),
        );
        let error = registry
            .execute("read", json!({"file_path": "a.bin"}), &context)
            .await
            .unwrap_err();
        assert!(error.message.contains("binary"));
    }
}
