use crate::{Tool, ToolContext};
use async_trait::async_trait;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use similar::TextDiff;

pub struct WriteTool;

#[derive(Deserialize)]
struct WriteInput {
    file_path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write a complete file, creating parent directories as needed. This overwrites \
         the existing file; use edit for targeted changes. Prefer editing over rewriting."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Path to write."},
                "content": {"type": "string", "description": "Full file content."}
            },
            "required": ["file_path", "content"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "edit"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn title(&self, input: &Value) -> String {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| format!("Write {path}"))
            .unwrap_or_else(|| "Write file".to_owned())
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| vec![path.to_owned()])
            .unwrap_or_default()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: WriteInput = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid write arguments: {error}")))?;
        let path = context.resolve_path(&input.file_path)?;
        let previous = tokio::fs::read_to_string(&path).await.ok();
        write_atomic(&path, &input.content).await?;
        let diff = previous
            .as_deref()
            .map(|previous| diff_text(previous, &input.content))
            .filter(|diff| !diff.is_empty());
        let mut output = format!(
            "Wrote {} bytes to {}\n",
            input.content.len(),
            display_path(&path)
        );
        if let Some(diff) = diff {
            output.push('\n');
            output.push_str(&crate::truncate_middle(&diff, 20_000));
        }
        Ok(ToolOutcome {
            title: Some(format!("Write {}", display_path(&path))),
            output,
            metadata: Some(json!({"path": path, "bytes": input.content.len()})),
            attachments: Vec::new(),
        })
    }
}

pub struct EditTool;

#[derive(Deserialize)]
struct EditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file. old_string must match exactly once unless \
         replace_all is true. Include enough surrounding context to make the match \
         unique. Read the file first. Line endings are handled automatically."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "File to edit."},
                "old_string": {"type": "string", "description": "Exact text to replace."},
                "new_string": {"type": "string", "description": "Replacement text."},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence. Default false."}
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "edit"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn title(&self, input: &Value) -> String {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| format!("Edit {path}"))
            .unwrap_or_else(|| "Edit file".to_owned())
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| vec![path.to_owned()])
            .unwrap_or_default()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: EditInput = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid edit arguments: {error}")))?;
        let path = context.resolve_path(&input.file_path)?;
        let previous = tokio::fs::read_to_string(&path).await.map_err(|error| {
            ToolFailure::new(format!("cannot read {}: {error}", path.display()))
        })?;
        let updated = apply_edit(
            &previous,
            &input.old_string,
            &input.new_string,
            input.replace_all,
        )?;
        write_atomic(&path, &updated).await?;
        let diff = diff_text(&previous, &updated);
        Ok(ToolOutcome {
            title: Some(format!("Edit {}", display_path(&path))),
            output: format!(
                "Edited {}\n\n{}",
                display_path(&path),
                crate::truncate_middle(&diff, 20_000)
            ),
            metadata: Some(json!({"path": path})),
            attachments: Vec::new(),
        })
    }
}

pub struct MultiEditTool;

#[derive(Deserialize)]
struct MultiEditInput {
    file_path: String,
    edits: Vec<EditOperation>,
}

#[derive(Deserialize)]
struct EditOperation {
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for MultiEditTool {
    fn name(&self) -> &str {
        "multi_edit"
    }

    fn description(&self) -> &str {
        "Apply several exact-string edits to one file atomically. Edits run in order and \
         all must apply; if any fails nothing is written. Cheaper than several edit calls."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {"type": "string"},
                            "new_string": {"type": "string"},
                            "replace_all": {"type": "boolean"}
                        },
                        "required": ["old_string", "new_string"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["file_path", "edits"],
            "additionalProperties": false
        })
    }

    fn permission(&self) -> &str {
        "edit"
    }

    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(|path| vec![path.to_owned()])
            .unwrap_or_default()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: MultiEditInput = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid multi_edit arguments: {error}")))?;
        if input.edits.is_empty() {
            return Err(ToolFailure::new("edits must not be empty"));
        }
        let path = context.resolve_path(&input.file_path)?;
        let previous = tokio::fs::read_to_string(&path).await.map_err(|error| {
            ToolFailure::new(format!("cannot read {}: {error}", path.display()))
        })?;
        let mut updated = previous.clone();
        for (index, edit) in input.edits.iter().enumerate() {
            updated = apply_edit(
                &updated,
                &edit.old_string,
                &edit.new_string,
                edit.replace_all,
            )
            .map_err(|error| {
                ToolFailure::new(format!("edit #{} failed: {}", index + 1, error.message))
            })?;
        }
        write_atomic(&path, &updated).await?;
        let diff = diff_text(&previous, &updated);
        Ok(ToolOutcome {
            title: Some(format!(
                "Edit {} ({} edits)",
                display_path(&path),
                input.edits.len()
            )),
            output: format!(
                "Applied {} edits to {}\n\n{}",
                input.edits.len(),
                display_path(&path),
                crate::truncate_middle(&diff, 20_000)
            ),
            metadata: Some(json!({"path": path, "edits": input.edits.len()})),
            attachments: Vec::new(),
        })
    }
}

