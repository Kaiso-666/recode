use recode_schema::id::ProjectId;
use recode_schema::project::ProjectInfo;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A discovered working tree.
#[derive(Clone, Debug)]
pub struct Project {
    pub info: ProjectInfo,
    pub root: PathBuf,
    pub branch: Option<String>,
    pub is_git: bool,
}

impl Project {
    pub fn discover(directory: impl AsRef<Path>) -> Result<Self, String> {
        let directory = directory
            .as_ref()
            .canonicalize()
            .map_err(|error| format!("cannot open {}: {error}", directory.as_ref().display()))?;
        if !directory.is_dir() {
            return Err(format!("{} is not a directory", directory.display()));
        }
        let git_root = find_git_root(&directory);
        let root = git_root.clone().unwrap_or_else(|| directory.clone());
        let is_git = git_root.is_some();
        let branch = if is_git {
            git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])
        } else {
            None
        };
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_owned());
        let id = project_id(&root);
        let info = ProjectInfo {
            id,
            directory: root.clone(),
            worktree: None,
            name,
            vcs: if is_git { Some("git".to_owned()) } else { None },
            created_at: recode_schema::now_ms(),
            metadata: serde_json::Value::Null,
        };
        Ok(Self {
            info,
            root,
            branch,
            is_git,
        })
    }

    /// Short git state for the system prompt, bounded to keep prompts small.
    pub fn git_summary(&self) -> Option<String> {
        if !self.is_git {
            return None;
        }
        let branch = self.branch.clone().unwrap_or_else(|| "unknown".to_owned());
        let status = git(
            &self.root,
            &["status", "--porcelain=v1", "--untracked-files=no"],
        )
        .unwrap_or_default();
        let count = status.lines().count();
        Some(if count == 0 {
            format!("branch {branch}, clean")
        } else {
            format!("branch {branch}, {count} modified file(s)")
        })
    }
}

fn find_git_root(directory: &Path) -> Option<PathBuf> {
    for ancestor in directory.ancestors() {
        if ancestor.join(".git").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn git(directory: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn project_id(root: &Path) -> ProjectId {
    let mut hasher = DefaultHasher::new();
    root.to_string_lossy().to_lowercase().hash(&mut hasher);
    ProjectId::from_existing(format!("prj_{:016x}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::discover(dir.path()).unwrap();
        assert_eq!(project.root, dir.path().canonicalize().unwrap());
        assert!(!project.info.id.0.is_empty());
    }
}
