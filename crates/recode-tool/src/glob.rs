use crate::{Tool, ToolContext};
use async_trait::async_trait;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct GlobTool;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

const DEFAULT_LIMIT: usize = 200;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern (for example `**/*.rs`, `src/**/*.ts`, `*.json`). \
         Respects .gitignore and skips .git. Results are sorted by modification time, \
         newest first. Use grep to search file contents."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Glob pattern, relative to the search root."},
                "path": {"type": "string", "description": "Directory to search; defaults to the project root."},
                "limit": {"type": "integer", "description": "Maximum results (default 200).", "minimum": 1}
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn default_decision(&self) -> Decision {
        Decision::Allow
    }

    fn title(&self, input: &Value) -> String {
        format!(
            "Glob {}",
            input.get("pattern").and_then(Value::as_str).unwrap_or("*")
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid glob arguments: {error}")))?;
        let root = match input.path.as_deref() {
            Some(path) => context.resolve_path(path)?,
            None => context.directory.clone(),
        };
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 2000);
        let matcher = build_matcher(&input.pattern)?;
        let root_for_task = root.clone();
        let files =
            tokio::task::spawn_blocking(move || walk_matches(&root_for_task, matcher, limit))
                .await
                .map_err(|error| ToolFailure::new(format!("glob task failed: {error}")))??;
        let mut output = String::new();
        for file in &files {
            output.push_str(&file.display().to_string());
            output.push('\n');
        }
        if files.is_empty() {
            output.push_str("No files matched.\n");
        }
        Ok(ToolOutcome {
            title: Some(format!("Glob {} ({} files)", input.pattern, files.len())),
            output,
            metadata: Some(json!({"pattern": input.pattern, "count": files.len()})),
            attachments: Vec::new(),
        })
    }
}

fn build_matcher(pattern: &str) -> Result<GlobSet, ToolFailure> {
    let pattern = pattern.replace('\\', "/");
    let glob: Glob = GlobBuilder::new(&pattern)
        .literal_separator(false)
        .backslash_escape(false)
        .build()
        .map_err(|error| ToolFailure::new(format!("invalid glob pattern {pattern:?}: {error}")))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder
        .build()
        .map_err(|error| ToolFailure::new(format!("invalid glob pattern: {error}")))
}

fn walk_matches(
    root: &std::path::Path,
    matcher: GlobSet,
    limit: usize,
) -> Result<Vec<PathBuf>, ToolFailure> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(path);
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !matcher.is_match(&relative) && !matcher.is_match(path.file_name().unwrap_or_default()) {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        files.push((modified, path.to_path_buf()));
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.truncate(limit);
    Ok(files.into_iter().map(|(_, path)| path).collect())
}