fn apply_edit(
    source: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, ToolFailure> {
    if old_string.is_empty() {
        return Err(ToolFailure::new("old_string must not be empty"));
    }
    if old_string == new_string {
        return Err(ToolFailure::new("old_string and new_string are identical"));
    }
    let mut haystack = source.to_owned();
    let mut needle = old_string.to_owned();
    // Adapt to CRLF files when the model supplied LF-only text.
    if !haystack.contains(&needle) && haystack.contains("\r\n") && !needle.contains('\r') {
        let crlf = needle.replace('\n', "\r\n");
        if haystack.contains(&crlf) {
            needle = crlf;
        }
    }
    if !haystack.contains(&needle) && !haystack.contains('\r') && needle.contains("\r\n") {
        let lf = needle.replace("\r\n", "\n");
        if haystack.contains(&lf) {
            needle = lf;
        }
    }
    let count = haystack.matches(&needle).count();
    if count == 0 {
        return Err(ToolFailure::new(format!(
            "old_string was not found in the file. Read the file and copy the exact text, including indentation. \
             Searched for: {:?}",
            summarize(old_string)
        )));
    }
    if count > 1 && !replace_all {
        return Err(ToolFailure::new(format!(
            "old_string matches {count} locations. Add more surrounding context to make it unique, \
             or set replace_all to true."
        )));
    }
    let replacement = new_string;
    if replace_all {
        haystack = haystack.replace(&needle, replacement);
    } else {
        haystack = haystack.replacen(&needle, replacement, 1);
    }
    Ok(haystack)
}

fn summarize(value: &str) -> String {
    crate::truncate_middle(value, 200)
}

pub fn diff_text(previous: &str, updated: &str) -> String {
    TextDiff::from_lines(previous, updated)
        .unified_diff()
        .context_radius(3)
        .header("before", "after")
        .to_string()
}

async fn write_atomic(path: &std::path::Path, content: &str) -> Result<(), ToolFailure> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ToolFailure::new(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_owned());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = path.with_file_name(format!(".{file_name}.{stamp}.recode-tmp"));
    tokio::fs::write(&temp, content)
        .await
        .map_err(|error| ToolFailure::new(format!("cannot write {}: {error}", temp.display())))?;
    match tokio::fs::rename(&temp, path).await {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows cannot atomically replace an existing file; fall back.
            let _ = tokio::fs::remove_file(path).await;
            tokio::fs::rename(&temp, path).await.map_err(|error| {
                ToolFailure::new(format!("cannot replace {}: {error}", path.display()))
            })
        }
    }
}

fn display_path(path: &std::path::Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_requires_unique_match() {
        let source = "a\nb\nb\n";
        let error = apply_edit(source, "b", "c", false).unwrap_err();
        assert!(error.message.contains("matches 2"));
        let updated = apply_edit(source, "b", "c", true).unwrap();
        assert_eq!(updated, "a\nc\nc\n");
    }

    #[test]
    fn edit_adapts_to_crlf() {
        let source = "one\r\ntwo\r\n";
        let updated = apply_edit(source, "one\ntwo", "three", false).unwrap();
        assert_eq!(updated, "three\r\n");
    }

    #[tokio::test]
    async fn write_then_edit_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let registry = crate::ToolRegistry::builtin();
        let context = ToolContext::new(
            recode_schema::id::SessionId::new(),
            recode_schema::id::MessageId::new(),
            "call",
            dir.path().to_path_buf(),
        );
        registry
            .execute(
                "write",
                json!({"file_path": "a.txt", "content": "hello\n"}),
                &context,
            )
            .await
            .unwrap();
        registry
            .execute(
                "edit",
                json!({"file_path": "a.txt", "old_string": "hello", "new_string": "goodbye"}),
                &context,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "goodbye\n"
        );
    }
}
