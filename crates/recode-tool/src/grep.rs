use crate::{Tool, ToolContext};
use async_trait::async_trait;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolOutcome, ToolResult};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

pub struct GrepTool;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    case_sensitive: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
}

const DEFAULT_LIMIT: usize = 100;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a regular expression. Respects .gitignore and skips \
         .git and binary files. `include` filters filenames with a glob (for example \
         `*.rs`). Returns path:line: text, up to the result limit."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Rust regex pattern."},
                "path": {"type": "string", "description": "Directory or file to search."},
                "include": {"type": "string", "description": "Optional glob filter for filenames."},
                "case_sensitive": {"type": "boolean", "description": "Default false."},
                "limit": {"type": "integer", "description": "Maximum matches (default 100).", "minimum": 1}
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
            "Grep {}",
            crate::truncate_middle(
                input.get("pattern").and_then(Value::as_str).unwrap_or(""),
                60
            )
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolFailure::new(format!("invalid grep arguments: {error}")))?;
        let root = match input.path.as_deref() {
            Some(path) => context.resolve_path(path)?,
            None => context.directory.clone(),
        };
        let expression = if input.case_sensitive.unwrap_or(false) {
            Regex::new(&input.pattern)
        } else {
            Regex::new(&format!("(?i){}", input.pattern))
        }
        .map_err(|error| ToolFailure::new(format!("invalid regex: {error}")))?;
        let include = match input.include.as_deref() {
            Some(pattern) => Some(build_include(pattern)?),
            None => None,
        };
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 2000);
        let root_for_task = root.clone();
        let matches = tokio::task::spawn_blocking(move || {
            search(&root_for_task, &expression, include.as_ref(), limit)
        })
        .await
        .map_err(|error| ToolFailure::new(format!("grep task failed: {error}")))??;
        let mut output = String::new();
        for (path, line, text) in &matches {
            output.push_str(&format!("{path}:{line}: {text}\n"));
        }
        if matches.is_empty() {
            output.push_str("No matches found.\n");
        }
        Ok(ToolOutcome {
            title: Some(format!("Grep {}", input.pattern)),
            output,
            metadata: Some(json!({"pattern": input.pattern, "matches": matches.len()})),
            attachments: Vec::new(),
        })
    }
}

fn build_include(pattern: &str) -> Result<GlobSet, ToolFailure> {
    let glob = GlobBuilder::new(&pattern.replace('\\', "/"))
        .literal_separator(false)
        .backslash_escape(false)
        .build()
        .map_err(|error| ToolFailure::new(format!("invalid include glob: {error}")))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder
        .build()
        .map_err(|error| ToolFailure::new(format!("invalid include glob: {error}")))
}

fn search(
    root: &Path,
    expression: &Regex,
    include: Option<&GlobSet>,
    limit: usize,
) -> Result<Vec<(String, usize, String)>, ToolFailure> {
    let mut matches = Vec::new();
    if root.is_file() {
        search_file(
            root,
            root.parent().unwrap_or(root),
            expression,
            &mut matches,
            limit,
        );
        return Ok(matches);
    }
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker {
        if matches.len() >= limit {
            break;
        }
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
        if let Some(include) = include {
            if !include.is_match(entry.path()) {
                continue;
            }
        }
        search_file(entry.path(), root, expression, &mut matches, limit);
    }
    Ok(matches)
}

fn search_file(
    path: &Path,
    root: &Path,
    expression: &Regex,
    matches: &mut Vec<(String, usize, String)>,
    limit: usize,
) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() > MAX_FILE_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return;
    }
    let display = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    for (index, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
        if matches.len() >= limit {
            return;
        }
        if expression.is_match(line) {
            matches.push((
                display.clone(),
                index + 1,
                crate::truncate_middle(line.trim_end(), 400),
            ));
        }
    }
}
