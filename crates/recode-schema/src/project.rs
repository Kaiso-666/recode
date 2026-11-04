use crate::id::ProjectId;
use crate::Timestamp;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A project is the stable identity of a working tree. Sessions belong to a
/// project; the same directory can be opened as a worktree of one project.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: ProjectId,
    pub directory: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<String>,
    pub created_at: Timestamp,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl ProjectInfo {
    pub fn name_or_directory(&self) -> String {
        if self.name.is_empty() {
            self.directory
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| self.directory.display().to_string())
        } else {
            self.name.clone()
        }
    }
}
