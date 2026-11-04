use crate::config::ConfigManager;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InstructionBlock {
    pub source: PathBuf,
    pub content: String,
}

#[derive(Clone, Debug, Default)]
pub struct Instructions {
    pub blocks: Vec<InstructionBlock>,
}

impl Instructions {
    /// Discover project and global instruction files, in precedence order.
    ///
    /// Looked up:
    /// - `AGENTS.md` and `CLAUDE.md` in the project root and in ancestors of
    ///   the session directory (nested files refine root rules).
    /// - `AGENTS.md` in the global config directory.
    /// - every glob in `config.instructions`.
    pub fn load(config: &ConfigManager, project_root: &Path, session_directory: &Path) -> Self {
        let mut blocks = Vec::new();
        let mut seen = BTreeSet::new();

        let global = config.paths.config_dir.join("AGENTS.md");
        push_file(&mut blocks, &mut seen, &global);

        push_file(&mut blocks, &mut seen, &project_root.join("AGENTS.md"));
        push_file(&mut blocks, &mut seen, &project_root.join("CLAUDE.md"));

        // Nested instructions between the project root and the working dir.
        if session_directory.starts_with(project_root) {
            let mut ancestors: Vec<&Path> = session_directory.ancestors().collect();
            ancestors.reverse();
            for ancestor in ancestors {
                if !ancestor.starts_with(project_root) || ancestor == project_root {
                    continue;
                }
                push_file(&mut blocks, &mut seen, &ancestor.join("AGENTS.md"));
                push_file(&mut blocks, &mut seen, &ancestor.join("CLAUDE.md"));
            }
        }

        for pattern in &config.merged.instructions {
            for file in expand_instruction(pattern, project_root) {
                push_file(&mut blocks, &mut seen, &file);
            }
        }

        Self { blocks }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Render for the system prompt, capped to keep a runaway AGENTS.md from
    /// consuming the context window.
    pub fn render(&self, limit: usize) -> String {
        let mut output = String::new();
        for block in &self.blocks {
            if output.len() >= limit {
                output.push_str("\n… additional instruction files omitted …\n");
                break;
            }
            let remaining = limit.saturating_sub(output.len());
            let content = if block.content.len() > remaining {
                &block.content[..floor_char_boundary(&block.content, remaining)]
            } else {
                &block.content
            };
            output.push_str(&format!(
                "\nInstructions from {}:\n{}\n",
                block.source.display(),
                content.trim()
            ));
        }
        output
    }
}

fn push_file(blocks: &mut Vec<InstructionBlock>, seen: &mut BTreeSet<PathBuf>, path: &Path) {
    let Ok(canonical) = path.canonicalize() else {
        return;
    };
    if !seen.insert(canonical.clone()) {
        return;
    }
    let Ok(content) = std::fs::read_to_string(&canonical) else {
        return;
    };
    let content = truncate(&content, 64 * 1024);
    if content.trim().is_empty() {
        return;
    }
    blocks.push(InstructionBlock {
        source: canonical,
        content,
    });
}

fn expand_instruction(pattern: &str, root: &Path) -> Vec<PathBuf> {
    let pattern = pattern.replace('\\', "/");
    // Plain relative file.
    if !pattern.contains('*') && !pattern.contains('?') {
        let path = root.join(&pattern);
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let Ok(glob) = globset::GlobBuilder::new(&pattern)
        .literal_separator(false)
        .backslash_escape(false)
        .build()
    else {
        return Vec::new();
    };
    let matcher = match globset::GlobSetBuilder::new().add(glob).build() {
        Ok(set) => set,
        Err(_) => return Vec::new(),
    };
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        if matcher.is_match(&relative) {
            files.push(entry.path().to_path_buf());
        }
    }
    files
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let end = floor_char_boundary(value, limit);
    let mut output = value[..end].to_owned();
    output.push_str("\n… [truncated]");
    output
}

fn floor_char_boundary(value: &str, index: usize) -> usize {
    if index >= value.len() {
        return value.len();
    }
    let mut index = index;
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RecodePaths;

    #[test]
    fn loads_project_agents_md_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Use tabs.\n").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "No swearing.\n").unwrap();
        let paths = RecodePaths {
            config_dir: dir.path().join("config"),
            data_dir: dir.path().join("data"),
            db_path: dir.path().join("db.sqlite"),
            auth_path: dir.path().join("auth.json"),
            log_dir: dir.path().join("log"),
            global_config: dir.path().join("config/recode.json"),
        };
        let config = ConfigManager::load(paths, dir.path());
        let instructions = Instructions::load(&config, dir.path(), dir.path());
        let rendered = instructions.render(100_000);
        assert!(rendered.contains("Use tabs."));
        assert!(rendered.contains("No swearing."));
        assert_eq!(rendered.matches("Use tabs.").count(), 1);
    }
}
