//! Shadow-git snapshots of the worktree.
//!
//! Every prompt is preceded by a commit in a private repository stored under
//! the data directory. The repository has no relation to the user's own `.git`
//! (which is explicitly excluded), so snapshots work in plain directories and
//! never create branches, stashes or commits in the project.
//!
//! Restoring a snapshot rewrites the worktree to that tree, deletes files that
//! did not exist then (ignored files are left alone), and leaves the snapshot
//! history intact so the restore itself can be undone.

use crate::paths::RecodePaths;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Directories that must never enter a snapshot even when the project has no
/// `.gitignore`: the real repository metadata, and this database directory if
/// it happens to live inside the worktree.
const ALWAYS_EXCLUDED: &[&str] = &[".git"];

#[derive(Clone)]
pub struct SnapshotService {
    git_dir: PathBuf,
    work_tree: PathBuf,
    excludes_file: Option<PathBuf>,
    enabled: bool,
    lock: Arc<Mutex<()>>,
}

impl SnapshotService {
    pub fn new(paths: &RecodePaths, project_id: &str, work_tree: PathBuf, enabled: bool) -> Self {
        let git_dir = paths.data_dir.join("snapshots").join(project_id);
        let gitignore = work_tree.join(".gitignore");
        let excludes_file = gitignore.is_file().then_some(gitignore);
        Self {
            git_dir,
            work_tree,
            excludes_file,
            enabled,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Commit the current worktree and return the commit id. `Ok(None)` means
    /// snapshots are disabled by configuration.
    pub async fn snapshot(&self, label: &str) -> Result<Option<String>, String> {
        if !self.enabled {
            return Ok(None);
        }
        let _guard = self.lock.lock().await;
        let service = self.clone();
        let label = label.to_owned();
        let hash = tokio::task::spawn_blocking(move || service.snapshot_blocking(&label))
            .await
            .map_err(|error| format!("snapshot task failed: {error}"))??;
        Ok(Some(hash))
    }

    /// Restore the worktree to `commit`. Files created after that snapshot are
    /// removed unless they are ignored; ignored files are never touched.
    pub async fn restore(&self, commit: &str) -> Result<(), String> {
        if !self.enabled {
            return Err("file snapshots are disabled".into());
        }
        validate_commit(commit)?;
        let _guard = self.lock.lock().await;
        let service = self.clone();
        let commit = commit.to_owned();
        tokio::task::spawn_blocking(move || service.restore_blocking(&commit))
            .await
            .map_err(|error| format!("restore task failed: {error}"))?
    }

    /// Files that differ between `commit` and the current worktree.
    pub async fn changed_files(&self, commit: &str) -> Result<Vec<String>, String> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        validate_commit(commit)?;
        let _guard = self.lock.lock().await;
        let service = self.clone();
        let commit = commit.to_owned();
        tokio::task::spawn_blocking(move || service.changed_files_blocking(&commit))
            .await
            .map_err(|error| format!("diff task failed: {error}"))?
    }

    /// Unified diff between `commit` and the current worktree.
    pub async fn diff(&self, commit: &str) -> Result<String, String> {
        if !self.enabled {
            return Ok(String::new());
        }
        validate_commit(commit)?;
        let _guard = self.lock.lock().await;
        let service = self.clone();
        let commit = commit.to_owned();
        tokio::task::spawn_blocking(move || {
            service.ensure_init()?;
            service.run(&["diff", "--no-color", &commit])
        })
        .await
        .map_err(|error| format!("diff task failed: {error}"))?
    }

    fn snapshot_blocking(&self, label: &str) -> Result<String, String> {
        self.ensure_init()?;
        self.run(&["add", "-A", "--", "."])?;
        self.run(&[
            "commit",
            "--quiet",
            "--no-verify",
            "--allow-empty",
            "-m",
            label,
        ])?;
        self.run(&["rev-parse", "HEAD"])
    }

    fn restore_blocking(&self, commit: &str) -> Result<(), String> {
        self.ensure_init()?;
        self.run(&["cat-file", "-e", &format!("{commit}^{{commit}}")])?;
        // Point the index at the snapshot, materialize it, then drop every
        // untracked file (which now includes files created after the snapshot).
        self.run(&["read-tree", commit])?;
        self.run(&["checkout-index", "-a", "-f"])?;
        self.run(&["clean", "-fdq"])?;
        Ok(())
    }

    fn changed_files_blocking(&self, commit: &str) -> Result<Vec<String>, String> {
        let output = self.run(&["diff", "--name-only", commit])?;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn ensure_init(&self) -> Result<(), String> {
        if self.git_dir.join("HEAD").is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.git_dir)
            .map_err(|error| format!("cannot create {}: {error}", self.git_dir.display()))?;
        self.run(&["init", "--quiet"])?;
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info)
            .map_err(|error| format!("cannot create {}: {error}", info.display()))?;
        let mut exclude = String::new();
        for pattern in ALWAYS_EXCLUDED {
            exclude.push_str(pattern);
            exclude.push('\n');
        }
        if let Some(relative) = relative_inside(&self.work_tree, &self.git_dir) {
            exclude.push('/');
            exclude.push_str(&relative.to_string_lossy().replace('\\', "/"));
            exclude.push('/');
            exclude.push('\n');
        }
        std::fs::write(info.join("exclude"), exclude)
            .map_err(|error| format!("cannot write snapshot excludes: {error}"))?;
        Ok(())
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("git");
        command
            .current_dir(&self.work_tree)
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.work_tree)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "-c",
                "user.name=Re:Code",
                "-c",
                "user.email=recode@localhost",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.safecrlf=false",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "gc.auto=0",
            ]);
        if let Some(excludes) = &self.excludes_file {
            command
                .arg("-c")
                .arg(format!("core.excludesFile={}", excludes.display()));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command.args(args);
        let output = command
            .output()
            .map_err(|error| format!("cannot run git: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = stderr.trim();
            return Err(if message.is_empty() {
                format!("git {} failed ({})", args.join(" "), output.status)
            } else {
                message.to_owned()
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

fn validate_commit(commit: &str) -> Result<(), String> {
    if commit.len() < 4
        || commit.len() > 64
        || !commit
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(format!("invalid snapshot id: {commit}"));
    }
    Ok(())
}

fn relative_inside(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = recode_tool::canonicalish(root);
    let path = recode_tool::canonicalish(path);
    path.strip_prefix(&root).ok().map(Path::to_path_buf)
}
