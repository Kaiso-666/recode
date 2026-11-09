//! Minimal YAML frontmatter plus discovery for markdown agents and commands.
//!
//! opencode files start with `---`, a flat YAML mapping, then `---`. We keep
//! only the subset those files use: scalar fields and one level of nested
//! `key: value` maps (for example `tools:`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Frontmatter {
    fields: BTreeMap<String, String>,
    maps: BTreeMap<String, BTreeMap<String, String>>,
    pub body: String,
}

impl Frontmatter {
    pub fn parse(content: &str) -> Self {
        let content = content.strip_prefix('\u{feff}').unwrap_or(content);
        let mut lines = content.lines();
        let first = lines.next().unwrap_or_default().trim();
        if first != "---" {
            return Self {
                body: content.trim().to_owned(),
                ..Self::default()
            };
        }
        let mut fields = BTreeMap::new();
        let mut maps: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut current = None;
        let mut consumed = 1;
        for line in lines {
            consumed += 1;
            let trimmed = line.trim();
            if trimmed == "---" {
                break;
            }
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = line.len().saturating_sub(line.trim_start().len());
            let Some((key, value)) = trimmed.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = unquote(value.trim());
            if indent == 0 {
                current = Some(key.to_owned());
                if !value.is_empty() {
                    fields.insert(key.to_owned(), value);
                } else {
                    maps.entry(key.to_owned()).or_default();
                }
            } else if let Some(parent) = &current {
                if !value.is_empty() {
                    maps.entry(parent.clone())
                        .or_default()
                        .insert(key.to_owned(), value);
                }
            }
        }
        let body = content
            .lines()
            .skip(consumed)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
        Self { fields, maps, body }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn boolean(&self, key: &str) -> bool {
        self.get(key).is_some_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "1")
        })
    }

    /// Raw `key: value` entries of a nested map (for example `permission`).
    pub fn map(&self, key: &str) -> BTreeMap<String, String> {
        self.maps.get(key).cloned().unwrap_or_default()
    }

    /// A nested map whose values are booleans (for example `tools`).
    pub fn bool_map(&self, key: &str) -> BTreeMap<String, bool> {
        self.map(key)
            .into_iter()
            .filter_map(|(key, value)| {
                let enabled = match value.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" => true,
                    "false" | "no" | "0" => false,
                    _ => return None,
                };
                Some((key, enabled))
            })
            .collect()
    }
}

/// One markdown file found under a configuration directory. `primary` is set
/// for the legacy `mode(s)` directories, whose files always become primary.
pub struct MarkdownFile {
    pub path: PathBuf,
    pub primary: bool,
}

/// Scan `(subdirectory, recursive, primary)` sources under `root` for `.md`
/// files, sorted by path for deterministic precedence.
pub fn discover(root: &Path, sources: &[(&str, bool, bool)]) -> Vec<MarkdownFile> {
    let mut files = Vec::new();
    for (name, recursive, primary) in sources {
        collect(&root.join(name), *recursive, *primary, &mut files);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    files
}

fn collect(directory: &Path, recursive: bool, primary: bool, files: &mut Vec<MarkdownFile>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if recursive {
                collect(&path, true, primary, files);
            }
            continue;
        }
        if kind.is_file() && path.extension().is_some_and(|extension| extension == "md") {
            files.push(MarkdownFile { path, primary });
        }
    }
}

/// The file's path under `root` minus its first component and `.md`:
/// `agent/review/security.md` becomes `review/security`.
pub fn markdown_name(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?.to_string_lossy();
    let relative = relative.replace('\\', "/");
    let (_, name) = relative.split_once('/')?;
    let name = name.strip_suffix(".md")?;
    (!name.is_empty()).then(|| name.to_owned())
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value);
    value.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fields_and_nested_tools() {
        let parsed = Frontmatter::parse(
            r#"---
description: "Review code"
mode: subagent
temperature: 0.2
tools:
  write: false
  edit: false
  read: true
---
You are a reviewer.
"#,
        );
        assert_eq!(parsed.get("description"), Some("Review code"));
        assert_eq!(parsed.get("mode"), Some("subagent"));
        assert_eq!(parsed.body, "You are a reviewer.");
        let tools = parsed.bool_map("tools");
        assert_eq!(tools.get("write"), Some(&false));
        assert_eq!(tools.get("read"), Some(&true));
    }

    #[test]
    fn treats_missing_frontmatter_as_body() {
        let parsed = Frontmatter::parse("Just a prompt.\n");
        assert!(parsed.fields.is_empty());
        assert_eq!(parsed.body, "Just a prompt.");
    }
}
