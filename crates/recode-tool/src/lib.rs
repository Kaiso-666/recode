//! Tool registry and built-in tools.
//!
//! Tools are capabilities: each declares a JSON schema, a permission action,
//! and receives a [`ToolContext`] pinning it to a session directory and an
//! abort token. Tools never write session events; the runner persists results.

mod bash;
mod fs;
mod glob;
mod grep;
mod read;
pub mod todo;
mod webfetch;
mod write;

pub use todo::{todos_for, TodoItem, TodoPriority, TodoStatus};

use async_trait::async_trait;
use recode_schema::id::{MessageId, SessionId};
use recode_schema::llm::ToolSpec;
use recode_schema::permission::Decision;
use recode_schema::tool::{ToolFailure, ToolResult};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const MAX_TOOL_OUTPUT: usize = 200_000;

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub call_id: String,
    /// Working directory for relative paths and shell commands.
    pub directory: PathBuf,
    /// Whether absolute paths outside `directory` are allowed.
    pub allow_outside: bool,
    pub abort: CancellationToken,
    /// Optional streaming progress sink for long-running tools.
    pub progress: Option<ProgressFn>,
}

impl ToolContext {
    pub fn new(
        session_id: SessionId,
        message_id: MessageId,
        call_id: impl Into<String>,
        directory: PathBuf,
    ) -> Self {
        Self {
            session_id,
            message_id,
            call_id: call_id.into(),
            directory,
            allow_outside: false,
            abort: CancellationToken::new(),
            progress: None,
        }
    }

    pub fn progress(&self, message: impl Into<String>) {
        if let Some(progress) = &self.progress {
            progress(message.into());
        }
    }

    pub fn resolve_path(&self, raw: &str) -> Result<PathBuf, ToolFailure> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ToolFailure::new("path must not be empty"));
        }
        let path = Path::new(raw);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.directory.join(path)
        };
        let normalized = normalize_path(&absolute);
        let root = canonicalish(&self.directory);
        if !self.allow_outside && !is_within(&canonicalish(&normalized), &root) {
            return Err(ToolFailure::new(format!(
                "path escapes the project directory: {raw}\nResolved: {}\nAllowed root: {}",
                normalized.display(),
                root.display()
            )));
        }
        Ok(normalized)
    }
}

pub type ProgressFn = Arc<dyn Fn(String) + Send + Sync>;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    /// JSON Schema (draft 2020-12 subset) describing the arguments object.
    fn schema(&self) -> Value;

    /// Permission action evaluated before execution, for example `bash` or
    /// `edit`. Defaults to the tool name.
    fn permission(&self) -> &str {
        self.name()
    }

    /// Decision when no configured rule matches.
    fn default_decision(&self) -> Decision {
        Decision::Ask
    }

    /// Human-readable summary shown while the tool runs.
    fn title(&self, input: &Value) -> String {
        let _ = input;
        self.name().to_owned()
    }

    /// Patterns evaluated by the permission engine (commands, paths, URLs).
    fn permission_patterns(&self, input: &Value) -> Vec<String> {
        let _ = input;
        Vec::new()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> ToolResult;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The standard tool set available to agents.
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(bash::BashTool));
        registry.register(Arc::new(read::ReadTool));
        registry.register(Arc::new(write::WriteTool));
        registry.register(Arc::new(write::EditTool));
        registry.register(Arc::new(write::MultiEditTool));
        registry.register(Arc::new(fs::LsTool));
        registry.register(Arc::new(glob::GlobTool));
        registry.register(Arc::new(grep::GrepTool));
        registry.register(Arc::new(todo::TodoTool));
        registry.register(Arc::new(webfetch::WebFetchTool));
        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }

    /// Tool definitions advertised to the model, filtered by an agent's tool
    /// map. `None` means all tools; a map with any `true` is an allowlist.
    pub fn specs(&self, filter: Option<&BTreeMap<String, bool>>) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .filter(|(name, _)| enabled(name, filter))
            .map(|(name, tool)| ToolSpec {
                name: name.clone(),
                description: tool.description().to_owned(),
                input_schema: tool.schema(),
                namespace: None,
            })
            .collect()
    }

    pub async fn execute(&self, name: &str, input: Value, context: &ToolContext) -> ToolResult {
        let Some(tool) = self.get(name) else {
            return Err(ToolFailure::new(format!(
                "unknown tool {name:?}; available tools: {}",
                self.names().join(", ")
            )));
        };
        if context.abort.is_cancelled() {
            return Err(ToolFailure::new("aborted before the tool started"));
        }
        let mut output = tool.execute(input, context).await?;
        output.output = truncate_middle(&output.output, MAX_TOOL_OUTPUT);
        Ok(output)
    }
}

pub fn enabled(name: &str, filter: Option<&BTreeMap<String, bool>>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if let Some(value) = filter.get(name) {
        return *value;
    }
    if filter.values().any(|enabled| *enabled) {
        return false;
    }
    true
}

/// Clamp text to `limit` characters, preserving both ends so errors at the
/// tail remain visible.
pub fn truncate_middle(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_owned();
    }
    let head_len = limit / 2;
    let tail_len = limit - head_len;
    let head: String = value.chars().take(head_len).collect();
    let tail: String = value.chars().skip(count - tail_len).collect();
    let truncated = count - limit;
    format!("{head}\n… [{truncated} characters truncated] …\n{tail}")
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

pub fn canonicalish(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return strip_verbatim(path);
    }
    if let Some(parent) = path.parent() {
        if let Ok(parent) = parent.canonicalize() {
            if let Some(name) = path.file_name() {
                return strip_verbatim(parent).join(name);
            }
        }
    }
    normalize_path(path)
}

pub fn is_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().to_lowercase().replace('/', "\\");
        let root = root.to_string_lossy().to_lowercase().replace('/', "\\");
        let root = root.trim_end_matches('\\');
        path == root || path.starts_with(&format!("{root}\\"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_filters_follow_allowlist_semantics() {
        let mut filter = BTreeMap::new();
        filter.insert("read".to_owned(), true);
        assert!(enabled("read", Some(&filter)));
        assert!(!enabled("bash", Some(&filter)));

        let mut deny = BTreeMap::new();
        deny.insert("write".to_owned(), false);
        assert!(enabled("read", Some(&deny)));
        assert!(!enabled("write", Some(&deny)));
        assert!(enabled("bash", None));
    }

    #[test]
    fn truncation_preserves_both_ends() {
        let value = "a".repeat(100);
        let truncated = truncate_middle(&value, 20);
        assert!(truncated.contains("truncated"));
        assert!(truncated.starts_with("aaaaaaaaaa"));
    }

    #[test]
    fn paths_cannot_escape_the_project() {
        let dir = std::env::temp_dir().join("recode-path-test");
        std::fs::create_dir_all(&dir).unwrap();
        let context = ToolContext::new(SessionId::new(), MessageId::new(), "call", dir.clone());
        assert!(context.resolve_path("inside.txt").is_ok());
        assert!(context.resolve_path("../outside.txt").is_err());
        let mut context = context;
        context.allow_outside = true;
        assert!(context.resolve_path("../outside.txt").is_ok());
    }
}
